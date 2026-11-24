//! What the Usage page reads, as one trait: the environments with their
//! access and limits, and one method per RPC. [`FixtureUsageSource`] feeds
//! scenes; [`LiveUsageSource`] talks to connected environments.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;
use futures::FutureExt as _;
use gpui_kit::{App, SharedString};
use jiff::tz::TimeZone;
use t3code_client_runtime::preview::environment_link::EnvironmentLink;
use t3code_contracts::rpc::methods::{ProviderConsumeResetCredit, ServerGetUsageSummary, ServerRefreshProviders, ServerRefreshUsageRates, ServerUpdateSettings};
use t3code_contracts::{
    AuthEnvironmentScope, AuthSessionState, ClientServerSettingsPatch, EnvironmentId, ServerUpdateSettingsPayload, ProviderConsumeResetCreditInput, ProviderConsumeResetCreditResult,
    ServerConfig, ServerConfigProvidersItem, ServerConfigStreamEvent, ServerProviderUpdatedPayload,
    ServerRefreshProvidersPayload, UsageModelFilter, UsagePricing, UsageSummary, UsageSummaryInput,
};
use t3code_shared::usage_limits::LimitEnvironment;
use tokio::sync::watch;

use super::usage_price_table::UsagePriceTarget;
use crate::connection::ConnectionRuntime;
use crate::i18n::t;

/// A failed call, worded for the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError(pub SharedString);

/// One RPC's answer.
pub type UsageFuture<T> = BoxFuture<'static, Result<T, UsageError>>;

/// Whether this connection may read usage (`resolveUsageAccess`).
#[derive(Clone, Debug, PartialEq)]
pub struct UsageAccess {
    pub can_read_diagnostics: bool,
    /// The session check has not finished; keep the environment out of totals for now.
    pub is_pending: bool,
    pub error: Option<SharedString>,
}

/// One saved environment as the Usage page sees it.
#[derive(Clone, Debug)]
pub struct UsageEnvironment {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub is_connected: bool,
    pub access: UsageAccess,
    /// The server's `usageModelFilter` setting, sent with each summary request.
    pub model_filter: Option<UsageModelFilter>,
    /// Providers and limit hubs from the server config.
    pub limits: LimitEnvironment,
    /// Whether this session may redeem reset credits (`providers:manage`).
    pub can_manage_providers: bool,
    /// The server's custom prices and model mappings, for the model prices dialog.
    pub price_target: UsagePriceTarget,
}

/// The Usage page's data. Each method but [`Self::environments`] is one RPC.
pub trait UsageSource: 'static {
    /// Every saved environment, updated as connections, grants and server
    /// configs (`subscribeServerConfig`) change.
    fn environments(&self) -> watch::Receiver<Arc<[UsageEnvironment]>>;
    /// `server.getUsageSummary`.
    fn usage_summary(&self, environment: &EnvironmentId, input: UsageSummaryInput) -> UsageFuture<Arc<UsageSummary>>;
    /// `server.refreshUsageRates`.
    fn refresh_usage_rates(&self, environment: &EnvironmentId) -> UsageFuture<UsagePricing>;
    /// `server.refreshProviders`: re-reads subscription limits.
    fn refresh_providers(&self, environment: &EnvironmentId) -> UsageFuture<ServerProviderUpdatedPayload>;
    /// `server.updateSettings` with `patch`.
    fn update_settings(&self, environment: &EnvironmentId, patch: ClientServerSettingsPatch) -> UsageFuture<()>;
    /// `provider.consumeResetCredit`.
    fn consume_reset_credit(
        &self,
        environment: &EnvironmentId,
        input: ProviderConsumeResetCreditInput,
    ) -> UsageFuture<ProviderConsumeResetCreditResult>;
    /// The clock limits and windows are measured against, in epoch ms.
    fn now_ms(&self) -> i64;
    /// The viewer's zone and its IANA name.
    fn time_zone(&self) -> (TimeZone, String);
}

