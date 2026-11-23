//! Root view of the main window. Ports `AppRoot.tsx`, `__root.tsx` and the
//! `_chat` layout route: the sidebar layout with route content in the main
//! pane, the right panel and the terminal drawer, all driven by one
//! [`Router`]. The sidebar, links, the titlebar toggles, shortcuts and the
//! application menu navigate through the router and edit the panel stores;
//! this view applies the result to the panes and persists it.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::*;

use crate::components::app_sidebar_layout::{AppSidebarLayout, LayoutEvent, PanelToggle, TitlebarPlatform};
use crate::components::chat::composer::ChatComposer;
use crate::components::no_active_thread_state::NoActiveThreadState;
use crate::components::persistent_thread_terminals::PersistentThreadTerminals;
use crate::components::right_panel_tabs::{RightPanel, RightPanelEvent};
use crate::components::sidebar::model::ThreadKey;
use crate::components::sidebar::{Sidebar, SidebarEvent};
use crate::components::thread_route_view::{ThreadRouteEvent, ThreadRouteView};
use crate::components::thread_terminal_drawer::{BackendFactory, TerminalLaunch};
use crate::components::ui::toast::Toaster;
use crate::fixtures::{FixtureEvent, FixtureStore, scoped_key};
use crate::right_panel_store::RightPanelStore;
use crate::router::{Route, RouteChanged, Router, ThreadTarget};
use crate::routes::RoutePage;
use crate::terminal::local_pty::LocalPty;
use crate::terminal_ui_state_store::{self as terminal_store, TerminalUiStateStore};
use crate::ui_state_store::{StateDir, UiStateStore, files};

actions!(
    app,
    [
        /// `sidebar.toggle`
        ToggleSidebar,
        /// `terminal.toggle`
        ToggleTerminal,
        /// `rightPanel.toggle`
        ToggleRightPanel,
        /// `rightPanel.toggleMaximized`
        ToggleMaximizePanel,
        /// `rightPanel.new`: show the panel's surface launcher.
        NewPanelTab,
        /// `rightPanel.close`
        ClosePanelTab,
        /// `view.reopenClosed`
        ReopenClosedView,
        /// `navigation.back`
        NavigateBack,
        /// `navigation.forward`
        NavigateForward,
        /// `chat.new`
        NewThread,
        /// Settings… (mod+,)
        OpenSettings,
        /// Quit the app, writing state first.
        Quit,
        /// View → Toggle Full Screen.
        ToggleFullScreen,
        /// Window → Minimize.
        Minimize,
        /// Window → Zoom.
        ZoomWindow,
    ]
);

const CONTEXT: &str = "AppRoot";

/// The web keymap's `mod`: `cmd` on macOS, `ctrl` elsewhere.
macro_rules! mod_key {
    ($keys:literal) => {
        if cfg!(target_os = "macos") { concat!("cmd-", $keys) } else { concat!("ctrl-", $keys) }
    };
}

/// Binds the app shortcuts from the default keymap (`keybindings.ts`).
/// Terminal and composer contexts sit deeper, so their own bindings win.
pub fn init(cx: &mut App) {
    let not_terminal = "AppRoot && !Terminal";
    cx.bind_keys([
        KeyBinding::new(mod_key!("b"), ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new(mod_key!("j"), ToggleTerminal, Some(CONTEXT)),
        KeyBinding::new(mod_key!("alt-b"), ToggleRightPanel, Some(CONTEXT)),
        KeyBinding::new(mod_key!("["), NavigateBack, Some(not_terminal)),
        KeyBinding::new(mod_key!("]"), NavigateForward, Some(not_terminal)),
        KeyBinding::new(mod_key!("t"), NewPanelTab, Some(not_terminal)),
        KeyBinding::new(mod_key!("w"), ClosePanelTab, Some(not_terminal)),
        KeyBinding::new(mod_key!("shift-t"), ReopenClosedView, Some(CONTEXT)),
        KeyBinding::new(mod_key!("n"), NewThread, Some(not_terminal)),
        KeyBinding::new(mod_key!("shift-o"), NewThread, Some(CONTEXT)),
        KeyBinding::new(mod_key!(","), OpenSettings, None),
        KeyBinding::new(mod_key!("q"), Quit, None),
    ]);
}

/// How to build the window: which data, which titlebar, where state lives
/// and how terminals start.
pub struct AppRootOptions {
    pub store: FixtureStore,
    pub platform: TitlebarPlatform,
    pub state_dir: StateDir,
    pub terminal_backend: BackendFactory,
}

impl AppRootOptions {
    /// The showcase fixtures, nothing persisted, terminals on a local shell.
    pub fn fixtures(platform: TitlebarPlatform) -> Self {
        Self { store: FixtureStore::showcase(), platform, state_dir: StateDir(None), terminal_backend: local_shell_backend() }
    }
}

/// Terminals as a local login shell in the thread's directory (or home).
pub fn local_shell_backend() -> BackendFactory {
    Rc::new(|launch: &TerminalLaunch, _: &mut App| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cwd = Some(PathBuf::from(launch.cwd.as_ref())).filter(|p| p.is_dir()).or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        Box::new(LocalPty::new(&shell, &["-i"], cwd))
    })
}

