//! The thread details card's Lineage and Side chats sections. Ports `ThreadRelationshipsPanel`
//! (`ThreadRelationshipsControl.tsx`), `ThreadRelationshipIcon.tsx`, `ThreadLineageWorkflowRow.tsx`,
//! `SubagentTooltipContent.tsx` and `AgentElapsed.tsx`: related threads as 2rem rows with a status
//! dot on their icon, child agents with their provider mark, elapsed time, hover card and stop
//! button, workflow coordinators that expand into phases and members, tasks on linked
//! environments, the fork's source as a split row with Merge back, a "Disconnect agent session"
//! menu in the heading, collapsible agent groups that page six then twelve rows at a time, and the
//! side chats under their own heading. Rows come from [`crate::state::thread_relationships`];
//! clicks come back as [`RelationshipEvent`]s. Tooltips open to the rows' left.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{AnyElement, App, Hsla, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div, prelude::*, px};
use t3code_client_runtime::state::thread_relationships::RelationshipStatus;
use t3code_contracts::{EnvironmentId, NodeId, OrchestrationV2TurnItemStatus as ItemStatus, ThreadId};

use super::thread_details_panel_styles::{row_base, row_hover, row_icon, row_inset, split_secondary_width};
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::components::ui::tooltip::{TooltipAnchor, TooltipSide, TooltipSurface, TooltipVariant, tooltip};
use crate::i18n::{t, t_rich};
use crate::state::thread_relationships::{
    AgentClock, PhaseStatus, ProviderMark, RelationshipGroup, RelationshipIcon, RelationshipRow, RelationshipsInput, SubagentCard, WorkflowMember,
    WorkflowRow,
};
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
    /// Open the delegating thread on its environment, once it is known to still exist.
    OpenDelegatedFrom,
    /// Open or close the heading's "More thread actions" menu at a window position.
    ToggleMenu(Option<gpui_kit::Point<gpui_kit::Pixels>>),
    /// Stop a child agent (`interruptTurn` with its subagent id on the thread that owns it).
    StopSubagent { owner: ThreadId, subagent: NodeId },
    /// Stop a running workflow (`orchestration.stopWorkflow`).
    StopWorkflow(NodeId),
    /// Open a task's thread on the linked environment it runs on.
    OpenRemote { environment: EnvironmentId, thread: ThreadId },
}

/// The action a click is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipBusy {
    Merge,
    Detach,
}

/// Per-group expansion and paging, and which workflows and phases are open, kept by the card
/// for the shown thread.
#[derive(Clone, Debug, Default)]
pub struct RelationshipViewState {
    expanded: HashMap<&'static str, bool>,
    visible: HashMap<&'static str, usize>,
    workflows: HashSet<NodeId>,
    phases: HashMap<(NodeId, u64), bool>,
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

/// What every row reads besides its own data.
struct RowCx<'a> {
    theme: &'a AppTheme,
    input: &'a RelationshipsInput,
    view: &'a RelationshipViewState,
    busy: Option<RelationshipBusy>,
    emit: &'a RelationshipEmit,
    state: &'a RelationshipState,
    /// Wall-clock milliseconds for running timers.
    now: i64,
}

#[cfg(feature = "test-support")]
thread_local! {
    static PROBES: std::cell::RefCell<HashMap<String, gpui_kit::Bounds<gpui_kit::Pixels>>> = Default::default();
}

/// Where a row control (`relationship-row-<thread id>`) or stop button was last painted, for
/// headless checks that hover and click it as a pointer would.
#[cfg(feature = "test-support")]
pub fn probed_bounds(id: &str) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
    PROBES.with(|p| p.borrow().get(id).copied())
}