/// The environment's providers and hubs as [`LimitEnvironment`].
pub fn limit_environment(environment_id: &EnvironmentId, label: &str, config: Option<&ServerConfig>) -> LimitEnvironment {
    LimitEnvironment {
        environment_id: environment_id.clone(),
        label: label.to_owned(),
        providers: config.map_or_else(|| Arc::from(Vec::new()), |c| Arc::from(c.providers.clone())),
        usage_limit_sources: config
            .and_then(|c| c.usage_limit_sources.clone())
            .map_or_else(|| Arc::from(Vec::new()), Arc::from),
    }
}

/// Live data from the app's connections. Each environment's config follows
/// its `subscribeServerConfig` stream: snapshots, provider statuses, settings
/// and limit hub updates.
pub struct LiveUsageSource {
    environments: watch::Receiver<Arc<[UsageEnvironment]>>,
    links: Arc<Mutex<HashMap<EnvironmentId, EnvironmentLink>>>,
    handle: tokio::runtime::Handle,
}

type Followed = Arc<Mutex<HashMap<EnvironmentId, LiveEnvironment>>>;

/// Publishes the followed environments, in saved order.
#[derive(Clone)]
struct Publisher {
    followed: Followed,
    order: Arc<Mutex<Vec<EnvironmentId>>>,
    sender: Arc<watch::Sender<Arc<[UsageEnvironment]>>>,
}

impl Publisher {
    fn update(&self, id: &EnvironmentId, change: impl FnOnce(&mut LiveEnvironment)) {
        if let Some(entry) = self.followed.lock().unwrap().get_mut(id) {
            change(entry);
        }
        self.publish();
    }

    fn publish(&self) {
        let followed = self.followed.lock().unwrap();
        let list: Vec<UsageEnvironment> =
            self.order.lock().unwrap().iter().filter_map(|id| followed.get(id)).map(LiveEnvironment::usage).collect();
        self.sender.send_replace(list.into());
    }
}

impl LiveUsageSource {
    /// Starts following every saved environment. Needs [`ConnectionRuntime`].
    pub fn new(cx: &mut App) -> Rc<Self> {
        let handle = ConnectionRuntime::global(cx).handle().clone();
        let (sender, environments) = watch::channel(Arc::from(Vec::new()));
        let links: Arc<Mutex<HashMap<EnvironmentId, EnvironmentLink>>> = Arc::default();
        let publisher = Publisher { followed: Arc::default(), order: Arc::default(), sender: Arc::new(sender) };
        let sync = {
            let (links, handle, publisher) = (links.clone(), handle.clone(), publisher.clone());
            move |cx: &mut App| {
                let state = ConnectionRuntime::state(cx).read(cx).environments.clone();
                let runtime = ConnectionRuntime::global(cx);
                *publisher.order.lock().unwrap() = state.iter().map(|e| e.id().clone()).collect();
                for environment in &state {
                    let id = environment.id().clone();
                    {
                        let mut followed = publisher.followed.lock().unwrap();
                        let entry = followed.entry(id.clone()).or_insert_with(|| LiveEnvironment::new(id.clone()));
                        entry.label = environment.entry.label.clone().into();
                        entry.connected = environment.is_connected();
                    }
                    let mut links = links.lock().unwrap();
                    if links.contains_key(&id) {
                        continue;
                    }
                    let Some(link) = runtime.environment_link(&id) else { continue };
                    links.insert(id, link.clone());
                    handle.spawn(follow(link, publisher.clone()));
                }
                publisher.publish();
            }
        };
        sync(cx);
        let state = ConnectionRuntime::state(cx);
        cx.observe(&state, move |_, cx| sync(cx)).detach();
        Rc::new(Self { environments, links, handle })
    }

    fn call<T: Send + 'static>(
        &self,
        environment: &EnvironmentId,
        run: impl FnOnce(EnvironmentLink) -> BoxFuture<'static, Result<T, UsageError>> + Send + 'static,
    ) -> UsageFuture<T> {
        let Some(link) = self.links.lock().unwrap().get(environment).cloned() else {
            return async { Err(UsageError(t!("This environment is not connected."))) }.boxed();
        };
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.handle.spawn(async move {
            _ = sender.send(run(link).await);
        });
        async move { receiver.await.unwrap_or_else(|_| Err(UsageError("The app is shutting down.".into()))) }.boxed()
    }
}

