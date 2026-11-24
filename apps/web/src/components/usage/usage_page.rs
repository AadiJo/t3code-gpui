//! The Usage page. Ports `UsagePage.tsx` and `UsageModelDialog.tsx`, with the
//! selection and loading rules of `state/usage.ts` (`useUsage`) and
//! `usageProgress.ts`: cost or tokens over a window with a chart, totals,
//! share bars and a breakdown, or subscription limits.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use jiff::tz::TimeZone;
use t3code_contracts::{EnvironmentId, UsageProviderKind, UsageResolution, UsageSummary, UsageSummaryInput};
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand};
use t3code_shared::usage_format::{enumerate_days, enumerate_hour_starts, make_window, usage_source_messages};
use t3code_shared::usage_merge::{
    EnvironmentUsage, MergedUsage, ModelTotals, USAGE_CONTRACT_VERSION, is_compatible_usage_contract_version, merge_usage, number,
};

use super::usage_breakdown::{
    cache_hit_rate, cost_per_million_tokens, cost_type_segments, model_share, sort_models_by_tokens, speed_cost_segments, token_type_segments,
};
use super::usage_format::{format_date_time_short, format_day_short, format_hour_short};
use super::usage_limits::UsageLimitsView;
use super::usage_provider_chart::{ChartInput, ChartResolution, UsageProviderChart};
use super::usage_providers::{PROVIDER_ORDER, provider_color, provider_label, provider_mark, providers_with_usage, rank_providers_by_metric};
use super::usage_share_bar::UsageShareBar;
use super::usage_shortcuts::{UsageMetric, UsagePagePreferences, UsageWindowDays};
use super::usage_source::{UsageEnvironment, UsageSource};
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use super::usage_price_overrides::{UsagePriceOverrides, UsagePriceOverridesClosed, price_targets, usage_models};
use super::usage_price_table::UsagePriceTarget;
use crate::components::ui::scroll_area::scroll_area;
use crate::components::ui::skeleton::{Skeleton, SkeletonShape};
use crate::components::ui::toggle::{Toggle, ToggleSize, ToggleVariant};
use crate::components::ui::tooltip::tooltip;
use crate::i18n::locales::Locale;
use crate::i18n::messages::active_locale;
use crate::i18n::number_format::{format_count, format_percent, format_tokens, format_usd};
use crate::i18n::{t, t_rich};
use crate::keybindings::WhenKey;
use crate::shortcuts::commands::on_commands;
use crate::shortcuts::keymap::ShortcutState;
use crate::theme_color::with_alpha;
use crate::timestamp_format::{TimestampFormat, resolve_hour12};

/// Limits refresh themselves at most this often when the page opens.
const AUTO_LIMITS_REFRESH_MS: i64 = 5 * 60_000;

/// The page asks its owner to close it.
#[derive(Clone, Debug)]
pub enum UsagePageEvent {
    Close,
}

/// One environment's answer for the current window.
#[derive(Clone, Default)]
struct Fetch {
    /// The window this answer belongs to.
    window: Option<Arc<UsageSummaryInput>>,
    summary: Option<Arc<UsageSummary>>,
    pending: bool,
    failed: bool,
}

/// One environment's usage state (`EnvironmentUsageStatus`).
#[derive(Clone)]
struct EnvironmentStatus {
    environment: UsageEnvironment,
    is_pending: bool,
    can_read_diagnostics: bool,
    error: Option<SharedString>,
    summary: Option<Arc<UsageSummary>>,
}

/// `usageEnvironmentProgress`.
#[derive(Clone, Debug, PartialEq)]
enum Progress {
    Inactive,
    Loading,
    Stale,
    Ready,
    Partway(Vec<UsageProviderKind>),
}

fn progress(status: &EnvironmentStatus, refreshing: bool) -> Progress {
    if !status.environment.is_connected || status.error.is_some() {
        return Progress::Inactive;
    }
    let Some(summary) = &status.summary else { return Progress::Loading };
    if refreshing {
        return Progress::Stale;
    }
    if !status.is_pending {
        return Progress::Ready;
    }
    let mut providers: Vec<UsageProviderKind> = Vec::new();
    for source in &summary.sources {
        if source.refreshing.is_some() && !providers.contains(&source.fingerprint.provider) {
            providers.push(source.fingerprint.provider);
        }
    }
    if providers.is_empty() { Progress::Stale } else { Progress::Partway(providers) }
}

/// Which figures are still loading.
#[derive(Clone, Default)]
struct Loading {
    partial: bool,
    every_provider: bool,
    providers: Vec<UsageProviderKind>,
}

impl Loading {
    fn has(&self, provider: UsageProviderKind) -> bool {
        self.every_provider || self.providers.contains(&provider)
    }
}

/// What the overview shows, derived once per change.
struct Overview {
    merged: Rc<MergedUsage>,
    window: Arc<UsageSummaryInput>,
    loading: Loading,
    active_providers: Vec<UsageProviderKind>,
    ranked_providers: Vec<UsageProviderKind>,
    providers_with_data: Vec<UsageProviderKind>,
    source_messages: Vec<String>,
    periods: Vec<String>,
}

