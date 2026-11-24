//! Dragging a thread row between sections (`Sidebar.tsx` DndContext,
//! `SidebarListMarkers.tsx`). While a row is dragged the list shows the
//! "Pinned" and "Active" boundary labels and, under a closed Settled shelf,
//! a dashed "Settled" drop box. Dropping on a section moves the thread there:
//! Pinned pins it, Active unpins, un-settles or wakes it, Settled settles it.
//! The Working shelf orders active threads by time, so there is no manual
//! reordering among active rows.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::context_menu::MenuCommand;
use super::model::{SidebarProvider, ThreadKey};
use super::sidebar_thread_row::project_favicon;
use crate::components::icons::{Icon, IconName, IconSize};
use super::style::*;
use super::{Sidebar, SidebarEvent};
use crate::components::sidebar_logic::{DragMarker, SidebarListEntry, SidebarSection};

/// The value a dragged row carries, and what its floating copy draws.
#[derive(Clone)]
pub struct DraggedThread {
    pub key: ThreadKey,
    pub section: SidebarSection,
    project_title: SharedString,
    project_label: SharedString,
    title: SharedString,
    branch: Option<SharedString>,
    provider: Option<SidebarProvider>,
    width: Pixels,
    /// The section under the pointer, shared with the sidebar, which hit-tests it.
    target: Rc<Cell<Option<SidebarSection>>>,
}

/// `resolveSidebarDropVerb`: what dropping `from` into `to` does, as the
/// dragged row's badge says it.
fn drop_verb(from: SidebarSection, to: Option<SidebarSection>) -> Option<(IconName, SharedString)> {
    use SidebarSection as S;
    let to = to?;
    if to == from || matches!(to, S::Working | S::Snoozed) {
        return None;
    }
    Some(match (from, to) {
        (_, S::Pinned) => (IconName::Pin, crate::i18n::t!("Pin")),
        (_, S::Settled) => (IconName::CircleCheck, crate::i18n::t!("Settle")),
        (S::Pinned, _) => (IconName::PinOff, crate::i18n::t!("Unpin")),
        (S::Settled, _) => (IconName::Undo2, crate::i18n::t!("Un-settle")),
        _ => (IconName::AlarmClockOff, crate::i18n::t!("Wake")),
    })
}

/// The row that follows the pointer while dragging.
pub struct DragPreview(DraggedThread);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ActiveAppTheme as _;
        let colors = ShellColors::from_theme(cx.app_theme());
        let d = &self.0;
        let verb = drop_verb(d.section, d.target.get()).map(|(icon, label)| {
            // `h-5 gap-1 rounded-sm border border-primary/40 bg-primary/10 px-1.5 text-2xs font-medium text-primary`.
            div()
                .ml_auto()
                .flex()
                .flex_none()
                .h(colors.r(20.))
                .items_center()
                .gap(colors.r(4.))
                .px(colors.r(6.))
                .rounded(colors.r(4.))
                .border_1()
                .border_color(colors.primary.opacity(0.4))
                .bg(colors.primary.opacity(0.1))
                .text_size(colors.r(10.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.primary)
                .child(Icon::new(icon).size(IconSize::Custom(colors.r(12.))).color(colors.primary))
                .child(label)
        });
        div()
            .w(d.width)
            .rounded(colors.s(ROW_RADIUS))
            .bg(colors.sidebar_row_active)
            .text_color(colors.sidebar_foreground)
            .child(
                div()
                    .h(colors.s(if d.branch.is_some() { CARD_ROW_HEIGHT } else { CARD_ROW_HEIGHT_NO_META }))
                    .px(colors.s(ROW_CONTENT_INSET))
                    .py(colors.s(CONTENT_INSET))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .h(colors.r(20.))
                            .items_center()
                            .gap(colors.r(6.))
                            .child(project_favicon(d.project_title.clone(), &colors))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .line_height(colors.r(16.))
                                    .text_color(colors.secondary_label)
                                    .child(d.project_label.clone()),
                            )
                            .children(verb),
                    )
                    .child(
                        div()
                            .mt_1()
                            .truncate()
                            .text_sm()
                            .line_height(colors.r(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.foreground)
                            .child(d.title.clone()),
                    )
                    .children(d.branch.clone().map(|branch| {
                        div()
                            .mt(colors.r(2.))
                            .flex()
                            .items_center()
                            .text_xs()
                            .line_height(colors.r(16.))
                            .text_color(colors.secondary_label)
                            .child(div().flex_1().truncate().child(branch))
                            .children(d.provider.as_ref().map(|p| {
                                super::sidebar_thread_row::provider_stack(p, &colors, cx)
                            }))
                    })),
            )
    }
}