/// One environment's followed state.
#[derive(Clone)]
struct LiveEnvironment {
    id: EnvironmentId,
    label: SharedString,
    connected: bool,
    config: Option<Arc<ServerConfig>>,
    session: Option<AuthSessionState>,
    session_error: bool,
}

impl LiveEnvironment {
    fn new(id: EnvironmentId) -> Self {
        Self { id, label: SharedString::default(), connected: false, config: None, session: None, session_error: false }
    }

    fn usage(&self) -> UsageEnvironment {
        let access = match (&self.session, self.session_error) {
            (_, true) => UsageAccess {
                can_read_diagnostics: false,
                is_pending: false,
                error: Some(t!("Could not check this connection's access to diagnostics and usage.")),
            },
            (None, _) => UsageAccess {
                can_read_diagnostics: false,
                is_pending: self.connected,
                error: (!self.connected).then(|| t!("This environment is not connected.")),
            },
            (Some(session), _) => {
                let can = session.grants_scope(AuthEnvironmentScope::DiagnosticsRead);
                UsageAccess {
                    can_read_diagnostics: can,
                    is_pending: false,
                    error: (!can).then(|| t!("This connection does not have access to diagnostics and usage.")),
                }
            }
        };
        UsageEnvironment {
            environment_id: self.id.clone(),
            label: self.label.clone(),
            is_connected: self.connected,
            access,
            model_filter: self.config.as_ref().and_then(|c| c.settings.usage_model_filter.clone()),
            limits: limit_environment(&self.id, &self.label, self.config.as_deref()),
            can_manage_providers: self.session.as_ref().is_some_and(|s| s.grants_scope(AuthEnvironmentScope::ProvidersManage)),
            price_target: self.price_target(),
        }
    }

    /// `priceTargetsAtom`'s row for this environment.
    fn price_target(&self) -> UsagePriceTarget {
        let settings = self.config.as_ref().map(|c| &c.settings);
        let maps_models = self.config.as_ref().is_some_and(|c| c.environment.capabilities.usage_model_aliases == Some(true));
        let unavailable = if !self.connected {
            Some(t!("Offline"))
        } else if settings.is_none() {
            Some(t!("Prices not loaded"))
        } else if self.session.is_none() && !self.session_error {
            Some(t!("Checking permissions…"))
        } else if self.session_error || !self.session.as_ref().is_some_and(|s| s.grants_scope(AuthEnvironmentScope::SettingsWrite)) {
            Some(t!("Read-only access"))
        } else {
            None
        };
        UsagePriceTarget {
            environment_id: self.id.clone(),
            label: self.label.clone(),
            prices: settings.map(|s| Arc::new(s.usage_price_overrides.clone().unwrap_or_default())),
            aliases: settings.filter(|_| maps_models).map(|s| Arc::new(s.usage_model_aliases.clone().unwrap_or_default())),
            unavailable,
        }
    }
}

/// Follows one environment's sessions: its grants and its config stream.
async fn follow(link: EnvironmentLink, publisher: Publisher) {
    let id = link.environment_id.clone();
    let mut sessions = link.supervisor.session.clone();
    loop {
        let session = sessions.borrow_and_update().clone();
        let Some(session) = session else {
            publisher.update(&id, |entry| entry.session = None);
            if sessions.changed().await.is_err() {
                return;
            }
            continue;
        };
        let state = link.session_state().await;
        publisher.update(&id, |entry| {
            entry.config = Some(session.initial_config.clone());
            entry.session_error = state.is_err();
            entry.session = state.ok();
        });
        let mut events = session.config_events();
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(event) => publisher.update(&id, |entry| apply_config_event(entry, &event)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                },
                changed = sessions.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
            }
        }
    }
}