/// Records the parent's box under `id` in test builds; nothing otherwise.
fn probe(id: String) -> Option<AnyElement> {
    #[cfg(feature = "test-support")]
    {
        let element = gpui_kit::canvas(move |bounds, _, _| PROBES.with(|p| p.borrow_mut().insert(id.clone(), bounds)), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .into_any_element();
        return Some(element);
    }
    #[cfg(not(feature = "test-support"))]
    {
        let _ = id;
        None
    }
}

/// Rows open their tooltips after 200ms (`delay={200}`).
const ROW_TOOLTIP_DELAY: Duration = Duration::from_millis(200);

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

/// `formatElapsedSeconds`: "8s", "1m 05s", "2h 03m".
fn format_elapsed(ms: i64) -> SharedString {
    let seconds = ms.max(0) / 1000;
    let minutes = seconds / 60;
    if minutes == 0 {
        return t!("{seconds}s", context = "elapsed time, seconds", seconds = seconds);
    }
    let hours = minutes / 60;
    if hours == 0 {
        return t!("{minutes}m {seconds}s", context = "elapsed time, minutes and seconds", minutes = minutes, seconds = format!("{:02}", seconds % 60));
    }
    t!("{hours}h {minutes}m", context = "elapsed time, hours and minutes", hours = hours, minutes = format!("{:02}", minutes % 60))
}

/// `formatCompactElapsedSeconds`: "8s", "4m", "1.5h", "12h".
fn format_compact_elapsed(ms: i64) -> SharedString {
    let seconds = ms.max(0) / 1000;
    if seconds < 60 {
        return t!("{seconds}s", context = "elapsed time, seconds", seconds = seconds);
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return t!("{minutes}m", context = "elapsed time, minutes", minutes = minutes);
    }
    let hours = if minutes >= 600 { (minutes / 60) as f64 } else { (minutes / 6) as f64 / 10. };
    t!("{hours}h", context = "elapsed time, hours", hours = hours)
}

/// The driver's mark, or its initials, in `size` (`ProviderInstanceIcon`).
fn provider_mark(theme: &AppTheme, mark: &ProviderMark, size: gpui_kit::Pixels, color: Hsla) -> AnyElement {
    let dark = theme.is_dark();
    let icon: AnyElement = match crate::components::settings::provider_settings_panel::driver_brand(&mark.driver, dark) {
        Some(brand) => Icon::new(brand).size(IconSize::Custom(size)).color(color).into_any_element(),
        None => div()
            .ui_text(theme, TextSize::Xs3, weight::SEMIBOLD)
            .line_height(px(TextSize::Xs3.size(theme.fonts.interface)))
            .text_color(color)
            .child(crate::provider_instances::provider_instance_initials(&mark.display_name))
            .into_any_element(),
    };
    let badge = mark.show_badge.then(|| {
        let accent = mark.accent.as_deref().and_then(crate::theme_color::parse_css_color).map(|c| c.hsla());
        div()
            .absolute()
            .right(theme.rem(-0.1875))
            .bottom(theme.rem(-0.1875))
            .h(theme.rem(0.75))
            .min_w(theme.rem(0.75))
            .rounded_full()
            .border_1()
            .border_color(theme.colors.card())
            .bg(accent.unwrap_or(theme.colors.card()))
    });
    div().relative().flex().flex_none().items_center().justify_center().child(icon).children(badge).into_any_element()
}

/// `ThreadRelationshipIcon`: the driver mark or the fallback icon in a 1rem box with a 0.5rem
/// status dot ringed in the card colour, above the icon when any provider instance shows a badge.
pub fn relationship_icon(theme: &AppTheme, icon: RelationshipIcon, mark: Option<&ProviderMark>, status: Option<RelationshipStatus>, badge_above: bool) -> AnyElement {
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
    let glyph = match mark {
        Some(mark) => provider_mark(theme, mark, theme.rem(1.), theme.colors.muted_foreground),
        None => row_icon(theme, name).into_any_element(),
    };
    let offset = theme.rem(-0.25);
    div()
        .relative()
        .flex()
        .flex_none()
        .size(theme.rem(1.))
        .items_center()
        .justify_center()
        .child(glyph)
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

/// `LineageTooltipContent`: the full title in medium weight, then what a click does.
fn lineage_tip(title: &SharedString, hint: &SharedString) -> TooltipSurface {
    TooltipSurface::new(title.clone()).line(hint.clone()).medium_title()
}

/// The words of a subagent status in its hover card (`subagent status`, capitalized).
fn card_status_label(status: ItemStatus) -> SharedString {
    let word = match status {
        ItemStatus::Idle => t!("idle", context = "subagent status"),
        ItemStatus::Pending => t!("pending", context = "subagent status"),
        ItemStatus::Running => t!("running", context = "subagent status"),
        ItemStatus::Waiting => t!("waiting", context = "subagent status"),
        ItemStatus::Completed => t!("completed", context = "subagent status"),
        ItemStatus::Failed => t!("failed", context = "subagent status"),
        ItemStatus::Cancelled => t!("cancelled", context = "subagent status"),
        ItemStatus::Interrupted => t!("interrupted", context = "subagent status"),
    };
    // `capitalize`.
    word.split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

/// `SubagentTooltipContent` in `ThreadHoverCard`, as a glass hover card that ticks while the
/// agent runs.
fn subagent_card(card: &SubagentCard) -> TooltipSurface {
    let card = card.clone();
    let live = card.clock.live;
    TooltipSurface::content(move |_, cx| {
        let theme = crate::ActiveAppTheme::app_theme(cx).clone();
        let theme = &theme;
        let muted = theme.colors.muted_foreground;
        let small = theme.rem(0.75);
        let line = |icon: AnyElement, value: AnyElement| div().flex().min_w_0().items_center().gap(theme.rem(0.5)).child(icon).child(value);
        let working = matches!(card.status, ItemStatus::Running | ItemStatus::Pending | ItemStatus::Waiting);
        let failed = card.status == ItemStatus::Failed;
        let (status_icon, status_color) = if working {
            (IconName::CircleDashed, theme.fixed.info)
        } else if failed {
            (IconName::CircleX, theme.colors.error)
        } else if card.status == ItemStatus::Completed {
            (IconName::Check, theme.fixed.success)
        } else {
            (IconName::CircleDashed, muted)
        };
        let mark = match &card.mark {
            Some(mark) => div().opacity(0.6).child(provider_mark(theme, mark, small, muted)).into_any_element(),
            None => Icon::new(IconName::Bot).size(IconSize::Custom(small)).color(muted).into_any_element(),
        };
        let model = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.25))
            .text_color(with_alpha(theme.colors.foreground(), 0.75))
            .child(div().min_w_0().truncate().child(card.model.clone()))
            .children(card.effort.clone().map(|effort| div().flex_none().child(SharedString::from(format!(" · {effort}")))));
        let elapsed = card.clock.elapsed_ms(t3code_client_runtime::state::models::now_ms()).map(format_elapsed);
        let status = div()
            .flex()
            .min_w_0()
            .items_center()
            .justify_between()
            .gap(theme.rem(1.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.rem(0.25))
                    .font_weight(weight::MEDIUM)
                    .text_color(status_color)
                    .child(Icon::new(status_icon).size(IconSize::Custom(small)).color(status_color))
                    .child(card_status_label(card.status)),
            )
            .children(elapsed);
        let rows = card.metadata.iter().map(|(value, branch)| {
            let icon = if *branch { IconName::GitBranch } else { IconName::Folder };
            line(Icon::new(icon).size(IconSize::Custom(small)).color(muted).into_any_element(), crate::components::sidebar::sidebar_thread_row::middle_truncate(value).into_any_element())
        });
        let preview = card.preview.as_ref().map(|preview| {
            line(Icon::new(IconName::Terminal).size(IconSize::Custom(small)).color(muted).into_any_element(), crate::components::sidebar::sidebar_thread_row::middle_truncate(preview).into_any_element())
        });
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(theme.rem(20.))
            .gap(theme.rem(0.5))
            .px(theme.rem(0.25))
            .py(theme.rem(0.5))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                    .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.25))
                    .text_color(theme.colors.foreground())
                    .child(card.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.375))
                    .pl(theme.rem(0.125))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(muted)
                    .child(line(mark, model.into_any_element()))
                    .child(status)
                    .children(rows)
                    .children(preview),
            )
            .into_any_element()
    })
    .variant(TooltipVariant::Glass)
    .ticking(live)
}

