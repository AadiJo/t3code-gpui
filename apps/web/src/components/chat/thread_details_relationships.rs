//! The thread details card's Lineage and Side chats sections. Ports `ThreadRelationshipsPanel`
//! (`ThreadRelationshipsControl.tsx`) and `ThreadRelationshipIcon.tsx`: related threads as 2rem
//! rows with a status dot on their icon, the fork's source as a split row with Merge back, a
//! "Disconnect agent session" menu in the heading, collapsible agent groups that page six then
//! twelve rows at a time, and the side chats under their own heading. Rows come from
//! [`crate::state::thread_relationships`]; clicks come back as [`RelationshipEvent`]s.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{AnyElement, App, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, prelude::*, px};
use t3code_client_runtime::state::thread_relationships::RelationshipStatus;
use t3code_contracts::ThreadId;

use super::thread_details_panel_styles::{row_base, row_hover, row_icon, row_inset, split_secondary_width};
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::i18n::{t, t_rich};
use crate::state::thread_relationships::{RelationshipGroup, RelationshipIcon, RelationshipRow, RelationshipsInput};
use crate::theme_color::with_alpha;

/// Rows a group shows before "Show N more" (`THREAD_LINEAGE_INITIAL_COUNT`).
pub const LINEAGE_INITIAL_COUNT: usize = 6;
/// Rows each "Show N more" adds (`THREAD_LINEAGE_PAGE_COUNT`).
pub const LINEAGE_PAGE_COUNT: usize = 12;

/// What a Lineage or Side chats control asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum RelationshipEvent {
    /// Open a related thread (a side chat opens as a tab of its parent).
    Open(ThreadId),
    /// Merge this fork's latest run back into its source.
    MergeBack,
    /// Stop the thread's provider session.
    Disconnect,
    /// Open the delegating thread on its environment.
    OpenDelegatedFrom,
    /// Open or close the heading's "More thread actions" menu at a window position.
    ToggleMenu(Option<gpui_kit::Point<gpui_kit::Pixels>>),
}

/// The action a click is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipBusy {
    Merge,
    Detach,
}

/// Per-group expansion and paging, kept by the card for the shown thread.
#[derive(Clone, Debug, Default)]
pub struct RelationshipViewState {
    expanded: HashMap<&'static str, bool>,
    visible: HashMap<&'static str, usize>,
}

impl RelationshipViewState {
    fn expanded(&self, group: &RelationshipGroup) -> bool {
        self.expanded.get(group.id).copied().unwrap_or(group.expanded)
    }

    fn visible(&self, id: &'static str) -> usize {
        self.visible.get(id).copied().unwrap_or(LINEAGE_INITIAL_COUNT)
    }
}

/// Emits a relationship event.
pub type RelationshipEmit = Rc<dyn Fn(RelationshipEvent, &mut Window, &mut App)>;
/// Changes the view state (expand a group, show more).
pub type RelationshipState = Rc<dyn Fn(&dyn Fn(&mut RelationshipViewState), &mut Window, &mut App)>;

/// `ThreadDetailsControl`: a row control with its 1px transparent border, which insets the
/// content by a pixel.
fn control(id: impl Into<gpui_kit::ElementId>, theme: &AppTheme, disabled: bool, open: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
    row_base(id, theme, disabled, open).border_1().border_color(gpui_kit::transparent_black())
}

/// `threadRelationshipStatusLabel`.
pub fn status_label(status: Option<RelationshipStatus>) -> SharedString {
    use RelationshipStatus as S;
    match status {
        Some(S::Preparing | S::Starting) => t!("Starting", context = "agent status"),
        Some(S::Running | S::InProgress) => t!("Running", context = "agent status"),
        Some(S::Pending | S::Queued) => t!("Queued", context = "agent status"),
        Some(S::Waiting | S::Blocked) => t!("Waiting", context = "agent status"),
        Some(S::Completed) => t!("Done", context = "agent status"),
        Some(S::Failed | S::Error) => t!("Failed", context = "agent status"),
        Some(S::Cancelled | S::Interrupted) => t!("Stopped", context = "agent status"),
        Some(S::RolledBack) => t!("Reverted", context = "agent status"),
        Some(S::ResolvedNative) => t!("Resolved (native)", context = "agent status"),
        Some(S::ResolvedPortable) => t!("Resolved (portable)", context = "agent status"),
        Some(S::Consumed) => t!("Consumed", context = "agent status"),
        Some(S::Superseded) => t!("Superseded", context = "agent status"),
        Some(S::Idle) => t!("Idle", context = "agent status"),
        Some(S::Unknown) | None => t!("Unknown", context = "agent status"),
    }
}