fn apply_config_event(entry: &mut LiveEnvironment, event: &ServerConfigStreamEvent) {
    let Some(config) = entry.config.as_mut() else { return };
    match event {
        ServerConfigStreamEvent::Snapshot(snapshot) => *config = Arc::new(snapshot.config.clone()),
        ServerConfigStreamEvent::ProviderStatuses(statuses) => {
            Arc::make_mut(config).providers = statuses.payload.providers.clone();
        }
        ServerConfigStreamEvent::UsageLimitSourcesUpdated(update) => {
            Arc::make_mut(config).usage_limit_sources = Some(update.payload.sources.clone());
        }
        ServerConfigStreamEvent::SettingsUpdated(update) => {
            Arc::make_mut(config).settings = update.payload.settings.clone();
        }
        _ => {}
    }
}

fn rpc_error(error: impl std::fmt::Display) -> UsageError {
    UsageError(error.to_string().into())
}

impl UsageSource for LiveUsageSource {
    fn environments(&self) -> watch::Receiver<Arc<[UsageEnvironment]>> {
        self.environments.clone()
    }

    fn usage_summary(&self, environment: &EnvironmentId, input: UsageSummaryInput) -> UsageFuture<Arc<UsageSummary>> {
        self.call(environment, move |link| {
            async move {
                let session = link.session(std::time::Duration::from_secs(10)).await.map_err(rpc_error)?;
                session.client.call::<ServerGetUsageSummary>(input).await.map(Arc::new).map_err(rpc_error)
            }
            .boxed()
        })
    }

    fn refresh_usage_rates(&self, environment: &EnvironmentId) -> UsageFuture<UsagePricing> {
        self.call(environment, |link| {
            async move {
                let session = link.session(std::time::Duration::from_secs(10)).await.map_err(rpc_error)?;
                session.client.call::<ServerRefreshUsageRates>(Default::default()).await.map_err(rpc_error)
            }
            .boxed()
        })
    }

    fn refresh_providers(&self, environment: &EnvironmentId) -> UsageFuture<ServerProviderUpdatedPayload> {
        self.call(environment, |link| {
            async move {
                let session = link.session(std::time::Duration::from_secs(10)).await.map_err(rpc_error)?;
                session.client.call::<ServerRefreshProviders>(ServerRefreshProvidersPayload::default()).await.map_err(rpc_error)
            }
            .boxed()
        })
    }

    fn update_settings(&self, environment: &EnvironmentId, patch: ClientServerSettingsPatch) -> UsageFuture<()> {
        self.call(environment, move |link| {
            async move {
                let session = link.session(std::time::Duration::from_secs(10)).await.map_err(rpc_error)?;
                let input = ServerUpdateSettingsPayload { patch, provider_instance_mutation: None };
                session.client.call::<ServerUpdateSettings>(input).await.map(|_| ()).map_err(rpc_error)
            }
            .boxed()
        })
    }

    fn consume_reset_credit(
        &self,
        environment: &EnvironmentId,
        input: ProviderConsumeResetCreditInput,
    ) -> UsageFuture<ProviderConsumeResetCreditResult> {
        self.call(environment, move |link| {
            async move {
                let session = link.session(std::time::Duration::from_secs(10)).await.map_err(rpc_error)?;
                session.client.call::<ProviderConsumeResetCredit>(input).await.map_err(rpc_error)
            }
            .boxed()
        })
    }

    fn now_ms(&self) -> i64 {
        crate::timestamp_format::now_ms()
    }

    fn time_zone(&self) -> (TimeZone, String) {
        let zone = TimeZone::system();
        let name = zone.iana_name().unwrap_or("UTC").to_owned();
        (zone, name)
    }
}


/// Scene data: three environments (one unavailable), a fixed clock, usage
/// generated for whatever window is asked, and Codex, Claude and Cursor
/// limits with two Codex accounts in one pool.
pub struct FixtureUsageSource {
    environments: watch::Receiver<Arc<[UsageEnvironment]>>,
    now_ms: i64,
    /// Summaries never answer, so the page shows its loading state.
    hold_summaries: bool,
}

/// The fixture's clock: 2026-11-20 15:37:12 UTC.
pub const FIXTURE_NOW_MS: i64 = 1_795_189_032_000;

impl FixtureUsageSource {
    /// Usage that never finishes loading.
    pub fn loading() -> Rc<Self> {
        let fixture = Self::new();
        Rc::new(Self { environments: fixture.environments.clone(), now_ms: fixture.now_ms, hold_summaries: true })
    }

