//! The Limits view of the Usage page. Ports `UsageLimits.tsx`
//! (`UsageLimitsSection`, the reset credit flow), `UsageLimitsPooled.tsx` and
//! the copy helpers of `usageLimitsCopy.ts`: every driver's accounts pooled
//! into windows, each drawn as one segmented bar with a pace tick.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{AnyElement, Context, Hsla, IntoElement, Render, SharedString, Window, canvas, div, point, prelude::*, px, relative};
use t3code_contracts::{
    ProviderConsumeResetCreditInput, ProviderConsumeResetCreditOutcome,
    ServerConfigProvidersItemUsageLimitsResetCredits as ResetCredits,
    ServerConfigProvidersItemUsageLimitsWindowsItem as UsageWindow, UsageProviderKind,
};
use t3code_shared::usage_limits::{
    CHATGPT_USAGE_URL, LimitAccount, LimitEnvironment, LimitPace, LimitPool, LimitPoolWindow, LimitsNotice, ResetCountdown,
    collect_external_usage_links, collect_limit_accounts, collect_limit_pools, cursor_window_hides_details, display_limit_windows,
    limits_notice_reason, pace_remaining_percent, providers_with_limits, remaining_percent, reset_countdown,
};
use t3code_shared::usage_format::parse_instant_ms;

use super::usage_providers::{driver_color, driver_label, driver_mark, provider_driver};
use super::usage_source::{UsageEnvironment, UsageSource};
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::popover::{PopoverSurface, PopoverWidth, below};
use crate::components::ui::tooltip::tooltip;
use crate::i18n::t;
use crate::theme_color::with_alpha;
use crate::timestamp_format::{TimestampFormat, format_upcoming_timestamp};

const MINUTE: f64 = 60_000.0;
const HOUR: f64 = 60.0 * MINUTE;
const DAY: f64 = 24.0 * HOUR;
/// `@2xl/pool`: segments show names and chips from 42rem of bar width.
const WIDE_POOL_REM: f32 = 42.0;

/// `formatDuration`: "2d 5h", "3h 10m", "7m".
pub fn format_duration(ms: f64) -> SharedString {
    let remaining = ms.max(0.0);
    let days = (remaining / DAY).floor() as i64;
    let hours = ((remaining % DAY) / HOUR).floor() as i64;
    let minutes = ((remaining % HOUR) / MINUTE).floor() as i64;
    if days > 0 {
        t!("{days}d {hours}h", days = days, hours = hours)
    } else if hours > 0 {
        t!("{hours}h {minutes}m", hours = hours, minutes = minutes)
    } else {
        t!("{minutes}m", minutes = minutes)
    }
}

/// How [`format_resets_in`] words the countdown.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResetsInStyle {
    /// "resets in 2h 5m"
    Full,
    /// "in 2h 5m"
    In,
    /// "↻ 2h 5m"
    Refill,
}

/// `formatResetsIn`.
pub fn format_resets_in(window: &UsageWindow, now: f64, style: ResetsInStyle) -> Option<SharedString> {
    Some(match reset_countdown(window, now)? {
        ResetCountdown::Now => t!("resets now"),
        ResetCountdown::Momentarily => t!("Resets momentarily"),
        ResetCountdown::In { ms } => {
            let duration = format_duration(ms);
            match style {
                ResetsInStyle::In => t!("in {duration}", context = "time until a usage window resets", duration = duration),
                ResetsInStyle::Refill => t!("↻ {duration}", duration = duration),
                ResetsInStyle::Full => t!("resets in {duration}", duration = duration),
            }
        }
    })
}

/// One redeemable reset, already naming its credit.
#[derive(Clone, Debug)]
pub struct ResetCreditChoice {
    pub key: SharedString,
    pub summary: SharedString,
    pub action_label: SharedString,
    pub confirm_description: SharedString,
    pub available_count: u64,
    pub input: ProviderConsumeResetCreditInput,
}

