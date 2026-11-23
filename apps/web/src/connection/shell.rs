//! The window root once the runtime runs: the app shell, the first-run
//! empty state in its main pane while no environment is saved, and the
//! settings modal. Ports the `hosted-static` branch of `_chat.index.tsx` and
//! the settings route mounting of `_chat.tsx`.

use gpui_kit::{
    AnyView, AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window, div, prelude::*,
};

use super::{ConnectionRuntime, ConnectionsState};
use crate::AppRoot;
use crate::components::hosted_empty_state::{HostedEmptyState, HostedEmptyStateEvent};
use crate::components::settings::settings_search::SettingsPath;
use crate::components::sidebar::SidebarEvent;
use crate::components::ui::dialog::Modal;
use crate::routes::settings::{SettingsRoute, SettingsRouteEvent};

/// Root view of the main window. Requires [`ConnectionRuntime::init`].
pub struct EnvironmentShell {
    app: Entity<AppRoot>,
    state: Entity<ConnectionsState>,
    empty: Entity<HostedEmptyState>,
    showing_empty: bool,
    settings: Option<Entity<SettingsRoute>>,
    _subscriptions: Vec<Subscription>,
}

impl EnvironmentShell {
    pub fn new(app: Entity<AppRoot>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = ConnectionRuntime::state(cx);
        let layout = app.read(cx).layout().clone();
        let (height, left) = {
            let layout = layout.read(cx);
            (layout.platform().topbar_height(), layout.header_content_left())
        };
        let empty = cx.new(|cx| HostedEmptyState::new(state.clone(), height, left, cx));
        let sidebar = layout.read(cx).sidebar().clone();
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.sync_main(cx)),
            cx.subscribe_in(&sidebar, window, |this, _, event: &SidebarEvent, window, cx| {
                if let SidebarEvent::OpenSettings = event {
                    this.open_settings(SettingsPath::General, window, cx);
                }
            }),
            cx.subscribe_in(&empty, window, |this, _, event: &HostedEmptyStateEvent, window, cx| match event {
                HostedEmptyStateEvent::OpenEnvironments => this.open_settings(SettingsPath::Connections, window, cx),
                HostedEmptyStateEvent::SignIn => {
                    this.open_settings(SettingsPath::Connections, window, cx);
                    if let Some(settings) = &this.settings {
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
        let mut this = Self { app, state, empty, showing_empty: false, settings: None, _subscriptions: subscriptions };
        this.sync_main(cx);
        this
    }

    pub fn app(&self) -> &Entity<AppRoot> {
        &self.app
    }

    /// The open settings modal, if any.
    pub fn settings(&self) -> Option<&Entity<SettingsRoute>> {
        self.settings.as_ref()
    }

    /// Opens settings on `path`, or moves the open modal there.
    pub fn open_settings(&mut self, path: SettingsPath, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(settings) = &self.settings {
            settings.update(cx, |settings, cx| settings.navigate(path, cx));
            return;
        }
        let settings = cx.new(|cx| SettingsRoute::new(path, window, cx));
        let subscription = cx.subscribe(&settings, |this, _, event: &SettingsRouteEvent, cx| match event {
            SettingsRouteEvent::Close => this.close_settings(cx),
        });
        self._subscriptions.push(subscription);
        self.settings = Some(settings);
        cx.notify();
    }

    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings = None;
        cx.notify();
    }

    /// Shows the empty state while nothing is saved, and hands the main pane
    /// back to the app otherwise.
    fn sync_main(&mut self, cx: &mut Context<Self>) {
        let empty = self.state.read(cx).environments.is_empty();
        if empty == self.showing_empty {
            return;
        }
        self.showing_empty = empty;
        let layout = self.app.read(cx).layout().clone();
        let view: AnyView = self.empty.clone().into();
        layout.update(cx, |layout, cx| if empty { layout.set_main(view, cx) } else { layout.clear_main(cx) });
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
            .children(self.settings.clone().map(|settings| Modal::dialog("settings-route", settings, close)))
    }
}