    /// The environments with no provider reporting limits, as the reference server (Replay only) is.
    pub fn without_limits() -> Rc<Self> {
        let fixture = Self::new();
        let environments: Arc<[UsageEnvironment]> = fixture
            .environments
            .borrow()
            .iter()
            .cloned()
            .map(|mut environment| {
                environment.limits.providers = Arc::from([]);
                environment
            })
            .collect();
        let (_, receiver) = watch::channel(environments);
        Rc::new(Self { environments: receiver, now_ms: fixture.now_ms, hold_summaries: false })
    }

    pub fn new() -> Rc<Self> {
        let iso = |minutes: i64| t3code_shared::usage_format::iso_millis(FIXTURE_NOW_MS + minutes * 60_000);
        let window = |id: &str, kind: &str, label: &str, used: f64, resets_in_minutes: i64, length: Option<u64>| {
            serde_json::json!({
                "id": id, "kind": kind, "label": label, "usedPercent": used,
                "resetsAt": iso(resets_in_minutes),
                "windowDurationMins": length,
            })
        };
        let provider = |instance: &str, driver: &str, name: Option<&str>, email: &str, plan: &str, windows: serde_json::Value, credits: u64| {
            serde_json::json!({
                "instanceId": instance, "driver": driver, "displayName": name, "enabled": true, "installed": true,
                "status": "ready", "checkedAt": iso(-2), "models": [],
                "auth": { "status": "authenticated", "email": email, "label": plan },
                "usageLimits": {
                    "checkedAt": iso(-2), "windows": windows,
                    "resetCredits": if credits > 0 { serde_json::json!({ "availableCount": credits, "nextCreditId": "credit-1", "nextExpiresAt": iso(60 * 24 * 5) }) } else { serde_json::Value::Null },
                },
            })
        };
        let mut laptop = serde_json::json!([
            provider("codex", "codex", None, "dev@example.test", "ChatGPT Pro 200 Subscription", serde_json::json!([
                window("primary", "session", "5 hour", 38.0, 172, Some(300)),
                window("secondary", "weekly", "Weekly", 61.0, 60 * 24 * 3 + 50, Some(10080)),
            ]), 2),
            provider("claudeAgent", "claudeAgent", None, "dev@example.test", "Max", serde_json::json!([
                window("five_hour", "session", "5 hour", 72.0, 95, Some(300)),
                window("seven_day", "weekly", "Weekly", 44.0, 60 * 24 * 2, Some(10080)),
                window("seven_day_opus", "weekly", "Weekly Opus", 18.0, 60 * 24 * 2, Some(10080)),
            ]), 0),
            provider("cursor", "cursor", None, "dev@example.test", "Pro", serde_json::json!([
                window("autoPercentUsed", "monthly", "Auto", 27.0, 60 * 24 * 11, None),
                window("apiPercentUsed", "monthly", "API", 83.0, 60 * 24 * 11, None),
            ]), 0),
        ]);
        // The laptop's Codex signs in through ChatGPT sharing, so the cost view shows its row.
        laptop[0]["auth"]["subscriptionSharing"] = serde_json::json!(true);
        let build_box = serde_json::json!([
            provider("codex-work", "codex", Some("Work"), "work@example.test", "ChatGPT Plus Subscription", serde_json::json!([
                window("primary", "session", "5 hour", 91.0, 41, Some(300)),
                window("secondary", "weekly", "Weekly", 35.0, 60 * 24 * 5, Some(10080)),
            ]), 0),
        ]);
        let config = |providers: serde_json::Value| -> Vec<ServerConfigProvidersItem> {
            serde_json::from_value(providers).expect("fixture providers decode")
        };
        let environment = |id: &str, label: &str, providers: Vec<ServerConfigProvidersItem>, access: UsageAccess| {
            let environment_id = EnvironmentId::new(id);
            UsageEnvironment {
                limits: LimitEnvironment {
                    environment_id: environment_id.clone(),
                    label: label.into(),
                    providers: providers.into(),
                    usage_limit_sources: Arc::from(Vec::new()),
                },
                environment_id,
                label: label.to_owned().into(),
                is_connected: access.error.is_none(),
                access,
                model_filter: None,
                can_manage_providers: true,
                price_target: UsagePriceTarget {
                    environment_id: EnvironmentId::new(id),
                    label: label.to_owned().into(),
                    prices: Some(Arc::new(fixture_prices(id))),
                    aliases: Some(Arc::new(fixture_aliases(id))),
                    unavailable: None,
                },
            }
        };
        let ok = UsageAccess { can_read_diagnostics: true, is_pending: false, error: None };
        let list: Vec<UsageEnvironment> = vec![
            environment("env-laptop", "MacBook Pro", config(laptop), ok.clone()),
            environment("env-build", "Build box", config(build_box), ok),
        ];
        let (_, environments) = watch::channel(Arc::from(list));
        Rc::new(Self { environments, now_ms: FIXTURE_NOW_MS, hold_summaries: false })
    }
}

