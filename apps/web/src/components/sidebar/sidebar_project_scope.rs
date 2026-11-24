//! The "Filter threads by project" popup (`Sidebar.tsx:3009-3230`): a search
//! field over "All projects" and every open project group. A click scopes the
//! list to one group, Shift+click (or Shift+Enter) adds or removes one, and a
//! right-click opens the group's settings. The trigger shows the scoped
//! groups' icons.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::context_menu::MenuCommand;
use super::sidebar_project_grouping::ProjectGroup;
use super::style::*;
use super::{Sidebar, SidebarEvent};
use crate::ActiveAppTheme as _;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::project_favicon::ProjectFavicon;

/// The open popup: its search field and highlighted row.
pub(crate) struct ProjectScopeMenu {
    query: Entity<InputState>,
    /// Index into [`Sidebar::scope_items`].
    highlighted: usize,
    _subscription: Subscription,
}

/// One popup row: "All projects" (`None`) or a group, by index into `list.groups`.
type ScopeItem = Option<usize>;

impl Sidebar {
    /// Opens or closes the popup (the trigger's click).
    pub(crate) fn toggle_scope_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scope_menu.take().is_some() {
            cx.notify();
            return;
        }
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::t!("Search projects...")));
        let subscription = cx.subscribe_in(&query, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(menu) = this.scope_menu.as_mut()
            {
                // `autoHighlight`: the first match is highlighted.
                menu.highlighted = 0;
                cx.notify();
            }
        });
        query.update(cx, |state, cx| state.focus(window, cx));
        self.hover_card = None;
        self.scope_menu = Some(ProjectScopeMenu { query, highlighted: 0, _subscription: subscription });
        cx.notify();
    }

    fn close_scope_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scope_menu.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        }
    }

    /// "All projects" then the open groups (pinned first) whose name holds the query.
    fn scope_items(&self, cx: &App) -> Vec<ScopeItem> {
        let query = self.scope_menu.as_ref().map(|m| m.query.read(cx).value().trim().to_lowercase()).unwrap_or_default();
        let all = crate::i18n::t!("All projects").to_lowercase();
        let groups = &self.list.groups;
        let order = groups.iter().enumerate().filter(|(_, g)| g.pinned).chain(groups.iter().enumerate().filter(|(_, g)| !g.pinned));
        std::iter::once(None)
            .filter(|_| query.is_empty() || all.contains(&query))
            .chain(order.filter(|(_, g)| query.is_empty() || g.label.to_lowercase().contains(&query)).map(|(ix, _)| Some(ix)))
            .collect()
    }

    fn scope_selected(&self, item: ScopeItem) -> bool {
        match item {
            None => self.options.project_scope.is_empty(),
            Some(ix) => self.list.groups.get(ix).is_some_and(|g| self.options.project_scope.contains(&g.key)),
        }
    }

    /// Picks a row: plain scopes to it and closes; `toggle` adds or removes it and stays open.
    fn pick_scope(&mut self, item: ScopeItem, toggle: bool, window: &mut Window, cx: &mut Context<Self>) {
        let key = item.and_then(|ix| self.list.groups.get(ix)).map(|g| g.key.clone());
        match (key, toggle) {
            (None, _) => {
                self.set_project_scope(Vec::new(), cx);
                self.close_scope_menu(window, cx);
            }
            (Some(key), true) => {
                let mut scope = self.options.project_scope.clone();
                match scope.iter().position(|k| *k == key) {
                    Some(ix) => {
                        scope.remove(ix);
                    }
                    None => scope.push(key),
                }
                self.set_project_scope(scope, cx);
            }
            (Some(key), false) => {
                self.set_project_scope(vec![key], cx);
                self.close_scope_menu(window, cx);
            }
        }
    }

    fn open_group_settings(&mut self, group: &ProjectGroup, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = group.member_refs.first().cloned() {
            cx.emit(SidebarEvent::ProjectCommand(project, MenuCommand::ProjectSettings));
        }
        self.close_scope_menu(window, cx);
    }

    fn scope_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.scope_items(cx);
        let Some(menu) = self.scope_menu.as_mut() else { return };
        let len = items.len();
        match event.keystroke.key.as_str() {
            "down" if len > 0 => menu.highlighted = (menu.highlighted + 1) % len,
            "up" if len > 0 => menu.highlighted = (menu.highlighted + len - 1) % len,
            "escape" => {
                cx.stop_propagation();
                self.close_scope_menu(window, cx);
                return;
            }
            "enter" => {
                cx.stop_propagation();
                if let Some(item) = items.get(menu.highlighted).copied() {
                    let shift = event.keystroke.modifiers.shift;
                    if item.is_some() || !shift {
                        self.pick_scope(item, shift, window, cx);
                    }
                }
                return;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The trigger's icon: the ListFilter glyph, the scoped group's favicon,
    /// or the first two scoped groups' favicons overlapped.
    pub(crate) fn scope_trigger_icon(&self, hovered: bool, colors: &ShellColors) -> AnyElement {
        let scoped: Vec<&ProjectGroup> = self
            .options
            .project_scope
            .iter()
            .filter_map(|key| self.list.groups.iter().find(|g| &g.key == key))
            .collect();
        let favicon = |group: &ProjectGroup, size: f32| {
            let title = group.member_refs.first().and_then(|r| self.list.project_titles.get(r)).cloned().unwrap_or_else(|| group.label.clone());
            ProjectFavicon::new(title).size(colors.r(size))
        };
        match scoped[..] {
            [] => Icon::new(IconName::ListFilter)
                .size(IconSize::Custom(colors.r(16.)))
                .color(if hovered { colors.sidebar_foreground } else { colors.sidebar_icon })
                .into_any_element(),
            [one] => favicon(one, 16.).into_any_element(),
            [first, second, ..] => div()
                .flex()
                .flex_none()
                .child(favicon(first, 14.))
                // `-ml-1 rounded-[25%] bg-sidebar ring-2 ring-sidebar`.
                .child(
                    div()
                        .ml(colors.r(-4.))
                        .rounded(colors.r(3.5))
                        .border_2()
                        .border_color(colors.sidebar)
                        .bg(colors.sidebar)
                        .m(colors.r(-2.))
                        .child(favicon(second, 14.)),
                )
                .into_any_element(),
        }
    }

    /// The trigger's accessible label and tooltip.
    pub(crate) fn scope_trigger_label(&self) -> SharedString {
        let names: Vec<&str> = self
            .options
            .project_scope
            .iter()
            .filter_map(|key| self.list.groups.iter().find(|g| &g.key == key))
            .map(|g| g.label.as_ref())
            .collect();
        if names.is_empty() {
            crate::i18n::t!("Filter threads by project")
        } else {
            crate::i18n::t!("Filter threads by project: {projects}", projects = names.join(", "))
        }
    }

    /// The popup (`ComboboxPopup align="start"`, `w-[min(18rem,var(--available-width))]`).
    pub(crate) fn render_scope_menu(&mut self, colors: &ShellColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.scope_menu.as_ref()?;
        let theme = cx.app_theme();
        let query = menu.query.clone();
        let highlighted = menu.highlighted;
        let items = self.scope_items(cx);
        let search = div()
            .flex()
            .items_center()
            // The input's 18.75px left padding clears the 15px icon it sits over.
            .gap(colors.r(4.))
            .mx(colors.r(12.))
            .mt(colors.r(8.8))
            .h(colors.r(34.13))
            .pb(colors.r(6.))
            .border_b_1()
            .border_color(theme.colors.focus)
            .child(Icon::new(IconName::Search).size(IconSize::Custom(colors.r(16.))).color(colors.muted_foreground.opacity(0.55)))
            .child(Input::new(&query).appearance(false).bordered(false).focus_bordered(false).p_0().h(colors.r(26.)).text_sm());
        let mut list = div().flex().flex_col().p(colors.r(4.));
        if items.is_empty() {
            list = list.child(
                div()
                    .py(colors.r(24.))
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(crate::i18n::t!("No matching projects.")),
            );
        }
        for (row, item) in items.iter().copied().enumerate() {
            let is_highlighted = row == highlighted;
            let selected = self.scope_selected(item);
            let group = item.and_then(|ix| self.list.groups.get(ix)).cloned();
            let icon = match &group {
                Some(group) => {
                    let title =
                        group.member_refs.first().and_then(|r| self.list.project_titles.get(r)).cloned().unwrap_or_else(|| group.label.clone());
                    ProjectFavicon::new(title).size(colors.r(16.)).into_any_element()
                }
                None => Icon::new(IconName::Folder).size(IconSize::Custom(colors.r(16.))).color(colors.muted_foreground).into_any_element(),
            };
            let icon = div().relative().flex().flex_none().child(icon).when(selected, |d| {
                // `-right-1 -bottom-1 size-3 rounded-full bg-primary ring-2 ring-popover`.
                d.child(
                    div()
                        .absolute()
                        .right(colors.r(-6.))
                        .bottom(colors.r(-6.))
                        .size(colors.r(16.))
                        .rounded_full()
                        .border_2()
                        .border_color(colors.glass)
                        .bg(theme.colors.message_action)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Check).size(IconSize::Custom(colors.r(8.))).stroke(4.).color(theme.colors.message_action_foreground)),
                )
            });
            let label = group.as_ref().map_or_else(|| crate::i18n::t!("All projects"), |g| g.label.clone());
            let action_button = |id: (&'static str, usize), glyph: IconName, colors: &ShellColors| {
                div()
                    .id(id)
                    .occlude()
                    .size(colors.r(24.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(colors.r(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.accent))
                    .child(Icon::new(glyph).size(IconSize::Custom(colors.r(14.))).color(colors.muted_foreground))
            };
            let mut row_el = div()
                .id(("scope-item", row))
                .flex()
                .items_center()
                .gap(colors.r(8.))
                .min_h(colors.r(32.))
                .px(colors.r(8.))
                .rounded(colors.r(4.))
                .cursor_pointer()
                .text_sm()
                .text_color(if is_highlighted { colors.accent_foreground } else { colors.foreground })
                .when(is_highlighted, |d| d.bg(colors.accent))
                .child(icon)
                .child(div().min_w_0().truncate().child(label))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && let Some(menu) = this.scope_menu.as_mut() {
                        menu.highlighted = row;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.pick_scope(item, event.modifiers().shift, window, cx);
                }));
            if let Some(group) = group {
                let settings_group = group.clone();
                let context_group = group.clone();
                row_el = row_el
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| this.open_group_settings(&context_group, window, cx)),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .flex_none()
                            .items_center()
                            .when(is_highlighted, |d| {
                                d.child(
                                    action_button(("scope-settings", row), IconName::Settings, colors)
                                        .tooltip(crate::components::ui::tooltip::tooltip(crate::i18n::t!(
                                            "Project settings for {project}",
                                            project = group.label.clone()
                                        )))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.open_group_settings(&settings_group, window, cx);
                                        })),
                                )
                            })
                            .child(match (is_highlighted || group.pinned, group.member_refs.first().cloned()) {
                                (true, Some(project)) => {
                                    let pinned = group.pinned;
                                    action_button(("scope-pin", row), if pinned { IconName::PinOff } else { IconName::Pin }, colors)
                                        .tooltip(crate::components::ui::tooltip::tooltip(if pinned {
                                            crate::i18n::t!("Unpin {name}", name = group.label.clone())
                                        } else {
                                            crate::i18n::t!("Pin {name}", name = group.label.clone())
                                        }))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            let command = if pinned { MenuCommand::UnpinProject } else { MenuCommand::PinProject };
                                            cx.emit(SidebarEvent::ProjectCommand(project.clone(), command));
                                        }))
                                        .into_any_element()
                                }
                                _ => div().size(colors.r(24.)).into_any_element(),
                            }),
                    );
            }
            list = list.child(row_el);
        }
        Some(
            crate::components::ui::popover::glass_surface(theme)
                .id("sidebar-project-scope")
                .occlude()
                .w(colors.r(288.))
                .overflow_hidden()
                .capture_key_down(cx.listener(Self::scope_key_down))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_scope_menu(window, cx)))
                .child(search)
                .child(list)
                .into_any_element(),
        )
    }
}
