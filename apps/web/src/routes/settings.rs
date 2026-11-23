//! The settings route. Ports `routes/_chat.settings.tsx`, `AppRouteModal`
//! and `SettingsSidebarNav.tsx`: a large modal with the nav list and search
//! on the left, a breadcrumb header and the page on the right. Escape, the
//! close button or a press on the backdrop closes it (the caller wraps it in
//! a [`Modal`](crate::components::ui::dialog::Modal)).

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    TestSupportExt as _,
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, ScrollHandle, SharedString,
    Subscription, Window, div, prelude::*, px,
};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::settings::connections_settings::ConnectionsSettings;
use crate::components::settings::settings_layout::page_container;
use crate::components::settings::settings_pages::{SettingsPages, is_device_page};
use crate::components::settings::settings_search::{SearchItem, SettingsPath, nav_groups, search_settings};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::kbd::Kbd;
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// The settings modal asks its owner to close it.
#[derive(Clone, Debug)]
pub enum SettingsRouteEvent {
    Close,
}

/// The open settings modal: which page, the search, and the built pages.
pub struct SettingsRoute {
    path: SettingsPath,
    search: Entity<InputState>,
    query: SharedString,
    active_result: usize,
    environments: Entity<ConnectionsSettings>,
    pages: Entity<SettingsPages>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsRouteEvent> for SettingsRoute {}

impl Focusable for SettingsRoute {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsRoute {
    /// Opens on `path`. `/settings` itself opens General on the web; the
    /// caller picks the page.
    pub fn new(path: SettingsPath, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = crate::connection::ConnectionRuntime::state(cx);
        Self::with_state(path, state, window, cx)
    }

    /// Settings over given connection state (scenes use fixtures).
    pub fn with_state(
        path: SettingsPath,
        state: Entity<crate::connection::ConnectionsState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search")));
        let subscription = cx.subscribe_in(&search, window, |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                this.query = state.read(cx).value();
                this.active_result = 0;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                if let Some(item) = search_settings(&this.query).get(this.active_result).cloned() {
                    this.open_result(item, window, cx);
                }
            }
            _ => {}
        });
        let environments = cx.new(|cx| ConnectionsSettings::with_state(state, window, cx));
        let pages = cx.new(|cx| SettingsPages::new(path, window, cx));
        Self {
            path,
            search,
            query: SharedString::default(),
            active_result: 0,
            environments,
            pages,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            _subscriptions: vec![subscription],
        }
    }

    pub fn path(&self) -> SettingsPath {
        self.path
    }

    /// The Environments page, for callers that open one of its dialogs.
    pub fn environments(&self) -> &Entity<ConnectionsSettings> {
        &self.environments
    }

    /// The device preference pages, for callers and scripted checks.
    pub fn pages(&self) -> &Entity<SettingsPages> {
        &self.pages
    }

