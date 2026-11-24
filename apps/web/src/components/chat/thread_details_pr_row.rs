//! A pull request in the thread details card, and its linked issues. Ports
//! `ThreadDetailsPrRow.tsx`, `ThreadDetailsIssueRows.tsx` and the row action of
//! `pullRequestDetail.logic.ts`: a plain "#N: title" row until the detail
//! loads, then the title over a meta line with the checks rollup and one
//! action pill (Resolve, Ready, Fix or Merge).

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, TestSupportExt as _, Window, div,
    prelude::*, px,
};
use t3code_client_runtime::pull_request_checks::group_pull_request_checks;
use t3code_contracts::{
    PullRequestAction, PullRequestCheck, PullRequestCheckStatus, PullRequestDetail, PullRequestMergeMethod,
    PullRequestMergeability, PullRequestState, ThreadIssueLink,
};

use super::thread_details_panel_styles::{
    ROW_LINE_HEIGHT, row_base, row_font, row_height, row_hover, row_icon_size, row_inset, split_row,
    split_secondary_width,
};
use crate::components::pull_request::pull_request_detail_logic::{
    allowed_pull_request_merge_methods, resolve_pull_request_merge_method, summarize_pull_request_checks,
};
use crate::components::pull_request::pull_request_presentation::{check_status_icon, check_status_label, open_tone, pending_tone};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::popover::{PopoverSurface, PopoverWidth, below};
use crate::source_control_presentation::{ChangeRequestTermKind, source_control_definition};
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
    /// The project's default merge method, if it sets one.
    pub project_merge_method: Option<PullRequestMergeMethod>,
    /// The method last chosen on this device (`pullRequestMergeMethod`).
    pub last_merge_method: PullRequestMergeMethod,
}

impl PullRequestRowData {
    /// The host's noun for the change request (`changeRequestTermKind`).
    pub fn term(&self) -> ChangeRequestTermKind {
        let kind = self.detail.as_ref().map(|d| d.provider.as_str().to_owned());
        source_control_definition(kind.as_deref()).terminology.term_kind()
    }

    /// The method Merge uses (`resolvePullRequestMergeMethod`).
    pub fn merge_method(&self) -> PullRequestMergeMethod {
        let allowed = self.detail.as_ref().map(allowed_pull_request_merge_methods).unwrap_or_default();
        resolve_pull_request_merge_method(&allowed, None, self.project_merge_method, self.last_merge_method)
    }
}

/// The ICU `method` select key for a merge method.
pub fn merge_method_key(method: PullRequestMergeMethod) -> &'static str {
    match method {
        PullRequestMergeMethod::Merge => "merge",
        PullRequestMergeMethod::Squash => "squash",
        PullRequestMergeMethod::Rebase => "rebase",
        PullRequestMergeMethod::FastForward => "fastForward",
    }
}

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a pull request row does when used.
#[derive(Clone)]
pub struct PullRequestRowHandlers {
    /// Opens the pull request (the title row).
    pub open: Handler,
    /// Runs the row's action pill.
    pub action: Rc<dyn Fn(PullRequestRowAction, &mut Window, &mut App)>,
    /// Opens or closes the checks popover.
    pub toggle_checks: Handler,
    /// Stops watching it; `None` when the thread does not watch it.
    pub stop_watching: Option<Handler>,
}

/// `pullRequestChecksStatePresentation`: the rollup's label, icon and tone.
pub fn checks_state_presentation(theme: &AppTheme, state: ChecksState) -> (SharedString, IconName, Hsla) {
    match state {
        ChecksState::Failing => (t!("Some checks were not successful"), IconName::CircleX, theme.colors.destructive()),
        ChecksState::Pending => (t!("Some checks haven't completed yet"), IconName::CircleDot, pending_tone(theme)),
        _ => (t!("All checks have passed"), IconName::CircleCheck, open_tone(theme)),
    }
}

/// The watch control: an eye that turns into a crossed eye on hover.
fn watch_control(index: usize, theme: &AppTheme, secondary: bool, on_stop: Handler) -> gpui_kit::Stateful<gpui_kit::Div> {
    let group: SharedString = format!("watch-{index}").into();
    let size = IconSize::Custom(theme.rem(if secondary { 1. } else { 0.875 }));
    let muted = theme.colors.muted_foreground;
    let control = if secondary {
        row_base(("pr-watch", index), theme, false, false)
            .flex_none()
            .w(split_secondary_width(theme))
            .px(px(0.))
            .justify_center()
            .rounded_l(px(0.))
    } else {
        row_base(("pr-watch", index), theme, false, false).flex_none().size(theme.rem(1.5)).px(px(0.)).justify_center().rounded(theme.radii.md)
    };
    control
        .group(group.clone())
        .tooltip(tooltip(t!(
            "Watching: the agent wakes when checks finish, someone comments, or the branch conflicts. Click to stop."
        )))
        .child(div().group_hover(group.clone(), |d| d.invisible()).child(Icon::new(IconName::Eye).size(size).color(muted)))
        .child(
            div()
                .absolute()
                .invisible()
                .group_hover(group, |d| d.visible())
                .child(Icon::new(IconName::EyeOff).size(size).color(muted)),
        )
        .on_click(move |_, window, cx| on_stop(window, cx))
}