impl Sidebar {
    /// The drag value for thread entry `ix`, if it can be dragged: never
    /// while renaming or in project folders (folders order by time).
    pub(crate) fn drag_value(&self, ix: usize, colors: &ShellColors) -> Option<DraggedThread> {
        let Some(SidebarListEntry::Thread { thread, section, .. }) = self.list.entries.get(ix) else { return None };
        if self.renaming.is_some() || self.snapshot.settings.project_folders {
            return None;
        }
        let project = thread.project_ref();
        Some(DraggedThread {
            key: thread.key.clone(),
            section: *section,
            project_title: self.list.project_titles.get(&project).cloned().unwrap_or_default(),
            project_label: self.list.project_labels.get(&project).cloned().unwrap_or_default(),
            title: thread.title.clone(),
            branch: thread.branch.clone(),
            provider: thread.provider.clone(),
            width: px(240.) * colors.rem_scale,
            target: self.drag_target.clone(),
        })
    }

    /// The drag started: the boundary labels and drop box appear.
    pub(crate) fn begin_drag(&mut self, key: ThreadKey, section: SidebarSection, cx: &mut Context<Self>) {
        self.hover_card = None;
        self.hover_card_open = None;
        self.options.dragging = Some(section);
        self.options.drag_key = Some(key);
        self.drag_target.set(None);
        self.rebuild(cx);
    }

