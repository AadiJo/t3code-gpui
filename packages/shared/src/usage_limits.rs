//! The subscription-limit parts of `packages/shared/src/usageLimits.ts`: one
//! account per login across environments and limit hubs, pooled per driver
//! into windows with a remaining share, a pace, and reset times.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use t3code_contracts::{
    EnvironmentId, ProviderConsumeResetCreditInput, ProviderConsumeResetCreditInputNoUndefined, ProviderInstanceId,
    ServerConfigProvidersItem, ServerConfigProvidersItemUsageLimits as UsageLimits,
    ServerConfigProvidersItemUsageLimitsUnavailableReason as UnavailableReason,
    ServerConfigProvidersItemUsageLimitsWindowsItem as UsageWindow,
    ServerConfigProvidersItemUsageLimitsWindowsItemKind as WindowKind, ServerConfigUsageLimitSourcesItem,
    ServerProviderAuthStatus, ServerProviderAvailability, UsageLimitSourceConsumeResetCreditInput,
};

use crate::usage_format::parse_instant_ms;
use crate::usage_merge::number;

const MINUTE: f64 = 60_000.0;

/// Where ChatGPT shows usage for a connected account.
pub const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/#settings/Usage";

/// One environment's providers and limit hubs, as its server config reports them.
#[derive(Clone, Debug)]
pub struct LimitEnvironment {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub providers: Arc<[ServerConfigProvidersItem]>,
    pub usage_limit_sources: Arc<[ServerConfigUsageLimitSourcesItem]>,
}

/// The percentage of a window still unused, 0 to 100.
pub fn remaining_percent(window: &UsageWindow) -> f64 {
    js_round(100.0 - number(&window.used_percent).clamp(0.0, 100.0))
}

fn reset_millis(window: &UsageWindow) -> Option<f64> {
    window.resets_at.as_deref().and_then(parse_instant_ms).map(|ms| ms as f64)
}

/// How much of the window's length has passed, 0 to 1, when it has a clock.
pub fn elapsed_share(window: &UsageWindow, now: f64) -> Option<f64> {
    let resets_at = reset_millis(window)?;
    let length = window.window_duration_mins? as f64 * MINUTE;
    if length <= 0.0 {
        return None;
    }
    Some(((length - (resets_at - now)) / length).clamp(0.0, 1.0))
}

/// Where even spending would leave the bar: the share of the window left.
pub fn pace_remaining_percent(window: &UsageWindow, now: f64) -> Option<f64> {
    elapsed_share(window, now).map(|elapsed| js_round((1.0 - elapsed) * 100.0))
}

/// Spending compared with the time elapsed in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LimitPace {
    Ahead,
    On,
    Under,
}

/// The pace of one window, when it has a reset clock and a length.
pub fn pace_of(window: &UsageWindow, now: f64) -> Option<LimitPace> {
    elapsed_share(window, now).map(|elapsed| pace_of_shares(number(&window.used_percent), elapsed))
}

fn pace_of_shares(used_percent: f64, elapsed: f64) -> LimitPace {
    let gap = used_percent - elapsed * 100.0;
    if gap > 5.0 {
        LimitPace::Ahead
    } else if gap < -5.0 {
        LimitPace::Under
    } else {
        LimitPace::On
    }
}

/// Time until a window resets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ResetCountdown {
    Now,
    Momentarily,
    In { ms: f64 },
}

/// `resetCountdown`.
pub fn reset_countdown(window: &UsageWindow, now: f64) -> Option<ResetCountdown> {
    let resets_at = reset_millis(window)?;
    Some(if resets_at <= now {
        ResetCountdown::Now
    } else if resets_at - now < MINUTE {
        ResetCountdown::Momentarily
    } else {
        ResetCountdown::In { ms: resets_at - now }
    })
}

/// Enabled, installed, available providers that report limits.
pub fn providers_with_limits(providers: &[ServerConfigProvidersItem]) -> impl Iterator<Item = &ServerConfigProvidersItem> {
    providers.iter().filter(|provider| {
        provider.enabled
            && provider.installed
            && provider.availability != Some(ServerProviderAvailability::Unavailable)
            && provider.usage_limits.is_some()
    })
}

