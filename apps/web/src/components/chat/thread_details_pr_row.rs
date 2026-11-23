//! A pull request in the thread details card, and its linked issues. Ports
//! `ThreadDetailsPrRow.tsx`, `ThreadDetailsIssueRows.tsx` and the row action of
//! `pullRequestDetail.logic.ts`: a plain "#N: title" row until the detail
//! loads, then the title over a meta line with the checks rollup and one
//! action pill (Resolve, Ready, Fix or Merge).

use gpui_kit::{AnyElement, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::*, px};
use t3code_contracts::{
    PullRequestAction, PullRequestCheck, PullRequestCheckStatus, PullRequestDetail, PullRequestMergeability,
    PullRequestState, ThreadIssueLink,
};

use super::thread_details_panel_styles::{ROW_LINE_HEIGHT, row, row_base, row_font, row_hover};
use crate::AppTheme;
use crate::appearance_fonts::weight;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::tooltip::tooltip;
use crate::components::ui::TextStyleExt as _;
use crate::appearance_fonts::TextSize;
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// `PullRequestChecksState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksState {
    None,
    Pending,
    Failing,
    Passing,
}

/// `classifyPullRequestChecks`.
pub fn classify_checks(checks: &[PullRequestCheck]) -> ChecksState {
    use PullRequestCheckStatus as S;
    if checks.is_empty() {
        ChecksState::None
    } else if checks.iter().any(|c| matches!(c.status, S::Failure | S::Cancelled)) {
        ChecksState::Failing
    } else if checks.iter().any(|c| matches!(c.status, S::Pending | S::ActionRequired)) {
        ChecksState::Pending
    } else {
        ChecksState::Passing
    }
}

/// `pullRequestChecksState`: the rollup icon, `None` when nothing reported.
pub fn checks_rollup(checks: &[PullRequestCheck]) -> Option<ChecksState> {
    use PullRequestCheckStatus as S;
    if checks.iter().any(|c| matches!(c.status, S::Failure | S::Cancelled)) {
        Some(ChecksState::Failing)
    } else if checks.iter().any(|c| matches!(c.status, S::Pending | S::ActionRequired)) {
        Some(ChecksState::Pending)
    } else {
        checks.iter().any(|c| c.status == S::Success).then_some(ChecksState::Passing)
    }
}

/// The row's one action (`ThreadPanelPullRequestAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRequestRowAction {
    Resolve,
    Ready,
    Fix,
    Merge,
}

fn can_perform(detail: &PullRequestDetail, action: PullRequestAction) -> bool {
    detail.capabilities.actions.contains(&action) && detail.viewer_permissions.actions.contains(&action)
}

/// `allowedPullRequestMergeMethods` is non-empty.
fn has_merge_method(detail: &PullRequestDetail) -> bool {
    use t3code_contracts::PullRequestMergeMethod as M;
    let caps = &detail.merge_capabilities;
    detail.capabilities.merge_methods.iter().any(|m| match m {
        M::Merge => caps.merge,
        M::Squash => caps.squash,
        M::Rebase => caps.rebase,
        M::FastForward => caps.fast_forward == Some(true),
    })
}

/// `resolveThreadPanelPullRequestAction`.
pub fn resolve_row_action(detail: Option<&PullRequestDetail>) -> Option<PullRequestRowAction> {
    let detail = detail.filter(|d| d.state == PullRequestState::Open)?;
    if detail.mergeability == PullRequestMergeability::Conflicting {
        return Some(PullRequestRowAction::Resolve);
    }
    if detail.is_draft {
        return can_perform(detail, PullRequestAction::Ready).then_some(PullRequestRowAction::Ready);
    }
    match classify_checks(&detail.checks) {
        ChecksState::Failing => Some(PullRequestRowAction::Fix),
        ChecksState::Pending => None,
        _ => (can_perform(detail, PullRequestAction::Merge) && has_merge_method(detail)).then_some(PullRequestRowAction::Merge),
    }
}