/// `resetCreditChoices`.
pub fn reset_credit_choices(credits: &ResetCredits, input: &ProviderConsumeResetCreditInput, now: f64, compact: bool) -> Vec<ResetCreditChoice> {
    let expires_in = |at: Option<&str>| at.and_then(parse_instant_ms).map(|ms| format_duration(ms as f64 - now));
    if let (Some(options), ProviderConsumeResetCreditInput::NoUndefined(native)) = (&credits.options, input) {
        return options
            .iter()
            .map(|option| {
                let mut input = native.clone();
                input.credit_id = Some(option.credit_id.clone());
                ResetCreditChoice {
                    key: option.credit_id.clone().into(),
                    summary: match expires_in(option.expires_at.as_deref()) {
                        Some(expires) => t!("{label} · {count} left · expires in {expires}", label = option.label.clone(), count = option.available_count, expires = expires),
                        None => t!("{label} · {count} left", label = option.label.clone(), count = option.available_count),
                    },
                    action_label: t!("Use", context = "button that redeems a reset credit"),
                    confirm_description: t!("{label}: {description} This cannot be undone.", label = option.label.clone(), description = option.description.clone()),
                    available_count: option.available_count,
                    input: ProviderConsumeResetCreditInput::NoUndefined(input),
                }
            })
            .collect();
    }
    let count = credits.available_count;
    let expires = expires_in(credits.next_expires_at.as_deref());
    let summary = match (count, compact, expires) {
        (0, _, _) => t!("No reset credits banked"),
        (_, true, Some(expires)) => t!("{count} banked · expires in {expires}", count = count, expires = expires),
        (_, true, None) => t!("{count} banked", count = count),
        (_, false, Some(expires)) => t!(
            "{count, plural, one {{count} reset credit banked · next expires in {expires}} other {{count} reset credits banked · next expires in {expires}}}",
            count = count,
            expires = expires
        ),
        (_, false, None) => t!("{count, plural, one {{count} reset credit banked} other {{count} reset credits banked}}", count = count),
    };
    vec![ResetCreditChoice {
        key: "next".into(),
        summary,
        action_label: t!("Use reset"),
        confirm_description: t!(
            "This redeems one credit on your account and clears the current rate-limit windows. It cannot be undone."
        ),
        available_count: count,
        input: input.clone(),
    }]
}

fn outcome_text(outcome: ProviderConsumeResetCreditOutcome) -> SharedString {
    match outcome {
        ProviderConsumeResetCreditOutcome::Reset => t!("Reset applied. Your windows have cleared."),
        ProviderConsumeResetCreditOutcome::NothingToReset => t!("Nothing to reset right now."),
        ProviderConsumeResetCreditOutcome::NoCredit => t!("No reset credit left."),
        ProviderConsumeResetCreditOutcome::AlreadyRedeemed => t!("That credit was already redeemed."),
    }
}

/// `limitsNotice` worded for the page.
fn limits_notice(reason: LimitsNotice) -> SharedString {
    match reason {
        LimitsNotice::Unsupported(Some(message)) | LimitsNotice::ProbeFailed(Some(message)) => message.to_owned().into(),
        LimitsNotice::Unsupported(None) => t!("This account has no subscription limits."),
        LimitsNotice::ProbeFailed(None) => t!("Could not read limits."),
        LimitsNotice::NoWindows => t!("No limits reported."),
    }
}

/// `collectLimitNotices`: why some providers and hubs show no limits.
pub fn collect_limit_notices(environments: &[LimitEnvironment]) -> Vec<SharedString> {
    let label = |environment: &str, subject: &str| if environments.len() > 1 { format!("{environment} · {subject}") } else { subject.to_owned() };
    let mut notices: Vec<SharedString> = Vec::new();
    let mut push = |notice: SharedString| {
        if !notices.contains(&notice) {
            notices.push(notice);
        }
    };
    for environment in environments {
        for provider in providers_with_limits(&environment.providers) {
            let Some(limits) = &provider.usage_limits else { continue };
            let Some(reason) = limits_notice_reason(limits) else { continue };
            if matches!(reason, LimitsNotice::Unsupported(_)) {
                continue;
            }
            let name = provider.display_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&provider.driver);
            push(format!("{}: {}", label(&environment.label, name), limits_notice(reason)).into());
        }
        for source in environment.usage_limit_sources.iter() {
            if let Some(error) = &source.error {
                push(format!("{}: {error}", label(&environment.label, &source.label)).into());
            } else if source.accounts.is_empty() {
                push(t!("{source}: No accounts reported.", source = label(&environment.label, &source.label)));
            }
        }
    }
    notices
}

fn pace_icon(pace: LimitPace, theme: &AppTheme, id: impl Into<SharedString>) -> impl IntoElement {
    let (icon, label) = match pace {
        LimitPace::Ahead => (IconName::TrendingUp, t!("Ahead of pace: spending faster than the window elapses")),
        LimitPace::On => (IconName::Gauge, t!("On pace with the window")),
        LimitPace::Under => (IconName::TrendingDown, t!("Under pace: headroom left for the rest of the window")),
    };
    div()
        .id(id.into())
        .flex()
        .child(Icon::new(icon).size(IconSize::Sm).color(theme.colors.muted_foreground))
        .tooltip(tooltip(label))
}