/// The fixture's custom prices: both environments price Kimi, at different input rates.
fn fixture_prices(environment: &str) -> std::collections::BTreeMap<String, t3code_contracts::UsageModelPriceOverride> {
    let kimi = |input: f64| t3code_contracts::UsageModelPriceOverride {
        input_cost_per_million_tokens: input,
        output_cost_per_million_tokens: 2.5,
        cache_read_cost_per_million_tokens: Some(0.15),
        cache_write_cost_per_million_tokens: None,
    };
    [("kimi-k2".to_owned(), kimi(if environment == "env-laptop" { 0.6 } else { 0.55 }))].into()
}

/// The fixture's model mappings: the laptop counts `auto` as Sonnet.
fn fixture_aliases(environment: &str) -> std::collections::BTreeMap<String, String> {
    if environment == "env-laptop" { [("auto".to_owned(), "claude-sonnet-4-5".to_owned())].into() } else { Default::default() }
}

/// Deterministic usage for `environment` over `input`'s days or hours.
fn fixture_summary(environment: &EnvironmentId, input: &UsageSummaryInput) -> UsageSummary {
    use t3code_shared::usage_format::{enumerate_days, enumerate_hour_starts};
    let laptop = environment.as_str() == "env-laptop";
    let models: &[(&str, &str, f64)] = if laptop {
        &[("codex", "gpt-5.1-codex", 1.0), ("claude", "claude-opus-4-5", 0.8), ("claude", "claude-sonnet-4-5", 0.5), ("cursor", "auto", 0.2), ("grok", "grok-code-fast-1", 0.15)]
    } else {
        &[("codex", "gpt-5.1-codex", 0.6), ("codex", "gpt-5.1-mini", 0.3), ("opencode", "kimi-k2", 0.1)]
    };
    let hourly = input.since_time.is_some() && input.until_time.is_some();
    let periods: Vec<(String, Option<String>)> = if hourly {
        enumerate_hour_starts(input.since_time.as_deref().unwrap_or_default(), input.until_time.as_deref().unwrap_or_default())
            .into_iter()
            .map(|hour| (hour[..10].to_owned(), Some(hour)))
            .collect()
    } else {
        enumerate_days(input.since_day.as_str(), input.until_day.as_str()).into_iter().map(|day| (day, None)).collect()
    };
    let mut buckets = Vec::new();
    for (index, (day, hour_start)) in periods.iter().enumerate() {
        for (model_index, (provider, model, weight)) in models.iter().enumerate() {
            let wave = 0.55 + 0.45 * ((index as f64 * 0.7 + model_index as f64 * 1.3).sin());
            let scale = if hourly { 0.06 } else { 1.0 } * weight * wave;
            if scale < 0.02 {
                continue;
            }
            let tokens = |base: f64| (base * scale).round() as u64;
            let (uncached, cached, writes, output) = (tokens(900_000.0), tokens(6_500_000.0), tokens(400_000.0), tokens(260_000.0));
            let cost = ((uncached as f64 * 1.25 + cached as f64 * 0.125 + writes as f64 * 1.5 + output as f64 * 10.0) / 1e6 * 100.0).round() / 100.0;
            let unpriced = *provider == "cursor";
            buckets.push(serde_json::json!({
                "day": day, "hourStart": hour_start, "provider": provider, "model": model,
                "totals": { "uncachedInputTokens": uncached, "cachedInputTokens": cached, "cacheCreationTokens": writes, "outputTokens": output, "reasoningTokens": output / 3 },
                "costUsd": if unpriced { 0.0 } else { cost },
                "cacheSavingsUsd": (cached as f64 * 1.125 / 1e6 * 100.0).round() / 100.0,
                "categoryCostUsd": if unpriced { serde_json::Value::Null } else { serde_json::json!({ "input": uncached as f64 * 1.25 / 1e6, "cacheRead": cached as f64 * 0.125 / 1e6, "cacheWrite": writes as f64 * 1.5 / 1e6, "output": output as f64 * 10.0 / 1e6 }) },
                "fastCostUsd": if *model == "claude-opus-4-5" { cost * 0.2 } else { 0.0 },
                "speedPremiumUsd": if *model == "claude-opus-4-5" { cost * 0.1 } else { 0.0 },
                "costSource": if unpriced { "unpriced" } else { "modelPriced" },
                "records": 12, "unpricedRecords": if unpriced { 12 } else { 0 }, "sessions": 2,
            }));
        }
    }
    let source = |provider: &str| {
        serde_json::json!({
            "fingerprint": { "hostId": environment.as_str(), "provider": provider, "resolvedHomePath": format!("/home/dev/.{provider}"), "volumeId": "v1" },
            "status": "ok", "scannedFiles": 40, "skippedFiles": 0, "malformedRecords": 0, "distinctSessions": 31, "message": null,
        })
    };
    let providers: Vec<&str> = {
        let mut list: Vec<&str> = models.iter().map(|(p, _, _)| *p).collect();
        list.dedup();
        list
    };
    serde_json::from_value(serde_json::json!({
        "contractVersion": 6, "readAt": t3code_shared::usage_format::iso_millis(FIXTURE_NOW_MS), "timeZone": input.time_zone,
        "sinceDay": input.since_day, "untilDay": input.until_day, "buckets": buckets,
        "sources": providers.iter().map(|p| source(p)).collect::<Vec<_>>(),
        "pricing": { "status": "fresh", "source": "fixture", "fetchedAt": null, "knownModels": 12 },
        "scanDurationMs": 40,
    }))
    .expect("fixture summary decodes")
}