/// The Usage page view.
pub struct UsagePage {
    source: Rc<dyn UsageSource>,
    prefs: UsagePagePreferences,
    prefs_path: Option<PathBuf>,
    zone: TimeZone,
    window: Arc<UsageSummaryInput>,
    environments: Arc<[UsageEnvironment]>,
    selected: Option<BTreeSet<EnvironmentId>>,
    fetches: HashMap<EnvironmentId, Fetch>,
    /// The last answered totals, kept while a new window loads.
    kept: Option<(Arc<UsageSummaryInput>, Rc<MergedUsage>)>,
    statuses: Vec<EnvironmentStatus>,
    overview: Option<Overview>,
    refreshing: bool,
    limits_now: f64,
    limits_refreshed: HashMap<EnvironmentId, i64>,
    breakdown_by_time: bool,
    selected_model: Option<(UsageProviderKind, String)>,
    /// The open "Custom model prices" dialog.
    price_dialog: Option<(Entity<UsagePriceOverrides>, Subscription)>,
    chart: Entity<UsageProviderChart>,
    model_chart: Entity<UsageProviderChart>,
    limits: Entity<UsageLimitsView>,
    focus: FocusHandle,
    hour12: bool,
    locale: Locale,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<UsagePageEvent> for UsagePage {}

impl Focusable for UsagePage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn window_for(days: UsageWindowDays, now: i64, zone: &TimeZone, zone_name: &str) -> Arc<UsageSummaryInput> {
    let resolution = if days == UsageWindowDays::Day { UsageResolution::Hour } else { UsageResolution::Day };
    Arc::new(make_window(days.days(), now, zone, zone_name, resolution))
}

impl UsagePage {
    /// Opens the page over `source`. `prefs_path` keeps the metric, period and
    /// hidden providers between launches; `None` keeps them for this page only.
    pub fn new(source: Rc<dyn UsageSource>, prefs_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = prefs_path.as_deref().map(UsagePagePreferences::read).unwrap_or_default();
        let (zone, zone_name) = source.time_zone();
        let now = source.now_ms();
        let usage_window = window_for(prefs.window_days, now, &zone, &zone_name);
        let environments = source.environments();
        let initial = environments.borrow().clone();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        if cx.has_global::<ShortcutState>() {
            ShortcutState::set(WhenKey::UsagePageOpen, true, cx);
        }
        let release = cx.on_release(|_, cx| {
            if cx.has_global::<ShortcutState>() {
                ShortcutState::set(WhenKey::UsagePageOpen, false, cx);
            }
        });
        cx.spawn(async move |this, cx| {
            let mut environments = environments;
            while environments.changed().await.is_ok() {
                let next = environments.borrow_and_update().clone();
                if this.update(cx, |this, cx| this.set_environments(next, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let limits = cx.new(|_| UsageLimitsView::new(source.clone()));
        let mut this = Self {
            source,
            prefs,
            prefs_path,
            zone,
            window: usage_window,
            environments: Arc::from(Vec::new()),
            selected: None,
            fetches: HashMap::new(),
            kept: None,
            statuses: Vec::new(),
            overview: None,
            refreshing: false,
            limits_now: now as f64,
            limits_refreshed: HashMap::new(),
            breakdown_by_time: false,
            selected_model: None,
            price_dialog: None,
            chart: cx.new(|_| UsageProviderChart::new()),
            model_chart: cx.new(|_| UsageProviderChart::new()),
            limits,
            focus,
            hour12: resolve_hour12(TimestampFormat::Locale),
            locale: active_locale(),
            _subscriptions: vec![release],
        };
        this.set_environments(initial, cx);
        this
    }

    fn set_environments(&mut self, environments: Arc<[UsageEnvironment]>, cx: &mut Context<Self>) {
        self.environments = environments;
        self.fetch_missing(cx);
        if self.prefs.metric == UsageMetric::Limits {
            self.refresh_limits(true, cx);
        }
        self.recompute(cx);
    }

    fn save_prefs(&self) {
        if let Some(path) = &self.prefs_path {
            self.prefs.save(path);
        }
    }

    fn selected_environments(&self) -> impl Iterator<Item = &UsageEnvironment> {
        self.environments.iter().filter(|e| self.selected.as_ref().is_none_or(|s| s.contains(&e.environment_id)))
    }

    /// Asks every readable environment without an answer for this window.
    fn fetch_missing(&mut self, cx: &mut Context<Self>) {
        let window = self.window.clone();
        let wanted: Vec<UsageEnvironment> = self
            .environments
            .iter()
            .filter(|e| e.access.can_read_diagnostics)
            .filter(|e| self.fetches.get(&e.environment_id).is_none_or(|f| !f.window.as_ref().is_some_and(|w| Arc::ptr_eq(w, &window))))
            .cloned()
            .collect();
        for environment in wanted {
            self.fetch(environment, cx);
        }
    }

    fn fetch(&mut self, environment: UsageEnvironment, cx: &mut Context<Self>) {
        let window = self.window.clone();
        let id = environment.environment_id.clone();
        let fetch = self.fetches.entry(id.clone()).or_default();
        if !fetch.window.as_ref().is_some_and(|w| Arc::ptr_eq(w, &window)) {
            fetch.summary = None;
        }
        fetch.window = Some(window.clone());
        fetch.pending = true;
        fetch.failed = false;
        let mut input = (*window).clone();
        input.model_filter = environment.model_filter.clone();
        let call = self.source.usage_summary(&id, input);
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                let Some(fetch) = this.fetches.get_mut(&id) else { return };
                if !fetch.window.as_ref().is_some_and(|w| Arc::ptr_eq(w, &window)) {
                    return;
                }
                fetch.pending = false;
                match result {
                    Ok(summary) => fetch.summary = Some(summary),
                    Err(_) => fetch.failed = true,
                }
                this.recompute(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Shows `metric` over `days`, as the toggles do. Scenes use it.
    pub fn show(&mut self, metric: UsageMetric, days: UsageWindowDays, cx: &mut Context<Self>) {
        self.select_window(days, cx);
        self.select_metric(metric, cx);
    }

    /// Opens a model's detail dialog. Scenes use it.
    pub fn show_model(&mut self, provider: UsageProviderKind, model: &str, cx: &mut Context<Self>) {
        self.open_model(provider, model.to_owned(), cx);
    }

    /// Shows the breakdown by day or hour instead of by model.
    pub fn show_breakdown_by_time(&mut self, by_time: bool, cx: &mut Context<Self>) {
        self.breakdown_by_time = by_time;
        cx.notify();
    }

    fn select_metric(&mut self, metric: UsageMetric, cx: &mut Context<Self>) {
        if metric == UsageMetric::Limits {
            self.limits_now = self.source.now_ms() as f64;
        }
        self.prefs.metric = metric;
        self.save_prefs();
        if metric == UsageMetric::Limits {
            self.refresh_limits(true, cx);
        }
        self.recompute(cx);
    }

    fn select_window(&mut self, days: UsageWindowDays, cx: &mut Context<Self>) {
        self.prefs.window_days = days;
        self.save_prefs();
        let (_, zone_name) = self.source.time_zone();
        self.window = window_for(days, self.source.now_ms(), &self.zone, &zone_name);
        self.fetch_missing(cx);
        self.recompute(cx);
    }

    fn run_command(&mut self, command: StaticCommand, cx: &mut Context<Self>) {
        if let Some(metric) = UsageMetric::ALL.into_iter().find(|m| m.command() == command) {
            self.select_metric(metric, cx);
        } else if let Some(days) = UsageWindowDays::ALL.into_iter().find(|d| d.command() == command)
            && self.prefs.metric != UsageMetric::Limits
        {
            self.select_window(days, cx);
        }
    }

    /// `refreshUsageLimits`: `server.refreshProviders` on each selected,
    /// connected environment; automatic refreshes skip one refreshed in the last 5 minutes.
    fn refresh_limits(&mut self, automatic: bool, cx: &mut Context<Self>) {
        let now = self.source.now_ms();
        let targets: Vec<EnvironmentId> = self
            .selected_environments()
            .filter(|e| e.is_connected)
            .map(|e| e.environment_id.clone())
            .filter(|id| !automatic || self.limits_refreshed.get(id).is_none_or(|at| now - at >= AUTO_LIMITS_REFRESH_MS))
            .collect();
        if targets.is_empty() {
            return;
        }
        for id in &targets {
            self.limits_refreshed.insert(id.clone(), now);
        }
        let calls: Vec<_> = targets.iter().map(|id| self.source.refresh_providers(id)).collect();
        if !automatic {
            self.refreshing = true;
        }
        cx.spawn(async move |this, cx| {
            futures::future::join_all(calls).await;
            this.update(cx, |this, cx| {
                this.limits_now = this.source.now_ms() as f64;
                if !automatic {
                    this.refreshing = false;
                }
                this.recompute(cx);
            })
            .ok();
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        if self.prefs.metric == UsageMetric::Limits {
            self.refresh_limits(false, cx);
            self.recompute(cx);
            return;
        }
        let (_, zone_name) = self.source.time_zone();
        let next = window_for(self.prefs.window_days, self.source.now_ms(), &self.zone, &zone_name);
        if *next != *self.window {
            self.window = next;
        }
        let targets: Vec<UsageEnvironment> = self.selected_environments().filter(|e| e.access.can_read_diagnostics).cloned().collect();
        self.refreshing = true;
        let rates: Vec<_> = targets.iter().map(|e| self.source.refresh_usage_rates(&e.environment_id)).collect();
        cx.spawn(async move |this, cx| {
            futures::future::join_all(rates).await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                for environment in targets {
                    this.fetch(environment, cx);
                }
                this.recompute(cx);
            })
            .ok();
        })
        .detach();
        self.recompute(cx);
    }

    /// Rebuilds statuses, merged totals and the chart input after any change.
    fn recompute(&mut self, cx: &mut Context<Self>) {
        self.recompute_overview(cx);
        if let Some((dialog, _)) = &self.price_dialog {
            let (targets, models) = self.price_inputs();
            dialog.update(cx, |dialog, cx| dialog.set_environments(targets, models, cx));
        }
    }

    fn recompute_overview(&mut self, cx: &mut Context<Self>) {
        let window = self.window.clone();
        self.statuses = self
            .environments
            .iter()
            .map(|environment| {
                if !environment.access.can_read_diagnostics {
                    return EnvironmentStatus {
                        environment: environment.clone(),
                        is_pending: environment.access.is_pending,
                        can_read_diagnostics: false,
                        error: environment.access.error.clone(),
                        summary: None,
                    };
                }
                let fetch = self.fetches.get(&environment.environment_id).filter(|f| f.window.as_ref().is_some_and(|w| Arc::ptr_eq(w, &window)));
                EnvironmentStatus {
                    environment: environment.clone(),
                    is_pending: fetch.is_none_or(|f| f.pending),
                    can_read_diagnostics: true,
                    error: fetch.filter(|f| f.failed).map(|_| t!("This environment could not report usage.")),
                    summary: fetch.and_then(|f| f.summary.clone()),
                }
            })
            .collect();
        let hidden = &self.prefs.hidden_providers;
        let selected: Vec<&EnvironmentStatus> =
            self.statuses.iter().filter(|s| self.selected.as_ref().is_none_or(|ids| ids.contains(&s.environment.environment_id))).collect();
        let answered: Vec<EnvironmentUsage> = selected
            .iter()
            .filter_map(|status| {
                let summary = status.summary.as_ref()?;
                let summary = if hidden.is_empty() {
                    summary.clone()
                } else {
                    let mut filtered = (**summary).clone();
                    filtered.buckets.retain(|b| !hidden.contains(&b.provider));
                    filtered.sources.retain(|s| !hidden.contains(&s.fingerprint.provider));
                    Arc::new(filtered)
                };
                Some(EnvironmentUsage { environment_id: status.environment.environment_id.clone(), label: status.environment.label.to_string(), summary })
            })
            .collect();
        let merged = Rc::new(merge_usage(&answered, USAGE_CONTRACT_VERSION));
        let answered_count = answered.len();
        let still_reporting = selected.iter().filter(|s| s.summary.is_none() && s.error.is_none()).count();
        let is_pending = answered_count == 0 && still_reporting > 0;
        if answered_count > 0 {
            self.kept = Some((window.clone(), merged.clone()));
        }
        let shown = if answered_count > 0 || !is_pending {
            Some((window.clone(), merged.clone()))
        } else {
            self.kept.clone()
        };
        let refreshing_usage = self.refreshing && self.prefs.metric != UsageMetric::Limits;
        self.overview = shown.map(|(shown_window, merged)| {
            let showing_kept = !Arc::ptr_eq(&shown_window, &window);
            let loading = if showing_kept {
                Loading { partial: true, every_provider: true, providers: Vec::new() }
            } else {
                let mut loading = Loading::default();
                for status in &selected {
                    match progress(status, refreshing_usage) {
                        Progress::Loading | Progress::Stale => loading.every_provider = true,
                        Progress::Partway(providers) => {
                            for provider in providers {
                                if !hidden.contains(&provider) && !loading.providers.contains(&provider) {
                                    loading.providers.push(provider);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                loading.partial = loading.every_provider || !loading.providers.is_empty();
                loading
            };
            let providers_with_data = providers_with_usage(&merged.providers);
            let active_providers: Vec<UsageProviderKind> =
                PROVIDER_ORDER.into_iter().filter(|p| providers_with_data.contains(p) || loading.providers.contains(p)).collect();
            let by_cost = self.prefs.metric != UsageMetric::Tokens;
            let ranked_providers = rank_providers_by_metric(&active_providers, &merged.providers, by_cost);
            let sources: Vec<_> = selected
                .iter()
                .filter_map(|s| s.summary.as_ref())
                .flat_map(|summary| summary.sources.iter())
                .filter(|source| !hidden.contains(&source.fingerprint.provider))
                .collect();
            let source_messages = usage_source_messages(&sources, |count| {
                t!(
                    "{count, plural, one {{count} malformed or inconsistent usage record(s) were skipped; usage may be incomplete.} other {{count} malformed or inconsistent usage record(s) were skipped; usage may be incomplete.}}",
                    count = count
                )
                .to_string()
            });
            let hourly = shown_window.resolution == Some(UsageResolution::Hour);
            let periods = match (&shown_window.since_time, &shown_window.until_time) {
                (Some(since), Some(until)) if hourly => enumerate_hour_starts(since, until),
                _ => enumerate_days(shown_window.since_day.as_str(), shown_window.until_day.as_str()),
            };
            Overview { merged, window: shown_window, loading, active_providers, ranked_providers, providers_with_data, source_messages, periods }
        });
        if let Some(overview) = &self.overview {
            let input = self.chart_input(overview, &overview.merged, overview.active_providers.clone(), self.prefs.metric != UsageMetric::Tokens);
            self.chart.update(cx, |chart, cx| chart.set_input(input, cx));
        }
        self.update_model_chart(cx);
        let limits_environments: Vec<UsageEnvironment> = self.selected_environments().cloned().collect();
        let (hidden, now) = (self.prefs.hidden_providers.clone(), self.limits_now);
        self.limits.update(cx, |limits, cx| limits.set_input(limits_environments, hidden, now, cx));
        cx.notify();
    }

    fn chart_input(&self, overview: &Overview, merged: &MergedUsage, providers: Vec<UsageProviderKind>, by_cost: bool) -> ChartInput {
        let hourly = overview.window.resolution == Some(UsageResolution::Hour);
        let loading = providers.iter().copied().filter(|p| overview.loading.has(*p)).collect();
        ChartInput {
            providers,
            loading,
            periods: overview.periods.clone(),
            totals: if hourly { merged.hourly.clone().into() } else { merged.daily.clone().into() },
            by_cost,
            resolution: if hourly { ChartResolution::Hour } else { ChartResolution::Day },
            zone: self.zone.clone(),
            hour12: self.hour12,
            reference_time: overview.window.until_time.clone(),
            locale: self.locale,
        }
    }

    fn selected_model_totals(&self) -> Option<ModelTotals> {
        let (provider, model) = self.selected_model.as_ref()?;
        self.overview.as_ref()?.merged.models.iter().find(|m| m.provider == *provider && &m.model == model).cloned()
    }

    /// The model dialog merges only that model's buckets (`mergeAnsweredUsage` with a filter).
    fn model_usage(&self, model: &ModelTotals) -> MergedUsage {
        let answered: Vec<EnvironmentUsage> = self
            .statuses
            .iter()
            .filter(|s| self.selected.as_ref().is_none_or(|ids| ids.contains(&s.environment.environment_id)))
            .filter_map(|status| {
                let mut summary = (**status.summary.as_ref()?).clone();
                summary.buckets.retain(|b| b.provider == model.provider && b.model == model.model);
                Some(EnvironmentUsage { environment_id: status.environment.environment_id.clone(), label: status.environment.label.to_string(), summary: Arc::new(summary) })
            })
            .collect();
        merge_usage(&answered, USAGE_CONTRACT_VERSION)
    }

    fn update_model_chart(&mut self, cx: &mut Context<Self>) {
        let (Some(model), Some(overview)) = (self.selected_model_totals(), &self.overview) else { return };
        let usage = self.model_usage(&model);
        let by_cost = self.prefs.metric != UsageMetric::Tokens && !model.is_cost_unknown();
        let mut input = self.chart_input(overview, &usage, vec![model.provider], by_cost);
        input.loading.clear();
        self.model_chart.update(cx, |chart, cx| chart.set_input(input, cx));
    }

    /// The dialog's environments and the models each one's usage reports.
    fn price_inputs(&self) -> (Vec<UsagePriceTarget>, Vec<(EnvironmentId, Vec<String>)>) {
        let models = self.statuses.iter().map(|s| (s.environment.environment_id.clone(), usage_models(s.summary.as_deref()))).collect();
        (price_targets(&self.environments), models)
    }

    /// Opens "Custom model prices", with a new row for `model` when it has no custom price.
    pub fn show_prices(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        let (targets, models) = self.price_inputs();
        let (source, selected) = (self.source.clone(), self.selected.clone());
        let dialog = cx.new(|_| UsagePriceOverrides::new(source, targets, models, selected, model));
        let closed = cx.subscribe(&dialog, |this, _, _: &UsagePriceOverridesClosed, cx| {
            this.price_dialog = None;
            cx.notify();
        });
        self.price_dialog = Some((dialog, closed));
        cx.notify();
    }

    fn open_model(&mut self, provider: UsageProviderKind, model: String, cx: &mut Context<Self>) {
        self.selected_model = Some((provider, model));
        self.update_model_chart(cx);
        cx.notify();
    }

    fn shortcut_title(&self, label: SharedString, command: StaticCommand, cx: &App) -> SharedString {
        let shortcut = cx.try_global::<ShortcutState>().and_then(|s| s.label(&KeybindingCommand::Static(command)));
        match shortcut {
            Some(shortcut) => t!("{label} ({shortcut})", label = label, shortcut = shortcut),
            None => label,
        }
    }
}

fn locale_tag(locale: Locale) -> Option<&'static str> {
    Some(locale.tag())
}

fn muted_text(theme: &AppTheme, size: TextSize, text: impl Into<SharedString>) -> gpui_kit::Div {
    div().ui_text(theme, size, weight::NORMAL).text_color(theme.colors.muted_foreground).child(text.into())
}

fn heading(theme: &AppTheme, text: impl Into<SharedString>) -> gpui_kit::Div {
    div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(theme.colors.foreground()).child(text.into())
}

/// `opacity-40` on a figure still loading.
fn figure(loading: bool) -> f32 {
    if loading { 0.4 } else { 1.0 }
}

/// A control with a tooltip; `Toggle` draws none of its own.
fn with_tooltip(id: String, control: impl IntoElement, text: SharedString) -> impl IntoElement {
    div().id(SharedString::from(id)).child(control).tooltip(tooltip(text))
}

fn segmented_group(theme: &AppTheme) -> gpui_kit::Div {
    div().flex().gap(theme.sp(0.5)).rounded(theme.radii.lg).bg(with_alpha(theme.colors.input, 0.4)).p(theme.sp(0.5))
}

impl UsagePage {
    fn render_header(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let showing_limits = self.prefs.metric == UsageMetric::Limits;
        let header_width = window.viewport_size().width - px(96.);
        let wide = header_width >= theme.rem(56.);
        let all_selected = self.selected.is_none();
        let selected_count = self.selected_environments().count();
        let environment_label: SharedString = if all_selected {
            t!("All environments")
        } else if selected_count == 1 {
            self.selected_environments().next().map(|e| e.label.clone()).unwrap_or_default()
        } else {
            t!("{count, plural, one {{count} environments} other {{count} environments}}", count = selected_count)
        };
        let has_issue = self
            .statuses
            .iter()
            .filter(|s| self.selected.as_ref().is_none_or(|ids| ids.contains(&s.environment.environment_id)))
            .any(|s| s.error.is_some())
            || self.overview.as_ref().is_some_and(|o| !o.merged.contract_mismatches.is_empty());
        let env_trigger = div()
            .id("usage-environments-trigger")
            .flex()
            .items_center()
            .gap(theme.sp(1.))
            .min_w_0()
            .cursor_pointer()
            .text_color(c.foreground())
            .child(div().truncate().child(environment_label))
            .child(if !showing_limits && has_issue {
                Icon::new(IconName::CircleAlert).size(IconSize::Sm).color(c.warning_foreground)
            } else {
                Icon::new(IconName::ChevronDown).size(IconSize::Sm).color(with_alpha(c.muted_foreground, 0.6))
            });
        let page = cx.entity();
        let env_menu = dropdown_menu("usage-environments", env_trigger.test_support(), move |_, cx| page.update(cx, |this, cx| this.environment_menu(cx)));
        let visible: Vec<UsageProviderKind> =
            PROVIDER_ORDER.into_iter().filter(|p| !self.prefs.hidden_providers.contains(p)).collect();
        let provider_label_text: SharedString = match visible.len() {
            8 => t!("All providers"),
            0 => t!("No providers"),
            1 => provider_label(visible[0]).into(),
            count => t!("{count, plural, one {{count} providers} other {{count} providers}}", count = count),
        };
        let provider_trigger = div()
            .flex()
            .items_center()
            .gap(theme.sp(1.))
            .min_w_0()
            .cursor_pointer()
            .text_color(c.foreground())
            .child(div().truncate().child(provider_label_text))
            .child(Icon::new(IconName::ChevronDown).size(IconSize::Sm).color(with_alpha(c.muted_foreground, 0.6)));
        let page = cx.entity();
        let provider_menu = dropdown_menu("usage-providers", provider_trigger, move |_, cx| page.update(cx, |this, cx| this.provider_menu(cx)));
        let separator = || div().flex_none().text_color(c.icon_muted).child("/");
        let breadcrumb = div()
            .flex()
            .items_center()
            .gap(theme.sp(3.))
            .min_w_0()
            .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
            .text_color(c.muted_foreground)
            .child(div().flex_none().text_color(c.muted_foreground).child(t!("Usage")))
            .child(separator())
            .child(div().min_w(theme.sp(10.)).child(env_menu))
            .child(separator())
            .child(div().min_w(theme.sp(10.)).child(provider_menu));
        let window_label = (!showing_limits && header_width >= theme.rem(80.)).then(|| {
            let shown = self.overview.as_ref().map_or(self.window.clone(), |o| o.window.clone());
            let label = match (&shown.since_time, &shown.until_time, shown.resolution) {
                (Some(since), Some(until), Some(UsageResolution::Hour)) => t!(
                    "{start} to {end}",
                    start = format_date_time_short(since, &self.zone, self.hour12, self.locale),
                    end = format_date_time_short(until, &self.zone, self.hour12, self.locale)
                ),
                _ => t!(
                    "{start} to {end}",
                    start = format_day_short(shown.since_day.as_str(), self.locale),
                    end = format_day_short(shown.until_day.as_str(), self.locale)
                ),
            };
            muted_text(&theme, TextSize::Xs, label).truncate().min_w_0()
        });
        let period_group = (!showing_limits).then(|| {
            segmented_group(&theme).children(UsageWindowDays::ALL.into_iter().map(|days| {
                let title = self.shortcut_title(days.label(), days.command(), cx);
                let page = cx.entity();
                with_tooltip(
                    format!("usage-period-tip-{}", days.days()),
                    Toggle::new(SharedString::from(format!("usage-period-{}", days.days())))
                        .label(days.label())
                        .variant(ToggleVariant::Segmented)
                        .size(ToggleSize::Segmented)
                        .pressed(self.prefs.window_days == days)
                        .on_toggle(move |_, _, cx| page.update(cx, |this, cx| this.select_window(days, cx))),
                    title,
                )
            }))
        });
        let metric_group = segmented_group(&theme).children(UsageMetric::ALL.into_iter().map(|metric| {
            let title = self.shortcut_title(metric.label(), metric.command(), cx);
            let page = cx.entity();
            with_tooltip(
                format!("usage-metric-tip-{metric:?}"),
                Toggle::new(SharedString::from(format!("usage-metric-{metric:?}")))
                    .label(metric.label())
                    .variant(ToggleVariant::Segmented)
                    .size(ToggleSize::Segmented)
                    .pressed(self.prefs.metric == metric)
                    .on_toggle(move |_, _, cx| page.update(cx, |this, cx| this.select_metric(metric, cx))),
                title,
            )
        }));
        let can_read = self.selected_environments().any(|e| e.access.can_read_diagnostics);
        // An icon-sm ghost button holding `RefreshIcon size="sm"`, a 14px glyph.
        let refresh_disabled = self.refreshing || (!showing_limits && !can_read);
        let refresh = div()
            .id("usage-refresh")
            .size(theme.sp(7.))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(theme.radii.control)
            .child(Icon::new(IconName::RefreshCw).size(IconSize::Custom(theme.sp(3.5))).color(c.muted_foreground))
            .when(refresh_disabled, |el| el.opacity(crate::components::ui::DISABLED_OPACITY))
            .when(!refresh_disabled, |el| {
                el.cursor_pointer().hover(|el| el.bg(c.highlight())).on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
            })
;
        let controls = if wide {
            div().ml_auto().flex().flex_none().items_center().gap(theme.sp(2.)).children(period_group).child(metric_group).child(refresh).into_any_element()
        } else {
            let page = cx.entity();
            let metric_select = dropdown_menu(
                "usage-metric-select",
                div().flex().items_center().gap(theme.sp(1.)).ui_text(&theme, TextSize::Sm, weight::NORMAL).child(self.prefs.metric.label()).child(Icon::new(IconName::ChevronDown).size(IconSize::Sm)),
                move |_, cx| {
                    let current = page.read(cx).prefs.metric;
                    UsageMetric::ALL.into_iter().fold(MenuPopup::new(), |menu, metric| {
                        let page = page.clone();
                        menu.item(MenuItem::new(metric.label()).kind(MenuItemKind::Radio(metric == current)).on_select(move |_, cx| page.update(cx, |this, cx| this.select_metric(metric, cx))))
                    })
                },
            );
            div().ml_auto().flex().flex_none().items_center().gap(theme.sp(1.)).child(metric_select).child(refresh).into_any_element()
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.sp(1.))
            .min_h(theme.sp(12.))
            .pl(theme.sp(5.))
            .pr(theme.sp(2.))
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(theme.sp(3.))
                    .py(theme.sp(2.))
                    .child(breadcrumb)
                    .children(window_label)
                    .child(controls),
            )
            .child(
                Button::new("usage-close")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::IconSm)
                    .icon(IconName::X)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(UsagePageEvent::Close))),
            )
            .into_any_element()
    }

    /// "ChatGPT shared usage" with "Manage usage", shown when Codex is visible
    /// and a selected environment signs in to Codex through ChatGPT.
    fn chatgpt_sharing_row(&self, cx: &App) -> Option<AnyElement> {
        let shown = !self.prefs.hidden_providers.contains(&UsageProviderKind::Codex)
            && self
                .selected_environments()
                .any(|environment| environment.limits.providers.iter().any(t3code_shared::usage_limits::uses_chatgpt_sharing));
        if !shown {
            return None;
        }
        let theme = cx.app_theme();
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.sp(2.))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(theme.colors.muted_foreground)
                .child(t!("ChatGPT shared usage"))
                .child(
                    Button::new("usage-chatgpt-manage")
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Xs)
                        .label(t!("Manage usage"))
                        .trailing_icon(IconName::ExternalLink)
                        .on_click(|_, _, cx| cx.open_url(t3code_shared::usage_limits::CHATGPT_USAGE_URL)),
                )
                .into_any_element(),
        )
    }

    fn environment_menu(&mut self, cx: &mut Context<Self>) -> MenuPopup {
        let page = cx.entity();
        let showing_status = self.prefs.metric != UsageMetric::Limits;
        let mut menu = MenuPopup::new().min_width(px(240.)).item(
            MenuItem::new(t!("All environments")).kind(MenuItemKind::Checkbox(self.selected.is_none())).on_select({
                let page = page.clone();
                move |_, cx| {
                    page.update(cx, |this, cx| {
                        this.selected = if this.selected.is_none() { Some(BTreeSet::new()) } else { None };
                        this.recompute(cx);
                    })
                }
            }),
        );
        menu = menu.separator();
        for status in self.statuses.clone() {
            let id = status.environment.environment_id.clone();
            let checked = self.selected.as_ref().is_none_or(|ids| ids.contains(&id));
            let state: SharedString = if status.error.is_some() {
                t!("Unavailable")
            } else if status.summary.as_ref().is_some_and(|s| !is_compatible_usage_contract_version(number(&s.contract_version), USAGE_CONTRACT_VERSION)) {
                t!("Update required")
            } else if !status.environment.is_connected {
                t!("Connecting…")
            } else {
                match progress(&status, self.refreshing) {
                    Progress::Loading => t!("Scanning…"),
                    Progress::Stale => t!("Refreshing…"),
                    Progress::Partway(providers) if providers.len() == 1 => t!("Updating {provider}…", provider = provider_label(providers[0])),
                    Progress::Partway(providers) => t!("Updating {count} providers…", count = providers.len()),
                    _ => t!("Ready", context = "environment usage status"),
                }
            };
            let label: SharedString = if showing_status { format!("{}  ·  {state}", status.environment.label).into() } else { status.environment.label.clone() };
            let page = page.clone();
            menu = menu.item(MenuItem::new(label).kind(MenuItemKind::Checkbox(checked)).on_select(move |_, cx| {
                page.update(cx, |this, cx| {
                    let mut next: BTreeSet<EnvironmentId> = this.selected_environments().map(|e| e.environment_id.clone()).collect();
                    if !next.remove(&id) {
                        next.insert(id.clone());
                    }
                    this.selected = if next.len() == this.environments.len() { None } else { Some(next) };
                    this.fetch_missing(cx);
                    this.recompute(cx);
                })
            }));
        }
        if self.environments.is_empty() {
            menu = menu.label(t!("No environments connected."));
        }
        let partial = self.statuses.iter().any(|s| s.summary.is_some()) && self.statuses.iter().any(|s| s.summary.is_none() && s.error.is_none() && s.can_read_diagnostics);
        if showing_status && partial {
            menu = menu.label(t!("Totals are partial while selected environments scan."));
        }
        if showing_status {
            for status in self.statuses.iter().filter(|s| s.error.is_some()) {
                menu = menu.label(t!("{environment}: {error}", environment = status.environment.label.clone(), error = status.error.clone().unwrap_or_default()));
            }
            if let Some(overview) = &self.overview {
                for mismatch in &overview.merged.contract_mismatches {
                    let label = self.environments.iter().find(|e| e.environment_id == mismatch.environment_id).map(|e| e.label.clone()).unwrap_or_default();
                    menu = menu.label(match mismatch.direction {
                        t3code_shared::usage_merge::MismatchDirection::ServerBehind => {
                            t!("{environment} runs an older server version and is excluded from totals.", environment = label)
                        }
                        t3code_shared::usage_merge::MismatchDirection::ClientBehind => {
                            t!("This client is older than the server on {environment}; its usage is excluded from totals.", environment = label)
                        }
                    });
                }
                if !overview.merged.duplicate_sources.is_empty() {
                    menu = menu.label(t!(
                        "Counted once across environments sharing a transcript directory: {sources}",
                        sources = overview.merged.duplicate_sources.join(", ")
                    ));
                }
            }
        }
        let page = page.clone();
        menu.separator().item(
            MenuItem::new(t!("Model prices"))
                .id("usage-model-prices")
                .icon(IconName::SlidersHorizontal)
                .on_select(move |_, cx| page.update(cx, |this, cx| this.show_prices(None, cx))),
        )
    }

    fn provider_menu(&mut self, cx: &mut Context<Self>) -> MenuPopup {
        let page = cx.entity();
        let hidden = self.prefs.hidden_providers.clone();
        let set = |page: Entity<UsagePage>, next: Vec<UsageProviderKind>| {
            move |_: &mut Window, cx: &mut App| {
                let next = next.clone();
                page.update(cx, |this, cx| {
                    this.prefs.hidden_providers = next;
                    this.save_prefs();
                    this.recompute(cx);
                })
            }
        };
        let mut menu = MenuPopup::new().item(
            MenuItem::new(t!("All providers"))
                .kind(MenuItemKind::Checkbox(hidden.is_empty()))
                .on_select(set(page.clone(), if hidden.is_empty() { PROVIDER_ORDER.to_vec() } else { Vec::new() })),
        );
        menu = menu.separator();
        for provider in PROVIDER_ORDER {
            let shown = !hidden.contains(&provider);
            let next: Vec<UsageProviderKind> =
                PROVIDER_ORDER.into_iter().filter(|entry| if *entry == provider { shown } else { hidden.contains(entry) }).collect();
            menu = menu.item(MenuItem::new(provider_label(provider)).kind(MenuItemKind::Checkbox(shown)).on_select(set(page.clone(), next)));
        }
        menu
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let showing_limits = self.prefs.metric == UsageMetric::Limits;
        let selected_count = self.selected_environments().count();
        if selected_count == 0 {
            let text = match (self.environments.is_empty(), showing_limits) {
                (true, true) => t!("Connect an environment to see limits."),
                (true, false) => t!("Connect an environment to see usage."),
                (false, true) => t!("Select an environment to see limits."),
                (false, false) => t!("Select an environment to see usage."),
            };
            return muted_text(&theme, TextSize::Sm, text).into_any_element();
        }
        if showing_limits {
            return self.limits.clone().into_any_element();
        }
        let can_read = self.selected_environments().any(|e| e.access.can_read_diagnostics);
        if self.overview.is_none() {
            return self.render_skeleton(window, cx);
        }
        if !can_read {
            let multiple = selected_count > 1;
            let lines: Vec<SharedString> = self
                .statuses
                .iter()
                .filter(|s| self.selected.as_ref().is_none_or(|ids| ids.contains(&s.environment.environment_id)))
                .map(|s| {
                    let error = s.error.clone().unwrap_or_default();
                    if multiple { t!("{environment}: {error}", environment = s.environment.label.clone(), error = error) } else { error }
                })
                .collect();
            return div()
                .flex()
                .flex_col()
                .gap(theme.sp(2.))
                .py(theme.sp(12.))
                .items_center()
                .children(lines.into_iter().map(|line| muted_text(&theme, TextSize::Sm, line)))
                .into_any_element();
        }
        self.render_overview(window, cx)
    }

    fn render_overview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let breakdown = self.render_breakdown(window.viewport_size().width >= px(640.), cx);
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let Some(overview) = &self.overview else { return div().into_any_element() };
        let merged = overview.merged.clone();
        let by_cost = self.prefs.metric != UsageMetric::Tokens;
        let locale = locale_tag(self.locale);
        let loading_partial = overview.loading.partial;
        let hourly = overview.window.resolution == Some(UsageResolution::Hour);
        let viewport = window.viewport_size().width;
        let lg = viewport >= px(1024.);
        let md = viewport >= px(768.);
        let usd = move |value: f64| format_usd(value, locale);

        let headline = div()
            .flex()
            .flex_col()
            .gap(theme.sp(1.))
            .child(
                div()
                    .ui_text(&theme, TextSize::Xl3, weight::SEMIBOLD)
                    .text_size(theme.rem(2.25))
                    .line_height(theme.rem(2.5))
                    .text_color(c.foreground())
                    .opacity(figure(loading_partial))
                    .child(if by_cost { usd(merged.cost_usd) } else { format_tokens(merged.total_tokens) }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.))
                    .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .child(
                        div().opacity(figure(loading_partial)).child(t!(
                            "{count, plural, one {{formatted} sessions} other {{formatted} sessions}}",
                            count = merged.sessions as u64,
                            formatted = format_count(merged.sessions, locale)
                        )),
                    )
                    .when(by_cost, |el| el.child(format!(" · {}", t!("API estimate"))))
                    .when(by_cost && merged.cost_quality.unpriced_share > 0.0, |el| {
                        el.child(
                            div()
                                .id("usage-unpriced-info")
                                .child(Icon::new(IconName::Info).size(IconSize::Xs).color(c.muted_foreground))
                                .tooltip(tooltip(SharedString::from(
                                    t_rich!("API estimate excludes{\" \"}{share} unpriced records.", share = format_percent(merged.cost_quality.unpriced_share, 1))
                                        .plain(),
                                ))),
                        )
                    }),
            );
        let chatgpt_sharing = self.chatgpt_sharing_row(cx);
        let provider_rows = overview.ranked_providers.iter().map(|provider| {
            let totals = merged.providers.iter().find(|t| t.provider == *provider);
            let share = totals.map_or(0.0, |t| if by_cost { t.cost_share } else { t.token_share });
            let sessions = totals.map_or(0.0, |t| t.sessions);
            let provider_loading = overview.loading.has(*provider);
            let awaiting = provider_loading && !overview.providers_with_data.contains(provider);
            let (cost, tokens) = (totals.map_or(0.0, |t| t.cost_usd), totals.map_or(0.0, |t| t.total_tokens));
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(theme.sp(4.))
                        .child(
                            div()
                                .flex()
                                .min_w_0()
                                .items_center()
                                .gap(theme.sp(2.))
                                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                                .text_color(c.foreground())
                                .child(div().size(theme.sp(2.)).flex_none().rounded_full().bg(provider_color(*provider, &theme)))
                                .child(provider_mark(*provider, theme.sp(4.), cx))
                                .child(div().truncate().child(provider_label(*provider)))
                                .child(
                                    div()
                                        .flex_none()
                                        .ui_text(&theme, TextSize::Xs2, weight::NORMAL)
                                        .text_color(c.muted_foreground)
                                        .opacity(if awaiting { 0.0 } else { figure(provider_loading) })
                                        .child(t!(
                                            "{count, plural, one {{formatted} session} other {{formatted} sessions}}",
                                            count = sessions as u64,
                                            formatted = format_count(sessions, locale)
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
                                .text_color(c.foreground())
                                .opacity(figure(provider_loading))
                                .child(if awaiting { "-".to_owned() } else if by_cost { usd(cost) } else { format_tokens(tokens) }),
                        ),
                )
                .child(
                    div()
                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .opacity(if awaiting { 0.0 } else { figure(provider_loading) })
                        .child(if by_cost {
                            t!("{share} of cost · {tokens} tokens", share = format_percent(share, 1), tokens = format_tokens(tokens))
                        } else {
                            t!("{share} of tokens · {cost}", share = format_percent(share, 1), cost = usd(cost))
                        }),
                )
        });
        let left = div()
            .flex()
            .flex_col()
            .gap(theme.sp(5.))
            .min_w_0()
            .when(lg, |el| el.w(theme.rem(18.)).flex_none())
            .child(headline)
            .children(chatgpt_sharing)
            .children(provider_rows);
        let chart_title = match (hourly, by_cost) {
            (true, false) => t!("Hourly processed tokens"),
            (true, true) => t!("Hourly cost"),
            (false, false) => t!("Daily processed tokens"),
            (false, true) => t!("Daily cost"),
        };
        let right = div().flex().flex_col().flex_1().min_w_0().gap(theme.sp(3.)).child(heading(&theme, chart_title)).child(self.chart.clone());
        let top = if lg {
            div().flex().gap(theme.sp(6.)).child(left).child(right)
        } else {
            div().flex().flex_col().gap(theme.sp(6.)).child(left).child(right)
        };
        let metric = |label: SharedString, value: String| {
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(theme.sp(0.5))
                .child(muted_text(&theme, TextSize::Xs, label))
                .child(div().ui_text(&theme, TextSize::Base, weight::MEDIUM).text_color(c.foreground()).opacity(figure(loading_partial)).child(value))
        };
        let metrics = [
            (t!("Processed tokens"), format_tokens(merged.total_tokens)),
            (t!("Cached input"), format_tokens(merged.cached_input_tokens)),
            (t!("Uncached input"), format_tokens(merged.uncached_input_tokens)),
            (t!("Output", context = "output tokens"), format_tokens(merged.output_tokens)),
            (t!("Cache savings"), usd(merged.cost_quality.cache_savings_usd)),
        ];
        let columns = if md { 5 } else { 2 };
        let totals = div().flex().flex_col().gap(theme.sp(2.)).child(heading(&theme, t!("Totals"))).child(
            div().grid().grid_cols(columns).gap_x(theme.sp(6.)).gap_y(theme.sp(4.)).py(theme.sp(1.)).children(metrics.into_iter().map(|(label, value)| metric(label, value))),
        );
        let shares = (merged.total_tokens > 0.0).then(|| {
            let mut bars: Vec<AnyElement> = Vec::new();
            if by_cost {
                bars.push(UsageShareBar::new("cost-type", t!("Cost by type"), cost_type_segments(&merged.category_cost, &theme), usd).into_any_element());
                if merged.speed_cost.fast + merged.speed_cost.ultrafast > 0.0 {
                    bars.push(
                        UsageShareBar::new("cost-speed", t!("Cost by speed"), speed_cost_segments(&merged.speed_cost, &theme), usd)
                            .aside(speed_premium(&theme, usd(merged.speed_cost.premium)))
                            .into_any_element(),
                    );
                }
            } else {
                let tokens = t3code_shared::usage_merge::TokenTotals {
                    uncached_input_tokens: merged.uncached_input_tokens,
                    cached_input_tokens: merged.cached_input_tokens,
                    cache_creation_tokens: merged.cache_creation_tokens,
                    output_tokens: merged.output_tokens,
                    reasoning_tokens: merged.reasoning_tokens,
                };
                bars.push(UsageShareBar::new("token-type", t!("Tokens by type"), token_type_segments(&tokens, &theme), format_tokens).into_any_element());
            }
            div()
                .grid()
                .grid_cols(if lg { 2 } else { 1 })
                .gap_x(theme.sp(12.))
                .gap_y(theme.sp(8.))
                .opacity(figure(loading_partial))
                .children(bars)
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(6.))
            .children(overview.source_messages.iter().map(|m| muted_text(&theme, TextSize::Sm, m.clone()).mb(theme.sp(4.))))
            .child(top)
            .child(totals)
            .children(shares)
            .child(breakdown)
            .into_any_element()
    }

    fn render_breakdown(&mut self, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let Some(overview) = &self.overview else { return div().into_any_element() };
        let merged = overview.merged.clone();
        let by_cost = self.prefs.metric != UsageMetric::Tokens;
        let hourly = overview.window.resolution == Some(UsageResolution::Hour);
        let locale = locale_tag(self.locale);
        let usd = move |value: f64| format_usd(value, locale);
        let time_label = if hourly { t!("Hour") } else { t!("Day") };
        let (by_model, by_time) = (cx.entity(), cx.entity());
        let toggle = segmented_group(&theme)
            .child(
                Toggle::new("breakdown-model")
                    .label(t!("Model"))
                    .variant(ToggleVariant::Segmented)
                    .size(ToggleSize::Segmented)
                    .pressed(!self.breakdown_by_time)
                    .on_toggle(move |_, _, cx| by_model.update(cx, |this, cx| this.show_breakdown_by_time(false, cx))),
            )
            .child(
                Toggle::new("breakdown-time")
                    .label(time_label.clone())
                    .variant(ToggleVariant::Segmented)
                    .size(ToggleSize::Segmented)
                    .pressed(self.breakdown_by_time)
                    .on_toggle(move |_, _, cx| by_time.update(cx, |this, cx| this.show_breakdown_by_time(true, cx))),
            );
        let header_cell = |text: SharedString, right: bool| {
            div().py(theme.sp(2.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).when(right, |el| el.flex().justify_end()).child(text)
        };
        let empty_row = || div().py(theme.sp(6.)).flex().justify_center().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(t!("No activity in this window."));
        let table = if !self.breakdown_by_time {
            let models = if by_cost { merged.models.clone() } else { sort_models_by_tokens(&merged.models) };
            let peak = models.iter().fold(0.0f64, |peak, m| peak.max(if by_cost { m.cost_usd } else { m.total_tokens }));
            let show_share = wide;
            let head = div()
                .flex()
                .border_b_1()
                .border_color(c.border)
                .child(header_cell("#".into(), false).w(theme.sp(8.)).pl(theme.sp(2.)))
                .child(header_cell(t!("Model"), false).flex_1())
                .child(header_cell(t!("Cost"), true).w(theme.sp(24.)))
                .when(show_share, |el| el.child(header_cell(t!("Share", context = "column heading, percentage share"), true).w(theme.sp(20.))))
                .child(header_cell(t!("Tokens"), true).w(theme.sp(20.)).pr(theme.sp(2.)));
            let rows: Vec<AnyElement> = models
                .iter()
                .enumerate()
                .map(|(index, model)| {
                    let value = if by_cost { model.cost_usd } else { model.total_tokens };
                    let has_bar = value > 0.0 && peak > 0.0;
                    let row_loading = overview.loading.has(model.provider);
                    let (provider, name) = (model.provider, model.model.clone());
                    div()
                        .id(SharedString::from(format!("model-row-{}-{}", provider.as_str(), model.model)))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(with_alpha(c.border, 0.5))
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .cursor_pointer()
                        .hover(|s| s.bg(with_alpha(c.muted, 0.5)))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_model(provider, name.clone(), cx)))
                        .child(div().w(theme.sp(8.)).pl(theme.sp(2.)).py(theme.sp(2.5)).ui_text(&theme, TextSize::Xs, weight::NORMAL).child((index + 1).to_string()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .py(theme.sp(2.5))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(theme.sp(2.))
                                        .text_color(c.foreground())
                                        .when(!has_bar, |el| el.py(theme.sp(1.)))
                                        .child(provider_mark(model.provider, theme.sp(3.5), cx))
                                        .child(model.model.clone()),
                                )
                                .when(has_bar, |el| {
                                    el.child(
                                        div().mt(theme.sp(1.5)).h(theme.sp(0.5)).max_w(theme.sp(48.)).opacity(figure(row_loading)).child(
                                            div()
                                                .h_full()
                                                .rounded_full()
                                                .min_w(theme.sp(2.))
                                                .w(gpui_kit::relative((value / peak) as f32))
                                                .bg(provider_color(model.provider, &theme)),
                                        ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .w(theme.sp(24.))
                                .flex()
                                .justify_end()
                                .opacity(figure(row_loading))
                                .text_color(if model.is_cost_unknown() { c.muted_foreground } else { c.foreground() })
                                .child(if model.is_cost_unknown() { t!("Unpriced").to_string() } else { usd(model.cost_usd) }),
                        )
                        .when(show_share, |el| {
                            el.child(
                                div()
                                    .w(theme.sp(20.))
                                    .flex()
                                    .justify_end()
                                    .opacity(figure(row_loading))
                                    .child(model_share(model, by_cost).map(|s| format_percent(s, 1)).unwrap_or_default()),
                            )
                        })
                        .child(div().w(theme.sp(20.)).pr(theme.sp(2.)).flex().justify_end().opacity(figure(row_loading)).child(format_tokens(model.total_tokens)))
                        .into_any_element()
                })
                .collect();
            div()
                .flex()
                .flex_col()
                .child(head)
                .when(rows.is_empty(), |el| el.child(empty_row()))
                .children(rows)
                .into_any_element()
        } else {
            let periods: Vec<_> = if hourly { merged.hourly.iter().rev().collect() } else { merged.daily.iter().rev().collect() };
            let head = div()
                .flex()
                .border_b_1()
                .border_color(c.border)
                .child(header_cell(time_label, false).flex_1().pl(theme.sp(2.)))
                .children(overview.active_providers.iter().map(|p| header_cell(provider_label(*p).into(), true).w(theme.sp(24.))))
                .child(header_cell(t!("Total"), true).w(theme.sp(24.)))
                .child(header_cell(t!("Tokens"), true).w(theme.sp(20.)).pr(theme.sp(2.)));
            let rows: Vec<AnyElement> = periods
                .iter()
                .map(|period| {
                    let label = match &period.hour_start {
                        Some(hour) => format_hour_short(hour, &self.zone, self.hour12, self.locale),
                        None => format_day_short(&period.day, self.locale),
                    };
                    div()
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(with_alpha(c.border, 0.5))
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .child(div().flex_1().py(theme.sp(2.)).pl(theme.sp(2.)).text_color(c.foreground()).child(label))
                        .children(overview.active_providers.iter().map(|p| {
                            div()
                                .w(theme.sp(24.))
                                .flex()
                                .justify_end()
                                .text_color(c.muted_foreground)
                                .opacity(figure(overview.loading.has(*p)))
                                .child(usd(period.by_provider.get(p).map_or(0.0, |cell| cell.cost_usd)))
                        }))
                        .child(div().w(theme.sp(24.)).flex().justify_end().text_color(c.foreground()).opacity(figure(overview.loading.partial)).child(usd(period.cost_usd)))
                        .child(
                            div()
                                .w(theme.sp(20.))
                                .pr(theme.sp(2.))
                                .flex()
                                .justify_end()
                                .text_color(c.muted_foreground)
                                .opacity(figure(overview.loading.partial))
                                .child(format_tokens(period.total_tokens)),
                        )
                        .into_any_element()
                })
                .collect();
            div().flex().flex_col().child(head).when(rows.is_empty(), |el| el.child(empty_row())).children(rows).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(3.))
            .child(div().flex().items_center().justify_between().gap(theme.sp(3.)).child(heading(&theme, t!("Breakdown"))).child(toggle))
            .child(table)
            .into_any_element()
    }

    fn render_skeleton(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let lg = window.viewport_size().width >= px(1024.);
        let md = window.viewport_size().width >= px(768.);
        let block = |w: Option<f32>, h: f32| {
            let skeleton = Skeleton::new(SkeletonShape::Block).h(theme.sp(h));
            match w {
                Some(w) => skeleton.w(theme.sp(w)).into_any_element(),
                None => skeleton.w_full().into_any_element(),
            }
        };
        let pill = |size: f32| Skeleton::new(SkeletonShape::Pill).size(theme.sp(size)).flex_none().into_any_element();
        let left = div()
            .flex()
            .flex_col()
            .gap(theme.sp(5.))
            .when(lg, |el| el.w(theme.rem(18.)).flex_none())
            .child(div().flex().flex_col().gap(theme.sp(1.)).child(block(Some(36.), 10.)).child(block(Some(32.), 4.)))
            .children(PROVIDER_ORDER.iter().map(|_| {
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(1.))
                    .child(
                        div()
                            .flex()
                            .min_h(theme.sp(5.))
                            .items_center()
                            .justify_between()
                            .gap(theme.sp(4.))
                            .child(div().flex().items_center().gap(theme.sp(2.)).child(pill(2.)).child(pill(4.)).child(block(Some(20.), 3.5)))
                            .child(block(Some(14.), 3.5)),
                    )
                    .child(block(Some(36.), 4.))
            }));
        let right = div()
            .flex()
            .flex_col()
            .flex_1()
            .gap(theme.sp(3.))
            .child(block(Some(24.), 5.))
            .child(div().flex().flex_col().gap(theme.sp(1.)).child(div().ml(theme.sp(16.)).child(block(None, 56.))).child(div().ml(theme.sp(16.)).child(block(None, 4.))));
        let top = if lg { div().flex().gap(theme.sp(6.)).child(left).child(right) } else { div().flex().flex_col().gap(theme.sp(6.)).child(left).child(right) };
        let labels = [t!("Processed tokens"), t!("Cached input"), t!("Uncached input"), t!("Output", context = "output tokens"), t!("Cache savings")];
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(6.))
            .child(top)
            .child(
                div().flex().flex_col().gap(theme.sp(2.)).child(heading(&theme, t!("Totals"))).child(
                    div().grid().grid_cols(if md { 5 } else { 2 }).gap_x(theme.sp(6.)).gap_y(theme.sp(4.)).py(theme.sp(1.)).children(labels.into_iter().map(|label| {
                        div().flex().flex_col().gap(theme.sp(0.5)).child(muted_text(&theme, TextSize::Xs, label)).child(block(Some(16.), 6.))
                    })),
                ),
            )
            .child(div().flex().flex_col().gap(theme.sp(2.5)).w(if lg { theme.rem(28.) } else { theme.rem(20.) }).child(block(Some(28.), 5.)).child(block(None, 2.)).child(block(Some(72.), 4.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(heading(&theme, t!("Breakdown")))
                            .child(Skeleton::new(SkeletonShape::Card).h(theme.sp(7.)).w(theme.sp(28.))),
                    )
                    .child(block(None, 44.)),
            )
            .into_any_element()
    }

    fn render_model_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let model = self.selected_model_totals()?;
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let usage = self.model_usage(&model);
        let locale = locale_tag(self.locale);
        let usd = move |value: f64| format_usd(value, locale);
        let cost_unknown = model.is_cost_unknown();
        let stats: Vec<(SharedString, String)> = [
            Some((t!("Cost"), if cost_unknown { t!("Unpriced").to_string() } else { usd(model.cost_usd) })),
            Some((t!("Tokens"), format_tokens(model.total_tokens))),
            cost_per_million_tokens(&model).map(|v| (t!("Per 1M tokens"), usd(v))),
            cache_hit_rate(&model).map(|v| (t!("Cache hit"), format_percent(v, 1))),
        ]
        .into_iter()
        .flatten()
        .collect();
        let description = if cost_unknown {
            provider_label(model.provider).to_owned()
        } else {
            format!("{} · {}", provider_label(model.provider), t!("{share} of cost", share = format_percent(model.cost_share, 1)))
        };
        let mut bars: Vec<AnyElement> = Vec::new();
        if !cost_unknown {
            bars.push(UsageShareBar::new("model-cost-type", t!("Cost by type"), cost_type_segments(&usage.category_cost, &theme), usd).into_any_element());
        }
        bars.push(UsageShareBar::new("model-token-type", t!("Tokens by type"), token_type_segments(&model.tokens, &theme), format_tokens).into_any_element());
        if usage.speed_cost.fast + usage.speed_cost.ultrafast > 0.0 {
            bars.push(
                UsageShareBar::new("model-speed", t!("Cost by speed"), speed_cost_segments(&usage.speed_cost, &theme), usd)
                    .aside(speed_premium(&theme, usd(usage.speed_cost.premium)))
                    .into_any_element(),
            );
        }
        let close = cx.listener(|this, _, _, cx| {
            this.selected_model = None;
            cx.notify();
        });
        let mut surface = DialogSurface::new(model.model.clone()).description(description).on_close(move |window, cx| close(&gpui_kit::ClickEvent::default(), window, cx));
        if model.unpriced_tokens > 0.0 {
            let name = model.model.to_string();
            let set_price = cx.listener(move |this, _, _, cx| {
                this.selected_model = None;
                this.show_prices(Some(name.clone()), cx);
            });
            surface = surface
                .bare_footer()
                .footer_centered()
                .footer(muted_text(&theme, TextSize::Xs, t!("{tokens} tokens have no known price", tokens = format_tokens(model.unpriced_tokens))).mr_auto())
                .footer(Button::new("usage-model-set-price").label(t!("Set price")).on_click(set_price));
        }
        let surface = surface.child(
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(8.))
                .child(div().grid().grid_cols(4).gap_x(theme.sp(6.)).gap_y(theme.sp(4.)).children(stats.into_iter().map(|(label, value)| {
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(theme.sp(0.5))
                        .child(muted_text(&theme, TextSize::Xs, label))
                        .child(div().ui_text(&theme, TextSize::Xl2, weight::SEMIBOLD).text_color(c.foreground()).child(value))
                })))
                .child(self.model_chart.clone())
                .child(div().grid().grid_cols(2).gap_x(theme.sp(10.)).gap_y(theme.sp(8.)).children(bars)),
        );
        let entity = cx.entity();
        Some(
            Modal::dialog("usage-model-dialog", div().id("usage-model-dialog-content").occlude().w_full().max_w(theme.rem(48.)).child(surface), move |_, cx| {
                entity.update(cx, |this, cx| {
                    this.selected_model = None;
                    cx.notify();
                })
            })
            .into_any_element(),
        )
    }
}

/// "Premium $1.20", the figure in the foreground colour.
fn speed_premium(theme: &AppTheme, premium: String) -> impl IntoElement {
    let foreground = theme.colors.foreground();
    let text = t_rich!("Premium{\" \"}<span>{premium}</span>", premium = premium).styled(move |tag| {
        (tag == "span").then(|| gpui_kit::HighlightStyle { color: Some(foreground), ..Default::default() })
    });
    div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(text)
}

impl Render for UsagePage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.render_header(window, cx);
        let body = self.render_body(window, cx);
        let dialog = if self.prefs.metric != UsageMetric::Limits { self.render_model_dialog(cx) } else { None };
        let prices = self.price_dialog.as_ref().map(|(dialog, _)| dialog.clone());
        let theme = cx.app_theme();
        let page = cx.entity();
        let element = div()
            .id("usage-page")
            .track_focus(&self.focus)
            .key_context("UsagePage")
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .bg(theme.colors.background())
            .text_color(theme.colors.foreground())
            .child(header)
            .child(
                scroll_area("usage-scroll").flex_1().min_h_0().child(
                    div()
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .w_full()
                        .max_w(theme.rem(64.))
                        .gap(theme.sp(6.))
                        .px(theme.sp(6.))
                        .pt(theme.sp(6.))
                        .pb(theme.sp(12.))
                        .child(body),
                ),
            )
            .children(dialog);
        let page = on_commands(
            element,
            |command| UsageMetric::ALL.iter().any(|m| m.command() == command) || UsageWindowDays::ALL.iter().any(|d| d.command() == command),
            Rc::new(move |command, _, cx| page.update(cx, |this, cx| this.run_command(command, cx))),
        );
        // The prices dialog sits outside the page, as the fork portals it, so typing in its
        // fields never reaches the page's one-key shortcuts.
        div().size_full().child(page).children(prices)
    }
}
