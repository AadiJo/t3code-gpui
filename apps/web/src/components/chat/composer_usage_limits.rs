//! The `/usage-limits` banner's body (`ComposerUsageLimits.tsx` `UsageLimitsBannerBody`), with
//! the compact `LimitWindows`, `ResetCredits` and `RedactedSensitiveText` of `UsageLimits.tsx`
//! and `RedactedSensitiveText.tsx`. The live layer builds a [`UsageLimitsBody`] from
//! `collect_provider_usage_limits`; the composer draws it under the banner's row and raises
//! [`ComposerEvent`]s for the links and credits.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, Bounds, ContentMask, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, canvas, div, fill, point, prelude::FluentBuilder as _, px, size,
};
use t3code_shared::usage_limits::LimitPace;

use crate::components::chat::composer::{ChatComposer, ComposerEvent};
use crate::components::chat::composer_theme::ComposerColors;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::components::usage::usage_limits::ResetCreditChoice;
use crate::i18n::t;

/// What the banner lists: each account's windows or notice, its links and credits, then the
/// hubs' notices.
#[derive(Clone, Debug)]
pub struct UsageLimitsBody {
    pub accounts: Vec<UsageAccount>,
    pub notices: Vec<SharedString>,
}

/// One account of the report.
#[derive(Clone, Debug)]
pub struct UsageAccount {
    pub id: SharedString,
    /// Shown over the account when the report has several (`AccountSummary`).
    pub label: Option<AccountLabel>,
    /// Why there are no windows (`limitsNotice`).
    pub notice: Option<SharedString>,
    pub windows: Vec<UsageWindowRow>,
    /// The bar colour: the instance's accent, else the driver's.
    pub color: Hsla,
    /// `limits.externalUsage.url`: "Manage usage".
    pub manage_url: Option<SharedString>,
    pub credits: Vec<ResetCreditChoice>,
    /// The last redemption's outcome.
    pub status: Option<SharedString>,
    pub busy: bool,
    /// The session may manage providers (`AuthProvidersManageScope`).
    pub can_manage: bool,
}

/// An account's name and plan (`accountLabel`, `AccountSummary`).
#[derive(Clone, Debug, PartialEq)]
pub struct AccountLabel {
    pub text: SharedString,
    pub plan: Option<SharedString>,
}

impl AccountLabel {
    /// A label holding an email is redacted until clicked.
    pub fn sensitive(&self) -> bool {
        self.text.contains('@')
    }

    /// The summary as plain text, the email redacted (the banner's description).
    pub fn plain(&self) -> SharedString {
        let text = if self.sensitive() { redacted_placeholder(&self.text) } else { self.text.to_string() };
        match &self.plan {
            Some(plan) => format!("{text} · {plan}").into(),
            None => text.into(),
        }
    }
}

/// One limit window, measured at the panel's opening.
#[derive(Clone, Debug)]
pub struct UsageWindowRow {
    pub id: SharedString,
    pub label: SharedString,
    pub remaining: i64,
    /// Percent of the window left; draws the pace mark.
    pub time_left: Option<f64>,
    pub pace: Option<LimitPace>,
    pub resets_in: Option<SharedString>,
    /// The bar's tooltip: the first line, then muted lines.
    pub tooltip: Vec<SharedString>,
}

const REDACTED_TEXT_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// `redactedPlaceholder`: the same letters for the same value, separators kept.
pub fn redacted_placeholder(value: &str) -> String {
    let mut state: u32 = 0x811c_9dc5;
    for unit in value.encode_utf16() {
        state ^= unit as u32;
        state = state.wrapping_mul(0x0100_0193);
    }
    let mut next = || {
        state = (state ^ (state >> 13)).wrapping_mul(0x85eb_ca6b);
        state = (state ^ (state >> 16)).wrapping_mul(0xc2b2_ae35);
        let index = (state as i32).unsigned_abs() as usize % REDACTED_TEXT_ALPHABET.len();
        REDACTED_TEXT_ALPHABET[index] as char
    };
    value.chars().map(|c| if matches!(c, '@' | '.' | '-' | '_') { c } else { next() }).collect()
}

