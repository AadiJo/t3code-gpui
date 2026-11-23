//! How pull request facts look: state glyphs and their tones, check and review
//! icons, label chips, author avatars, diff counts and relative times. Ports
//! `pullRequestPresentation.tsx` and `pullRequestIcons.tsx`.

use gpui_kit::{AnyElement, App, Div, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, div, px, rgb};
use t3code_contracts::{PullRequestCheckStatus, PullRequestMergeability, PullRequestReviewDecision, PullRequestState};

use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::pull_request::pull_request_list_logic::pull_request_label_color;
use crate::components::ui::TextStyleExt as _;
use crate::diff_count::format_diff_count;
use crate::i18n::t;
use crate::timestamp_format::format_relative_time;
use crate::{ActiveAppTheme as _, AppTheme};

/// The Tailwind tones the web app uses for states, which themes do not change.
fn tone(theme: &AppTheme, light: u32, dark: u32, dark_alpha: f32) -> Hsla {
    if theme.is_dark() {
        let mut c: Hsla = rgb(dark).into();
        c.a = dark_alpha;
        c
    } else {
        rgb(light).into()
    }
}

/// `text-emerald-600 dark:text-emerald-300/90`.
pub fn open_tone(theme: &AppTheme) -> Hsla {
    tone(theme, 0x059669, 0x6ee7b7, 0.9)
}

/// `text-violet-600 dark:text-violet-300/90`.
pub fn merged_tone(theme: &AppTheme) -> Hsla {
    tone(theme, 0x7c3aed, 0xc4b5fd, 0.9)
}

/// `text-red-600 dark:text-red-300/90`.
pub fn closed_tone(theme: &AppTheme) -> Hsla {
    tone(theme, 0xdc2626, 0xfca5a5, 0.9)
}

/// `text-zinc-500 dark:text-zinc-400/80`.
pub fn draft_tone(theme: &AppTheme) -> Hsla {
    tone(theme, 0x71717a, 0xa1a1aa, 0.8)
}

/// `text-amber-600 dark:text-amber-300`.
pub fn pending_tone(theme: &AppTheme) -> Hsla {
    tone(theme, 0xd97706, 0xfcd34d, 1.)
}

/// A state's icon, tone and label.
pub fn pull_request_state_presentation(theme: &AppTheme, state: PullRequestState, is_draft: bool) -> (IconName, Hsla, SharedString) {
    match state {
        PullRequestState::Open if is_draft => (IconName::GitPullRequestDraft, draft_tone(theme), t!("Draft", context = "pull request state")),
        PullRequestState::Open => (IconName::GitPullRequestArrow, open_tone(theme), t!("Open", context = "pull request state")),
        PullRequestState::Closed => (IconName::GitPullRequestClosed, closed_tone(theme), t!("Closed", context = "pull request state")),
        PullRequestState::Merged => (IconName::GitMerge, merged_tone(theme), t!("Merged", context = "pull request state")),
    }
}

/// The 16px state glyph with the conflict badge an open, ready pull request shows.
pub fn state_glyph(theme: &AppTheme, state: PullRequestState, is_draft: bool, mergeability: Option<PullRequestMergeability>) -> Div {
    let (icon, color, _) = pull_request_state_presentation(theme, state, is_draft);
    let conflicting = state == PullRequestState::Open && !is_draft && mergeability == Some(PullRequestMergeability::Conflicting);
    div()
        .relative()
        .flex()
        .flex_none()
        .size(px(16.))
        .child(Icon::new(icon).size(IconSize::Md).color(color))
        .when_conflict(conflicting, theme)
}

trait ConflictBadge {
    fn when_conflict(self, conflicting: bool, theme: &AppTheme) -> Self;
}

impl ConflictBadge for Div {
    fn when_conflict(self, conflicting: bool, theme: &AppTheme) -> Self {
        if !conflicting {
            return self;
        }
        self.child(
            div()
                .absolute()
                .right(px(-4.))
                .bottom(px(-4.))
                .rounded_full()
                .bg(theme.colors.background())
                .child(Icon::new(IconName::TriangleAlert).size(IconSize::Xs).stroke(2.5).color(theme.colors.destructive())),
        )
    }
}