/// Why an account's limits are not shown, in the server's English, or `None`
/// when they are. The views word their own copy with the same conditions.
pub fn limits_notice_reason(limits: &UsageLimits) -> Option<LimitsNotice<'_>> {
    match &limits.unavailable {
        Some(unavailable) if unavailable.reason == UnavailableReason::Unsupported => {
            Some(LimitsNotice::Unsupported(unavailable.message.as_deref()))
        }
        Some(unavailable) if unavailable.reason == UnavailableReason::ProbeFailed => {
            Some(LimitsNotice::ProbeFailed(unavailable.message.as_deref()))
        }
        _ if limits.windows.is_empty() => Some(LimitsNotice::NoWindows),
        _ => None,
    }
}

/// Why limits are unavailable, with the server's message when it sent one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitsNotice<'a> {
    Unsupported(Option<&'a str>),
    ProbeFailed(Option<&'a str>),
    NoWindows,
}

/// Whether the provider is signed in through ChatGPT subscription sharing.
pub fn uses_chatgpt_sharing(provider: &ServerConfigProvidersItem) -> bool {
    provider.auth.status == ServerProviderAuthStatus::Authenticated && provider.auth.subscription_sharing == Some(true)
}

/// A provider's "Manage usage" page, with every account that links to it.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalUsageLink {
    pub label: String,
    pub url: String,
    pub message: Option<String>,
    pub accounts: Vec<String>,
}

/// `collectExternalUsageLinks`; `account_label(provider, environment)` names one account.
pub fn collect_external_usage_links(
    environments: &[LimitEnvironment],
    account_label: impl Fn(&str, &str) -> String,
) -> Vec<ExternalUsageLink> {
    let mut links: Vec<ExternalUsageLink> = Vec::new();
    for environment in environments {
        for provider in providers_with_limits(&environment.providers) {
            let Some(limits) = &provider.usage_limits else { continue };
            let Some(external) = &limits.external_usage else { continue };
            if provider.auth.status != ServerProviderAuthStatus::Authenticated {
                continue;
            }
            let account = account_label(provider.display_name.as_deref().unwrap_or(&provider.instance_id), &environment.label);
            let message = limits.unavailable.as_ref().and_then(|u| u.message.clone());
            match links.iter_mut().find(|link| link.url == external.url) {
                Some(link) => {
                    link.label = external.label.clone();
                    link.message = message;
                    if !link.accounts.contains(&account) {
                        link.accounts.push(account);
                    }
                }
                None => links.push(ExternalUsageLink {
                    label: external.label.clone(),
                    url: external.url.clone(),
                    message,
                    accounts: vec![account],
                }),
            }
        }
    }
    links
}

/// Where a reset credit is redeemed: the environment and the RPC input.
#[derive(Clone, Debug, PartialEq)]
pub struct RedeemTarget {
    pub environment_id: EnvironmentId,
    pub input: ProviderConsumeResetCreditInput,
}

/// One login, merged across every environment and hub that reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitAccount {
    pub key: String,
    pub driver: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub accent_color: Option<String>,
    pub environments: Vec<(EnvironmentId, String)>,
    pub source_label: Option<String>,
    pub redeem: Option<RedeemTarget>,
    pub limits: UsageLimits,
}

fn display_name_for_driver(driver: &str) -> Option<&'static str> {
    Some(match driver {
        "antigravity" => "Antigravity",
        "codex" => "Codex",
        "claudeAgent" => "Claude",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "fx" => "fx",
        "kiro" => "Kiro",
        "muse" => "Muse Code",
        "acpRegistry" => "ACP Registry",
        "pi" => "Pi",
        "opencode" => "OpenCode",
        _ => return None,
    })
}

struct AccountReport<'a> {
    driver: &'a str,
    email: Option<&'a str>,
    workspace_id: Option<&'a str>,
    limits: Option<&'a UsageLimits>,
}

fn account_key(driver: &str, email: Option<&str>, limits: Option<&UsageLimits>) -> Option<String> {
    let email = email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty());
    if let Some(email) = email {
        return Some(format!("{driver}:{email}"));
    }
    limits.and_then(|l| l.credential_fingerprint.as_ref()).map(|fp| format!("{driver}:credential:{fp}"))
}

