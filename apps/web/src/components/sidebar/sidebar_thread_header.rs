//! Search field and the filter / add project / new thread buttons. Ports
//! `SidebarThreadHeader.tsx` (`05-ui-shell.md` 6.3).

use crate::components::icons::{Icon, IconName, IconSize};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::style::*;
use super::{Sidebar, SidebarEvent};

/// A 28x28 sidebar icon button (`SidebarMenuButton size="icon"`): icon in
/// `--sidebar-icon-color`, hover fill `sidebar-row-hover` and icon
/// `sidebar-foreground`, pressed fill `sidebar-row-active`. Swap for the shared
/// Button once it lands.
pub fn sidebar_icon_button(
    id: impl Into<ElementId>,
    glyph: IconName,
    size: Pixels,
    hovered: bool,
    colors: &ShellColors,
) -> Stateful<Div> {
    let icon = Icon::new(glyph).size(IconSize::Custom(colors.r(16.))).color(if hovered {
        colors.sidebar_foreground
    } else {
        colors.sidebar_icon
    });
    sidebar_icon_button_with(id, icon, size, hovered, colors)
}

/// [`sidebar_icon_button`] around any icon (the project filter's favicons).
pub fn sidebar_icon_button_with(
    id: impl Into<ElementId>,
    icon: impl IntoElement,
    size: Pixels,
    hovered: bool,
    colors: &ShellColors,
) -> Stateful<Div> {
    let active = colors.sidebar_row_active;
    div()
        .id(id)
        .occlude()
        .relative()
        .size(size)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(colors.s(CONTROL_RADIUS))
        .cursor_pointer()
        .when(hovered, |d| d.bg(colors.sidebar_row_hover))
        .active(move |s| s.bg(active))
        .child(icon)
}

/// Header controls that track hover in sidebar state (icons swap colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderControl {
    Search,
    Filter,
    AddProject,
    NewThread,
    Settings,
    PullRequests,
    Issues,
    Usage,
    CheckForUpdates,
}

impl Sidebar {
    pub(crate) fn control_hover_listener(
        &self,
        control: HeaderControl,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(move |this, hovered: &bool, _, cx| {
            let next = if *hovered {
                Some(control)
            } else if this.hovered_control == Some(control) {
                None
            } else {
                return;
            };
            this.hovered_control = next;
            cx.notify();
        })
    }

    /// The `p-2` group holding search and the three action buttons.
    pub(crate) fn render_search_row(
        &mut self,
        colors: &ShellColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let search_focused = self.search.focus_handle(cx).is_focused(window);
        let searching = !self.options.search.is_empty();
        let search_hot = search_focused || self.hovered_control == Some(HeaderControl::Search);
        let scope_menu = self.render_scope_menu(colors, cx);
        let hovered = |control| self.hovered_control == Some(control);
        let field = div()
            .id("sidebar-search")
            .occlude()
            .flex()
            .h(colors.r(32.))
            .min_w_0()
            .flex_1()
            .items_center()
            .gap_2()
            .px_2()
            .py(colors.r(6.))
            .rounded(colors.s(ROW_RADIUS))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(if search_hot {
                colors.sidebar_foreground
            } else {
                colors.sidebar_muted_foreground
            })
            .when(search_hot, |d| d.bg(colors.sidebar_row_hover))
            .when(search_focused, |d| d.border_1().border_color(colors.ring.opacity(0.7)))
            .child(Icon::new(IconName::Search).size(IconSize::Custom(colors.r(16.))).color(colors.sidebar_icon))
            .child(
                // The placeholder is drawn here in `sidebarMutedForeground`: stock inputs
                // draw theirs in the global placeholder colour.
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .when(!searching, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .text_sm()
                                .line_height(colors.r(21.))
                                .text_color(colors.sidebar_muted_foreground)
                                .child(crate::i18n::t!("Search")),
                        )
                    })
                    .child(
                        Input::new(&self.search)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .p_0()
                            .h(colors.r(21.))
                            .text_sm(),
                    ),
            )
            .when(searching, |d| {
                d.child(
                    div()
                        .id("sidebar-search-clear")
                        .size(colors.r(20.))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(colors.r(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(colors.accent))
                        .child(Icon::new(IconName::X).size(IconSize::Custom(colors.r(12.))).color(colors.muted_foreground))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.search.update(cx, |state, cx| state.set_value("", window, cx));
                            this.options.search = SharedString::default();
                            this.rebuild(cx);
                        })),
                )
            })
            .on_hover(self.control_hover_listener(HeaderControl::Search, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.search.update(cx, |state, cx| state.focus(window, cx));
            }));

        div()
            .p(colors.s(CONTENT_INSET))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(field)
                    .child(
                        div()
                            .flex()
                            .child(
                                div()
                                    .relative()
                                    .child(
                                        sidebar_icon_button_with(
                                            "sidebar-filter",
                                            self.scope_trigger_icon(hovered(HeaderControl::Filter), colors),
                                            colors.r(28.),
                                            hovered(HeaderControl::Filter) || self.scope_menu.is_some(),
                                            colors,
                                        )
                                        .when(self.scope_menu.is_none(), |d| {
                                            d.tooltip(crate::components::ui::tooltip::tooltip(self.scope_trigger_label()))
                                        })
                                        .on_hover(self.control_hover_listener(HeaderControl::Filter, cx))
                                        .on_click(cx.listener(|this, _, window, cx| this.toggle_scope_menu(window, cx))),
                                    )
                                    .children(scope_menu.map(crate::components::ui::popover::below)),
                            )
                            .child(
                                sidebar_icon_button(
                                    "sidebar-add-project",
                                    IconName::FolderPlus,
                                    colors.r(28.),
                                    hovered(HeaderControl::AddProject),
                                    colors,
                                )
                                .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("Add project")))
                                .on_hover(self.control_hover_listener(HeaderControl::AddProject, cx))
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SidebarEvent::AddProject))),
                            )
                            .child(
                                sidebar_icon_button(
                                    "sidebar-new-thread",
                                    IconName::SquarePen,
                                    colors.r(28.),
                                    hovered(HeaderControl::NewThread),
                                    colors,
                                )
                                .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!("New thread")))
                                .on_hover(self.control_hover_listener(HeaderControl::NewThread, cx))
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(SidebarEvent::NewThread(None))
                                })),
                            ),
                    ),
            )
    }
}