/// A check's icon and tone.
pub fn check_status_icon(theme: &AppTheme, status: PullRequestCheckStatus) -> (IconName, Hsla) {
    let muted = theme.colors.muted_foreground;
    match status {
        PullRequestCheckStatus::Success => (IconName::CircleCheck, open_tone(theme)),
        PullRequestCheckStatus::Failure => (IconName::CircleX, closed_tone(theme)),
        PullRequestCheckStatus::Cancelled => (IconName::CircleSlash, closed_tone(theme)),
        PullRequestCheckStatus::Pending => (IconName::CircleDashed, pending_tone(theme)),
        PullRequestCheckStatus::ActionRequired => (IconName::CircleAlert, pending_tone(theme)),
        PullRequestCheckStatus::Skipped | PullRequestCheckStatus::Neutral => (IconName::CircleSlash, muted.opacity(0.7)),
    }
}

/// A check's status label.
pub fn check_status_label(status: PullRequestCheckStatus) -> SharedString {
    match status {
        PullRequestCheckStatus::Success => t!("Passed", context = "check status"),
        PullRequestCheckStatus::Failure => t!("Failed", context = "check status"),
        PullRequestCheckStatus::Cancelled => t!("Cancelled", context = "check status"),
        PullRequestCheckStatus::Pending => t!("Running", context = "check status"),
        PullRequestCheckStatus::ActionRequired => t!("Awaiting action"),
        PullRequestCheckStatus::Skipped => t!("Skipped", context = "check status"),
        PullRequestCheckStatus::Neutral => t!("Neutral", context = "check status"),
    }
}

/// The list row's review decision glyph.
pub fn review_decision_icon(theme: &AppTheme, decision: PullRequestReviewDecision) -> (IconName, Hsla, SharedString) {
    match decision {
        PullRequestReviewDecision::Approved => (IconName::Check, open_tone(theme), t!("Approved", context = "review decision")),
        PullRequestReviewDecision::ChangesRequested => (IconName::MessageSquareWarning, closed_tone(theme), t!("Changes requested")),
        PullRequestReviewDecision::ReviewRequired => (IconName::Eye, pending_tone(theme), t!("Review required")),
    }
}

/// A label chip: the host's colour as a dot and tinted border.
pub fn label_chip(theme: &AppTheme, name: &str, color: Option<&str>) -> Div {
    let dot = pull_request_label_color(color)
        .and_then(|hex| u32::from_str_radix(&hex[1..], 16).ok())
        .map(|value| Hsla::from(rgb(value)))
        .unwrap_or(theme.colors.muted_foreground);
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .h(px(16.))
        .px(px(6.))
        .rounded_full()
        .border_1()
        .border_color(dot.opacity(0.45))
        .bg(dot.opacity(0.1))
        .ui_text(theme, TextSize::Xs3, weight::MEDIUM)
        .text_color(theme.colors.text)
        .child(div().size(px(6.)).rounded_full().bg(dot))
        .child(div().max_w(px(140.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(name.to_owned())))
}

/// A round avatar with the login's initial, coloured from the login.
pub fn avatar(theme: &AppTheme, login: Option<&str>, size: f32) -> Div {
    let login = login.unwrap_or("ghost");
    let hash = login.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    let hue = (hash % 360) as f32 / 360.;
    let bg = gpui_kit::hsla(hue, 0.45, if theme.is_dark() { 0.32 } else { 0.82 }, 1.);
    let initial: String = login.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded_full()
        .bg(bg)
        .text_size(px((size * 0.55).max(7.)))
        .text_color(theme.colors.text)
        .child(initial)
}

/// `+12 −3` in the diff tones, or nothing for an unmeasured 0/0.
pub fn diff_stat(theme: &AppTheme, additions: u64, deletions: u64) -> Option<AnyElement> {
    if additions == 0 && deletions == 0 {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_none()
            .gap(px(4.))
            .mono_text(px(11.), weight::NORMAL)
            .child(div().text_color(theme.fixed.diff_addition_foreground).child(format!("+{}", format_diff_count(additions))))
            .child(div().text_color(theme.fixed.diff_deletion_foreground).child(format!("−{}", format_diff_count(deletions))))
            .into_any_element(),
    )
}

/// "5m ago" for a host timestamp at `now_ms`, or empty when it does not parse.
pub fn relative_label(iso: &str, now_ms: i64) -> SharedString {
    format_relative_time(iso, now_ms).map(|p| p.label).unwrap_or_default()
}

/// The theme, for helpers that only need to read it.
pub fn theme(cx: &App) -> &AppTheme {
    cx.app_theme()
}