/// The state tones of `PULL_REQUEST_STATE_PRESENTATION`. Merged and draft are
/// Tailwind's fixed violet and zinc, which no theme changes.
pub fn pull_request_state_tone(theme: &AppTheme, state: PullRequestState, is_draft: bool) -> (IconName, Hsla) {
    let dark = theme.is_dark();
    match (state, is_draft) {
        (PullRequestState::Open, true) => (
            IconName::GitPullRequestDraft,
            if dark { with_alpha(gpui_kit::rgb(0xa1a1aa).into(), 0.8) } else { gpui_kit::rgb(0x71717a).into() },
        ),
        (PullRequestState::Open, false) => (IconName::GitPullRequestArrow, theme.fixed.success),
        (PullRequestState::Closed, _) => (IconName::GitPullRequestClosed, theme.colors.destructive()),
        (PullRequestState::Merged, _) => (
            IconName::GitMerge,
            if dark { with_alpha(gpui_kit::rgb(0xc4b5fd).into(), 0.9) } else { gpui_kit::rgb(0x7c3aed).into() },
        ),
    }
}

/// What the row shows before and after the detail loads.
#[derive(Clone, Debug, PartialEq)]
pub struct PullRequestRowData {
    pub number: u64,
    pub title: SharedString,
    pub url: SharedString,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub detail: Option<PullRequestDetail>,
    /// The thread watches it (checks, comments, conflicts wake the agent).
    pub watching: bool,
}