/// The filled `SquareIcon size-3` of a stop button.
fn stop_glyph(theme: &AppTheme) -> AnyElement {
    // Lucide's square is an 18-unit rect with a 2-unit stroke in a 24-unit box; filled, it is
    // 20/24 of the icon with a 3/24 corner.
    let icon = theme.rem(0.75);
    div().size(icon * (20. / 24.)).rounded(icon * (3. / 24.)).bg(theme.colors.destructive()).into_any_element()
}

/// The `icon-xs` destructive stop control, shown while its row is hovered (`group-hover`).
fn stop_button(rc: &RowCx<'_>, id: SharedString, group: SharedString, label: SharedString, stopping: bool, event: RelationshipEvent) -> gpui_kit::Stateful<gpui_kit::Div> {
    let theme = rc.theme;
    let disabled = rc.input.stopping.is_some();
    let anchor = TooltipAnchor::new();
    let emit = rc.emit.clone();
    let glyph = if stopping { Spinner::new(SpinnerSize::Xs).into_any_element() } else { stop_glyph(theme) };
    let probe_id = id.to_string();
    control(id, theme, disabled, false)
        .children(probe(probe_id))
        .flex_none()
        .size(theme.rem(1.5))
        .px(px(0.))
        .gap(px(0.))
        .justify_center()
        .rounded(theme.radii.md)
        // `opacity-0 group-hover:opacity-100`: GPUI settles `visibility` before the group's hover
        // is known, so a hidden button would never take the click.
        .opacity(0.)
        .group_hover(group, |d| d.opacity(1.))
        .child(anchor.measure())
        .tooltip(anchor.tooltip(label, TooltipSide::Left))
        // It sits over the row's link: the click stops here. (Occluding it would end the row's
        // group hover, which is what shows the button.)
        .when(!disabled, |c| {
            c.on_click(move |_, window, cx| {
                cx.stop_propagation();
                emit(event.clone(), window, cx)
            })
        })
        .child(glyph)
}