/// One pull request row. `checks_popup` floats under the checks control while open.
pub fn pull_request_row(
    index: usize,
    theme: &AppTheme,
    data: &PullRequestRowData,
    handlers: PullRequestRowHandlers,
    checks_popup: Option<AnyElement>,
) -> AnyElement {
    let detail = data.detail.as_ref();
    let (state, draft) = detail.map_or((data.state, data.is_draft), |d| (d.state, d.is_draft));
    let (icon, tone) = pull_request_state_tone(theme, state, draft);
    let action = resolve_row_action(detail);
    let conflicting = detail.is_some_and(|d| d.state == PullRequestState::Open && d.mergeability == PullRequestMergeability::Conflicting);
    let rollup = detail.and_then(|d| checks_rollup(&d.checks)).filter(|_| !conflicting && !draft);
    let tooltip_text: SharedString = format!("#{}: {}", data.number, data.title).into();
    let title_icon = Icon::new(icon).size(row_icon_size(theme)).color(tone);
    let watching = data.watching && state == PullRequestState::Open;
    let stop_watching = handlers.stop_watching.clone().filter(|_| watching);
    if detail.is_none() || (rollup.is_none() && action.is_none() && stop_watching.is_none()) {
        let label: SharedString =
            if data.title.trim().is_empty() { format!("#{}", data.number).into() } else { format!("#{}: {}", data.number, data.title).into() };
        let open = handlers.open.clone();
        let primary = row_base(("pr-row", index), theme, false, false)
            .w_full()
            .child(title_icon)
            .child(div().flex_1().min_w_0().truncate().child(label))
            .tooltip(tooltip(tooltip_text))
            .on_click(move |_, window, cx| open(window, cx));
        return match stop_watching {
            Some(stop) => split_row("pr-split", theme, primary, watch_control(index, theme, true, stop), false, false)
                .into_any_element(),
            None => primary.into_any_element(),
        };
    }
    let muted = theme.colors.muted_foreground;
    let checks = rollup.map(|state| {
        let (_, icon, color) = checks_state_presentation(theme, state);
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
        let toggle = handlers.toggle_checks.clone();
        let hover = row_hover(theme);
        div()
            .id(("pr-checks", index))
            .relative()
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.25))
            .h(theme.rem(1.5))
            .px(theme.rem(0.375))
            .rounded(theme.radii.md)
            .font_weight(weight::MEDIUM)
            .text_color(muted)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .when(checks_popup.is_some(), |c| c.bg(hover))
            .tooltip(tooltip(t!("Open checks: {summary}", summary = summarize_pull_request_checks(checks).as_ref())))
            .on_click(move |_, window, cx| toggle(window, cx))
            .child(Icon::new(icon).size(IconSize::Custom(theme.rem(0.875))).color(color))
            .children(text.map(|t| div().min_w_0().truncate().child(t)))
            .children(checks_popup.map(|popup| div().absolute().left_0().bottom_0().child(below(popup))))
            .test_support()
            .into_any_element()
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
        let run = handlers.action.clone();
        div()
            .id(("pr-action", index))
            .flex()
            .items_center()
            .gap(theme.rem(0.25))
            .h(theme.rem(1.25))
            .px(theme.rem(0.375))
            .rounded(theme.radii.md)
            .bg(bg)
            .text_color(fg)
            .font_weight(weight::MEDIUM)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(label)
            .when(destructive, |p| p.child(Icon::new(IconName::ArrowUpRight).size(IconSize::Custom(theme.rem(0.75)))))
            .on_click(move |_, window, cx| run(action, window, cx))
            .test_support()
    });
    let hover = row_hover(theme);
    let open = handlers.open.clone();
    div()
        .flex()
        .flex_col()
        .pb(theme.rem(0.375))
        .child(
            row_font(div())
                .id(("pr-title", index))
                .flex()
                .items_start()
                .gap(row_inset(theme))
                .min_h(row_height(theme))
                .pt(theme.rem(0.375))
                .px(row_inset(theme))
                .rounded(theme.radii.lg)
                .text_color(with_alpha(theme.colors.foreground(), 0.8))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(tooltip(tooltip_text))
                .on_click(move |_, window, cx| open(window, cx))
                .child(div().mt(px(1.)).child(title_icon))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().line_clamp(2).line_height(theme.rem(1.125)).child(data.title.clone())),
        )
        .child(
            div()
                .flex()
                .h(theme.rem(1.25))
                .min_w_0()
                .items_center()
                .gap(theme.rem(0.125))
                .pl(theme.rem(2.25))
                .pr(row_inset(theme))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(muted)
                .child(div().mr(theme.rem(0.25)).flex_none().child(format!("#{}", data.number)))
                .children(checks)
                .children(stop_watching.map(|stop| watch_control(index, theme, false, stop)))
                .child(div().flex_1())
                .children(pill),
        )
        .into_any_element()
}