/// `ThreadRelationshipIcon` without a provider: the fallback icon in a 1rem box with a 0.5rem
/// status dot ringed in the card colour, above the icon when any provider instance shows a badge.
pub fn relationship_icon(theme: &AppTheme, icon: RelationshipIcon, status: Option<RelationshipStatus>, badge_above: bool) -> AnyElement {
    let name = match icon {
        RelationshipIcon::Parent => IconName::CornerLeftUp,
        RelationshipIcon::Agent => IconName::Bot,
        RelationshipIcon::SideChat => IconName::MessagesSquare,
        RelationshipIcon::Fork => IconName::GitFork,
    };
    let dot: Hsla = match status {
        Some(s) if s.is_working() => theme.fixed.info,
        Some(s) if s.is_failed() => theme.colors.destructive(),
        Some(RelationshipStatus::Completed) => theme.fixed.success,
        _ => with_alpha(theme.colors.muted_foreground, 0.45),
    };
    let offset = theme.rem(-0.25);
    div()
        .relative()
        .flex()
        .flex_none()
        .size(theme.rem(1.))
        .items_center()
        .justify_center()
        .child(row_icon(theme, name))
        // The 2px ring in the card colour, drawn as a filled circle under the dot: a GPUI border
        // on a 0.5rem circle antialiases into a visible halo.
        .child(
            div()
                .absolute()
                .right(offset)
                .when(badge_above, |d| d.top(offset))
                .when(!badge_above, |d| d.bottom(offset))
                .flex()
                .items_center()
                .justify_center()
                .size(theme.rem(0.5))
                .rounded_full()
                .bg(theme.colors.card())
                .child(div().size(theme.rem(0.5) - px(4.)).rounded_full().bg(dot)),
        )
        .into_any_element()
}

/// The 2xs status label after a row's title.
fn status_text(theme: &AppTheme, status: Option<RelationshipStatus>) -> gpui_kit::Div {
    let failed = status.is_some_and(RelationshipStatus::is_failed);
    div()
        .flex_none()
        .ui_text(theme, TextSize::Xs2, weight::NORMAL)
        .text_color(if failed { theme.colors.destructive() } else { theme.colors.muted_foreground })
        .child(status_label(status))
}

/// The title span: `text-sm font-medium leading-4 text-foreground/85`, truncating.
fn title_text(theme: &AppTheme, title: SharedString) -> gpui_kit::Div {
    div().flex_1().min_w_0().child(
        div()
            .truncate()
            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
            .line_height(theme.rem(1.))
            .text_color(with_alpha(theme.colors.foreground(), 0.85))
            .child(title),
    )
}

/// `ArrowRightIcon size-3`, shown while the row is hovered.
fn hover_arrow(theme: &AppTheme, group: SharedString) -> AnyElement {
    div()
        .flex_none()
        .invisible()
        .group_hover(group, |d| d.visible())
        .child(Icon::new(IconName::ArrowRight).size(IconSize::Custom(theme.rem(0.75))).color(theme.colors.muted_foreground))
        .into_any_element()
}