fn pace_label(pace: LimitPace) -> (IconName, SharedString) {
    match pace {
        LimitPace::Ahead => (IconName::TrendingUp, t!("Ahead of pace: spending faster than the window elapses")),
        LimitPace::On => (IconName::Gauge, t!("On pace with the window")),
        LimitPace::Under => (IconName::TrendingDown, t!("Under pace: headroom left for the rest of the window")),
    }
}

/// `WindowBar`: the track and fill inset 6px top and bottom; the pace tick sits in a 6px gap cut
/// from both, clamped 3px inside the ends, once the bar is 6px wide.
fn window_bar(window: &UsageWindowRow, color: Hsla, c: &ComposerColors) -> impl IntoElement {
    let (remaining, time_left) = ((window.remaining.clamp(0, 100) as f32) / 100., window.time_left);
    let (track, tick) = (c.muted, c.foreground);
    let (inset, tick_inset) = (c.r(px(6.)), c.r(px(4.)));
    let mut lines = window.tooltip.iter();
    let surface = lines.next().cloned().map(|first| lines.fold(TooltipSurface::new(first), |s, line| s.line(line.clone())));
    div()
        .id(SharedString::from(format!("usage-limits-bar-{}", window.id)))
        .h(c.r(px(24.)))
        .w_full()
        .flex_none()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let width = bounds.size.width;
                    let bar = Bounds::new(point(bounds.origin.x, bounds.origin.y + inset), size(width, bounds.size.height - inset * 2.));
                    let radius = bar.size.height / 2.;
                    let gap = time_left.filter(|_| width >= px(6.)).map(|left| {
                        let at = (width * (left as f32 / 100.)).clamp(px(3.), width - px(3.));
                        (bounds.origin.x + at - px(3.), bounds.origin.x + at + px(3.))
                    });
                    let mut paint = |window: &mut gpui_kit::Window| {
                        window.paint_quad(fill(bar, track).corner_radii(radius));
                        if remaining > 0. {
                            window.paint_quad(fill(Bounds::new(bar.origin, size(width * remaining, bar.size.height)), color).corner_radii(radius));
                        }
                    };
                    match gap {
                        None => paint(window),
                        Some((left, right)) => {
                            let full = bounds.size.height;
                            let before = Bounds::new(bounds.origin, size(left - bounds.origin.x, full));
                            let after = Bounds::new(point(right, bounds.origin.y), size(bounds.origin.x + width - right, full));
                            window.with_content_mask(Some(ContentMask { bounds: before }), |window| paint(window));
                            window.with_content_mask(Some(ContentMask { bounds: after }), |window| paint(window));
                            let mark = Bounds::new(point(left + px(2.), bounds.origin.y + tick_inset), size(px(2.), full - tick_inset * 2.));
                            window.paint_quad(fill(mark, tick).corner_radii(px(1.)));
                        }
                    }
                },
            )
            .size_full(),
        )
        .when_some(surface, |el, surface| el.tooltip(tooltip(surface)))
}

/// `LimitWindows compact`: a grid of `minmax(0,9rem) minmax(3rem,1fr) auto` with 12px between
/// columns and 2px between rows; every row is the bar's 24px.
fn limit_windows(account: &UsageAccount, c: &ComposerColors) -> AnyElement {
    let row = |c: &ComposerColors| div().h(c.r(px(24.))).flex().items_center().gap(c.r(px(8.))).text_size(c.r(px(12.))).line_height(c.r(px(16.)));
    let labels = account.windows.iter().map(|window| {
        row(c)
            .min_w_0()
            .child(div().min_w_0().truncate().text_color(c.muted_foreground).child(window.label.clone()))
            .child(div().ml_auto().flex_none().font_weight(gpui_kit::FontWeight::MEDIUM).text_color(c.foreground).child(t!("{remaining}% left", remaining = window.remaining)))
    });
    let bars = account.windows.iter().map(|window| window_bar(window, account.color, c));
    let resets = account.windows.iter().map(|window| {
        row(c)
            .whitespace_nowrap()
            .text_color(c.muted_foreground)
            .children(window.pace.map(|pace| {
                let (icon, label) = pace_label(pace);
                div()
                    .id(SharedString::from(format!("usage-limits-pace-{}", window.id)))
                    .flex()
                    .child(Icon::new(icon).size(IconSize::Custom(c.r(px(14.)))).color(c.muted_foreground))
                    .tooltip(tooltip(label))
            }))
            .child(div().ml_auto().flex_none().child(window.resets_in.clone().unwrap_or_default()))
    });
    let column = |c: &ComposerColors| div().flex().flex_col().gap(c.r(px(2.)));
    div()
        .flex()
        .gap(c.r(px(12.)))
        .child(column(c).min_w_0().w(c.r(px(144.))).flex_shrink(1.).children(labels))
        .child(column(c).flex_1().min_w(c.r(px(48.))).children(bars))
        .child(column(c).flex_none().children(resets))
        .into_any_element()
}