/// The main pane: the chat for a thread route, a page, or the no-thread state.
struct MainPane {
    layout: WeakEntity<AppSidebarLayout>,
    thread_view: Entity<ThreadRouteView>,
    /// `None` shows the thread view (or the no-thread state when there is no thread).
    page: Option<Route>,
    has_thread: bool,
}

impl Render for MainPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (topbar, inset) = self
            .layout
            .read_with(cx, |layout, _| (layout.platform().topbar_height(), layout.header_content_left()))
            .unwrap_or((px(40.), px(0.)));
        if let Some(route) = &self.page {
            return RoutePage { route: route.clone(), topbar, leading_inset: inset }.into_any_element();
        }
        if !self.has_thread {
            return NoActiveThreadState { topbar, leading_inset: inset }.into_any_element();
        }
        let chat = self.thread_view.read(cx).chat().clone();
        chat.update(cx, |chat, cx| chat.set_header_metrics(topbar, inset, cx));
        self.thread_view.clone().into_any_element()
    }
}

/// Root view of the main window.
pub struct AppRoot {
    layout: Entity<AppSidebarLayout>,
    sidebar: Entity<Sidebar>,
    router: Entity<Router>,
    store: Entity<FixtureStore>,
    thread_view: Entity<ThreadRouteView>,
    main: Entity<MainPane>,
    right_panel: Entity<RightPanel>,
    right_panels: Entity<RightPanelStore>,
    terminals: Entity<PersistentThreadTerminals>,
    /// The thread under the main pane; it stays shown under modal routes.
    shown: Option<ThreadTarget>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl AppRoot {
    /// The window with the showcase fixtures and nothing persisted.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_options(AppRootOptions::fixtures(TitlebarPlatform::current()), window, cx)
    }

    /// The window drawing `snapshot` with the given titlebar layout (shell checks).
    pub fn with_snapshot(
        snapshot: crate::components::sidebar::model::SidebarSnapshot,
        platform: TitlebarPlatform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let options = AppRootOptions { store: FixtureStore::from_sidebar(snapshot), ..AppRootOptions::fixtures(platform) };
        Self::with_options(options, window, cx)
    }

