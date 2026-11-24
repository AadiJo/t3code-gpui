//! Non-thread list items: shelf headers (`SidebarListMarkers.tsx`,
//! `collapsible-section-header.tsx`), project folder headers
//! (`SidebarProjectFolderHeader.tsx`), "Show N more" and the empty states.

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sidebar_thread_row::project_favicon;
use super::style::*;
use super::{Sidebar, SidebarEvent};
use crate::components::sidebar_logic::{EmptyState, FolderDot, Shelf};
use crate::components::thread_status_indicators::folder_dot;

/// Expanded points down, collapsed points right (the -90deg rotation).
fn chevron(expanded: bool, color: Hsla, colors: &ShellColors) -> Icon {
    Icon::new(if expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        }).size(IconSize::Custom(colors.r(12.))).color(color)
}

impl Sidebar {
    fn entry_focused(&self, ix: usize, window: &Window) -> bool {
        self.keyboard_nav && self.cursor == Some(ix) && self.focus_handle.is_focused(window)
    }

    /// Working / Snoozed / Settled shelf header.
    pub(crate) fn render_shelf_header(
        &mut self,
        ix: usize,
        shelf: Shelf,
        count: usize,
        expanded: bool,
        colors: &ShellColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match shelf {
            Shelf::Working => "Working",
            Shelf::Snoozed => "Snoozed",
            Shelf::Settled => "Settled",
        };
        let drag_tone = (shelf == Shelf::Settled).then(|| super::sidebar_drag::settled_header_drag_tone(self, colors)).flatten();
        let tone = drag_tone.unwrap_or(colors.sidebar_muted_foreground);
        div()
            .mx(colors.r(2.))
            .h(colors.s(SECTION_HEADER_HEIGHT))
            .child(
                div()
                    .id(("shelf", ix))
                    .flex()
                    .h(colors.s(SECTION_HEADER_HEIGHT))
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .rounded(colors.s(ROW_RADIUS))
                    .cursor_pointer()
                    .text_xs()
                    .line_height(colors.r(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tone)
                    .when(self.entry_focused(ix, window), |d| {
                        d.border_2().border_color(colors.ring.opacity(0.7))
                    })
                    .child(div().min_w_0().truncate().child(label))
                    // The rule hides when a count follows, so it never reads as a leader.
                    .child(div().h(px(1.)).min_w(colors.r(8.)).flex_1())
                    .child(
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .child(count.to_string()),
                    )
                    .child(chevron(expanded, tone, colors))

                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.cursor = Some(ix);
                        this.keyboard_nav = false;
                        this.toggle_shelf(shelf, cx);
                    })),
            )
            .into_any_element()
    }

    /// Project folder header in "boring sidebar" mode.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_folder_header(
        &mut self,
        ix: usize,
        group: usize,
        collapsed: bool,
        hidden_count: usize,
        dot: Option<FolderDot>,
        first: bool,
        colors: &ShellColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = self.list.groups.get(group).cloned() else {
            return div().into_any_element();
        };
        let hovered = self.hovered == Some(ix);
        let new_thread_ref = project.member_refs.first().cloned();
        div()
            .relative()
            .when(!first, |d| d.mt(colors.s(FOLDER_HEADER_GAP)))
            .child(
                div()
                    .id(("folder", ix))
                    .flex()
                    .h(colors.s(SECTION_HEADER_HEIGHT))
                    .w_full()
                    .items_center()
                    .gap(colors.r(10.))
                    .px(colors.s(ROW_CONTENT_INSET))
                    .rounded(colors.s(CONTROL_RADIUS))
                    .cursor_pointer()
                    .when(hovered, |d| d.bg(colors.sidebar_row_hover))
                    .when(self.entry_focused(ix, window), |d| d.border_2().border_color(colors.ring))
                    .child(project_favicon(
                        project.label.clone(),
                        Some(&crate::components::project_favicon::ProjectIconSource { icon: project.icon.clone(), favicon: project.favicon.clone() }),
                        colors,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .line_height(colors.r(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(colors.sidebar_foreground.opacity(0.9))
                            .child(project.label.clone()),
                    )
                    .children(dot.map(|dot| folder_dot(dot, colors)))
                    .when(collapsed && hidden_count > 0, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(colors.sidebar_muted_foreground)
                                .when(hovered, |d| d.invisible())
                                .child(hidden_count.to_string()),
                        )
                    })
                    .child(chevron(!collapsed, colors.sidebar_muted_foreground, colors))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.set_row_hover(ix, *hovered, cx);
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.cursor = Some(ix);
                        this.keyboard_nav = false;
                        this.toggle_folder(group, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.open_entry_menu(ix, event.position, cx);
                        }),
                    ),
            )
            .when(hovered, |d| {
                // The new-thread button sits over the count, left of the chevron.
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .right(colors.r(24.))
                        .h(colors.s(SECTION_HEADER_HEIGHT))
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .id(("folder-new-thread", ix))
                                .occlude()
                                .size(colors.r(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(colors.r(6.))
                                .cursor_pointer()
                                .hover(|s| s.bg(colors.accent))
                                .child(Icon::new(IconName::SquarePen).size(IconSize::Custom(colors.r(14.))).color(colors.muted_foreground))
                                .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("New thread in {project}", project = project.label.clone())))
                                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                    if *hovered {
                                        this.set_row_hover(ix, true, cx);
                                    }
                                }))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.emit(SidebarEvent::NewThread(new_thread_ref.clone()));
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    /// "Show N more" under the settled shelf.
    pub(crate) fn render_show_more(
        &mut self,
        ix: usize,
        count: usize,
        colors: &ShellColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hovered = self.hovered == Some(ix);
        let fg = if hovered {
            colors.sidebar_foreground
        } else {
            colors.sidebar_muted_foreground
        };
        div()
            .id(("show-more", ix))
            .flex()
            .h(colors.s(SLIM_ROW_HEIGHT))
            .items_center()
            .gap(colors.r(10.))
            .px(colors.r(10.))
            .rounded(colors.s(ROW_RADIUS))
            .cursor_pointer()
            .text_sm()
            .line_height(colors.r(20.))
            .text_color(fg)
            .when(hovered, |d| d.bg(colors.sidebar_row_hover))
            .when(self.entry_focused(ix, window), |d| d.border_2().border_color(colors.ring))
            .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(colors.r(16.))).color(fg))
            .child(format!("Show {count} more"))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_row_hover(ix, *hovered, cx);
            }))
            .on_click(cx.listener(|this, _, _, cx| this.show_more_settled(cx)))
            .into_any_element()
    }

    /// "No projects yet" / "No threads yet".
    pub(crate) fn render_empty(
        &mut self,
        state: &EmptyState,
        colors: &ShellColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let column = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .px_2()
            .py_6()
            .text_xs()
            .line_height(colors.r(16.))
            .text_color(colors.muted_foreground.opacity(0.6));
        let pill = |id: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(colors.r(6.))
                .px(colors.r(10.))
                .py_1()
                .rounded(colors.s(ROW_RADIUS))
                .border_1()
                .border_color(colors.sidebar_border)
                .cursor_pointer()
                .text_size(colors.r(11.))
                .line_height(colors.r(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.sidebar_muted_foreground)
                .hover(|s| s.bg(colors.sidebar_row_hover).text_color(colors.sidebar_foreground))
        };
        match state {
            EmptyState::NoThreads => column.child("No threads yet").into_any_element(),
            EmptyState::Scoped { project, has_settled } => {
                use crate::i18n::t;
                let message = match (project, has_settled) {
                    (Some(project), true) => t!("No active threads in {project}", project = project.clone()),
                    (None, true) => t!("No active threads in these projects"),
                    (Some(project), false) => t!("No threads in {project} yet", project = project.clone()),
                    (None, false) => t!("No threads in these projects yet"),
                };
                column
                    .child(message)
                    .child(
                        pill("empty-show-all-projects")
                            .child(t!("Show all projects"))
                            .on_click(cx.listener(|this, _, _, cx| this.set_project_scope(Vec::new(), cx))),
                    )
                    .into_any_element()
            }
            EmptyState::NoProjects => column
                .child("No projects yet")
                .child(
                    pill("empty-add-project")
                        .child(Icon::new(IconName::Plus).size(IconSize::Custom(colors.r(12.))).color(colors.sidebar_muted_foreground))
                        .child("Add project")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::AddProject))),
                )
                .into_any_element(),
        }
    }
}

/// "No threads found" for a search with no hits.
pub(crate) fn search_empty(colors: &ShellColors) -> AnyElement {
    div()
        .px_2()
        .py_6()
        .flex()
        .justify_center()
        .text_xs()
        .line_height(colors.r(16.))
        .text_color(colors.sidebar_muted_foreground)
        .child("No threads found")
        .into_any_element()
}