/// The trailing text of an agent row: its compact elapsed time, hidden while its stop shows.
fn trailing_elapsed(rc: &RowCx<'_>, clock: &AgentClock, group: &SharedString, hides: bool, min_width: bool) -> AnyElement {
    let theme = rc.theme;
    div()
        .flex_none()
        .ui_text(theme, TextSize::Xs2, weight::NORMAL)
        .text_color(theme.colors.muted_foreground)
        .when(min_width, |d| d.min_w(theme.rem(1.5)).flex().justify_end())
        .when(hides, |d| d.group_hover(group.clone(), |d| d.invisible()))
        .children(clock.elapsed_ms(rc.now).map(format_compact_elapsed))
        .into_any_element()
}

/// A related thread's row: open on click, with the title and hint in its tooltip (or a child
/// agent's hover card). The fork's source splits into open, Merge back and the status; a workflow
/// coordinator splits into open and expand.
fn relationship_row(rc: &RowCx<'_>, section: &'static str, index: usize, row: &RelationshipRow) -> AnyElement {
    if let Some(remote) = &row.remote {
        return remote_row(rc, section, index, row, remote);
    }
    let (theme, input, emit) = (rc.theme, rc.input, rc.emit);
    let group: SharedString = format!("relationship-{section}-{index}").into();
    let agent = row.agent.as_ref();
    let workflow = row.workflow.as_ref().filter(|_| agent.is_some());
    let tip = match agent {
        Some(agent) => subagent_card(&agent.card),
        None => lineage_tip(&row.title, &row.hint),
    };
    let open = {
        let (emit, thread) = (emit.clone(), row.thread_id.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| emit(RelationshipEvent::Open(thread.clone()), window, cx)
    };
    let icon = relationship_icon(theme, row.icon, row.mark.as_ref(), row.status, input.any_instance_badge);
    let anchor = TooltipAnchor::new();
    let item = div().group(group.clone()).relative().flex().h(theme.rem(2.)).items_center().rounded(theme.radii.lg);
    let failed = row.status.is_some_and(RelationshipStatus::is_failed);
    let can_stop_workflow = workflow.is_some_and(|w| w.can_stop);
    let can_stop = agent.is_some_and(|a| a.can_stop) && workflow.is_none();
    let trailing: Option<AnyElement> = match agent {
        Some(agent) if !failed && (agent.clock.elapsed_ms(0).is_some() || can_stop_workflow) => {
            Some(trailing_elapsed(rc, &agent.clock, &group, can_stop || can_stop_workflow, can_stop_workflow))
        }
        // A workflow row's status is read out, not shown.
        _ if workflow.is_some() || row.merge_target => None,
        _ => Some(status_text(theme, row.status).when(can_stop, |d| d.group_hover(group.clone(), |d| d.invisible())).into_any_element()),
    };
    let content = |control: gpui_kit::Stateful<gpui_kit::Div>| {
        control
            .child(icon)
            .child(title_text(theme, row.title.clone()))
            .when(agent.is_none(), |c| c.child(hover_arrow(theme, group.clone())))
            .children(trailing)
    };
    let link_id = (SharedString::from(format!("relationship-{section}")), index);
    if let (Some(workflow), Some(_)) = (workflow, agent) {
        let primary = content(control(link_id, theme, false, false).flex_1().rounded_r(px(0.)))
            .children(probe(format!("relationship-row-{}", row.thread_id)))
            .tooltip_show_delay(ROW_TOOLTIP_DELAY)
            .tooltip(anchor.tooltip(tip, TooltipSide::Left))
            .on_click(open);
        return workflow_row(rc, section, index, group, item.child(anchor.measure()), primary, workflow);
    }
    let merge = input.merge.as_ref().filter(|_| row.merge_target);
    let Some(merge) = merge else {
        let control = content(control(link_id, theme, false, false).w_full())
            .children(probe(format!("relationship-row-{}", row.thread_id)))
            .tooltip_show_delay(ROW_TOOLTIP_DELAY)
            .tooltip(anchor.tooltip(tip, TooltipSide::Left))
            .on_click(open);
        let stop = agent.filter(|a| a.can_stop).map(|agent| {
            let stopping = input.stopping.as_deref() == Some(agent.subagent_id.as_str());
            div().absolute().right(theme.rem(0.25)).top_0().bottom_0().flex().items_center().child(stop_button(
                rc,
                format!("relationship-{section}-stop-{index}").into(),
                group.clone(),
                t!("Stop subagent"),
                stopping,
                RelationshipEvent::StopSubagent { owner: agent.owner_thread_id.clone(), subagent: agent.subagent_id.clone() },
            ))
        });
        return item.child(anchor.measure()).child(control).children(stop).into_any_element();
    };
    let primary = content(control(link_id, theme, false, false).flex_1().rounded_r(px(0.)))
        .tooltip_show_delay(ROW_TOOLTIP_DELAY)
        .tooltip(anchor.tooltip(tip, TooltipSide::Left))
        .on_click(open);
    let busy = rc.busy;
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
    let merge_anchor = TooltipAnchor::new();
    let secondary = control(SharedString::from(format!("relationship-{section}-merge")), theme, !can_merge, false)
        .flex_none()
        .w(split_secondary_width(theme))
        .px(px(0.))
        .justify_center()
        .rounded_l(px(0.))
        .child(merge_anchor.measure())
        .tooltip(merge_anchor.tooltip(merge_tip, TooltipSide::Left))
        .when(can_merge, |c| {
            let emit = emit.clone();
            c.on_click(move |_, window, cx| emit(RelationshipEvent::MergeBack, window, cx))
        })
        .child(glyph);
    let hairline = with_alpha(theme.colors.border, 0.65);
    let hover = row_hover(theme);
    let split = div()
        .relative()
        .flex()
        .w_full()
        .items_center()
        .rounded(theme.radii.lg)
        .hover(move |s| s.bg(hover))
        .child(anchor.measure())
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

/// `RemoteSubagentRow`: a task on a linked environment, opened there when connected.
fn remote_row(rc: &RowCx<'_>, section: &'static str, index: usize, row: &RelationshipRow, remote: &crate::state::thread_relationships::RemoteRow) -> AnyElement {
    let theme = rc.theme;
    let failed = remote.raw_status == ItemStatus::Failed;
    let anchor = TooltipAnchor::new();
    let emit = rc.emit.clone();
    let event = RelationshipEvent::OpenRemote { environment: remote.environment_id.clone(), thread: row.thread_id.clone() };
    let control = control((SharedString::from(format!("relationship-{section}")), index), theme, !remote.connected, false)
        .w_full()
        .tooltip_show_delay(ROW_TOOLTIP_DELAY)
        .tooltip(anchor.tooltip(lineage_tip(&row.title, &row.hint), TooltipSide::Left))
        .when(remote.connected, |c| c.on_click(move |_, window, cx| emit(event.clone(), window, cx)))
        .child(relationship_icon(theme, RelationshipIcon::Agent, row.mark.as_ref(), row.status, rc.input.any_instance_badge))
        .child(title_text(theme, row.title.clone()))
        .child(
            div()
                .flex()
                .flex_none()
                .max_w(gpui_kit::relative(0.5))
                .gap(theme.rem(0.25))
                .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                .text_color(if failed { theme.colors.destructive() } else { theme.colors.muted_foreground })
                .child(div().min_w_0().truncate().child(SharedString::from(t_rich!("on {label}", label = remote.label.as_ref()).plain())))
                .child(div().flex_none().child(SharedString::from(format!("· {}", status_label(row.status))))),
        );
    div().relative().flex().h(theme.rem(2.)).items_center().rounded(theme.radii.lg).child(anchor.measure()).child(control).into_any_element()
}

/// `WorkflowMemberRow`: a member agent, opened when it has a thread.
fn workflow_member(rc: &RowCx<'_>, id: SharedString, member: &WorkflowMember, mark: Option<&ProviderMark>) -> AnyElement {
    let theme = rc.theme;
    let anchor = TooltipAnchor::new();
    let emit = rc.emit.clone();
    let thread = member.thread_id.clone();
    let status = Some(RelationshipStatus::from_wire(member.status.as_str()));
    let control = control(id, theme, thread.is_none(), false)
        .w_full()
        .tooltip_show_delay(ROW_TOOLTIP_DELAY)
        .tooltip(anchor.tooltip(subagent_card(&member.card), TooltipSide::Left))
        .when_some(thread, |c, thread| c.on_click(move |_, window, cx| emit(RelationshipEvent::Open(thread.clone()), window, cx)))
        .child(relationship_icon(theme, RelationshipIcon::Agent, mark, status, rc.input.any_instance_badge))
        .child(div().flex_1().min_w_0().truncate().child(member.title.clone()))
        .children(member.clock.started_at.map(|_| {
            div()
                .flex_none()
                .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                .text_color(theme.colors.muted_foreground)
                .children(member.clock.elapsed_ms(rc.now).map(format_elapsed))
        }));
    div().relative().child(anchor.measure()).child(control).into_any_element()
}

/// `ThreadLineageWorkflowRow`: the coordinator's link split with an expand chevron, a stop for
/// the whole workflow, and when expanded its phases (each a collapsible header) and members.
fn workflow_row(
    rc: &RowCx<'_>,
    section: &'static str,
    index: usize,
    group: SharedString,
    item: gpui_kit::Div,
    primary: gpui_kit::Stateful<gpui_kit::Div>,
    workflow: &WorkflowRow,
) -> AnyElement {
    let theme = rc.theme;
    let expanded = rc.view.workflows.contains(&workflow.coordinator);
    let label = workflow.label.clone();
    let coordinator = workflow.coordinator.clone();
    let toggle = {
        let (state, coordinator) = (rc.state.clone(), coordinator.clone());
        move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut App| {
            state(&|s| {
                if !s.workflows.remove(&coordinator) {
                    s.workflows.insert(coordinator.clone());
                }
            }, window, cx)
        }
    };
    let chevron = Icon::new(IconName::ChevronDown).size(IconSize::Custom(theme.rem(0.875))).color(theme.colors.muted_foreground).rotate(if expanded { 0. } else { -90. });
    let secondary = control(SharedString::from(format!("relationship-{section}-expand-{index}")), theme, false, false)
        .flex_none()
        .w(split_secondary_width(theme))
        .px(px(0.))
        .justify_center()
        .rounded_l(px(0.))
        .on_click(toggle)
        .child(chevron);
    let stop = workflow.can_stop.then(|| {
        let stopping = rc.input.stopping.as_deref() == Some(coordinator.as_str());
        div().absolute().right(theme.rem(2.25)).top_0().bottom_0().flex().items_center().child(stop_button(
            rc,
            format!("relationship-{section}-wfstop-{index}").into(),
            group.clone(),
            t!("Stop entire workflow"),
            stopping,
            RelationshipEvent::StopWorkflow(coordinator.clone()),
        ))
    });
    let hairline = with_alpha(theme.colors.border, 0.65);
    let hover = row_hover(theme);
    let split = item
        .w_full()
        .hover(move |s| s.bg(hover))
        .child(primary)
        .children(stop)
        .child(div().h(theme.rem(1.)).w(px(1.)).flex_none().group_hover(group.clone(), move |h| h.bg(hairline)))
        .child(secondary);
    let mut out = div().flex().flex_col().child(split);
    if expanded {
        let mut list = div().flex().flex_col().pl(theme.rem(1.625));
        let has_members = !workflow.phases.is_empty() || !workflow.unphased.is_empty();
        for phase in &workflow.phases {
            let key = (coordinator.clone(), phase.index);
            let open = rc.view.phases.get(&key).copied().unwrap_or(phase.running);
            let tone = if phase.running { with_alpha(theme.colors.sidebar_foreground, 0.8) } else { theme.colors.sidebar_muted_foreground };
            let count = t!(
                "{count, plural, one {{settled}/{count} agent} other {{settled}/{count} agents}}",
                count = phase.members.len() as i64,
                settled = phase.settled as i64
            );
            let state = rc.state.clone();
            let header = div()
                .id(SharedString::from(format!("relationship-{section}-{index}-phase-{}", phase.index)))
                .flex()
                .h(theme.rem(2.))
                .w_full()
                .items_center()
                .gap(theme.rem(0.5))
                .pl(theme.rem(0.5))
                .pr(theme.rem(0.625))
                .rounded(theme.radii.md)
                .cursor_pointer()
                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                .text_color(tone)
                .on_click(move |_, window, cx| {
                    let key = key.clone();
                    state(&|s| {
                        s.phases.insert(key.clone(), !open);
                    }, window, cx)
                })
                .child(div().min_w_0().truncate().child(phase.title.clone()))
                // The rule is hidden: a header with trailing counts drops it.
                .child(div().h(px(1.)).min_w(theme.rem(0.5)).flex_1())
                .children((phase.status == PhaseStatus::Failed).then(|| {
                    div()
                        .flex_none()
                        .ui_text(theme, TextSize::Xs3, weight::NORMAL)
                        .line_height(px(TextSize::Xs3.size(theme.fonts.interface)))
                        .text_color(theme.colors.destructive_foreground())
                        .child(t!("Failed", context = "workflow phase status"))
                }))
                .child(div().flex_none().ui_text(theme, TextSize::Xs2, weight::NORMAL).child(count))
                .children(phase.clock.elapsed_ms(rc.now).map(|ms| div().flex_none().ui_text(theme, TextSize::Xs2, weight::NORMAL).child(format_elapsed(ms))))
                .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(theme.rem(0.75))).color(tone).rotate(if open { 0. } else { -90. }));
            list = list.child(header);
            if open {
                for (ix, member) in phase.members.iter().enumerate() {
                    list = list.child(workflow_member(rc, format!("relationship-{section}-{index}-p{}-{ix}", phase.index).into(), member, workflow.mark.as_ref()));
                }
            }
        }
        for (ix, member) in workflow.unphased.iter().enumerate() {
            list = list.child(workflow_member(rc, format!("relationship-{section}-{index}-u{ix}").into(), member, workflow.mark.as_ref()));
        }
        if !has_members {
            list = list.child(
                div()
                    .px(theme.rem(0.625))
                    .py(theme.rem(0.375))
                    .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                    .text_color(with_alpha(theme.colors.muted_foreground, 0.7))
                    .child(t!("No agents yet")),
            );
        }
        out = out.child(list);
    }
    let _ = label;
    out.into_any_element()
}