/// A related thread's row: open on click, with the title and hint in its tooltip. The fork's
/// source splits into open, Merge back and the status.
fn relationship_row(
    theme: &AppTheme,
    section: &'static str,
    index: usize,
    row: &RelationshipRow,
    input: &RelationshipsInput,
    busy: Option<RelationshipBusy>,
    emit: &RelationshipEmit,
) -> AnyElement {
    let group: SharedString = format!("relationship-{section}-{index}").into();
    let tip = TooltipSurface::new(row.title.clone()).line(row.hint.clone());
    let open = {
        let (emit, thread) = (emit.clone(), row.thread_id.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| emit(RelationshipEvent::Open(thread.clone()), window, cx)
    };
    let icon = relationship_icon(theme, row.icon, row.status, input.any_instance_badge);
    let item = div().group(group.clone()).relative().flex().h(theme.rem(2.)).items_center().rounded(theme.radii.lg);
    let merge = input.merge.as_ref().filter(|_| row.merge_target);
    let Some(merge) = merge else {
        let control = control((SharedString::from(format!("relationship-{section}")), index), theme, false, false)
            .w_full()
            .tooltip(tooltip(tip))
            .on_click(open)
            .child(icon)
            .child(title_text(theme, row.title.clone()))
            .child(hover_arrow(theme, group.clone()))
            .child(status_text(theme, row.status));
        return item.child(control).into_any_element();
    };
    let primary = control((SharedString::from(format!("relationship-{section}")), index), theme, false, false)
        .flex_1()
        .rounded_r(px(0.))
        .tooltip(tooltip(tip))
        .on_click(open)
        .child(icon)
        .child(title_text(theme, row.title.clone()))
        .child(hover_arrow(theme, group.clone()));
    let can_merge = merge.run_id.is_some() && busy.is_none();
    let merge_tip = match (&merge.run_id, &merge.parent_title) {
        (None, _) => t!("Complete a run in this fork before merging it back"),
        (Some(_), Some(title)) => t!("Merge this conversation back into {parentTitle}", parentTitle = title.as_ref()),
        (Some(_), None) => t!("Merge this conversation back into its source"),
    };
    let glyph: AnyElement = if busy == Some(RelationshipBusy::Merge) {
        Spinner::new(SpinnerSize::Xs).into_any_element()
    } else {
        Icon::new(IconName::GitMerge).size(IconSize::Custom(theme.rem(0.75))).color(with_alpha(theme.colors.foreground(), 0.8)).into_any_element()
    };
    let secondary = control(SharedString::from(format!("relationship-{section}-merge")), theme, !can_merge, false)
        .flex_none()
        .w(split_secondary_width(theme))
        .px(px(0.))
        .justify_center()
        .rounded_l(px(0.))
        .tooltip(tooltip(merge_tip))
        .when(can_merge, |c| {
            let emit = emit.clone();
            c.on_click(move |_, window, cx| emit(RelationshipEvent::MergeBack, window, cx))
        })
        .child(glyph);
    let hairline = with_alpha(theme.colors.border, 0.65);
    let hover = row_hover(theme);
    let split = div()
        .flex()
        .w_full()
        .items_center()
        .rounded(theme.radii.lg)
        .hover(move |s| s.bg(hover))
        .child(primary)
        .child(div().h(theme.rem(1.)).w(px(1.)).flex_none().group_hover(group.clone(), move |h| h.bg(hairline)))
        .child(secondary)
        .child(
            div()
                .flex_none()
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .pl(theme.rem(0.25))
                .pr(theme.rem(0.625))
                .ui_text(theme, TextSize::Xs2, weight::MEDIUM)
                .text_color(theme.colors.muted_foreground)
                .child(status_label(row.status)),
        );
    item.child(split).into_any_element()
}

/// `ThreadLineageRowList`: the rows (scrolling past 13.5rem) and "Show N more".
fn row_list(theme: &AppTheme, id: &'static str, rows: Vec<AnyElement>, hidden: usize, state: &RelationshipState) -> Vec<AnyElement> {
    let mut out = vec![div().id(SharedString::from(format!("relationships-{id}"))).max_h(theme.rem(13.5)).overflow_y_scroll().children(rows).into_any_element()];
    if hidden > 0 {
        let state = state.clone();
        let hover = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.075) } else { gpui_kit::hsla(0., 0., 0., 0.055) };
        let text_hover = with_alpha(theme.colors.foreground(), 0.8);
        out.push(
            div()
                .id(SharedString::from(format!("relationships-{id}-more")))
                .flex()
                .h(theme.rem(2.))
                .w_full()
                .items_center()
                .gap(row_inset(theme))
                .px(row_inset(theme))
                .rounded(theme.radii.lg)
                .cursor_pointer()
                .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                .text_color(with_alpha(theme.colors.muted_foreground, 0.7))
                .hover(move |s| s.bg(hover).text_color(text_hover))
                .on_click(move |_, window, cx| {
                    state(&|s| *s.visible.entry(id).or_insert(LINEAGE_INITIAL_COUNT) += LINEAGE_PAGE_COUNT, window, cx)
                })
                .child(Icon::new(IconName::Plus).size(IconSize::Custom(theme.rem(1.))))
                .child(SharedString::from(t_rich!("Show {count} more", count = hidden.min(LINEAGE_PAGE_COUNT)).plain()))
                .into_any_element(),
        );
    }
    out
}

/// `ThreadDetailsSection` for these sections: the heading row may hide while the section stays.
fn section(theme: &AppTheme, title: SharedString, show_heading: bool, actions: Option<AnyElement>, children: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .px(theme.rem(0.5))
        .pt(theme.rem(0.5))
        .pb(theme.rem(0.625))
        .border_t_1()
        .border_color(with_alpha(theme.colors.border, 0.65))
        .when(show_heading, |s| {
            s.child(
                div()
                    .flex()
                    .min_h(theme.rem(2.))
                    .min_w_0()
                    .mb(theme.rem(0.25))
                    .px(row_inset(theme))
                    .items_center()
                    .justify_between()
                    .gap(theme.rem(0.5))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .ui_text(theme, TextSize::Xs2, weight::MEDIUM)
                            .text_color(theme.colors.muted_foreground)
                            .child(title),
                    )
                    .children(actions.map(|a| div().flex().flex_none().items_center().gap(theme.rem(0.25)).child(a))),
            )
        })
        .children(children)
        .into_any_element()
}