    pub fn navigate(&mut self, path: SettingsPath, cx: &mut Context<Self>) {
        self.pages.update(cx, |pages, cx| pages.set_path(path, cx));
        if self.path != path {
            self.path = path;
            self.scroll.set_offset(gpui_kit::point(px(0.), px(0.)));
        }
        cx.notify();
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |state, cx| state.set_value("", window, cx));
        self.query = SharedString::default();
        self.active_result = 0;
        cx.notify();
    }

    fn open_result(&mut self, item: SearchItem, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_search(window, cx);
        self.navigate(item.path, cx);
    }

    fn render_nav(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let searching = !self.query.trim().is_empty();
        let results = search_settings(&self.query);
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let muted = theme.derived.sidebar_menu_button_text;
        let search_row = div()
            .id("settings-search")
            .flex()
            .items_center()
            .gap(sp(2.))
            .h(sp(8.))
            .px(sp(2.))
            .rounded(theme.radii.md)
            .text_color(c.sidebar_muted_foreground)
            .hover(|s| s.bg(c.sidebar_row_hover))
            .when(search_focused, |s| s.bg(c.sidebar_row_hover))
            .child(Icon::new(IconName::Search).size(IconSize::Md).color(with_alpha(c.sidebar_muted_foreground, 0.8)))
            .child(
                div().flex_1().min_w_0().child(
                    Styled::h(Input::new(&self.search).appearance(false), sp(5.))
                        .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                        .w_full(),
                ),
            )
            .child(if searching {
                Button::new("settings-search-clear")
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::IconMicro)
                    .icon(IconName::X)
                    .on_click(cx.listener(|this, _, window, cx| this.clear_search(window, cx)))
                    .into_any_element()
            } else {
                Kbd::new(if cfg!(target_os = "macos") { "⌘F" } else { "Ctrl F" }).into_any_element()
            });
        let row = |id: SharedString, icon: IconName, label: SharedString, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(sp(2.))
                .h(sp(8.))
                .px(px(10.))
                .rounded(theme.radii.control)
                .cursor_pointer()
                .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                .text_color(if active { c.sidebar_foreground } else { muted })
                .when(active, |s| s.bg(c.sidebar_row_selected))
                .when(!active, |s| s.hover(|s| s.bg(c.sidebar_row_hover).text_color(c.sidebar_foreground)))
                .child(
                    Icon::new(icon)
                        .size(IconSize::Md)
                        .color(if active { c.sidebar_foreground } else { theme.derived.sidebar_icon }),
                )
                .child(div().flex_1().min_w_0().truncate().child(label))
        };
        let body = if searching {
            if results.is_empty() {
                div()
                    .px(sp(2.))
                    .py(sp(6.))
                    .flex()
                    .justify_center()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.sidebar_muted_foreground)
                    .child(t!("No settings found"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .children(results.into_iter().enumerate().map(|(ix, item)| {
                        let active = ix == self.active_result;
                        let section = item.path.section_label();
                        let picked = item.clone();
                        div()
                            .id(SharedString::from(format!("settings-result-{}-{}", item.path.slug(), item.id)))
                            .flex()
                            .items_start()
                            .gap(sp(2.))
                            .min_h(sp(10.))
                            .px(px(10.))
                            .py(sp(1.5))
                            .rounded(theme.radii.control)
                            .cursor_pointer()
                            .when(active, |s| s.bg(c.sidebar_row_selected))
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.active_result != ix {
                                    this.active_result = ix;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_result(picked.clone(), window, cx)))
                            .child(Icon::new(item.path.icon()).size(IconSize::Md).color(theme.derived.sidebar_icon))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                                            .text_color(c.sidebar_foreground)
                                            .child(item.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                            .text_color(with_alpha(c.sidebar_muted_foreground, 0.75))
                                            .child(section),
                                    ),
                            )
                    }))
                    .into_any_element()
            }
        } else {
            div()
                .flex()
                .flex_col()
                .gap(sp(4.))
                .children(nav_groups().into_iter().enumerate().map(|(_, group)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(1.))
                        .children(group.heading.map(|heading| {
                            div()
                                .px(sp(2.))
                                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                                .text_color(with_alpha(c.sidebar_muted_foreground, 0.8))
                                .child(heading)
                        }))
                        .children(group.paths.iter().map(|path| {
                            let path = *path;
                            row(
                                SharedString::from(format!("settings-nav-{}", path.slug())),
                                path.icon(),
                                path.nav_label(),
                                path == self.path,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.navigate(path, cx)))
                            .test_support()
                        }))
                        .into_any_element()
                }))
                .into_any_element()
        };
        div()
            .id("settings-nav")
            .flex()
            .flex_col()
            .flex_none()
            .w(theme.rem(15.))
            .h_full()
            .border_r_1()
            .border_color(c.sidebar_border)
            .bg(c.sidebar)
            .text_color(c.sidebar_foreground)
            .child(
                div()
                    .id("settings-nav-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(sp(2.))
                    .child(div().flex().flex_col().gap(sp(2.)).child(search_row).child(body)),
            )
            .children(crate::components::clerk::t3_connect_sidebar_sign_in(self.environments.clone(), cx))
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(sp(3.))
            .h(sp(12.))
            .pl(sp(5.))
            .pr(sp(2.))
            .border_b_1()
            .border_color(c.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .min_w_0()
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(div().text_color(c.muted_foreground).child(t!("Settings")))
                    .child(Icon::new(IconName::ChevronRight).size(IconSize::Sm).color(c.muted_foreground))
                    .child(div().truncate().font_weight(weight::MEDIUM).text_color(c.foreground()).child(self.path.section_label())),
            )
            .child(
                div().ml_auto().flex().items_center().gap(sp(1.)).children(SettingsPages::restore_button(&self.pages, cx)).child(
                    Button::new("settings-close")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::X)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsRouteEvent::Close))),
                ),
            );
        let body = if is_device_page(self.path) {
            self.pages.clone().into_any_element()
        } else if self.path.is_built() {
            self.environments.clone().into_any_element()
        } else {
            page_container(cx)
                .child(
                    div()
                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                        .text_color(c.muted_foreground)
                        .child(format!("{} is not built in this app yet.", self.path.section_label())),
                )
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(header)
            .child(
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}

impl Render for SettingsRoute {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = self.render_nav(window, cx).into_any_element();
        let content = self.render_content(cx).into_any_element();
        let theme = cx.app_theme();
        let viewport = window.viewport_size();
        // `sm:p-6 lg:p-10`, `sm:max-w-[80rem] sm:max-h-[60rem]`; the Modal adds 16px.
        let inset = if viewport.width >= px(1024.) { px(40.) } else { px(24.) } - px(16.);
        let width = (viewport.width - inset * 2. - px(32.)).min(theme.rem(80.));
        let height = (viewport.height - inset * 2. - px(32.)).min(theme.rem(60.));
        let search = self.search.clone();
        div()
            .id("settings-modal")
            .track_focus(&self.focus)
            .key_context("SettingsRoute")
            .on_key_down(cx.listener(move |this, event: &gpui_kit::KeyDownEvent, window, cx| {
                let keys = &event.keystroke;
                let find = keys.key == "f" && (keys.modifiers.platform || keys.modifiers.control) && !keys.modifiers.alt;
                let searching = !this.query.trim().is_empty();
                if find {
                    search.update(cx, |state, cx| state.focus(window, cx));
                    cx.stop_propagation();
                } else if keys.key == "escape" && searching {
                    this.clear_search(window, cx);
                    cx.stop_propagation();
                } else if searching && (keys.key == "down" || keys.key == "up") {
                    let count = search_settings(&this.query).len().max(1);
                    this.active_result = if keys.key == "down" {
                        (this.active_result + 1) % count
                    } else {
                        (this.active_result + count - 1) % count
                    };
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .flex()
            .w(width)
            .h(height)
            .overflow_hidden()
            .rounded(theme.radii.xxl)
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.background())
            .shadow(theme.shadows.dialog.clone())
            .text_color(theme.colors.foreground())
            .child(nav)
            .child(content)
    }
}