/// Includes every enabled login, even unreadable workspaces.
fn account_key_resolver<'a>(reports: impl Iterator<Item = AccountReport<'a>>) -> impl Fn(&AccountReport) -> Option<String> {
    let mut workspaces: HashMap<String, Vec<String>> = HashMap::new();
    for report in reports {
        if let (Some(key), Some(workspace)) = (account_key(report.driver, report.email, report.limits), report.workspace_id) {
            let known = workspaces.entry(key).or_default();
            if !known.iter().any(|w| w == workspace) {
                known.push(workspace.to_owned());
            }
        }
    }
    move |report| {
        let key = account_key(report.driver, report.email, report.limits)?;
        if let Some(workspace) = report.workspace_id {
            return Some(format!("{key}:{workspace}"));
        }
        match workspaces.get(&key) {
            None => Some(key),
            Some(known) if known.len() == 1 => Some(format!("{key}:{}", known[0])),
            Some(_) => None,
        }
    }
}

fn normalize_hostname(host: &str) -> String {
    host.trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_lowercase()
}

fn is_local_loopback_host(host: &str) -> bool {
    let host = normalize_hostname(host);
    host == "localhost" || host == "::1" || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.octets()[0] == 127)
}

// Source ids are environment-local, so only a remote endpoint names the same hub across environments.
fn hub_identity(environment_id: &EnvironmentId, source: &ServerConfigUsageLimitSourcesItem) -> String {
    let host = source.endpoint.as_deref().and_then(|endpoint| {
        let rest = endpoint.strip_prefix("http://").or_else(|| endpoint.strip_prefix("https://"))?;
        let rest = rest.split(['/', '?', '#']).next()?;
        if rest.contains('@') {
            return None;
        }
        Some(if rest.starts_with('[') {
            rest.split_inclusive(']').next()?.to_owned()
        } else {
            rest.split(':').next()?.to_owned()
        })
    });
    let local = host.as_deref().is_none_or(|host| {
        let lower = host.to_lowercase();
        is_local_loopback_host(host)
            || matches!(host, "0.0.0.0" | "::" | "[::]")
            || lower.trim_end_matches('.').ends_with(".localhost")
            || lower.starts_with("[::ffff:127.")
            || lower.starts_with("[::ffff:7f")
    });
    match &source.endpoint {
        Some(endpoint) if !local => endpoint.clone(),
        _ => format!("{environment_id}:{}", source.id),
    }
}

fn checked_at(account: &LimitAccount) -> f64 {
    parse_instant_ms(&account.limits.checked_at).map_or(f64::NAN, |ms| ms as f64)
}