fn account_initials(email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    let initials: String = local.chars().take(1).chain(domain.chars().take(1)).collect::<String>().to_uppercase();
    if initials.is_empty() { "?".into() } else { initials }
}

fn account_hue(email: &str) -> f32 {
    let mut hash: i32 = 0;
    for unit in email.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(unit as i32);
    }
    (hash.unsigned_abs() % 360) as f32
}

/// A round chip with the account's initials, coloured by its e-mail.
fn account_chip(email: &str, theme: &AppTheme) -> impl IntoElement {
    let hue = account_hue(email);
    let oklch = |l: f64, c: f64| crate::theme_color::oklch_to_srgb(crate::theme_color::Oklch { l, c, h: hue as f64 }, 1.0).hsla();
    div()
        .size(theme.sp(4.))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(oklch(0.85, 0.08))
        .text_color(oklch(0.35, 0.1))
        .ui_text(theme, TextSize::Xs4, weight::SEMIBOLD)
        .child(account_initials(email))
}

fn account_name(account: &LimitAccount, theme: &AppTheme) -> AnyElement {
    if let Some(name) = &account.display_name {
        return div().truncate().child(name.clone()).into_any_element();
    }
    if let Some(email) = &account.email {
        return account_chip(email, theme).into_any_element();
    }
    div().child(driver_label(&account.driver)).into_any_element()
}

/// A redemption in flight or done, kept per account.
#[derive(Clone, Default)]
struct RedeemState {
    busy: bool,
    status: Option<SharedString>,
}

/// The open confirm dialog.
#[derive(Clone)]
struct Confirming {
    account_key: String,
    environment: t3code_contracts::EnvironmentId,
    choice: ResetCreditChoice,
}

/// The Limits view: pools for the selected environments, redemption state,
/// and the confirm dialog. Feed it with [`Self::set_input`].
pub struct UsageLimitsView {
    source: Rc<dyn UsageSource>,
    environments: Vec<UsageEnvironment>,
    hidden: Vec<UsageProviderKind>,
    now: f64,
    pools: Rc<[LimitPool]>,
    notices: Rc<[SharedString]>,
    links: Rc<[t3code_shared::usage_limits::ExternalUsageLink]>,
    redeem: HashMap<String, RedeemState>,
    confirming: Option<Confirming>,
    timestamp_format: TimestampFormat,
    /// The segment whose details are open.
    open_segment: Option<SharedString>,
    /// Whether the pointer is over the open segment or its details.
    hover: SegmentHover,
    _hover_timer: Option<gpui_kit::Task<()>>,
}

/// Where the pointer is relative to a segment's hover popup.
#[derive(Default)]
struct SegmentHover {
    segment: Option<SharedString>,
    over_popup: bool,
}

/// Base UI's `openOnHover` delay.
const HOVER_OPEN_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// Lets the pointer cross the gap between a segment and its details.
const HOVER_CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

impl UsageLimitsView {
    pub fn new(source: Rc<dyn UsageSource>) -> Self {
        Self {
            source,
            environments: Vec::new(),
            hidden: Vec::new(),
            now: 0.0,
            pools: Rc::from(Vec::new()),
            notices: Rc::from(Vec::new()),
            links: Rc::from(Vec::new()),
            redeem: HashMap::new(),
            confirming: None,
            timestamp_format: TimestampFormat::Locale,
            open_segment: None,
            hover: SegmentHover::default(),
            _hover_timer: None,
        }
    }

    /// Recomputes the pools for the selected environments without hidden providers' drivers.
    /// `now` advances only on an explicit refresh, so the page never repaints on a timer.
    pub fn set_input(&mut self, environments: Vec<UsageEnvironment>, hidden: Vec<UsageProviderKind>, now: f64, cx: &mut Context<Self>) {
        let hidden_drivers: Vec<&str> = hidden.iter().map(|p| provider_driver(*p)).collect();
        let visible = |driver: &str| !hidden_drivers.contains(&driver);
        let limits: Vec<LimitEnvironment> = environments
            .iter()
            .map(|environment| {
                let mut limits = environment.limits.clone();
                if !hidden_drivers.is_empty() {
                    limits.providers = limits.providers.iter().filter(|p| visible(&p.driver)).cloned().collect::<Vec<_>>().into();
                    limits.usage_limit_sources = limits
                        .usage_limit_sources
                        .iter()
                        .filter_map(|source| {
                            let mut source = source.clone();
                            let before = source.accounts.len();
                            source.accounts.retain(|a| visible(&a.driver));
                            (!(source.accounts.is_empty() && before > 0)).then_some(source)
                        })
                        .collect::<Vec<_>>()
                        .into();
                }
                limits
            })
            .collect();
        self.pools = collect_limit_pools(collect_limit_accounts(&limits), now).into();
        self.notices = collect_limit_notices(&limits).into();
        self.links = collect_external_usage_links(&limits, |provider, environment| {
            t!("{provider} on {environment}", provider = provider.to_owned(), environment = environment.to_owned()).to_string()
        })
        .into();
        self.environments = environments;
        self.hidden = hidden;
        self.now = now;
        cx.notify();
    }