/// `ThreadLineageRowList`: the rows (scrolling past 13.5rem, or `min(28rem, 55dvh)` while a
/// workflow is expanded) and "Show N more".
fn row_list(theme: &AppTheme, id: &'static str, rows: Vec<AnyElement>, hidden: usize, state: &RelationshipState, tall: Option<gpui_kit::Pixels>) -> Vec<AnyElement> {
    let max = tall.map_or(theme.rem(13.5), |viewport| theme.rem(28.).min(viewport * 0.55));
    let mut out = vec![div().id(SharedString::from(format!("relationships-{id}"))).max_h(max).overflow_y_scroll().children(rows).into_any_element()];
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

/// Whether a row on screen runs a timer, so the card redraws each second while it shows.
pub fn has_live_clock(input: &RelationshipsInput) -> bool {
    input.groups.iter().flat_map(|g| g.rows.iter()).chain(input.side_chats.iter()).any(|row| {
        row.agent.as_ref().is_some_and(|a| a.clock.live)
            || row.workflow.as_ref().is_some_and(|w| w.phases.iter().any(|p| p.clock.live || p.members.iter().any(|m| m.clock.live)) || w.unphased.iter().any(|m| m.clock.live))
    })
}

/// The Lineage and Side chats sections, as the card's last sections before usage. `viewport`
/// is the window height (`55dvh` for an expanded workflow's list).
#[allow(clippy::too_many_arguments)]
pub fn relationship_sections(
    theme: &AppTheme,
    input: &RelationshipsInput,
    view: &RelationshipViewState,
    busy: Option<RelationshipBusy>,
    emit: RelationshipEmit,
    state: RelationshipState,
    viewport: gpui_kit::Pixels,
) -> Vec<AnyElement> {
    let rc = RowCx { theme, input, view, busy, emit: &emit, state: &state, now: t3code_client_runtime::state::models::now_ms() };
    let tall = |rows: &[RelationshipRow]| rows.iter().any(|r| r.workflow.as_ref().is_some_and(|w| view.workflows.contains(&w.coordinator))).then_some(viewport);
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
            let anchor = TooltipAnchor::new();
            let row = control("relationship-delegated-from", theme, !from.connected, false)
                .w_full()
                .tooltip_show_delay(ROW_TOOLTIP_DELAY)
                .tooltip(anchor.tooltip(lineage_tip(&from.title, &hint), TooltipSide::Left))
                .when(from.connected, |r| r.on_click(move |_, window, cx| emit(RelationshipEvent::OpenDelegatedFrom, window, cx)))
                .child(relationship_icon(theme, RelationshipIcon::Parent, None, None, input.any_instance_badge))
                .child(title_text(theme, from.title.clone()))
                .children(from.label.as_ref().map(|label| {
                    div()
                        .flex_none()
                        .max_w(gpui_kit::relative(0.5))
                        .truncate()
                        .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                        .text_color(theme.colors.muted_foreground)
                        .child(SharedString::from(t_rich!("on {label}", label = label.as_ref()).plain()))
                }));
            children.push(div().relative().flex().h(theme.rem(2.)).items_center().rounded(theme.radii.lg).child(anchor.measure()).child(row).into_any_element());
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
                        // `SectionHeaderStatus`.
                        .children((failed > 0).then(|| {
                            div()
                                .flex_none()
                                .ui_text(theme, TextSize::Xs3, weight::NORMAL)
                                .line_height(px(TextSize::Xs3.size(theme.fonts.interface)))
                                .text_color(theme.colors.destructive_foreground())
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
                let rows = group.rows.iter().take(visible).enumerate().map(|(ix, row)| relationship_row(&rc, group.id, ix, row)).collect();
                children.extend(row_list(theme, group.id, rows, group.rows.len().saturating_sub(visible), &state, tall(&group.rows)));
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
        let rows = input.side_chats.iter().take(visible).enumerate().map(|(ix, row)| relationship_row(&rc, "side-chats", ix, row)).collect();
        let children = row_list(theme, "side-chats", rows, input.side_chats.len().saturating_sub(visible), &state, tall(&input.side_chats));
        let title = if input.running_side_chats > 0 {
            t!("Side chats · {runningSideChats} running", runningSideChats = input.running_side_chats as i64)
        } else {
            t!("Side chats")
        };
        out.push(section(theme, title, true, None, children));
    }
    out
}