/// `collectLimitAccounts`.
pub fn collect_limit_accounts(environments: &[LimitEnvironment]) -> Vec<LimitAccount> {
    let mut order: Vec<String> = Vec::new();
    let mut accounts: HashMap<String, LimitAccount> = HashMap::new();
    let mut credit_sources: HashMap<String, LimitAccount> = HashMap::new();
    let mut hub_redeems: HashMap<String, LimitAccount> = HashMap::new();
    let mut plan_sources: HashMap<String, LimitAccount> = HashMap::new();

    let mut merge = |key: String, next: LimitAccount| {
        let newer = |map: &HashMap<String, LimitAccount>| map.get(&key).is_none_or(|prev| checked_at(&next) > checked_at(prev));
        // Prefer hub redemption to clear its routing cooldown too.
        let is_hub = matches!(&next.redeem, Some(RedeemTarget { input: ProviderConsumeResetCreditInput::UsageLimitSource(_), .. }));
        if is_hub && newer(&hub_redeems) {
            hub_redeems.insert(key.clone(), next.clone());
        }
        if next.limits.reset_credits.is_some() && newer(&credit_sources) {
            credit_sources.insert(key.clone(), next.clone());
        }
        if next.plan.is_some() && newer(&plan_sources) {
            plan_sources.insert(key.clone(), next.clone());
        }
        let Some(previous) = accounts.get(&key).cloned() else {
            order.push(key.clone());
            accounts.insert(key, next);
            return;
        };
        let fresher = checked_at(&next) > checked_at(&previous);
        let mut environments = previous.environments.clone();
        for candidate in &next.environments {
            if !environments.iter().any(|(id, _)| *id == candidate.0) {
                environments.push(candidate.clone());
            }
        }
        let winner = if fresher { &next } else { &previous };
        let credit_source = credit_sources.get(&key);
        // A hub e-mail names the account better than the driver's default instance name.
        let hub_email = [&previous, &next].iter().any(|a| a.environments.is_empty() && a.email.is_some());
        let display_name = previous.display_name.clone().or(next.display_name.clone());
        let display_name = match (&display_name, display_name_for_driver(&previous.driver)) {
            (Some(name), Some(default)) if hub_email && name.to_lowercase() == default.to_lowercase() => None,
            _ => display_name,
        };
        let redeem = match hub_redeems.get(&key) {
            Some(hub) => hub.redeem.clone(),
            None => match credit_source {
                Some(source) => source.redeem.clone(),
                None => winner.redeem.clone().or(previous.redeem.clone()).or(next.redeem.clone()),
            },
        };
        let mut limits = winner.limits.clone();
        limits.reset_credits = credit_source.and_then(|s| s.limits.reset_credits.clone());
        let source_label =
            if environments.is_empty() { previous.source_label.clone().or(next.source_label.clone()) } else { None };
        let merged = LimitAccount {
            display_name,
            plan: plan_sources.get(&key).and_then(|s| s.plan.clone()).or(previous.plan.clone()).or(next.plan.clone()),
            accent_color: previous.accent_color.clone().or(next.accent_color.clone()),
            environments,
            source_label,
            redeem,
            limits,
            ..previous
        };
        accounts.insert(key, merged);
    };

    let reports = environments.iter().flat_map(|environment| {
        let native = environment.providers.iter().filter(|p| p.enabled).map(|p| AccountReport {
            driver: &p.driver,
            email: p.auth.email.as_deref(),
            workspace_id: p.auth.workspace_id.as_deref(),
            limits: p.usage_limits.as_ref(),
        });
        let hub = environment.usage_limit_sources.iter().flat_map(|source| source.accounts.iter()).map(|a| AccountReport {
            driver: &a.driver,
            email: a.email.as_deref(),
            workspace_id: a.workspace_id.as_deref(),
            limits: Some(&a.usage_limits),
        });
        native.chain(hub).collect::<Vec<_>>()
    });
    let resolve_key = account_key_resolver(reports);

    for environment in environments {
        for provider in providers_with_limits(&environment.providers) {
            let Some(limits) = &provider.usage_limits else { continue };
            if limits_notice_reason(limits).is_some() {
                continue;
            }
            let report = AccountReport {
                driver: &provider.driver,
                email: provider.auth.email.as_deref(),
                workspace_id: provider.auth.workspace_id.as_deref(),
                limits: Some(limits),
            };
            let local_key = format!("{}:{}", environment.environment_id, provider.instance_id);
            merge(
                resolve_key(&report).unwrap_or_else(|| local_key.clone()),
                LimitAccount {
                    key: local_key,
                    driver: provider.driver.clone(),
                    display_name: provider.display_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned),
                    email: provider.auth.email.clone(),
                    plan: provider.auth.label.clone(),
                    accent_color: provider.accent_color.clone(),
                    environments: vec![(environment.environment_id.clone(), environment.label.clone())],
                    source_label: None,
                    redeem: Some(RedeemTarget {
                        environment_id: environment.environment_id.clone(),
                        input: ProviderConsumeResetCreditInput::NoUndefined(ProviderConsumeResetCreditInputNoUndefined {
                            instance_id: ProviderInstanceId::new(provider.instance_id.as_str()),
                            credit_id: None,
                        }),
                    }),
                    limits: limits.clone(),
                },
            );
        }
    }
    let label_environment = environments.len() > 1;
    for environment in environments {
        for source in environment.usage_limit_sources.iter() {
            let source_label =
                if label_environment { format!("{} · {}", environment.label, source.label) } else { source.label.clone() };
            for account in &source.accounts {
                if limits_notice_reason(&account.usage_limits).is_some() {
                    continue;
                }
                let resolved = resolve_key(&AccountReport {
                    driver: &account.driver,
                    email: account.email.as_deref(),
                    workspace_id: account.workspace_id.as_deref(),
                    limits: Some(&account.usage_limits),
                });
                let hub = if resolved.is_some() {
                    format!("{}:{}", environment.environment_id, source.id)
                } else {
                    hub_identity(&environment.environment_id, source)
                };
                let key = match &account.workspace_id {
                    Some(workspace) => format!("{hub}:{}:{workspace}", account.id),
                    None => format!("{hub}:{}", account.id),
                };
                let redeem = account.usage_limits.reset_credits.as_ref().and_then(|c| c.next_credit_id.clone()).map(|credit_id| {
                    RedeemTarget {
                        environment_id: environment.environment_id.clone(),
                        input: ProviderConsumeResetCreditInput::UsageLimitSource(UsageLimitSourceConsumeResetCreditInput {
                            source_id: source.id.clone(),
                            account_id: account.id.clone(),
                            credit_id,
                        }),
                    }
                });
                let hub_name = account.id.strip_suffix(".json").or_else(|| account.id.strip_suffix(".JSON")).unwrap_or(&account.id);
                merge(
                    resolved.unwrap_or_else(|| key.clone()),
                    LimitAccount {
                        key,
                        driver: account.driver.clone(),
                        display_name: if account.email.is_some() { None } else { Some(hub_name.to_owned()) },
                        email: account.email.clone(),
                        plan: account.plan.clone(),
                        accent_color: None,
                        environments: Vec::new(),
                        source_label: Some(source_label.clone()),
                        redeem,
                        limits: account.usage_limits.clone(),
                    },
                );
            }
        }
    }
    order.into_iter().filter_map(|key| accounts.remove(&key)).collect()
}