    fn can_manage(&self, environment: &t3code_contracts::EnvironmentId) -> bool {
        self.environments.iter().any(|e| &e.environment_id == environment && e.can_manage_providers)
    }

    fn redeem(&mut self, cx: &mut Context<Self>) {
        let Some(confirming) = self.confirming.take() else { return };
        if !self.can_manage(&confirming.environment) {
            return;
        }
        let key = confirming.account_key.clone();
        self.redeem.insert(key.clone(), RedeemState { busy: true, status: None });
        let call = self.source.consume_reset_credit(&confirming.environment, confirming.choice.input.clone());
        cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |this, cx| {
                let status = match result {
                    Ok(result) => result.warning.map(SharedString::from).unwrap_or_else(|| outcome_text(result.outcome)),
                    Err(error) if !error.0.is_empty() => error.0,
                    Err(_) => t!("Could not use the reset credit."),
                };
                this.redeem.insert(key, RedeemState { busy: false, status: Some(status) });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn segment_details(&self, account: &Arc<LimitAccount>, window: &UsageWindow, restores: Option<f64>, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let now = self.now;
        let remaining = remaining_percent(window);
        let time_left = pace_remaining_percent(window, now);
        let resets_in = format_resets_in(window, now, ResetsInStyle::In);
        let row = |label: SharedString, value: SharedString| {
            div()
                .flex()
                .gap(theme.sp(3.))
                .child(div().w(theme.sp(18.)).flex_none().text_color(c.muted_foreground).child(label))
                .child(div().min_w_0().text_color(c.foreground()).child(value))
        };
        let where_ = if !account.environments.is_empty() {
            Some((t!("Signed in"), account.environments.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(", ")))
        } else {
            account.source_label.clone().map(|s| (t!("Via", context = "signed in through a source"), s))
        };
        let name = account.display_name.clone().unwrap_or_else(|| driver_label(&account.driver));
        let divider = || div().flex().flex_col().gap(theme.sp(1.)).border_t_1().border_color(with_alpha(c.border, 0.6)).pt(theme.sp(2.5));
        let choices = match (&account.redeem, &account.limits.reset_credits) {
            (Some(redeem), Some(credits)) if credits.available_count > 0 => reset_credit_choices(credits, &redeem.input, now, true),
            _ => Vec::new(),
        };
        let state = self.redeem.get(&account.key).cloned().unwrap_or_default();
        let can_manage = account.redeem.as_ref().is_some_and(|r| self.can_manage(&r.environment_id));
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.5))
            .w(theme.sp(72.))
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(0.5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(c.foreground())
                            .child(match &account.email {
                                Some(email) if account.redeem.is_none() => account_chip(email, theme).into_any_element(),
                                _ => driver_mark(&account.driver, &name, theme.sp(4.), cx),
                            })
                            .child(div().truncate().child(name)),
                    )
                    .children(account.email.clone().map(|email| div().text_color(c.muted_foreground).child(email))),
            )
            .child(
                divider()
                    .children(account.plan.clone().map(|plan| row(t!("Plan", context = "subscription plan"), plan.into())))
                    .children(where_.map(|(label, value)| row(label, value.into()))),
            )
            .child(
                divider()
                    .child(row(t!("Left", context = "remaining usage"), format!("{remaining}%").into()))
                    .children(window.resets_at.as_deref().map(|at| {
                        let upcoming = format_upcoming_timestamp(at, self.timestamp_format, now as i64, None);
                        let value = match &resets_in {
                            Some(resets_in) => format!("{upcoming} · {resets_in}"),
                            None => upcoming.to_string(),
                        };
                        row(t!("Resets", context = "when the usage window resets"), value.into())
                    }))
                    .children(time_left.map(|left| row(t!("Even pace"), t!("{timeLeft}% left by now", timeLeft = left))))
                    .children(restores.filter(|r| *r > 0.0).map(|percent| {
                        row(t!("Restores", context = "a reset credit restores part of the pool"), t!("+{percent}% of pool", percent = percent))
                    })),
            )
            .when(!choices.is_empty(), |el| {
                el.child(divider().gap(theme.sp(1.5)).text_color(c.muted_foreground).children(choices.into_iter().map(|choice| {
                    let (key, environment) = (account.key.clone(), account.redeem.as_ref().map(|r| r.environment_id.clone()));
                    let button_choice = choice.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.sp(3.))
                        .child(choice.summary.clone())
                        .when(choice.available_count > 0, |el| {
                            el.child(
                                div().ml_auto().child(
                                    Button::new(SharedString::from(format!("redeem-{key}-{}", choice.key)))
                                        .variant(ButtonVariant::Outline)
                                        .size(ButtonSize::Xs)
                                        .label(if state.busy { t!("Using…") } else { choice.action_label.clone() })
                                        .disabled(state.busy || !can_manage)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let Some(environment) = environment.clone() else { return };
                                            this.confirming = Some(Confirming { account_key: key.clone(), environment, choice: button_choice.clone() });
                                            cx.notify();
                                        })),
                                ),
                            )
                        })
                })))
            })
            .into_any_element()
    }

    fn render_segment(
        &self,
        pool: &LimitPoolWindow,
        position: usize,
        color: Hsla,
        wide: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let (account, member) = &pool.columns[position];
        let index = position + 1;
        let Some(member) = member else {
            if pool.kind == t3code_contracts::ServerConfigProvidersItemUsageLimitsWindowsItemKind::Monthly && account.limits.zero_spend_cap == Some(true) {
                return Some(
                    div()
                        .h(if wide { theme.sp(8.) } else { theme.sp(5.) })
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(theme.sp(1.5))
                        .px(if wide { theme.sp(2.) } else { px(0.) })
                        .when(!wide, |el| el.justify_center())
                        .rounded(theme.radii.md)
                        .border_1()
                        .border_dashed()
                        .border_color(c.border)
                        .ui_text(theme, if wide { TextSize::Xs } else { TextSize::Xs4 }, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .when(wide, |el| el.child(account_name(account, theme)))
                        .child(t!("$0 cap"))
                        .into_any_element(),
                );
            }
            return Some(div().into_any_element());
        };
        let window = &member.window;
        let now = self.now;
        let remaining = remaining_percent(window) as f32;
        let time_left = pace_remaining_percent(window, now);
        let resets_in = format_resets_in(window, now, ResetsInStyle::Full);
        let refill_in = format_resets_in(window, now, ResetsInStyle::Refill);
        let credits = account.limits.reset_credits.as_ref().map_or(0, |c| c.available_count);
        let reset = pool.resets.iter().find(|r| r.member.account.key == account.key);
        let show_name = pool.columns.len() > 1;
        let height = if wide { theme.sp(8.) } else { theme.sp(5.) };
        let hatch = (remaining < 100.0 && reset.is_some() && member.refill_percent > 0.0)
            .then(|| (100.0 - remaining).min(member.refill_percent as f32));
        let tick = time_left.map(|left| left as f32);
        let fill = div()
            .absolute()
            .inset_0()
            .rounded(theme.radii.md)
            .overflow_hidden()
            .child(div().absolute().top_0().bottom_0().left_0().w(relative(remaining / 100.0)).rounded(theme.radii.md).bg(with_alpha(color, 0.35)))
            .children(hatch.map(|width| {
                div().absolute().top_0().bottom_0().right_0().w(relative(width / 100.0)).child(refill_hatch(color)).overflow_hidden()
            }));
        let chip = (resets_in.is_some() || credits > 0).then(|| {
            div()
                .ml_auto()
                .flex()
                .items_center()
                .gap(theme.sp(1.5))
                .min_w_0()
                .overflow_hidden()
                .rounded(theme.radii.sm)
                .bg(with_alpha(c.background(), 0.85))
                .px(theme.sp(1.5))
                .py(theme.sp(0.5))
                .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                .text_color(c.foreground())
                .children(refill_in.clone())
                .when(credits > 0, |el| {
                    el.when(resets_in.is_some(), |el| el.child(div().text_color(c.muted_foreground).child("·")))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(theme.sp(0.5))
                                .font_weight(weight::SEMIBOLD)
                                .child(Icon::new(IconName::Ticket).size(IconSize::Xs))
                                .child(credits.to_string()),
                        )
                })
        });
        let content = if wide {
            div()
                .relative()
                .h_full()
                .flex()
                .items_center()
                .gap(theme.sp(1.5))
                .px(theme.sp(2.))
                .min_w_0()
                .overflow_hidden()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .when(show_name, |el| {
                    el.child(div().min_w_0().font_weight(weight::MEDIUM).text_color(c.foreground()).child(account_name(account, theme)))
                        .child(div().flex_none().font_weight(weight::SEMIBOLD).text_color(c.foreground()).child(format!("{remaining}%")))
                })
                .children(chip)
                .into_any_element()
        } else {
            div()
                .relative()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .ui_text(theme, TextSize::Xs4, weight::SEMIBOLD)
                .text_color(with_alpha(c.foreground(), 0.8))
                .child(index.to_string())
                .into_any_element()
        };
        let trigger = div()
            .relative()
            .h(height)
            .w_full()
            .min_w_0()
            .rounded(theme.radii.md)
            .bg(c.muted)
            .cursor_pointer()
            .child(fill)
            .children(tick.map(|left| {
                // The tick sits in a 6px gap cut from the bar, kept 3px inside each end.
                let bar = theme.sp(1.5);
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative((left / 100.0).clamp(0.0, 1.0)))
                    .ml(-bar / 2.)
                    .w(bar)
                    .bg(c.background())
                    .child(div().absolute().top(px(2.)).bottom(px(2.)).left(bar / 2. - px(1.)).w(px(2.)).rounded_full().bg(c.foreground()))
            }))
            .child(content);
        let restores = reset.map(|r| r.restores_percent);
        let key = SharedString::from(format!("segment-{}-{}-{}", pool.kind.as_str(), pool.id, account.key));
        let open = self.open_segment.as_ref() == Some(&key);
        // `data-[popup-open]:ring-1 ring-inset ring-border`.
        let ring = open.then(|| div().absolute().inset_0().rounded(theme.radii.md).border_1().border_color(c.border));
        let trigger = trigger.children(ring);
        let details = open.then(|| {
            let details = self.segment_details(account, window, restores, cx);
            below(
                div()
                    .id("segment-details")
                    .occlude()
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.hover_popup(*hovered, cx)))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.open_segment = None;
                        cx.notify();
                    }))
                    .child(PopoverSurface::new().width(PopoverWidth::Auto).child(details)),
            )
        });
        let (hover_key, click_key) = (key.clone(), key.clone());
        Some(
            div()
                .id(key)
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("trigger")
                        .child(trigger)
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| this.hover_segment(&hover_key, *hovered, cx)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_segment = if this.open_segment.as_ref() == Some(&click_key) { None } else { Some(click_key.clone()) };
                            cx.notify();
                        })),
                )
                .children(details)
                .into_any_element(),
        )
    }

    /// The pointer entered or left a segment: open its details after the
    /// hover delay, or close them once it is over neither the segment nor them.
    fn hover_segment(&mut self, key: &SharedString, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.hover.segment = Some(key.clone());
            if self.open_segment.as_ref() == Some(key) {
                return;
            }
            let key = key.clone();
            self._hover_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOVER_OPEN_DELAY).await;
                this.update(cx, |this, cx| {
                    if this.hover.segment.as_ref() == Some(&key) {
                        this.open_segment = Some(key);
                        cx.notify();
                    }
                })
                .ok();
            }));
        } else if self.hover.segment.as_ref() == Some(key) {
            self.hover.segment = None;
            self.schedule_hover_close(cx);
        }
    }

    fn hover_popup(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.hover.over_popup = hovered;
        if !hovered {
            self.schedule_hover_close(cx);
        }
    }

    fn schedule_hover_close(&mut self, cx: &mut Context<Self>) {
        self._hover_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_CLOSE_GRACE).await;
            this.update(cx, |this, cx| {
                let over_open = this.hover.segment.is_some() && this.hover.segment == this.open_segment;
                if !over_open && !this.hover.over_popup && this.open_segment.take().is_some() {
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn render_legend_row(&self, pool: &LimitPoolWindow, position: usize, color: Hsla, theme: &AppTheme) -> Option<AnyElement> {
        let c = &theme.colors;
        let (account, member) = &pool.columns[position];
        let member = member.as_ref()?;
        let refill = format_resets_in(&member.window, self.now, ResetsInStyle::Refill);
        let credits = account.limits.reset_credits.as_ref().map_or(0, |c| c.available_count);
        Some(
            div()
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .h(theme.sp(7.))
                .px(theme.sp(2.))
                .rounded(theme.radii.md)
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .child(
                    div()
                        .relative()
                        .size(theme.sp(4.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .bg(with_alpha(color, 0.35))
                        .ui_text(theme, TextSize::Xs4, weight::SEMIBOLD)
                        .child((position + 1).to_string()),
                )
                .child(div().min_w_0().font_weight(weight::MEDIUM).text_color(c.foreground()).child(account_name(account, theme)))
                .child(div().flex_none().font_weight(weight::SEMIBOLD).text_color(c.foreground()).child(format!("{}%", remaining_percent(&member.window))))
                .child(
                    div()
                        .ml_auto()
                        .flex()
                        .items_center()
                        .gap(theme.sp(1.5))
                        .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .children(refill.clone())
                        .when(credits > 0, |el| {
                            el.when(refill.is_some(), |el| el.child("·")).child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(theme.sp(0.5))
                                    .font_weight(weight::SEMIBOLD)
                                    .text_color(c.foreground())
                                    .child(Icon::new(IconName::Ticket).size(IconSize::Xs))
                                    .child(credits.to_string()),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_window_card(&self, pool: &LimitPool, window: &LimitPoolWindow, wide: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let theme = &theme;
        let c = &theme.colors;
        let pool_color = driver_color(&pool.driver, theme);
        let hides = if pool.driver == "cursor" { cursor_window_hides_details(&window.id, pool) } else { None };
        let label: Option<SharedString> = match hides {
            Some(true) => None,
            Some(false) => Some(match window.id.as_str() {
                "totalPercentUsed" => t!("Overall", context = "usage limit covering the whole plan"),
                "autoPercentUsed" => t!("Cursor Models"),
                _ => t!("Other Models"),
            }),
            None => Some(window.label.clone().into()),
        };
        let description = match (hides, window.id.as_str()) {
            (Some(false), "totalPercentUsed") => Some(t!("All usage on your plan this billing cycle.")),
            (Some(false), "autoPercentUsed") => Some(t!("Grok, Composer, and Auto use this allowance.")),
            (Some(false), _) => Some(t!("Usage allowance for third-party models such as Claude, GPT, and Gemini.")),
            _ => None,
        };
        let next_refill = window.resets.iter().find(|r| r.restores_percent > 0.0);
        let color_of = |account: &LimitAccount| {
            account.accent_color.as_deref().and_then(crate::theme_color::parse_css_color).map_or(pool_color, |c| c.hsla())
        };
        let count = window.columns.len();
        let segments: Vec<AnyElement> = (0..count)
            .filter_map(|position| {
                let color = color_of(&window.columns[position].0);
                self.render_segment(window, position, color, wide, cx).map(|segment| div().flex_1().min_w_0().child(segment).into_any_element())
            })
            .collect();
        let legend: Vec<AnyElement> = if wide {
            Vec::new()
        } else {
            (0..count).filter_map(|position| self.render_legend_row(window, position, color_of(&window.columns[position].0), theme)).collect()
        };
        let statuses: Vec<AnyElement> = window
            .columns
            .iter()
            .filter_map(|(account, _)| {
                let state = self.redeem.get(&account.key)?;
                let text = if state.busy { t!("Using reset credit…") } else { state.status.clone()? };
                Some(
                    div()
                        .flex()
                        .gap(theme.sp(1.))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(div().font_weight(weight::MEDIUM).text_color(c.foreground()).child(account_name(account, theme)))
                        .child(text)
                        .into_any_element(),
                )
            })
            .collect();
        let headline = div()
            .flex()
            .flex_col()
            .gap(theme.sp(1.))
            .w(theme.sp(44.))
            .flex_none()
            .children(label.map(|label| div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(label)))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(theme.sp(2.))
                    .child(div().ui_text(theme, TextSize::Xl3, weight::SEMIBOLD).text_color(c.foreground()).child(format!("{}%", window.remaining_percent)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .ui_text(theme, TextSize::Sm, weight::NORMAL)
                            .text_color(c.muted_foreground)
                            .child(t!("left", context = "remaining usage percentage"))
                            .children(window.pace.map(|pace| pace_icon(pace, theme, format!("pace-{}-{}", pool.driver, window.id)))),
                    ),
            )
            .when_some(next_refill.filter(|_| count > 1), |el, refill| {
                el.child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(c.foreground()).child(format!("↻ +{}%", refill.restores_percent)))
            });
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(3.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(with_alpha(c.border, 0.6))
            .p(theme.sp(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(6.))
                    .child(headline)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(1.))
                            .child(div().flex().gap(theme.sp(1.)).children(segments))
                            .children(legend)
                            .children(statuses),
                    ),
            )
            .children(description.map(|d| div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(d)))
            .into_any_element()
    }
}

impl Render for UsageLimitsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        // The bar's width decides the wide layout, as `@2xl/pool` does on the web.
        let page = (window.viewport_size().width - px(96.)).min(theme.rem(64.)) - theme.sp(12.);
        let bar = page - theme.sp(8.) - theme.sp(44.) - theme.sp(6.);
        let wide = bar >= theme.rem(WIDE_POOL_REM);
        let pools = self.pools.clone();
        let mut sections: Vec<AnyElement> = Vec::new();
        for pool in pools.iter() {
            let label = driver_label(&pool.driver);
            let cards: Vec<AnyElement> =
                display_limit_windows(pool).into_iter().map(|window| self.render_window_card(pool, window, wide, cx)).collect();
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.sp(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(2.))
                            .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(c.foreground())
                            .child(driver_mark(&pool.driver, &label, theme.sp(4.), cx))
                            .child(label),
                    )
                    .children(cards)
                    .into_any_element(),
            );
        }
        let empty = self.pools.is_empty() && self.notices.is_empty() && self.links.is_empty();
        let links = self.links.iter().map(|link| {
            let url = link.url.clone();
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(theme.sp(3.))
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(c.border)
                .p(theme.sp(4.))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap(theme.sp(3.))
                        .when(link.url == CHATGPT_USAGE_URL, |el| el.child(driver_mark("codex", "ChatGPT", theme.sp(5.), cx)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .gap(theme.sp(1.))
                                .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).child(link.label.clone()))
                                .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(link.accounts.join(", ")))
                                .children(if link.url == CHATGPT_USAGE_URL {
                                    Some(t!("View usage in ChatGPT with your connected account."))
                                } else {
                                    link.message.clone().map(SharedString::from)
                                }
                                .map(|text| div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(text))),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("manage-usage-{}", link.url)))
                        .variant(ButtonVariant::GhostMuted)
                        .size(ButtonSize::Xs)
                        .label(t!("Manage usage"))
                        .trailing_icon(IconName::ExternalLink)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
        });
        let notices = (!self.notices.is_empty()).then(|| {
            div()
                .flex()
                .gap(theme.sp(2.))
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(with_alpha(c.warning, 0.32))
                .bg(c.warning_surface)
                .px(theme.sp(3.5))
                .py(theme.sp(3.))
                .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
                .text_color(c.warning_foreground)
                .child(Icon::new(IconName::TriangleAlert).size(IconSize::Md).color(c.warning))
                .child(div().flex().flex_col().gap(theme.sp(1.)).children(self.notices.iter().cloned()))
        });
        let confirm = self.confirming.clone().map(|confirming| {
            let can_manage = self.can_manage(&confirming.environment);
            let close = cx.listener(|this, _, _, cx| {
                this.confirming = None;
                cx.notify();
            });
            let surface = DialogSurface::new(t!("Use a reset credit?"))
                .description(confirming.choice.confirm_description.clone())
                .footer(Button::new("reset-credit-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).on_click(close))
                .footer(
                    Button::new("reset-credit-confirm")
                        .label(t!("Use credit"))
                        .disabled(!can_manage)
                        .on_click(cx.listener(|this, _, _, cx| this.redeem(cx))),
                );
            let entity = cx.entity();
            Modal::dialog("reset-credit-dialog", div().id("reset-credit-content").occlude().w_full().max_w(theme.rem(28.)).child(surface), move |_, cx| {
                entity.update(cx, |this, cx| {
                    this.confirming = None;
                    cx.notify();
                })
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(8.))
            .when(empty, |el| {
                el.child(
                    div()
                        .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(t!("No provider on the selected environments reports subscription limits.")),
                )
            })
            .children(sections)
            .children(links)
            .children(notices)
            .children(confirm)
    }
}

/// The refill share's stripes: `repeating-linear-gradient(135deg, color 0 1px,
/// transparent 1px 5px)` at 20% opacity, as 1px lines every 5px along the diagonal.
fn refill_hatch(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let period = 5.0 * std::f32::consts::SQRT_2;
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let stroke = std::f32::consts::FRAC_1_SQRT_2;
            let mut path = gpui_kit::PathBuilder::stroke(px(1.));
            // Lines x + y = c, measured from the box's top-left; the gradient starts at its top-left corner.
            let mut c = stroke;
            while c < w + h + period {
                path.move_to(bounds.origin + point(px(c), px(0.)));
                path.line_to(bounds.origin + point(px(c - h), px(h)));
                c += period;
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, with_alpha(color, 0.2));
            }
        },
    )
    .size_full()
}