/// The Lineage and Side chats sections, as the card's last sections before usage.
pub fn relationship_sections(
    theme: &AppTheme,
    input: &RelationshipsInput,
    view: &RelationshipViewState,
    busy: Option<RelationshipBusy>,
    emit: RelationshipEmit,
    state: RelationshipState,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    if input.lineage_visible {
        let mut children = Vec::new();
        if let Some(from) = &input.delegated_from {
            let hint = match (&from.label, from.connected) {
                (Some(label), true) => t!("Open parent agent on {label}", label = label.as_ref()),
                (Some(label), false) => t!("Delegated from {label}. Connect to it to open the parent thread.", label = label.as_ref()),
                (None, _) => t!("Delegated from a linked environment. Connect to it to open the parent thread."),
            };
            let emit = emit.clone();
            let row = control("relationship-delegated-from", theme, !from.connected, false)
                .w_full()
                .tooltip(tooltip(TooltipSurface::new(from.title.clone()).line(hint)))
                .when(from.connected, |r| r.on_click(move |_, window, cx| emit(RelationshipEvent::OpenDelegatedFrom, window, cx)))
                .child(relationship_icon(theme, RelationshipIcon::Parent, None, input.any_instance_badge))
                .child(title_text(theme, from.title.clone()))
                .children(from.label.as_ref().map(|label| {
                    div()
                        .flex_none()
                        .max_w(theme.rem(8.))
                        .truncate()
                        .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                        .text_color(theme.colors.muted_foreground)
                        .child(SharedString::from(t_rich!("on {label}", label = label.as_ref()).plain()))
                }));
            children.push(div().flex().h(theme.rem(2.)).items_center().rounded(theme.radii.lg).child(row).into_any_element());
        }
        for group in &input.groups {
            if group.rows.is_empty() {
                continue;
            }
            let expanded = view.expanded(group);
            if let Some(label) = &group.label {
                let failed = group.rows.iter().filter(|r| r.status.is_some_and(RelationshipStatus::is_failed)).count();
                let (state, id) = (state.clone(), group.id);
                let default = group.expanded;
                children.push(
                    control(SharedString::from(format!("relationships-group-{id}")), theme, false, false)
                        .w_full()
                        .on_click(move |_, window, cx| {
                            state(&|s| {
                                let current = s.expanded.get(id).copied().unwrap_or(default);
                                s.expanded.insert(id, !current);
                            }, window, cx)
                        })
                        .child(row_icon(theme, IconName::Bot))
                        .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                        .children((failed > 0).then(|| {
                            div()
                                .flex_none()
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(theme.colors.destructive())
                                .child(SharedString::from(t_rich!("{count} failed", count = failed).plain()))
                        }))
                        .children((!expanded).then(|| {
                            div()
                                .flex_none()
                                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                                .text_color(theme.colors.muted_foreground)
                                .child(group.rows.len().to_string())
                        }))
                        .child(row_icon(theme, if expanded { IconName::ChevronDown } else { IconName::ChevronRight }))
                        .into_any_element(),
                );
            }
            if expanded {
                let visible = view.visible(group.id);
                let rows = group.rows.iter().take(visible).enumerate().map(|(ix, row)| relationship_row(theme, group.id, ix, row, input, busy, &emit)).collect();
                children.extend(row_list(theme, group.id, rows, group.rows.len().saturating_sub(visible), &state));
            }
        }
        let actions = input.can_detach.then(|| {
            let emit = emit.clone();
            control("relationships-more", theme, busy.is_some(), false)
                .flex_none()
                .size(theme.rem(1.5))
                .px(px(0.))
                .justify_center()
                .rounded(theme.radii.md)
                .tooltip(tooltip(t!("More thread actions")))
                .when(busy.is_none(), |c| {
                    c.on_click(move |event, window, cx| emit(RelationshipEvent::ToggleMenu(Some(event.position())), window, cx))
                })
                .child(Icon::new(IconName::Ellipsis).size(IconSize::Custom(theme.rem(0.875))).color(theme.colors.foreground()))
                .into_any_element()
        });
        let title = if input.running_count > 0 {
            t!("Lineage · {runningCount} running", runningCount = input.running_count as i64)
        } else {
            t!("Lineage", context = "thread details section: parent and child agents")
        };
        out.push(section(theme, title, input.show_heading, actions, children));
    }
    if input.side_chats_visible {
        let visible = view.visible("side-chats");
        let rows = input.side_chats.iter().take(visible).enumerate().map(|(ix, row)| relationship_row(theme, "side-chats", ix, row, input, busy, &emit)).collect();
        let children = row_list(theme, "side-chats", rows, input.side_chats.len().saturating_sub(visible), &state);
        let title = if input.running_side_chats > 0 {
            t!("Side chats · {runningSideChats} running", runningSideChats = input.running_side_chats as i64)
        } else {
            t!("Side chats")
        };
        out.push(section(theme, title, true, None, children));
    }
    out
}