    /// The drag ended, dropped or not.
    pub(crate) fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.options.dragging.take().is_some() {
            self.options.drag_key = None;
            self.drag_target.set(None);
            self.rebuild(cx);
        }
    }

    /// The section a drop at `position` lands in (`resolveSidebarDropTarget`).
    pub(crate) fn section_at(&self, position: Point<Pixels>) -> Option<SidebarSection> {
        use crate::components::sidebar_logic::Shelf;
        (0..self.list.entries.len()).find_map(|ix| {
            let bounds = self.list_state.bounds_for_item(ix)?;
            if !bounds.contains(&position) {
                return None;
            }
            match self.list.entries.get(ix)? {
                SidebarListEntry::Thread { section, .. } => Some(*section),
                SidebarListEntry::DragBoundary(DragMarker::Pinned) => Some(SidebarSection::Pinned),
                SidebarListEntry::DragBoundary(DragMarker::Active) => Some(SidebarSection::Active),
                SidebarListEntry::ShelfHeader { shelf: Shelf::Settled, .. } | SidebarListEntry::DragPlaceholder => Some(SidebarSection::Settled),
                SidebarListEntry::ShelfHeader { shelf: Shelf::Snoozed, .. } => Some(SidebarSection::Snoozed),
                SidebarListEntry::ShelfHeader { shelf: Shelf::Working, .. } => Some(SidebarSection::Working),
                _ => None,
            }
        })
    }

    /// The pointer moved during a drag: track the section under it.
    pub(crate) fn drag_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        // dnd-kit's auto-scroll (`threshold: { y: 0.05 }`): within 5% of the
        // list's top or bottom edge, or past it, the list scrolls toward it.
        let viewport = self.list_state.viewport_bounds();
        let edge = viewport.size.height * 0.05;
        if position.y > viewport.bottom() - edge {
            self.list_state.scroll_by((position.y - (viewport.bottom() - edge)).min(px(40.)).max(px(4.)));
            cx.notify();
        } else if position.y < viewport.top() + edge {
            self.list_state.scroll_by(-((viewport.top() + edge) - position.y).min(px(40.)).max(px(4.)));
            cx.notify();
        }
        let target = self.section_at(position);
        if self.drag_target.get() != target {
            self.drag_target.set(target);
            cx.notify();
        }
    }

    /// The command a drop into `target` runs for a row dragged from its section.
    fn drop_command(from: SidebarSection, target: SidebarSection) -> Option<MenuCommand> {
        use SidebarSection as S;
        match (from, target) {
            (S::Pinned, S::Pinned) | (S::Settled, S::Settled) => None,
            (_, S::Pinned) => Some(MenuCommand::Pin),
            (_, S::Settled) => Some(MenuCommand::Settle),
            (S::Pinned, S::Active) => Some(MenuCommand::Unpin),
            (S::Settled, S::Active) => Some(MenuCommand::Unsettle),
            (S::Snoozed, S::Active) => Some(MenuCommand::Wake),
            _ => None,
        }
    }

    /// Drops `dragged` into `target`.
    pub(crate) fn drop_into(&mut self, dragged: &DraggedThread, target: SidebarSection, cx: &mut Context<Self>) {
        let command = Self::drop_command(dragged.section, target);
        self.end_drag(cx);
        if let Some(command) = command {
            cx.emit(SidebarEvent::ThreadCommand(vec![dragged.key.clone()], command));
        }
    }

    /// `SidebarDragBoundary`: "Pinned" or "Active" and a rule, while dragging.
    pub(crate) fn render_drag_boundary(&mut self, marker: DragMarker, colors: &ShellColors, cx: &mut Context<Self>) -> AnyElement {
        let (label, target) = match marker {
            DragMarker::Pinned => (crate::i18n::t!("Pinned"), SidebarSection::Pinned),
            DragMarker::Active => (crate::i18n::t!("Active"), SidebarSection::Active),
        };
        let primary = colors.primary;
        let is_target = self.drag_target.get() == Some(target);
        let _ = cx;
        div()
            .id(match marker {
                DragMarker::Pinned => "drag-boundary-pinned",
                DragMarker::Active => "drag-boundary-active",
            })
            .mx(colors.r(2.))
            // `SIDEBAR_DRAG_LABEL_HEIGHT` is 24px at any rem, less the list gap.
            .h(px(23.))
            .px(colors.r(8.))
            .pt(colors.r(4.))
            .child(
                div()
                    .flex()
                    .h(colors.r(16.))
                    .items_center()
                    .gap(colors.r(8.))
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if is_target { primary } else { colors.sidebar_foreground.opacity(0.8) })
                    .child(div().flex_none().child(label))
                    .child(div().h(px(1.)).flex_1().bg(if is_target { primary.opacity(0.5) } else { colors.sidebar_foreground.opacity(0.25) })),
            )
            .into_any_element()
    }

    /// `SidebarSectionPlaceholder` under a closed Settled shelf while dragging.
    pub(crate) fn render_drag_placeholder(&mut self, colors: &ShellColors, cx: &mut Context<Self>) -> AnyElement {
        let primary = colors.primary;
        let is_target = self.drag_target.get() == Some(SidebarSection::Settled);
        let _ = cx;
        div()
            .id("drag-placeholder-settled")
            .mx(colors.r(2.))
            .flex()
            .h(colors.s(SLIM_ROW_HEIGHT))
            .items_center()
            .justify_center()
            .rounded(colors.s(ROW_RADIUS))
            .border_1()
            .border_dashed()
            .border_color(if is_target { primary.opacity(0.4) } else { colors.sidebar_foreground.opacity(0.25) })
            .when(is_target, |d| d.bg(primary.opacity(0.05)))
            .text_xs()
            .text_color(if is_target { primary } else { colors.sidebar_foreground.opacity(0.8) })
            .child(crate::i18n::t!("Settled"))
            .into_any_element()
    }

}

/// The drag preview's constructor for [`InteractiveElement::on_drag`].
pub(crate) fn drag_preview(
    sidebar: WeakEntity<Sidebar>,
) -> impl Fn(&DraggedThread, Point<Pixels>, &mut Window, &mut App) -> Entity<DragPreview> + 'static {
    move |dragged, _, _, cx| {
        let (key, section) = (dragged.key.clone(), dragged.section);
        sidebar.update(cx, |sidebar, cx| sidebar.begin_drag(key, section, cx)).ok();
        cx.new(|_| DragPreview(dragged.clone()))
    }
}

/// Settled shelf header colour while dragging: emphasized, primary under the
/// pointer (`tone="emphasized"` / `"accent"`).
pub(crate) fn settled_header_drag_tone(sidebar: &Sidebar, colors: &ShellColors) -> Option<Hsla> {
    sidebar.options.dragging?;
    Some(if sidebar.drag_target.get() == Some(SidebarSection::Settled) { colors.primary } else { colors.sidebar_foreground })
}