/// One account's reading of one pooled window. `window` is the reading as
/// displayed, which hides a session window while the weekly one is spent.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitPoolMember {
    pub account: Arc<LimitAccount>,
    pub window: UsageWindow,
    pub refill_percent: f64,
}

/// The next time a member resets and how much of the pool that restores.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolReset {
    pub member: LimitPoolMember,
    pub at: f64,
    pub restores_percent: f64,
}

/// One window kind pooled across a driver's accounts.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitPoolWindow {
    pub id: String,
    pub kind: WindowKind,
    pub label: String,
    pub members: Vec<LimitPoolMember>,
    /// One column per account in pool order; `None` where the account lacks this window.
    pub columns: Vec<(Arc<LimitAccount>, Option<LimitPoolMember>)>,
    pub remaining_percent: f64,
    pub used_percent: f64,
    pub pace: Option<LimitPace>,
    pub resets: Vec<PoolReset>,
}

/// Every account of one driver and their pooled windows.
#[derive(Clone, Debug, PartialEq)]
pub struct LimitPool {
    pub driver: String,
    pub accounts: Vec<Arc<LimitAccount>>,
    pub windows: Vec<LimitPoolWindow>,
}

fn window_kind_order(kind: WindowKind) -> u8 {
    match kind {
        WindowKind::Session => 0,
        WindowKind::Weekly => 1,
        WindowKind::Monthly => 2,
        WindowKind::Other => 3,
    }
}

fn plan_capacity(account: &LimitAccount, kind: WindowKind) -> Option<f64> {
    let plan = account.plan.as_deref()?;
    match account.driver.as_str() {
        "codex" if matches!(kind, WindowKind::Session | WindowKind::Weekly) => match plan {
            "ChatGPT Plus Subscription" => Some(1.0),
            "ChatGPT Pro 100 Subscription" => Some(5.0),
            "ChatGPT Pro 200 Subscription" => Some(20.0),
            _ => None,
        },
        // Claude's published plan multipliers describe the session allowance only.
        "claudeAgent" if kind == WindowKind::Session => match plan {
            "Pro" => Some(1.0),
            "Max" => Some(5.0),
            "Ultra" => Some(20.0),
            _ => None,
        },
        _ => None,
    }
}

// Codex ids name slots, so pool by kind; other drivers' ids identify distinct allowances.
fn pool_key(driver: &str, window: &UsageWindow) -> String {
    if driver == "codex" { window.kind.as_str().to_owned() } else { format!("{}:{}", window.kind.as_str(), window.id) }
}

fn pool_member(account: &Arc<LimitAccount>, reported: &UsageWindow, now: f64) -> LimitPoolMember {
    let spent_weekly = (reported.kind == WindowKind::Session)
        .then(|| {
            account.limits.windows.iter().find(|window| {
                number(&window.used_percent) >= 100.0
                    && match account.driver.as_str() {
                        "claudeAgent" => window.id == "seven_day",
                        "codex" => window.kind == WindowKind::Weekly,
                        _ => false,
                    }
            })
        })
        .flatten();
    let Some(spent_weekly) = spent_weekly else {
        return LimitPoolMember { account: account.clone(), window: reported.clone(), refill_percent: number(&reported.used_percent) };
    };
    let weekly_reset = reset_millis(spent_weekly);
    let session_reset = reset_millis(reported);
    let cutoff = now.max(parse_instant_ms(&account.limits.checked_at).map_or(f64::NAN, |ms| ms as f64));
    let (mut resets_at, mut refill) = (None, 0.0);
    // A stale or missing clock cannot tell when blocked quota becomes usable.
    if let (Some(weekly), Some(session)) = (weekly_reset, session_reset)
        && weekly > cutoff
        && session > cutoff
    {
        let used = number(&reported.used_percent);
        if session <= weekly {
            (resets_at, refill) = (spent_weekly.resets_at.clone(), 100.0);
        } else if used >= 100.0 {
            (resets_at, refill) = (reported.resets_at.clone(), 100.0);
        } else {
            (resets_at, refill) = (spent_weekly.resets_at.clone(), 100.0 - used.clamp(0.0, 100.0));
        }
    }
    let mut window = reported.clone();
    window.used_percent = t3code_contracts::SidebarAutoSettleAfterDays::Number(100.0);
    window.resets_at = resets_at;
    // Suppression is not session spending, so it has no session pace clock.
    window.window_duration_mins = None;
    LimitPoolMember { account: account.clone(), window, refill_percent: refill }
}

