//! The app inside the window root once the runtime runs: the app shell, the
//! first-run empty state in its main pane while no environment is saved and
//! no fixture data is loaded, and the settings modal. Ports the
//! `hosted-static` branch of `_chat.index.tsx` and the settings route
//! mounting of `_chat.tsx`. Settings is the router's `/settings/<panel>`
//! route: opening it navigates, and the modal follows the current route.

use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*,
};

use super::{ConnectionRuntime, ConnectionsState};
use crate::AppRoot;
use crate::components::hosted_empty_state::{HostedEmptyState, HostedEmptyStateEvent};
use crate::components::settings::settings_search::SettingsPath;
use crate::components::ui::dialog::Modal;
use crate::router::{Route, RouteChanged, Router};
use crate::routes::settings::{SettingsRoute, SettingsRouteEvent};

/// The app plus the environment screens. Requires [`ConnectionRuntime::init`].
pub struct EnvironmentShell {
    app: Entity<AppRoot>,
    router: Entity<Router>,
    state: Entity<ConnectionsState>,
    empty: Entity<HostedEmptyState>,
    showing_empty: bool,
    settings: Option<(Entity<SettingsRoute>, [Subscription; 2])>,
    _subscriptions: Vec<Subscription>,
}

impl EnvironmentShell {
    pub fn new(app: Entity<AppRoot>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = ConnectionRuntime::state(cx);
        let router = app.read(cx).router().clone();
        let layout = app.read(cx).layout().clone();
        let (height, left) = {
            let layout = layout.read(cx);
            (layout.platform().topbar_height(), layout.header_content_left())
        };
        let empty = cx.new(|cx| HostedEmptyState::new(state.clone(), height, left, cx));
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.sync_main(cx)),
            cx.subscribe_in(&router, window, |this, _, _: &RouteChanged, window, cx| this.sync_settings(window, cx)),
            cx.subscribe_in(&empty, window, |this, _, event: &HostedEmptyStateEvent, window, cx| match event {
                HostedEmptyStateEvent::OpenEnvironments => this.open_settings(SettingsPath::Connections, window, cx),
                HostedEmptyStateEvent::SignIn => {
                    this.open_settings(SettingsPath::Connections, window, cx);
                    if let Some(settings) = this.settings() {
                        let page = settings.read(cx).environments().clone();
                        page.update(cx, |page, cx| page.open_sign_in(window, cx));
                    }
                }
            }),
            // Coming to the foreground retries now instead of waiting out a backoff.
            cx.observe_window_activation(window, |_, window, cx| {
                if window.is_window_active() {
                    ConnectionRuntime::wake(cx);
                }
            }),
        ];
        let mut this =
            Self { app, router, state, empty, showing_empty: false, settings: None, _subscriptions: subscriptions };
        this.sync_main(cx);
        this.sync_settings(window, cx);
        this
    }

    pub fn app(&self) -> &Entity<AppRoot> {
        &self.app
    }

    /// The open settings modal, if any.
    pub fn settings(&self) -> Option<&Entity<SettingsRoute>> {
        self.settings.as_ref().map(|(settings, _)| settings)
    }

    /// Opens settings on `path` (navigates to its route), or moves the open
    /// modal there.
    pub fn open_settings(&mut self, path: SettingsPath, window: &mut Window, cx: &mut Context<Self>) {
        self.router.update(cx, |router, cx| router.navigate(Route::settings(path), cx));
        self.sync_settings(window, cx);
    }

    /// Leaves the settings route, back to the page under it.
    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.router.update(cx, |router, cx| router.close_modal(cx));
    }

    /// Opens, moves or closes the modal to match the current route.
    fn sync_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Route::Settings { panel, .. } = self.router.read(cx).current().clone() else {
            if self.settings.take().is_some() {
                cx.notify();
            }
            return;
        };
        let path = panel.settings_path();
        if let Some(settings) = self.settings() {
            if settings.read(cx).path() != path {
                settings.update(cx, |settings, cx| settings.navigate(path, cx));
            }
            return;
        }
        let settings = cx.new(|cx| SettingsRoute::new(path, window, cx));
        // The server pages read the environment the window shows.
        if let Some(pages) = self.app.update(cx, |app, cx| app.server_settings_pages(cx)) {
            settings.update(cx, |settings, cx| settings.set_server_pages(pages, cx));
        }
        let subscriptions = [
            cx.subscribe(&settings, |this, _, event: &SettingsRouteEvent, cx| match event {
                SettingsRouteEvent::Close => this.close_settings(cx),
            }),
            // The modal's own navigation (nav list, search) moves the route with it.
            cx.observe(&settings, |this, settings, cx| {
                let path = settings.read(cx).path();
                let current = this.router.read(cx).current().clone();
                if let Route::Settings { panel, .. } = current
                    && panel.settings_path() != path
                {
                    this.router.update(cx, |router, cx| router.replace(Route::settings(path), cx));
                }
            }),
        ];
        self.settings = Some((settings, subscriptions));
        cx.notify();
    }

    /// Shows the empty state while nothing is saved and the app has no
    /// fixture data, and hands the main pane back to the app otherwise.
    fn sync_main(&mut self, cx: &mut Context<Self>) {
        let empty = self.state.read(cx).environments.is_empty() && !self.app.read(cx).has_fixture_data(cx);
        if empty == self.showing_empty {
            return;
        }
        self.showing_empty = empty;
        let layout = self.app.read(cx).layout().clone();
        let view: AnyView = if empty { self.empty.clone().into() } else { self.app.read(cx).main_view() };
        layout.update(cx, |layout, cx| layout.set_main(view, cx));
    }
}

impl Render for EnvironmentShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close = {
            let this = cx.entity().downgrade();
            move |_: &mut Window, cx: &mut gpui_kit::App| {
                this.update(cx, |this, cx| this.close_settings(cx)).ok();
            }
        };
        div()
            .size_full()
            .child(self.app.clone())
            .children(self.settings().cloned().map(|settings| Modal::dialog("settings-route", settings, close)))
    }
}