/// The checks popover (`PullRequestChecksPopover`): the rollup label, a summary,
/// then attention, running and (behind "Show all") completed checks.
pub fn checks_popover(
    theme: &AppTheme,
    checks: &[PullRequestCheck],
    show_all: bool,
    on_show_all: impl Fn(&mut Window, &mut App) + 'static,
    on_open_url: impl Fn(SharedString, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let rollup = checks_rollup(checks).unwrap_or(ChecksState::Passing);
    let (label, _, _) = checks_state_presentation(theme, rollup);
    let groups = group_pull_request_checks(checks);
    let can_collapse = groups.attention.len() + groups.running.len() > 0 && !groups.completed.is_empty();
    let visible: Vec<&PullRequestCheck> = groups
        .attention
        .iter()
        .chain(groups.running.iter())
        .chain(groups.completed.iter().filter(|_| show_all || !can_collapse))
        .copied()
        .collect();
    let on_open_url = Rc::new(on_open_url);
    let xs = |d: gpui_kit::Div| d.ui_text(theme, TextSize::Xs, weight::NORMAL);
    let body: AnyElement = if checks.is_empty() {
        xs(div()).text_color(theme.colors.muted_foreground).child(t!("No checks reported")).into_any_element()
    } else {
        div()
            .id("pr-checks-list")
            .flex()
            .flex_col()
            .gap(theme.rem(0.25))
            .max_h(theme.rem(16.))
            .overflow_y_scroll()
            .children(visible.into_iter().enumerate().map(|(i, check)| {
                let (icon, tone) = check_status_icon(theme, check.status);
                let url = check.url.clone();
                let open = on_open_url.clone();
                xs(div())
                    .flex()
                    .items_center()
                    .gap(theme.rem(0.5))
                    .child(Icon::new(icon).size(IconSize::Custom(theme.rem(0.875))).color(tone))
                    .child(
                        div()
                            .id(("pr-check-name", i))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .tooltip(tooltip(SharedString::from(check.description.clone().unwrap_or_else(|| check.name.clone()))))
                            .child(check.name.clone()),
                    )
                    .child(div().flex_none().text_color(theme.colors.muted_foreground).child(check_status_label(check.status)))
                    .children(url.map(|url| {
                        let url: SharedString = url.into();
                        let primary = theme.colors.primary();
                        div()
                            .id(("pr-check-details", i))
                            .flex_none()
                            .text_color(primary)
                            .cursor_pointer()
                            .hover(|s| s.underline())
                            .on_click(move |_, window, cx| open(url.clone(), window, cx))
                            .child(t!("Details"))
                    }))
            }))
            .into_any_element()
    };
    PopoverSurface::new()
        .width(PopoverWidth::Md)
        .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).child(label))
        .child(xs(div()).text_color(theme.colors.muted_foreground).child(summarize_pull_request_checks(checks)))
        .child(body)
        .when(can_collapse, |s| {
            s.child(
                Button::new("pr-checks-show-all")
                    .variant(ButtonVariant::Ghost)
                    .size(ButtonSize::Xs)
                    .label(if show_all { t!("Show less") } else { t!("Show all") })
                    .on_click(move |_, window, cx| on_show_all(window, cx)),
            )
        })
        .into_any_element()
}

/// One linked issue (`ThreadDetailsIssueRows`): a ticket icon and "KEY title".
pub fn issue_row(index: usize, theme: &AppTheme, link: &ThreadIssueLink, on_open: impl Fn(&mut gpui_kit::Window, &mut gpui_kit::App) + 'static) -> AnyElement {
    let title: SharedString = link.title.clone().into();
    row_base(("issue-row", index), theme, false, false)
        .w_full()
        .child(Icon::new(IconName::CircleDot).size(IconSize::Custom(theme.rem(1.))).color(theme.colors.muted_foreground))
        .child(div().flex_none().text_color(theme.colors.muted_foreground).child(link.display_key.clone()))
        .child(div().flex_1().min_w_0().truncate().line_height(ROW_LINE_HEIGHT).child(title.clone()))
        .tooltip(tooltip(t!("Issue {key}: {title}", key = link.display_key.as_str(), title = title.as_ref())))
        .on_click(move |_, window, cx| on_open(window, cx))
        .into_any_element()
}