fn pool_windows(accounts: &[Arc<LimitAccount>], now: f64) -> Vec<LimitPoolWindow> {
    let mut keys: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, Vec<LimitPoolMember>> = HashMap::new();
    for account in accounts {
        for reported in &account.limits.windows {
            let member = pool_member(account, reported, now);
            let pooled = pool_key(&account.driver, &member.window);
            let taken = by_key.get(&pooled).is_some_and(|list| list.iter().any(|m| Arc::ptr_eq(&m.account, account)));
            let key = if taken { format!("{}:{}", member.window.kind.as_str(), member.window.id) } else { pooled };
            if !by_key.contains_key(&key) {
                keys.push(key.clone());
            }
            by_key.entry(key).or_default().push(member);
        }
    }
    let mut pools: Vec<LimitPoolWindow> = keys
        .into_iter()
        .map(|key| {
            let members = by_key.remove(&key).unwrap_or_default();
            let first = members[0].window.clone();
            let id = if members[0].account.driver == "codex" { key } else { first.id.clone() };
            // One unknown plan makes the pool incomparable by plan, so every member then counts once.
            let capacities: Vec<Option<f64>> = members.iter().map(|m| plan_capacity(&m.account, first.kind)).collect();
            let weights: Vec<f64> = if capacities.iter().all(Option::is_some) {
                capacities.into_iter().flatten().collect()
            } else {
                vec![1.0; members.len()]
            };
            let total_weight: f64 = weights.iter().sum();
            let used: f64 =
                members.iter().zip(&weights).map(|(m, w)| number(&m.window.used_percent) * w).sum::<f64>() / total_weight;
            // Judge pace only for windows with reset clocks.
            let timed: Vec<(f64, f64, f64)> = members
                .iter()
                .zip(&weights)
                .filter_map(|(m, w)| elapsed_share(&m.window, now).map(|e| (number(&m.window.used_percent), e, *w)))
                .collect();
            let timed_weight: f64 = timed.iter().map(|t| t.2).sum();
            let timed_used = timed.iter().map(|t| t.0 * t.2).sum::<f64>() / timed_weight;
            let mean_elapsed = (!timed.is_empty()).then(|| timed.iter().map(|t| t.1 * t.2).sum::<f64>() / timed_weight);
            let mut resets: Vec<PoolReset> = members
                .iter()
                .zip(&weights)
                .filter_map(|(member, weight)| {
                    reset_millis(&member.window).map(|at| PoolReset {
                        member: member.clone(),
                        at,
                        restores_percent: js_round(member.refill_percent * weight / total_weight),
                    })
                })
                .collect();
            resets.sort_by(|a, b| a.at.total_cmp(&b.at));
            let columns = accounts
                .iter()
                .map(|account| (account.clone(), members.iter().find(|m| m.account.key == account.key).cloned()))
                .collect();
            LimitPoolWindow {
                id,
                kind: first.kind,
                label: first.label.clone(),
                columns,
                used_percent: js_round(used),
                remaining_percent: js_round(100.0 - used),
                pace: mean_elapsed.map(|elapsed| pace_of_shares(timed_used, elapsed)),
                resets,
                members,
            }
        })
        .collect();
    pools.sort_by_key(|pool| window_kind_order(pool.kind));
    pools
}