impl UsageSource for FixtureUsageSource {
    fn environments(&self) -> watch::Receiver<Arc<[UsageEnvironment]>> {
        self.environments.clone()
    }

    fn usage_summary(&self, environment: &EnvironmentId, input: UsageSummaryInput) -> UsageFuture<Arc<UsageSummary>> {
        if self.hold_summaries {
            return futures::future::pending().boxed();
        }
        let summary = Arc::new(fixture_summary(environment, &input));
        async move { Ok(summary) }.boxed()
    }

    fn refresh_usage_rates(&self, _: &EnvironmentId) -> UsageFuture<UsagePricing> {
        async { Err(UsageError("fixture".into())) }.boxed()
    }

    fn refresh_providers(&self, _: &EnvironmentId) -> UsageFuture<ServerProviderUpdatedPayload> {
        async { Ok(ServerProviderUpdatedPayload { providers: Vec::new() }) }.boxed()
    }

    fn update_settings(&self, _: &EnvironmentId, _: ClientServerSettingsPatch) -> UsageFuture<()> {
        async { Ok(()) }.boxed()
    }

    fn consume_reset_credit(&self, _: &EnvironmentId, _: ProviderConsumeResetCreditInput) -> UsageFuture<ProviderConsumeResetCreditResult> {
        async { Ok(serde_json::from_value(serde_json::json!({ "outcome": "reset" })).expect("outcome decodes")) }.boxed()
    }

    fn now_ms(&self) -> i64 {
        self.now_ms
    }

    fn time_zone(&self) -> (TimeZone, String) {
        (TimeZone::UTC, "UTC".into())
    }
}