/// One pull request row. `on_open` opens the URL; `on_action` runs the pill.
pub fn pull_request_row(
    index: usize,
    theme: &AppTheme,
    data: &PullRequestRowData,
    on_open: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
    on_action: impl Fn(PullRequestRowAction, &mut gpui_kit::Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let detail = data.detail.as_ref();
    let (state, draft) = detail.map_or((data.state, data.is_draft), |d| (d.state, d.is_draft));
    let (icon, tone) = pull_request_state_tone(theme, state, draft);
    let action = resolve_row_action(detail);
    let conflicting = detail.is_some_and(|d| d.state == PullRequestState::Open && d.mergeability == PullRequestMergeability::Conflicting);
    let rollup = detail.and_then(|d| checks_rollup(&d.checks)).filter(|_| !conflicting && !draft);
    let tooltip_text: SharedString = format!("#{}: {}", data.number, data.title).into();
    let open = std::rc::Rc::new(on_open);
    let open_click = open.clone();
    let title_icon = Icon::new(icon).size(IconSize::Md).color(tone);
    if detail.is_none() || (rollup.is_none() && action.is_none() && !data.watching) {
        let label: SharedString =
            if data.title.trim().is_empty() { format!("#{}", data.number).into() } else { format!("#{}: {}", data.number, data.title).into() };
        return row(("pr-row", index), theme, Some(title_icon), label, false)
            .tooltip(tooltip(tooltip_text))
            .on_click(move |_, window, cx| open_click(window, cx))
            .into_any_element();
    }
    let muted = theme.colors.muted_foreground;
    let checks = rollup.map(|state| {
        let (icon, color) = match state {
            ChecksState::Failing => (IconName::CircleX, theme.colors.destructive()),
            ChecksState::Pending => (IconName::CircleDot, theme.colors.warning),
            _ => (IconName::CircleCheck, theme.fixed.success),
        };
        let checks = &detail.expect("rollup needs detail").checks;
        let incomplete = checks.iter().filter(|c| matches!(c.status, PullRequestCheckStatus::Pending | PullRequestCheckStatus::ActionRequired)).count();
        let failed = checks.iter().filter(|c| matches!(c.status, PullRequestCheckStatus::Failure | PullRequestCheckStatus::Cancelled)).count();
        let text: Option<SharedString> = if incomplete > 0 {
            Some(format!("{}/{} checks", checks.len() - incomplete, checks.len()).into())
        } else if failed > 0 {
            Some(t!("{count, plural, one {{count} failed} other {{count} failed}}", count = failed))
        } else {
            None
        };
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(24.))
            .px(px(6.))
            .rounded(theme.radii.md)
            .text_color(muted)
            .child(Icon::new(icon).size(IconSize::Sm).color(color))
            .children(text)
    });
    let pill = action.map(|action| {
        let destructive = matches!(action, PullRequestRowAction::Resolve | PullRequestRowAction::Fix);
        let label = match action {
            PullRequestRowAction::Resolve => t!("Resolve", context = "verb, resolves merge conflicts"),
            PullRequestRowAction::Ready => t!("Ready", context = "verb, marks a draft pull request ready for review"),
            PullRequestRowAction::Fix => t!("Fix", context = "verb, fixes failing checks"),
            PullRequestRowAction::Merge => t!("Merge", context = "verb, merges a pull request"),
        };
        let (bg, hover, fg) = if destructive {
            (with_alpha(theme.colors.destructive(), 0.1), with_alpha(theme.colors.destructive(), 0.16), theme.colors.destructive())
        } else if theme.is_dark() {
            (gpui_kit::hsla(0., 0., 1., 0.08), gpui_kit::hsla(0., 0., 1., 0.12), with_alpha(theme.colors.foreground(), 0.8))
        } else {
            (gpui_kit::hsla(0., 0., 0., 0.035), gpui_kit::hsla(0., 0., 0., 0.06), with_alpha(theme.colors.foreground(), 0.8))
        };
        div()
            .id(("pr-action", index))
            .flex()
            .items_center()
            .gap(px(4.))
            .h(px(20.))
            .px(px(6.))
            .rounded(theme.radii.md)
            .bg(bg)
            .text_color(fg)
            .font_weight(weight::MEDIUM)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(label)
            .when(destructive, |p| p.child(Icon::new(IconName::ArrowUpRight).size(IconSize::Xs)))
            .on_click(move |_, window, cx| on_action(action, window, cx))
    });
    let hover = row_hover(theme);
    div()
        .flex()
        .flex_col()
        .pb(px(6.))
        .child(
            row_font(div())
                .id(("pr-title", index))
                .flex()
                .items_start()
                .gap(px(10.))
                .min_h(px(32.))
                .pt(px(6.))
                .px(px(10.))
                .rounded(theme.radii.lg)
                .text_color(with_alpha(theme.colors.foreground(), 0.8))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(tooltip(tooltip_text))
                .on_click(move |_, window, cx| open(window, cx))
                .child(div().mt(px(1.)).child(title_icon))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().line_clamp(2).line_height(px(18.)).child(data.title.clone())),
        )
        .child(
            div()
                .flex()
                .h(px(20.))
                .min_w_0()
                .items_center()
                .gap(px(2.))
                .pl(px(36.))
                .pr(px(10.))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(muted)
                .child(div().mr(px(4.)).flex_none().child(format!("#{}", data.number)))
                .children(checks)
                .child(div().flex_1())
                .children(pill),
        )
        .into_any_element()
}

/// One linked issue (`ThreadDetailsIssueRows`): a ticket icon and "KEY title".
pub fn issue_row(index: usize, theme: &AppTheme, link: &ThreadIssueLink, on_open: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App) + 'static) -> AnyElement {
    let title: SharedString = link.title.clone().into();
    row_base(("issue-row", index), theme, false, false)
        .w_full()
        .child(Icon::new(IconName::CircleDot).size(IconSize::Md).color(theme.colors.muted_foreground))
        .child(div().flex_none().text_color(theme.colors.muted_foreground).child(link.display_key.clone()))
        .child(div().flex_1().min_w_0().truncate().line_height(ROW_LINE_HEIGHT).child(title.clone()))
        .tooltip(tooltip(t!("Issue {key}: {title}", key = link.display_key.as_str(), title = title.as_ref())))
        .on_click(move |_, window, cx| on_open(window, cx))
        .into_any_element()
}