/// `Math.round`: halves round toward positive infinity.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `collectLimitPools`: accounts grouped by driver, each pool's accounts in
/// order of their soonest reset.
pub fn collect_limit_pools(accounts: Vec<LimitAccount>, now: f64) -> Vec<LimitPool> {
    let mut drivers: Vec<String> = Vec::new();
    let mut by_driver: HashMap<String, Vec<Arc<LimitAccount>>> = HashMap::new();
    for account in accounts {
        if !by_driver.contains_key(&account.driver) {
            drivers.push(account.driver.clone());
        }
        by_driver.entry(account.driver.clone()).or_default().push(Arc::new(account));
    }
    drivers
        .into_iter()
        .map(|driver| {
            let mut members = by_driver.remove(&driver).unwrap_or_default();
            let order_window = members
                .iter()
                .flat_map(|account| account.limits.windows.iter())
                .enumerate()
                .min_by_key(|(index, window)| (window_kind_order(window.kind), *index))
                .map(|(_, window)| window.clone());
            let order_reset = |account: &Arc<LimitAccount>| {
                order_window
                    .as_ref()
                    .and_then(|order| account.limits.windows.iter().find(|w| pool_key(&driver, w) == pool_key(&driver, order)))
                    .and_then(|window| reset_millis(&pool_member(account, window, now).window))
                    .unwrap_or(f64::INFINITY)
            };
            let sort_name = |a: &LimitAccount| a.display_name.as_ref().or(a.email.as_ref()).unwrap_or(&a.key).to_lowercase();
            members.sort_by(|left, right| {
                order_reset(left)
                    .total_cmp(&order_reset(right))
                    .then_with(|| sort_name(left).cmp(&sort_name(right)))
                    .then_with(|| left.key.cmp(&right.key))
            });
            let windows = pool_windows(&members, now);
            LimitPool { driver, accounts: members, windows }
        })
        .collect()
}

fn has_duplicate_cursor_total(pool: &LimitPool) -> bool {
    if pool.windows.len() != 2 {
        return false;
    }
    let total = pool.windows.iter().find(|w| w.id == "totalPercentUsed");
    let auto = pool.windows.iter().find(|w| w.id == "autoPercentUsed");
    let (Some(total), Some(auto)) = (total, auto) else { return false };
    total.members.len() == auto.members.len()
        && total.members.iter().all(|member| {
            member.account.limits.windows.iter().any(|candidate| {
                candidate.id == auto.id
                    && candidate.kind == auto.kind
                    && candidate.used_percent == member.window.used_percent
                    && candidate.resets_at == member.window.resets_at
            })
        })
}

const CURSOR_WINDOW_IDS: [&str; 3] = ["totalPercentUsed", "autoPercentUsed", "apiPercentUsed"];

fn cursor_window_rank(id: &str) -> usize {
    CURSOR_WINDOW_IDS.iter().position(|known| *known == id).unwrap_or(CURSOR_WINDOW_IDS.len())
}

/// Whether the Cursor window `id` drops its label and description because it
/// is the only or a duplicated total. `None` for ids Cursor does not name.
pub fn cursor_window_hides_details(id: &str, pool: &LimitPool) -> Option<bool> {
    CURSOR_WINDOW_IDS
        .contains(&id)
        .then(|| (pool.windows.len() == 1 && id == "totalPercentUsed") || has_duplicate_cursor_total(pool))
}

/// `displayLimitWindows`: the windows a pool shows, in display order.
pub fn display_limit_windows(pool: &LimitPool) -> Vec<&LimitPoolWindow> {
    let mut windows: Vec<&LimitPoolWindow> = pool.windows.iter().collect();
    match pool.driver.as_str() {
        "antigravity" => {
            let family = |id: &str| {
                ["-5h", "-weekly", "-quota"].iter().find_map(|suffix| id.strip_suffix(suffix)).unwrap_or(id).to_owned()
            };
            windows.sort_by(|left, right| {
                (!left.id.starts_with("gemini-"))
                    .cmp(&!right.id.starts_with("gemini-"))
                    .then_with(|| family(&left.id).cmp(&family(&right.id)))
                    .then_with(|| window_kind_order(left.kind).cmp(&window_kind_order(right.kind)))
            });
            windows
        }
        "cursor" => {
            let has = |id: &str| pool.windows.iter().any(|w| w.id == id);
            let hide_total = (has("autoPercentUsed") && has("apiPercentUsed")) || has_duplicate_cursor_total(pool);
            windows.retain(|w| !hide_total || w.id != "totalPercentUsed");
            windows.sort_by_key(|w| cursor_window_rank(&w.id));
            windows
        }
        _ => windows,
    }
}