    pub fn with_options(options: AppRootOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let AppRootOptions { store, platform, state_dir, terminal_backend } = options;
        let ui = UiStateStore::load(state_dir.clone());
        let layout_state = ui.layout.clone();
        let right_panels = cx.new(|_| {
            state_dir.read(files::RIGHT_PANEL).map(|json| RightPanelStore::from_json(&json)).unwrap_or_default()
        });
        if let Some(json) = state_dir.read(files::TERMINAL) {
            cx.set_global(TerminalUiStateStore::from_json(&json));
        }
        cx.set_global(ui);

        let store = cx.new(|_| store);
        let initial = Route::parse(&layout_state.last_route);
        let initial = match initial.thread_target() {
            Some(target) if !store.read(cx).thread_exists(&target) => Route::Index,
            _ => initial,
        };
        let router = cx.new(|_| Router::new(initial));

        let snapshot = store.read(cx).snapshot(None);
        let sidebar = cx.new(|cx| Sidebar::new(snapshot, window, cx));
        let layout = cx.new(|cx| {
            let mut layout = AppSidebarLayout::new(sidebar.clone(), platform, cx);
            layout.set_controlled(true);
            layout.restore(
                layout_state.sidebar_open,
                px(layout_state.sidebar_width),
                px(layout_state.right_panel_width),
                cx,
            );
            layout
        });
        let thread_view = cx.new(|cx| ThreadRouteView::new(store.clone(), cx));
        let main = cx.new(|_| MainPane { layout: layout.downgrade(), thread_view: thread_view.clone(), page: None, has_thread: false });
        let right_panel = cx.new(|cx| RightPanel::new(right_panels.clone(), layout.downgrade(), cx));
        let terminals = cx.new(|cx| PersistentThreadTerminals::new(terminal_backend, cx));
        layout.update(cx, |layout, cx| {
            layout.set_main(main.clone().into(), cx);
            layout.set_right_panel(right_panel.clone().into(), cx);
            layout.set_terminal(terminals.clone().into(), cx);
        });

        let subscriptions = vec![
            cx.subscribe_in(&router, window, |this, _, event: &RouteChanged, window, cx| {
                this.apply_route(event.route.clone(), window, cx)
            }),
            cx.subscribe_in(&sidebar, window, Self::on_sidebar_event),
            cx.subscribe_in(&layout, window, Self::on_layout_event),
            cx.subscribe(&store, Self::on_store_event),
            cx.subscribe(&thread_view, |this, _, event: &ThreadRouteEvent, cx| {
                let ThreadRouteEvent::Promoted(target) = event;
                if let ThreadTarget::Server(key) = target {
                    this.router.update(cx, |router, cx| router.replace(Route::Thread(key.clone()), cx));
                }
            }),
            cx.subscribe_in(&right_panel, window, |this, _, event: &RightPanelEvent, window, cx| match event {
                RightPanelEvent::Changed => this.panels_changed(window, cx),
                RightPanelEvent::OpenTerminal => this.set_terminal_open(true, window, cx),
            }),
            cx.observe_global_in::<TerminalUiStateStore>(window, |this, window, cx| this.panels_changed(window, cx)),
        ];

        let mut this = Self {
            layout,
            sidebar,
            router,
            store,
            thread_view,
            main,
            right_panel,
            right_panels,
            terminals,
            shown: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        let route = this.router.read(cx).current().clone();
        this.apply_route(route, window, cx);
        this
    }

    pub fn layout(&self) -> &Entity<AppSidebarLayout> {
        &self.layout
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub fn router(&self) -> &Entity<Router> {
        &self.router
    }

    pub fn store(&self) -> &Entity<FixtureStore> {
        &self.store
    }

    pub fn thread_view(&self) -> &Entity<ThreadRouteView> {
        &self.thread_view
    }

    pub fn right_panels(&self) -> &Entity<RightPanelStore> {
        &self.right_panels
    }

    pub fn terminals(&self) -> &Entity<PersistentThreadTerminals> {
        &self.terminals
    }

    pub fn composer(&self, cx: &App) -> Entity<ChatComposer> {
        self.thread_view.read(cx).composer().clone()
    }

    /// Starts live clocks (durations, relative times). Off in scenes.
    pub fn start_clock(&self, cx: &mut App) {
        self.sidebar.update(cx, |sidebar, cx| sidebar.start_clock(cx));
    }

    /// Navigates to `route` (pushes a history entry).
    pub fn navigate(&self, route: Route, cx: &mut App) {
        self.router.update(cx, |router, cx| router.navigate(route, cx));
    }

    /// The scoped key of the thread on screen, for per-thread panel state.
    fn shown_key(&self) -> Option<ThreadKey> {
        match &self.shown {
            Some(ThreadTarget::Server(key)) => Some(key.clone()),
            _ => None,
        }
    }

    /// The panel-state key: server threads by scoped key, drafts by draft id.
    fn panel_key(&self) -> Option<SharedString> {
        match &self.shown {
            Some(ThreadTarget::Server(key)) => Some(scoped_key(key)),
            Some(ThreadTarget::Draft(id)) => Some(format!("draft:{}", id.0).into()),
            None => None,
        }
    }

    fn terminal_key(&self) -> Option<terminal_store::ThreadKey> {
        self.panel_key().map(|key| terminal_store::ThreadKey(key.to_string()))
    }

    /// Shows what `route` names. Selecting a thread updates the chat view,
    /// the sidebar's active row and both panels in the same frame.
    fn apply_route(&mut self, route: Route, _window: &mut Window, cx: &mut Context<Self>) {
        if route == Route::Index {
            // `_chat.index.tsx`: start a draft in the most recent project.
            let draft = self.store.update(cx, |store, _| {
                let project = store.most_recent_project()?;
                Some(store.create_draft(Some(project)))
            });
            if let Some(draft) = draft {
                self.router.update(cx, |router, cx| router.replace(Route::Draft(draft), cx));
                return;
            }
        }
        if let Some(target) = route.thread_target() {
            let shown = self.thread_view.update(cx, |view, cx| view.show(target.clone(), cx));
            self.shown = shown.then_some(target);
        } else if !route.is_modal() {
            self.shown = None;
        }
        let page = (route.thread_target().is_none() && route != Route::Index).then(|| route.clone());
        let has_thread = self.shown.is_some();
        self.main.update(cx, |main, cx| {
            main.page = page;
            main.has_thread = has_thread;
            cx.notify();
        });
        let active = self.shown_key();
        self.sidebar.update(cx, |sidebar, cx| sidebar.set_active_thread(active.clone(), cx));
        if let Some(key) = &active {
            let visited = jiff::Timestamp::now().to_string();
            let key = scoped_key(key);
            UiStateStore::update_ui(cx, |ui| ui.mark_thread_visited(&key, visited));
        }
        let path = route.path();
        UiStateStore::update_layout(cx, |layout| layout.last_route = path);
        self.sync_panels(cx);
    }

    /// Pushes the shown thread's panel state into the layout, panel and drawer.
    fn sync_panels(&mut self, cx: &mut Context<Self>) {
        let key = self.panel_key();
        let (right_open, maximized) = key
            .as_ref()
            .map(|k| {
                let state = self.right_panels.read(cx).state(k);
                (state.is_open, state.maximized)
            })
            .unwrap_or((false, false));
        let terminal_open = self
            .terminal_key()
            .is_some_and(|k| cx.global::<TerminalUiStateStore>().state(&k).terminal_open);
        self.right_panel.update(cx, |panel, cx| panel.set_thread(key.clone(), cx));
        let terminal_thread = self.terminal_key().map(|k| {
            let cwd = self.thread_cwd(cx);
            (k, cwd)
        });
        self.terminals.update(cx, |terminals, cx| terminals.set_active_thread(terminal_thread, cx));
        self.layout.update(cx, |layout, cx| layout.set_panels(right_open, maximized, terminal_open, cx));
    }

    fn thread_cwd(&self, cx: &mut Context<Self>) -> SharedString {
        let Some(target) = self.shown.clone() else { return SharedString::default() };
        self.store.update(cx, |store, _| store.thread_view(&target).map(|v| v.cwd).unwrap_or_default())
    }

    /// A store edit happened: persist it and re-sync the panes.
    fn panels_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(json) = self.right_panels.read(cx).to_json() {
            UiStateStore::schedule_write(files::RIGHT_PANEL, json, cx);
        }
        if let Ok(json) = cx.global::<TerminalUiStateStore>().to_json() {
            UiStateStore::schedule_write(files::TERMINAL, json, cx);
        }
        self.sync_panels(cx);
    }

    fn edit_right_panel(&mut self, window: &mut Window, cx: &mut Context<Self>, edit: impl FnOnce(&mut RightPanelStore, &str)) {
        let Some(key) = self.panel_key() else { return };
        let was_open = self.right_panels.read(cx).is_open(&key);
        self.right_panels.update(cx, |store, cx| {
            edit(store, &key);
            cx.notify();
        });
        self.panels_changed(window, cx);
        if was_open && !self.right_panels.read(cx).is_open(&key) {
            self.restore_focus(window, cx);
        }
    }

    fn set_terminal_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.terminal_key() else { return };
        cx.update_global::<TerminalUiStateStore, _>(|store, _| store.set_terminal_open(&key, open));
        self.panels_changed(window, cx);
        if open {
            if let Some(drawer) = self.terminals.read(cx).drawer(&key).cloned() {
                let focus = drawer.focus_handle(cx);
                window.focus(&focus, cx);
            }
        } else {
            self.restore_focus(window, cx);
        }
    }

    fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.terminal_key() else { return };
        let open = cx.global::<TerminalUiStateStore>().state(&key).terminal_open;
        self.set_terminal_open(!open, window, cx);
    }

    /// Focus goes back to the composer when a panel closes (or to the root
    /// when no thread is shown), so typing continues where it was.
    fn restore_focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown.is_some() {
            let composer = self.composer(cx);
            window.focus(&composer.focus_handle(cx), cx);
        } else {
            window.focus(&self.focus, cx);
        }
    }

    fn on_sidebar_event(&mut self, _: &Entity<Sidebar>, event: &SidebarEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SidebarEvent::ActivateThread(key) => self.navigate(Route::Thread(key.clone()), cx),
            SidebarEvent::NewThread(project) => {
                let project = project.clone();
                let draft = self.store.update(cx, |store, _| {
                    let project = project.or_else(|| store.most_recent_project());
                    store.create_draft(project)
                });
                self.navigate(Route::Draft(draft), cx);
                self.restore_focus(window, cx);
            }
            SidebarEvent::OpenSettings => self.navigate(Route::parse("/settings/general"), cx),
            SidebarEvent::OpenPullRequests => self.navigate(Route::parse("/pull-requests"), cx),
            SidebarEvent::OpenIssues => self.navigate(Route::parse("/issues"), cx),
            SidebarEvent::OpenUsage => self.navigate(Route::Usage, cx),
            SidebarEvent::OpenNotification(_) => {}
            SidebarEvent::DiscardDraft(key) => {
                let route = Route::Thread(key.clone());
                self.router.update(cx, |router, _| router.retain_history(|r| *r != route));
            }
            SidebarEvent::AddProject
            | SidebarEvent::OpenProjectFilter
            | SidebarEvent::ThreadCommand(..)
            | SidebarEvent::ProjectCommand(..)
            | SidebarEvent::MarkNotificationRead(_)
            | SidebarEvent::MarkAllNotificationsRead => {}
        }
    }

    fn on_layout_event(&mut self, layout: &Entity<AppSidebarLayout>, event: &LayoutEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            LayoutEvent::Toggle(PanelToggle::Terminal) => self.toggle_terminal(window, cx),
            LayoutEvent::Toggle(PanelToggle::RightPanel) => {
                self.edit_right_panel(window, cx, |store, key| store.toggle_visibility(key))
            }
            LayoutEvent::Toggle(PanelToggle::Maximize) => self.edit_right_panel(window, cx, |store, key| {
                let maximized = store.state(key).maximized;
                store.set_maximized(key, !maximized);
            }),
            LayoutEvent::Toggle(PanelToggle::Details) => {
                layout.update(cx, |layout, cx| layout.toggle_details(cx));
            }
            LayoutEvent::Changed => {
                let (open, width, right) = {
                    let layout = layout.read(cx);
                    (layout.is_sidebar_open(), layout.sidebar_width(), layout.right_panel_width())
                };
                UiStateStore::update_layout(cx, |state| {
                    state.sidebar_open = open;
                    state.sidebar_width = f32::from(width);
                    state.right_panel_width = f32::from(right);
                });
            }
        }
    }

    fn on_store_event(&mut self, store: Entity<FixtureStore>, event: &FixtureEvent, cx: &mut Context<Self>) {
        if let FixtureEvent::SnapshotChanged = event {
            let active = self.shown_key();
            let snapshot = store.read(cx).snapshot(active.as_ref());
            self.sidebar.update(cx, |sidebar, cx| sidebar.set_snapshot(snapshot, cx));
        }
    }

    /// Writes every store now (quit).
    pub fn flush_state(&self, cx: &mut App) {
        let dir = cx.global::<UiStateStore>().dir.clone();
        if let Ok(json) = self.right_panels.read(cx).to_json() {
            dir.write(files::RIGHT_PANEL, &json);
        }
        if let Ok(json) = cx.global::<TerminalUiStateStore>().to_json() {
            dir.write(files::TERMINAL, &json);
        }
        UiStateStore::flush(cx);
    }
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("app-root")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.layout.update(cx, |l, cx| l.toggle_sidebar(cx))))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| this.toggle_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleRightPanel, window, cx| {
                this.edit_right_panel(window, cx, |store, key| store.toggle_visibility(key))
            }))
            .on_action(cx.listener(|this, _: &ToggleMaximizePanel, window, cx| {
                this.edit_right_panel(window, cx, |store, key| {
                    let maximized = store.state(key).maximized;
                    store.set_maximized(key, !maximized);
                })
            }))
            .on_action(cx.listener(|this, _: &NewPanelTab, window, cx| this.edit_right_panel(window, cx, |store, key| store.show(key))))
            .on_action(cx.listener(|this, _: &ClosePanelTab, window, cx| {
                this.edit_right_panel(window, cx, |store, key| store.close_active(key))
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedView, window, cx| {
                this.edit_right_panel(window, cx, |store, key| {
                    store.reopen_closed(key);
                })
            }))
            .on_action(cx.listener(|this, _: &NavigateBack, _, cx| this.router.update(cx, |r, cx| r.back(cx))))
            .on_action(cx.listener(|this, _: &NavigateForward, _, cx| this.router.update(cx, |r, cx| r.forward(cx))))
            .on_action(cx.listener(|this, _: &NewThread, window, cx| {
                this.on_sidebar_event(&this.sidebar.clone(), &SidebarEvent::NewThread(None), window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.navigate(Route::parse("/settings/general"), cx)))
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .child(base::TextSelectionLayer)
            .child(self.layout.clone())
            .child(Toaster::view(cx))
    }
}