impl ChatComposer {
    /// The account's name, redacted until clicked when it is an email.
    fn account_summary(&self, account: &UsageAccount, label: &AccountLabel, c: &ComposerColors, cx: &mut Context<Self>) -> AnyElement {
        let plan = label.plan.clone().map(|plan| SharedString::from(format!(" · {plan}")));
        if !label.sensitive() {
            return div().min_w_0().truncate().child(SharedString::from(format!("{}{}", label.text, plan.unwrap_or_default()))).into_any_element();
        }
        let revealed = self.usage_revealed.contains(&account.id);
        let id = account.id.clone();
        let shown: SharedString = if revealed { label.text.clone() } else { redacted_placeholder(&label.text).into() };
        let hint = if revealed { t!("Click to hide account") } else { t!("Click to reveal account") };
        let hover = c.foreground;
        div()
            .min_w_0()
            .flex()
            .items_baseline()
            .child(
                div()
                    .id(SharedString::from(format!("usage-limits-account-{id}")))
                    .min_w_0()
                    .truncate()
                    .rounded(c.r(px(4.)))
                    .cursor_pointer()
                    .text_color(c.muted_foreground)
                    .hover(move |s| s.text_color(hover))
                    .child(shown)
                    .tooltip(tooltip(hint))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.usage_revealed.remove(&id) {
                            this.usage_revealed.insert(id.clone());
                        }
                        cx.notify();
                    })),
            )
            .children(plan)
            .into_any_element()
    }

    /// `ResetCredits`: each choice's summary and its button, then the outcome.
    fn reset_credits(&self, account: &UsageAccount, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let available: u64 = account.credits.iter().map(|choice| choice.available_count).sum();
        if account.credits.is_empty() || (available == 0 && account.status.is_none()) {
            return None;
        }
        let rows = account.credits.iter().map(|choice| {
            let button = (choice.available_count > 0).then(|| {
                let (account_id, choice) = (account.id.clone(), choice.clone());
                Button::new(SharedString::from(format!("usage-limits-credit-{}-{}", account.id, choice.key)))
                    .size(ButtonSize::Xs)
                    .variant(ButtonVariant::Outline)
                    .label(if account.busy { t!("Using…") } else { choice.action_label.clone() })
                    .disabled(account.busy || !account.can_manage)
                    .on_click({
                        let this = cx.entity().downgrade();
                        move |_, _, cx| {
                            _ = this.update(cx, |this, cx| {
                                this.usage_confirming = Some((account_id.clone(), choice.clone()));
                                cx.notify();
                            });
                        }
                    })
            });
            div().flex().flex_wrap().items_center().gap_x(c.r(px(12.))).gap_y(c.r(px(4.))).child(div().child(choice.summary.clone())).children(button)
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(c.r(px(4.)))
                .text_size(c.r(px(12.)))
                .line_height(c.r(px(16.)))
                .text_color(c.muted_foreground)
                .children(rows)
                .children(account.status.clone().map(|status| div().text_color(c.foreground).child(status)))
                .into_any_element(),
        )
    }

    /// The body under the banner's row (`ComposerBanner.Scroll` > `ComposerBanner.Body`):
    /// scrolls past `min(24rem, 40dvh)`.
    pub(super) fn render_usage_limits_body(&self, banner_id: &SharedString, body: &Arc<UsageLimitsBody>, c: &ComposerColors, max_height: Pixels, cx: &mut Context<Self>) -> AnyElement {
        let several = body.accounts.len() > 1;
        let accounts = body.accounts.iter().map(|account| {
            let summary = account.label.as_ref().filter(|_| several).map(|label| {
                div().text_size(c.r(px(12.))).line_height(c.r(px(16.))).text_color(c.muted_foreground).min_w_0().child(self.account_summary(account, label, c, cx))
            });
            let windows = match &account.notice {
                Some(notice) => div().text_size(c.r(px(12.))).line_height(c.r(px(16.))).text_color(c.muted_foreground).child(notice.clone()).into_any_element(),
                None => limit_windows(account, c),
            };
            let manage = account.manage_url.clone().map(|url| {
                div().flex().child(
                    Button::new(SharedString::from(format!("usage-limits-manage-{}", account.id)))
                        .size(ButtonSize::Sm)
                        .variant(ButtonVariant::Link)
                        .label(t!("Manage usage"))
                        .on_click({
                            let this = cx.entity().downgrade();
                            move |_, _, cx| {
                                _ = this.update(cx, |_, cx| cx.emit(ComposerEvent::OpenUrl(url.clone())));
                            }
                        }),
                )
            });
            div()
                .flex()
                .flex_col()
                .gap(c.r(px(4.)))
                .min_w_0()
                .children(summary)
                .child(windows)
                .children(manage)
                .children(self.reset_credits(account, c, cx))
        });
        let notices = body.notices.iter().map(|notice| div().text_size(c.r(px(12.))).line_height(c.r(px(16.))).text_color(c.muted_foreground).child(notice.clone()));
        div()
            .id(SharedString::from(format!("{banner_id}-scroll")))
            .max_h(max_height)
            .overflow_y_scroll()
            .child(
                div()
                    .min_w_0()
                    .pl(c.r(px(28.)))
                    .pt(c.r(px(4.)))
                    .pb(c.r(px(6.)))
                    .pr(c.r(px(8.)))
                    .flex()
                    .flex_col()
                    .gap(c.r(px(8.)))
                    .children(accounts)
                    .children(notices),
            )
            .into_any_element()
    }

    /// The reset credit confirmation (`ResetCreditDialog`), while one is open.
    pub(super) fn render_reset_credit_dialog(&self, c: &ComposerColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::components::ui::dialog::{DialogSurface, Modal};
        let (account_id, choice) = self.usage_confirming.clone()?;
        let can_manage = self.state().banners.iter().filter_map(|b| b.body.as_ref()).flat_map(|b| b.accounts.iter()).find(|a| a.id == account_id).is_some_and(|a| a.can_manage);
        let close = cx.listener(|this, _, _, cx| {
            this.usage_confirming = None;
            cx.notify();
        });
        let confirm = cx.listener(move |this, _, _, cx| {
            if let Some((account, choice)) = this.usage_confirming.take() {
                cx.emit(ComposerEvent::RedeemResetCredit { account, choice });
            }
            cx.notify();
        });
        let surface = DialogSurface::new(t!("Use a reset credit?"))
            .description(choice.confirm_description.clone())
            .footer(Button::new("usage-limits-credit-cancel").variant(ButtonVariant::Outline).label(t!("Cancel")).on_click(close))
            .footer(Button::new("usage-limits-credit-confirm").label(t!("Use credit")).disabled(!can_manage).on_click(confirm));
        let entity = cx.entity();
        Some(
            Modal::dialog("usage-limits-credit-dialog", div().id("usage-limits-credit-content").occlude().w_full().max_w(c.r(px(448.))).child(surface), move |_, cx| {
                entity.update(cx, |this, cx| {
                    this.usage_confirming = None;
                    cx.notify();
                })
            })
            .into_any_element(),
        )
    }
}
