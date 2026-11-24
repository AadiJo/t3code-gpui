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
use crate::components::chat::thread_details_panel::ThreadDetailsMode;
use crate::components::no_active_thread_state::NoActiveThreadState;
use crate::components::persistent_thread_terminals::PersistentThreadTerminals;
use crate::components::chat::expanded_image_dialog::{ExpandedImageDialog, ExpandedImageEvent, ExpandedImageItem};
use crate::components::chat::timeline_entries::Attachment;
use crate::components::media::media_actions::MediaKind;
use crate::components::media::media_source::MediaBytes;
use crate::components::right_panel_tabs::{RightPanel, RightPanelEvent, ScriptBackendFactory};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::pull_request::link_pull_request_dialog::{LinkPullRequest, LinkPullRequestClosed, LinkPullRequestDialog};
use crate::components::ui::dialog::Modal;
use crate::state::StateBridge;
use crate::components::issue::issue_detail_panel::IssueDetailPanel;
use crate::components::issue::issues_page::{IssuesPage, IssuesPageEvent};
use crate::components::pull_request::pull_request_detail_panel::PullRequestDetailPanel;
use crate::components::pull_request::pull_requests_page::{PullRequestsPage, PullRequestsPageEvent};
use crate::components::usage::usage_page::UsagePageEvent;
use crate::routes::usage::UsageRoute;
use crate::file_source::absolute_path;
use crate::thread_find_store::ThreadFindStore;
use crate::i18n::t;
use crate::components::settings::settings_search::SettingsPath;
use crate::components::sidebar::model::{ProjectRef, ThreadKey};
use crate::components::sidebar::{Sidebar, SidebarEvent};
use crate::components::thread_route_view::{ThreadRouteEvent, ThreadRouteView};
use crate::components::thread_terminal_drawer::{BackendFactory, TerminalLaunch};
use crate::components::sidebar::context_menu::MenuCommand;
use crate::components::command_palette::CommandPalette;
use crate::components::command_palette_logic::PaletteEvent;
use crate::components::command_palette_lookups::CommandPaletteSource;
use crate::components::sidebar_logic::SidebarListEntry;
use crate::components::thread_notification_coordinator::{NotificationInput, ThreadNotificationCoordinator};
use crate::connection::shell::EnvironmentShell;
use crate::desktop_notifications::{InboxEvent, NotificationInbox};
use crate::router::SettingsPanel;
use crate::shortcuts::ShortcutRoot;
use crate::shortcuts::commands::{
    CloseRightPanelTab, CopyThreadReference, FindInThread, FocusNextWorkspaceArea, FocusPreviousWorkspaceArea,
    OpenFavoriteEditor, ToggleFilePicker, ToggleProjectSearch, ToggleThreadPanel, JumpToThread1, JumpToThread2, JumpToThread3, JumpToThread4, JumpToThread5, JumpToThread6,
    JumpToThread7, JumpToThread8, JumpToThread9, NavigateBack, NavigateForward, NewChat, NewChatWithoutProject, NewLocalChat,
    NewRightPanelTab, NextThread, OpenUsage, PreviousThread, ReopenClosedView, ToggleDiff, TogglePreview,
    ToggleRightPanel, ToggleRightPanelMaximized, ToggleSidebar, ToggleTerminal,
};
use crate::fixtures::app_data::FixtureData;
use crate::fixtures::{FixtureStore, scoped_key};
use crate::state::app_data::{CommandOutcome, DataEvent, DataEvents, SharedAppData};
use crate::right_panel_store::{RightPanelStore, SingletonKind};
use crate::router::{DraftId, Route, RouteChanged, Router, ThreadTarget};
use crate::routes::RoutePage;
#[cfg(unix)]
use crate::terminal::local_pty::LocalPty;
use crate::terminal_ui_state_store::{self as terminal_store, TerminalUiStateStore};
use crate::ui_state_store::{StateDir, UiStateStore, files};

actions!(
    app,
    [
        /// Settings… (mod+,). The web app opens settings from the sidebar
        /// and the palette; the desktop menu adds this.
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

/// The right panel's width for a thread with none saved (`PREVIEW_PANEL_DEFAULT_WIDTH`).
const RIGHT_PANEL_DEFAULT_WIDTH: f32 = 540.;

/// The web keymap's `mod`: `cmd` on macOS, `ctrl` elsewhere.
macro_rules! mod_key {
    ($keys:literal) => {
        if cfg!(target_os = "macos") { concat!("cmd-", $keys) } else { concat!("ctrl-", $keys) }
    };
}

/// Binds the desktop menu's shortcuts. App commands (sidebar, panels,
/// history, new thread, palette) bind through [`crate::shortcuts`] from the
/// keybinding rules, and this view handles their registry actions.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(mod_key!(","), OpenSettings, None), KeyBinding::new(mod_key!("q"), Quit, None)]);
}

/// Where the window's data comes from.
pub enum DataSource {
    /// The in-memory fixtures (checks, scenes, the parity tool).
    Fixtures(FixtureStore),
    /// The saved environments' stores and sessions. Needs the connection runtime.
    Live,
}

/// The details card's mode and each mode's open state.
#[derive(Clone, Copy, Debug)]
struct DetailsVisibility {
    mode: ThreadDetailsMode,
    inline_open: bool,
    popover_open: bool,
}

/// How to build the window: which data, which titlebar and where state lives.
pub struct AppRootOptions {
    pub data: DataSource,
    pub platform: TitlebarPlatform,
    pub state_dir: StateDir,
}

impl AppRootOptions {
    /// The showcase fixtures, nothing persisted, terminals on a local shell.
    pub fn fixtures(platform: TitlebarPlatform) -> Self {
        Self { data: DataSource::Fixtures(FixtureStore::showcase()), platform, state_dir: StateDir(None) }
    }

    /// No fixture data and no environment: the first-run empty state.
    pub fn without_fixtures(platform: TitlebarPlatform) -> Self {
        Self { data: DataSource::Fixtures(FixtureStore::empty()), ..Self::fixtures(platform) }
    }

    /// The saved environments' data.
    pub fn live(platform: TitlebarPlatform) -> Self {
        Self { data: DataSource::Live, ..Self::fixtures(platform) }
    }
}

/// The main window's views, outermost first: keyboard control, palette,
/// toaster and announcer; the environment screens and settings modal; the app.
pub struct WindowRoot {
    pub shortcuts: Entity<ShortcutRoot>,
    pub shell: Entity<EnvironmentShell>,
    pub app: Entity<AppRoot>,
}

impl WindowRoot {
    /// Builds the window's views. Requires [`ConnectionRuntime::init`](crate::connection::ConnectionRuntime::init).
    pub fn new(options: AppRootOptions, window: &mut Window, cx: &mut App) -> Self {
        let app = cx.new(|cx| AppRoot::with_options(options, window, cx));
        let shell = cx.new(|cx| EnvironmentShell::new(app.clone(), window, cx));
        let source = app.read(cx).data.palette_source();
        let shortcuts = cx.new(|cx| {
            let mut root = ShortcutRoot::new(shell.clone().into(), source, window, cx);
            root.set_default_focus(app.read(cx).focus.clone());
            root
        });
        let palette = shortcuts.read(cx).palette().clone();
        app.update(cx, |app, cx| app.attach_palette(&palette, window, cx));
        Self { shortcuts, shell, app }
    }

    /// Opens a window with these views as its root.
    pub fn open_window(
        window_options: WindowOptions,
        options: AppRootOptions,
        cx: &mut App,
    ) -> anyhow::Result<(AnyWindowHandle, Self)> {
        let mut parts = None;
        let (handle, _) = gpui_kit::open_window(window_options, cx, |window, cx| {
            let root = Self::new(options, window, cx);
            let shortcuts = root.shortcuts.clone();
            parts = Some(root);
            shortcuts
        })?;
        Ok((handle, parts.expect("window built")))
    }
}

/// Terminals as a local login shell in the thread's directory (or home).
#[cfg(unix)]
pub fn local_shell_backend() -> BackendFactory {
    Rc::new(|launch: &TerminalLaunch, _: &mut App| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cwd = Some(PathBuf::from(launch.cwd.as_ref())).filter(|p| p.is_dir()).or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        Box::new(LocalPty::new(&shell, &["-i"], cwd))
    })
}

/// Windows has no local shell backend; terminals stay empty until they run
/// on the server's terminal RPCs.
#[cfg(not(unix))]
pub fn local_shell_backend() -> BackendFactory {
    Rc::new(|_: &TerminalLaunch, _: &mut App| Box::new(crate::terminal::backend::RecordedBackend::new(Vec::new())))
}

/// Project scripts as a local login shell that runs the command, then stays
/// open as an interactive shell (the output stays readable).
#[cfg(unix)]
pub fn local_script_backend() -> ScriptBackendFactory {
    Rc::new(|launch: &TerminalLaunch, command: &str, _: &mut App| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let cwd = Some(PathBuf::from(launch.cwd.as_ref())).filter(|p| p.is_dir()).or_else(|| std::env::var_os("HOME").map(PathBuf::from));
        let script = format!("{command}; exec {shell} -i");
        Box::new(LocalPty::new(&shell, &["-ic", &script], cwd))
    })
}

/// Windows has no local script backend either; see [`local_shell_backend`].
#[cfg(not(unix))]
pub fn local_script_backend() -> ScriptBackendFactory {
    Rc::new(|_: &TerminalLaunch, _: &str, _: &mut App| Box::new(crate::terminal::backend::RecordedBackend::new(Vec::new())))
}

/// The main pane: the chat for a thread route, a page, or the no-thread state.
struct MainPane {
    layout: WeakEntity<AppSidebarLayout>,
    thread_view: Entity<ThreadRouteView>,
    /// `None` shows the thread view (or the no-thread state when there is no thread).
    page: Option<Route>,
    /// The built view of a page route (pull requests, issues), kept while it shows.
    page_view: Option<(Route, Entity<ListPage>)>,
    has_thread: bool,
}

/// The pull requests or issues page, with the item it opened beside it.
struct ListPage {
    list: AnyView,
    detail: Option<AnyView>,
    width: Pixels,
    _subscription: Subscription,
}

impl Render for ListPage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(div().flex_1().min_w_0().h_full().child(self.list.clone()))
            .children(self.detail.clone().map(|detail| div().flex_none().w(self.width).h_full().child(detail)))
    }
}

impl Render for MainPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (topbar, inset) = self
            .layout
            .read_with(cx, |layout, _| (layout.platform().topbar_height(), layout.header_content_left()))
            .unwrap_or((px(40.), px(0.)));
        if let Some(route) = &self.page {
            if let Some((built, view)) = &self.page_view
                && built == route
            {
                return view.clone().into_any_element();
            }
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
    data: SharedAppData,
    /// The fixture data, when the window runs on it (checks script it).
    fixture: Option<Rc<FixtureData>>,
    thread_view: Entity<ThreadRouteView>,
    main: Entity<MainPane>,
    right_panel: Entity<RightPanel>,
    right_panels: Entity<RightPanelStore>,
    terminals: Entity<PersistentThreadTerminals>,
    /// The media viewer over the whole window, while open.
    expanded: Option<(Entity<ExpandedImageDialog>, Subscription)>,
    /// "Link pull request", hosted here so it outlives the palette.
    link_dialog: Option<(Entity<LinkPullRequestDialog>, Subscription)>,
    /// The Usage page in the large route modal, while its route shows.
    usage: Option<(Entity<UsageRoute>, Subscription)>,
    /// The thread details card: docked or popover by the canvas width, each with its own
    /// open state (`rightPanelStore` visibility: docked starts open, the popover closed).
    details: DetailsVisibility,
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
        let options = AppRootOptions { data: DataSource::Fixtures(FixtureStore::from_sidebar(snapshot)), ..AppRootOptions::fixtures(platform) };
        Self::with_options(options, window, cx)
    }

    pub fn with_options(options: AppRootOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let AppRootOptions { data, platform, state_dir } = options;
        let ui = UiStateStore::load(state_dir.clone());
        let layout_state = ui.layout.clone();
        let right_panels = cx.new(|_| {
            state_dir.read(files::RIGHT_PANEL).map(|json| RightPanelStore::from_json(&json)).unwrap_or_default()
        });
        if let Some(json) = state_dir.read(files::TERMINAL) {
            cx.set_global(TerminalUiStateStore::from_json(&json));
        }
        cx.set_global(ui);

        let (data, fixture): (SharedAppData, _) = match data {
            DataSource::Fixtures(store) => {
                let fixture = FixtureData::new(store, cx);
                (fixture.clone(), Some(fixture))
            }
            DataSource::Live => (crate::live::LiveData::new(cx), None),
        };
        // A modal route (settings) is not restored: the app opens on the page under it.
        let initial = Route::parse(&layout_state.last_route);
        // Live stores open on tokio, so a saved thread may not be listed yet; the route stays and
        // the thread shows once the shell lists it.
        let initial = match initial.thread_target() {
            Some(target) if fixture.is_some() && !data.thread_exists(&target, cx) => Route::Index,
            _ if initial.is_modal() => Route::Index,
            _ => initial,
        };
        let router = cx.new(|_| Router::new(initial));

        let snapshot = data.snapshot(None, cx);
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
        let thread_view = cx.new(|cx| ThreadRouteView::new(data.clone(), window, cx));
        let main = cx.new(|_| MainPane { layout: layout.downgrade(), thread_view: thread_view.clone(), page: None, page_view: None, has_thread: false });
        let backends = data.terminal_backends(cx);
        let shell_backend = backends.shell.clone();
        let right_panel = cx.new(|cx| RightPanel::new(right_panels.clone(), layout.downgrade(), data.clone(), backends, cx));
        let terminals = cx.new(|cx| PersistentThreadTerminals::new(shell_backend, cx));
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
            // Drags end wherever the pointer is released, so widths are saved as they change.
            cx.observe(&layout, |_, layout, cx| Self::persist_layout(&layout, cx)),
            cx.subscribe_in(&data.events(), window, Self::on_data_event),
            cx.subscribe_in(&thread_view, window, Self::on_thread_event),
            cx.subscribe_in(&right_panel, window, |this, _, event: &RightPanelEvent, window, cx| match event {
                RightPanelEvent::Changed => this.panels_changed(window, cx),
                RightPanelEvent::InsertReference(text) => {
                    this.thread_view.update(cx, |view, cx| view.insert_reference(text, window, cx));
                }
                RightPanelEvent::AddThreadContext(text) => {
                    this.thread_view.update(cx, |view, cx| view.append_reference(text, cx));
                }
            }),
            cx.observe_global_in::<TerminalUiStateStore>(window, |this, window, cx| this.panels_changed(window, cx)),
            // A native notification or an inbox row opened a thread.
            cx.subscribe(&NotificationInbox::global(cx), |this, _, event: &InboxEvent, cx| {
                let InboxEvent::OpenThread(key) = event;
                this.navigate(Route::Thread(key.clone()), cx);
            }),
        ];

        let mut this = Self {
            layout,
            sidebar,
            router,
            data,
            fixture,
            thread_view,
            main,
            right_panel,
            right_panels,
            terminals,
            expanded: None,
            link_dialog: None,
            usage: None,
            details: DetailsVisibility { mode: ThreadDetailsMode::Popover, inline_open: true, popover_open: false },
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

    /// The window's data.
    pub fn data(&self) -> &SharedAppData {
        &self.data
    }

    /// The fixture store, when the window runs on fixtures.
    pub fn store(&self) -> Option<&Entity<FixtureStore>> {
        self.fixture.as_ref().map(|fixture| fixture.store())
    }

    /// The fixture data, when the window runs on fixtures.
    pub fn fixture_data(&self) -> Option<&Rc<FixtureData>> {
        self.fixture.as_ref()
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

    pub fn right_panel(&self) -> &Entity<RightPanel> {
        &self.right_panel
    }

    /// The media viewer, while open.
    pub fn expanded_image(&self) -> Option<&Entity<ExpandedImageDialog>> {
        self.expanded.as_ref().map(|(dialog, _)| dialog)
    }

    /// Whether the data has anything to show; without it the environment shell
    /// shows the first-run empty state.
    pub fn has_fixture_data(&self, cx: &App) -> bool {
        self.fixture.is_some() && self.data.has_data(cx)
    }

    /// The app's own main pane view (route content), for whoever swapped it out.
    pub fn main_view(&self) -> AnyView {
        self.main.clone().into()
    }

    /// The palette's data source.
    /// "Link pull request" is open.
    pub fn link_dialog_open(&self) -> bool {
        self.link_dialog.is_some()
    }

    pub fn palette_source(&self) -> Rc<dyn CommandPaletteSource> {
        self.data.palette_source()
    }

    /// Handles what the window's command palette asks for.
    pub fn attach_palette(&mut self, palette: &Entity<CommandPalette>, window: &mut Window, cx: &mut Context<Self>) {
        let subscription = cx.subscribe_in(palette, window, Self::on_palette_event);
        self._subscriptions.push(subscription);
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
    fn apply_route(&mut self, route: Route, window: &mut Window, cx: &mut Context<Self>) {
        if route == Route::Index {
            // `_chat.index.tsx`: start a draft in the most recent project.
            let draft = self.data.most_recent_project(cx).map(|project| self.data.create_draft(Some(project), cx));
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
        // Usage opens in the route modal over whatever the main pane showed.
        if route == Route::Usage {
            self.sync_usage(&route, window, cx);
            return self.persist_route(&route, cx);
        }
        // Settings is a modal over whatever the main pane showed.
        let page = (route.thread_target().is_none() && route != Route::Index && !route.is_modal()).then(|| route.clone());
        if route.is_modal() && self.main.read(cx).page.is_some() {
            return self.persist_route(&route, cx);
        }
        let has_thread = self.shown.is_some();
        let page_view = page.as_ref().and_then(|route| self.page_view(route, window, cx));
        self.sync_usage(&route, window, cx);
        self.main.update(cx, |main, cx| {
            if page_view.is_some() || page.is_none() {
                main.page_view = page_view;
            }
            main.page = page;
            main.has_thread = has_thread;
            cx.notify();
        });
        let active = self.shown_key();
        let shown_key = self.panel_key();
        self.restore_right_panel_width(cx);
        ThreadFindStore::retain_for_route(cx, route.is_modal(), shown_key.as_deref());
        self.sidebar.update(cx, |sidebar, cx| sidebar.set_active_thread(active.clone(), cx));
        self.sync_sidebar_drafts(cx);
        self.data.set_active_thread(active.as_ref(), cx);
        ThreadNotificationCoordinator::apply(NotificationInput::Route(active.clone()), cx);
        if let Some(key) = &active {
            let visited = jiff::Timestamp::now().to_string();
            let key = scoped_key(key);
            UiStateStore::update_ui(cx, |ui| ui.mark_thread_visited(&key, visited));
        }
        self.persist_route(&route, cx);
        let panel_focused = self.right_panel.focus_handle(cx).contains_focused(window, cx);
        self.sync_panels(cx);
        // Focus in a panel the new thread does not show would leave shortcuts with no handler.
        if panel_focused && !self.layout.read(cx).is_right_panel_open() {
            self.restore_focus(window, cx);
        }
    }

    /// The pull requests or issues page for `route`, reused while the route stays.
    fn page_view(&mut self, route: &Route, window: &mut Window, cx: &mut Context<Self>) -> Option<(Route, Entity<ListPage>)> {
        if let Some((built, view)) = &self.main.read(cx).page_view
            && built == route
        {
            return Some((built.clone(), view.clone()));
        }
        let width = self.layout.read(cx).right_panel_width();
        let view = match route {
            Route::PullRequests { .. } => {
                let sources = self.data.pull_request_sources(cx);
                let page = cx.new(|cx| PullRequestsPage::new(sources, window, cx));
                // Weak: a page kept past the window's teardown must not drop the app's data
                // (and its subscriptions) while GPUI is releasing subscribers.
                let data = Rc::downgrade(&self.data);
                cx.new(|cx| {
                    let subscription = cx.subscribe(&page, move |this: &mut ListPage, _, event: &PullRequestsPageEvent, cx| {
                        let PullRequestsPageEvent::Open { environment_id, reference } = event;
                        let Some(source) = data.upgrade().and_then(|data| data.pull_request_detail(environment_id, cx)) else { return };
                        let reference = reference.clone();
                        this.detail = Some(cx.new(|cx| PullRequestDetailPanel::new(source, reference, cx)).into());
                        cx.notify();
                    });
                    ListPage { list: page.into(), detail: None, width, _subscription: subscription }
                })
            }
            Route::Issues { .. } => {
                let sources = self.data.issue_sources(cx);
                let lookup = sources.clone();
                let page = cx.new(|cx| IssuesPage::new(sources, window, cx));
                cx.new(|cx| {
                    let subscription = cx.subscribe_in(&page, window, move |this: &mut ListPage, _, event: &IssuesPageEvent, window, cx| {
                        let IssuesPageEvent::Open { environment_id, reference } = event else { return };
                        let Some(source) = lookup.iter().find(|s| &s.environment_id == environment_id).map(|s| s.source.clone()) else { return };
                        let reference = reference.clone();
                        this.detail = Some(cx.new(|cx| IssueDetailPanel::new(source, reference, window, cx)).into());
                        cx.notify();
                    });
                    ListPage { list: page.into(), detail: None, width, _subscription: subscription }
                })
            }
            _ => return None,
        };
        Some((route.clone(), view))
    }

    /// Opens the Usage page over the window while its route shows, and closes it after.
    fn sync_usage(&mut self, route: &Route, window: &mut Window, cx: &mut Context<Self>) {
        if *route != Route::Usage {
            if self.usage.take().is_some() {
                cx.notify();
            }
            return;
        }
        if self.usage.is_some() {
            return;
        }
        let source = self.data.usage_source(cx);
        let prefs = cx.global::<UiStateStore>().dir.0.as_ref().map(|dir| dir.join("usage-preferences.json"));
        let usage = cx.new(|cx| UsageRoute::new(source, prefs, window, cx));
        let subscription = cx.subscribe(&usage, |this, _, _: &UsagePageEvent, cx| this.router.update(cx, |router, cx| router.back(cx)));
        window.focus(&usage.focus_handle(cx), cx);
        self.usage = Some((usage, subscription));
        cx.notify();
    }

    /// The server settings pages over the live data, for the settings modal.
    pub fn server_settings_pages(&self, cx: &mut App) -> Option<Entity<crate::components::settings::server_settings_pages::ServerSettingsPages>> {
        let model = self.data.server_settings(cx)?;
        Some(cx.new(|_| crate::components::settings::server_settings_pages::ServerSettingsPages::new(model)))
    }

    fn persist_route(&self, route: &Route, cx: &mut Context<Self>) {
        let path = route.path();
        UiStateStore::update_layout(cx, |layout| layout.last_route = path);
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
        let shown = self.shown.clone();
        self.right_panel.update(cx, |panel, cx| panel.set_thread(key.clone(), shown, cx));
        let terminal_thread = self.terminal_key().map(|k| {
            let cwd = self.thread_cwd(cx);
            (k, cwd)
        });
        self.terminals.update(cx, |terminals, cx| terminals.set_active_thread(terminal_thread, cx));
        self.layout.update(cx, |layout, cx| layout.set_panels(right_open, maximized, terminal_open, cx));
    }

    fn thread_cwd(&self, cx: &mut Context<Self>) -> SharedString {
        let Some(target) = self.shown.clone() else { return SharedString::default() };
        self.data.thread_view(&target, cx).map(|v| v.cwd).unwrap_or_default()
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

    fn on_thread_event(&mut self, _: &Entity<ThreadRouteView>, event: &ThreadRouteEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ThreadRouteEvent::Promoted(target) => {
                let route = match target {
                    ThreadTarget::Server(key) => Route::Thread(key.clone()),
                    ThreadTarget::Draft(draft) => Route::Draft(draft.clone()),
                };
                self.router.update(cx, |router, cx| router.replace(route, cx));
            }
            ThreadRouteEvent::OpenThread(key) => self.navigate(Route::Thread(key.clone()), cx),
            ThreadRouteEvent::OpenDiff { turn, file } => {
                let (turn, file) = (turn.as_ref().map(|t| t.0.clone()), file.clone());
                self.right_panel.update(cx, |panel, cx| panel.open_diff(turn, file, cx));
            }
            ThreadRouteEvent::OpenImage { attachments, index } => self.open_attachments(attachments, *index, window, cx),
            ThreadRouteEvent::OpenLink(target) => self.open_link(target, window, cx),
            ThreadRouteEvent::RunScript(script) => {
                let script = script.clone();
                self.right_panel.update(cx, |panel, cx| panel.run_script(script, cx));
            }
            ThreadRouteEvent::DetailsClosed => {
                self.details.popover_open = false;
                self.apply_details(cx);
            }
            ThreadRouteEvent::DetailsModeChanged(mode) => {
                self.details.mode = *mode;
                self.apply_details(cx);
            }
            ThreadRouteEvent::OpenTerminal(id) => self.right_panel.update(cx, |panel, cx| panel.add_terminal_tab(id.to_string(), cx)),
            ThreadRouteEvent::Navigate(path) => self.navigate(Route::parse(path), cx),
            ThreadRouteEvent::OpenLinkedItems => self.right_panel.update(cx, |panel, cx| panel.open_linked_items(cx)),
            ThreadRouteEvent::OpenBrowser(tab) => {
                let tab = tab.clone();
                self.right_panel.update(cx, |panel, cx| panel.open_browser_tab(&tab, cx));
            }
        }
    }

    /// Opens or closes the thread details card in its current mode.
    fn toggle_details(&mut self, cx: &mut Context<Self>) {
        if self.shown.is_none() {
            return;
        }
        let open = match self.details.mode {
            ThreadDetailsMode::Inline => &mut self.details.inline_open,
            ThreadDetailsMode::Popover => &mut self.details.popover_open,
        };
        *open = !*open;
        self.apply_details(cx);
    }

    /// Shows the card when its mode's state is open. Only the docked card presses the
    /// titlebar toggle (fork `12be669d98`).
    fn apply_details(&mut self, cx: &mut Context<Self>) {
        let mode = self.details.mode;
        let open = match mode {
            ThreadDetailsMode::Inline => self.details.inline_open,
            ThreadDetailsMode::Popover => self.details.popover_open,
        };
        if self.thread_view.read(cx).details_open() != open {
            self.thread_view.update(cx, |view, cx| view.set_details_open(open, cx));
        }
        let pressed = crate::components::chat::thread_details_panel::thread_details_toggle_pressed(mode, open);
        self.layout.update(cx, |layout, cx| layout.set_details_open(pressed, cx));
    }

    /// Opens the media viewer over the window.
    fn open_media(&mut self, items: Vec<ExpandedImageItem>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        let dialog = cx.new(|cx| ExpandedImageDialog::new(items, index, window, cx));
        let subscription = cx.subscribe_in(&dialog, window, |this, _, event: &ExpandedImageEvent, window, cx| {
            this.expanded = None;
            if let ExpandedImageEvent::OpenFile(path) = event {
                let path = path.clone();
                this.right_panel.update(cx, |panel, cx| panel.open_file(path, None, window, cx));
            } else {
                this.restore_focus(window, cx);
            }
            cx.notify();
        });
        window.focus(&dialog.focus_handle(cx), cx);
        self.expanded = Some((dialog, subscription));
        cx.notify();
    }

    /// A message's image attachments in the viewer.
    fn open_attachments(&mut self, attachments: &[Attachment], index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let items = attachments
            .iter()
            .filter_map(|attachment| {
                // Live attachments are files in the image cache; fixtures are bundled assets.
                let path = std::path::Path::new(attachment.source.as_ref());
                let bytes: Vec<u8> = if path.is_absolute() {
                    std::fs::read(path).ok()?
                } else {
                    cx.asset_source().load(&attachment.source).ok().flatten()?.into_owned()
                };
                Some(ExpandedImageItem {
                    name: attachment.name.clone(),
                    kind: MediaKind::Image,
                    bytes: MediaBytes::Ready(bytes.into()),
                    relative_path: None,
                    absolute_path: None,
                })
            })
            .collect();
        self.open_media(items, index, window, cx);
    }

    /// A file chip or relative link: images open in the viewer, other files
    /// in the Files tab at their line (`path:12` or `path#L12`).
    fn open_link(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (path, line) = split_line_suffix(target);
        let Some(workspace) = self.right_panel.update(cx, |panel, cx| panel.workspace(cx)) else { return };
        let root = workspace.files.root().to_string();
        let relative: SharedString = path.strip_prefix(&format!("{root}/")).unwrap_or(path).to_string().into();
        let is_image = crate::components::media::media_source::image_format(&relative, &[]).is_some();
        if is_image {
            let name: SharedString = relative.rsplit('/').next().unwrap_or(&relative).to_string().into();
            let item = ExpandedImageItem {
                name,
                kind: MediaKind::Image,
                bytes: MediaBytes::Workspace { files: workspace.files.clone(), path: relative.clone() },
                absolute_path: Some(absolute_path(&root, &relative).into()),
                relative_path: Some(relative),
            };
            self.open_media(vec![item], 0, window, cx);
        } else {
            self.right_panel.update(cx, |panel, cx| panel.open_file(relative, line, window, cx));
        }
    }

    /// Copies a link that opens the shown thread from anywhere (`thread.copyReference`).
    fn copy_thread_reference(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.shown_key() else { return };
        let segment = |s: &str| -> String {
            s.bytes()
                .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
                .collect()
        };
        let link = format!("https://app.t3.codes/threads/{}/{}", segment(&key.environment_id.0), segment(&key.thread_id.0));
        cx.write_to_clipboard(ClipboardItem::new_string(link));
        Toaster::push(ToastData::new(ToastKind::Success, t!("Thread copied")), cx);
    }

    /// Moves focus to the next or previous open area: sidebar, chat, right
    /// panel, terminal drawer (`workspace.focusNext` / `workspace.focusPrevious`).
    fn focus_area(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (sidebar_open, right_open, terminal_open) = {
            let layout = self.layout.read(cx);
            (layout.is_sidebar_open(), layout.is_right_panel_open(), layout.is_terminal_open())
        };
        let mut areas: Vec<FocusHandle> = Vec::new();
        if sidebar_open {
            areas.push(self.sidebar.focus_handle(cx));
        }
        if self.shown.is_some() {
            areas.push(self.composer(cx).focus_handle(cx));
        }
        if right_open {
            areas.push(self.right_panel.focus_handle(cx));
        }
        if terminal_open && let Some(drawer) = self.terminal_key().and_then(|k| self.terminals.read(cx).drawer(&k).cloned()) {
            areas.push(drawer.focus_handle(cx));
        }
        if areas.is_empty() {
            return;
        }
        let current = areas.iter().position(|area| area.contains_focused(window, cx));
        let next = match (current, forward) {
            (Some(i), true) => (i + 1) % areas.len(),
            (Some(i), false) => (i + areas.len() - 1) % areas.len(),
            (None, true) => 0,
            (None, false) => areas.len() - 1,
        };
        window.focus(&areas[next], cx);
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
            SidebarEvent::NewThread(project) => self.new_thread(project.clone(), window, cx),
            SidebarEvent::OpenSettings => self.open_settings(cx),
            SidebarEvent::OpenPullRequests => self.navigate(Route::parse("/pull-requests"), cx),
            SidebarEvent::OpenIssues => self.navigate(Route::parse("/issues"), cx),
            SidebarEvent::OpenUsage => self.navigate(Route::Usage, cx),
            SidebarEvent::DiscardDraft(key) => {
                self.data.discard_draft(key, cx);
                let route = Route::Thread(key.clone());
                self.router.update(cx, |router, _| router.retain_history(|r| *r != route));
            }
            SidebarEvent::ActivateDraft(id) => self.navigate(Route::Draft(DraftId(id.clone())), cx),
            SidebarEvent::DiscardDraftSession(id) => self.discard_draft_session(DraftId(id.clone()), cx),
            SidebarEvent::DraftCommand(id, command) => self.draft_command(DraftId(id.clone()), *command, cx),
            SidebarEvent::ThreadCommand(keys, command) => self.thread_command(keys, *command, window, cx),
            SidebarEvent::RenameThread(key, title) => self.data.rename_thread(key, title.clone(), cx),
            SidebarEvent::ProjectCommand(project, MenuCommand::ProjectSettings) => self.open_project_settings(project, cx),
            SidebarEvent::SearchChanged(query) => self.data.search_threads(query.clone(), cx),
            SidebarEvent::SettledShelf(request) => self.data.show_older_settled(request.clone(), cx),
            SidebarEvent::AddProject | SidebarEvent::OpenProjectFilter | SidebarEvent::ProjectCommand(..) => {}
        }
    }

    /// Discards a new-thread draft; when it is the one shown, the index route takes its place.
    fn discard_draft_session(&mut self, id: DraftId, cx: &mut Context<Self>) {
        self.data.discard_draft_session(&id, cx);
        let route = Route::Draft(id.clone());
        self.router.update(cx, |router, _| router.retain_history(|r| *r != route));
        if self.shown == Some(ThreadTarget::Draft(id)) {
            self.navigate(Route::Index, cx);
        }
        self.sync_sidebar_drafts(cx);
    }

    /// A draft row's menu (`buildDraftActionMenuItems`): copy its project's path, open the
    /// project's settings, or discard it.
    fn draft_command(&mut self, id: DraftId, command: MenuCommand, cx: &mut Context<Self>) {
        let project = self.sidebar.read(cx).snapshot().drafts.iter().find(|d| d.id == id.0).map(|d| d.project.clone());
        match (command, project) {
            (MenuCommand::Delete, _) => self.discard_draft_session(id, cx),
            (MenuCommand::ProjectSettings, Some(project)) => self.open_project_settings(&project, cx),
            (MenuCommand::CopyPath, Some(project)) => {
                let snapshot = self.sidebar.read(cx).snapshot();
                if let Some(root) = snapshot.projects.iter().find(|p| p.project_ref == project).map(|p| p.workspace_root.to_string()) {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(root));
                }
            }
            _ => {}
        }
    }

    /// Settings > Projects on `project`, as the palette's "Project settings" opens it.
    fn open_project_settings(&self, project: &ProjectRef, cx: &mut App) {
        self.navigate(
            Route::Settings { panel: SettingsPanel::Projects, query: format!("project={}", project.project_id.0).into() },
            cx,
        );
    }

    /// Gives the sidebar the current unsent drafts and the shown one.
    fn sync_sidebar_drafts(&self, cx: &mut Context<Self>) {
        let (drafts, active) = self.sidebar_drafts(cx);
        self.sidebar.update(cx, |sidebar, cx| sidebar.set_drafts(drafts, active, cx));
    }

    /// The unsent drafts and the shown one's id, as the sidebar lists them.
    fn sidebar_drafts(&self, cx: &App) -> (Vec<crate::components::sidebar::model::SidebarDraft>, Option<SharedString>) {
        let active = match &self.shown {
            Some(ThreadTarget::Draft(id)) => Some(id.0.clone()),
            _ => None,
        };
        (self.data.drafts(cx), active)
    }

    /// Runs a sidebar row or context-menu command on threads.
    fn thread_command(&mut self, keys: &[ThreadKey], command: MenuCommand, window: &mut Window, cx: &mut Context<Self>) {
        match self.data.thread_command(keys, command, cx) {
            CommandOutcome::Done => {}
            CommandOutcome::Rename(key) => self.sidebar.update(cx, |sidebar, cx| sidebar.start_rename(key, window, cx)),
            CommandOutcome::NewThread(project) => self.new_thread(project, window, cx),
        }
    }

    /// Starts a draft in `project` (or the most recent one) and shows it (`chat.new`).
    fn new_thread(&mut self, project: Option<ProjectRef>, window: &mut Window, cx: &mut Context<Self>) {
        let project = project.or_else(|| self.data.most_recent_project(cx));
        let draft = self.data.create_draft(project, cx);
        self.navigate(Route::Draft(draft), cx);
        self.restore_focus(window, cx);
    }

    /// Opens settings on General, as the sidebar gear and the palette do.
    fn open_settings(&self, cx: &mut App) {
        self.navigate(Route::settings(SettingsPath::General), cx);
    }

    /// Shows the thread `offset` rows from the shown one in sidebar order
    /// (`thread.previous` / `thread.next`), or the `n`th row (`thread.jump.n`).
    fn step_thread(&mut self, step: ThreadStep, cx: &mut Context<Self>) {
        let keys: Vec<ThreadKey> = self
            .sidebar
            .read(cx)
            .list()
            .entries
            .iter()
            .filter_map(|entry| match entry {
                SidebarListEntry::Thread { thread, .. } => Some(thread.key.clone()),
                _ => None,
            })
            .collect();
        let current = self.shown_key().and_then(|key| keys.iter().position(|k| *k == key));
        let target = match step {
            ThreadStep::Jump(n) => keys.get(n),
            ThreadStep::Next => keys.get(current.map_or(0, |ix| (ix + 1) % keys.len().max(1))),
            ThreadStep::Previous => keys.get(current.map_or(0, |ix| (ix + keys.len() - 1) % keys.len().max(1))),
        };
        if let Some(key) = target.cloned() {
            self.navigate(Route::Thread(key), cx);
        }
    }

    fn on_palette_event(&mut self, _: &Entity<CommandPalette>, event: &PaletteEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PaletteEvent::OpenThread { thread, .. } => self.navigate(Route::Thread(thread.clone()), cx),
            PaletteEvent::NewThreadIn(project) => self.new_thread(Some(project.clone()), window, cx),
            PaletteEvent::NewThreadWithoutProject(_) => self.new_thread(None, window, cx),
            PaletteEvent::OpenSettings => self.open_settings(cx),
            PaletteEvent::OpenSetting(id) => {
                let panel = SettingsPanel::from_slug(id).unwrap_or(SettingsPanel::General);
                self.navigate(Route::Settings { panel, query: SharedString::default() }, cx);
            }
            PaletteEvent::OpenProjectSettings(project) => self.open_project_settings(project, cx),
            PaletteEvent::OpenPullRequests => self.navigate(Route::parse("/pull-requests"), cx),
            PaletteEvent::OpenIssues => self.navigate(Route::parse("/issues"), cx),
            PaletteEvent::ShowLinkedItems => self.right_panel.update(cx, |panel, cx| panel.open_linked_items(cx)),
            PaletteEvent::LinkPullRequest => self.open_link_pull_request(window, cx),
            PaletteEvent::RestartAgentSession => {
                if let Some(key) = self.shown_key()
                    && StateBridge::is_running(cx)
                {
                    crate::state::palette::restart_agent_session(&key, cx);
                }
            }
            PaletteEvent::Triage => self.start_triage(cx),
            // Need the sync layer or server features the fixture store does not have.
            PaletteEvent::CopyPromptLink { .. }
            | PaletteEvent::ShareWithGuest
            | PaletteEvent::NewProject
            | PaletteEvent::AddProject(_)
            | PaletteEvent::UseInAnotherApp
            | PaletteEvent::ToggleProjectPin(_)
            | PaletteEvent::PortForward(_) => {}
        }
    }

    /// Opens "Link pull request" for the thread on screen (`openLinkPullRequestDialog`).
    fn open_link_pull_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.shown_key() else { return };
        let live = StateBridge::is_running(cx);
        let project = live.then(|| crate::state::palette::link_project(&key, cx)).flatten();
        let link: LinkPullRequest = if live {
            Rc::new(move |link, cx: &mut App| crate::state::palette::link_pull_request(&key, link, cx))
        } else {
            Rc::new(|_, _: &mut App| Task::ready(Ok(())))
        };
        let dialog = cx.new(|cx| LinkPullRequestDialog::new(project, link, window, cx));
        let subscription = cx.subscribe_in(&dialog, window, |this, _, _: &LinkPullRequestClosed, window, cx| {
            this.link_dialog = None;
            this.restore_focus(window, cx);
            cx.notify();
        });
        self.link_dialog = Some((dialog, subscription));
        cx.notify();
    }

    /// `useStartTriage`: once the server has the playbook and the scratch project, a No project
    /// draft opens with the prompt, unless the user went elsewhere meanwhile.
    fn start_triage(&mut self, cx: &mut Context<Self>) {
        if !StateBridge::is_running(cx) {
            return;
        }
        let Some(environment) = crate::state::palette::triage_environment(self.shown_key().as_ref(), cx) else { return };
        let route = self.router.read(cx).current().clone();
        let task = crate::state::palette::start_triage(environment, cx);
        cx.spawn(async move |this, cx| {
            let Some((project, prompt)) = task.await else { return };
            this.update(cx, |this, cx| {
                if this.router.read(cx).current() != &route {
                    return;
                }
                let draft = this.data.create_draft(Some(project), cx);
                let key = crate::composer_draft_store::ComposerDraftKey(format!("draft:{}", draft.0).into());
                let seeded = crate::composer_draft_store::ComposerDraft { prompt, ..Default::default() };
                this.data.draft_store(cx).borrow_mut().save(&key, &seeded);
                this.navigate(Route::Draft(draft), cx);
            })
            .ok();
        })
        .detach();
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
            LayoutEvent::Toggle(PanelToggle::Details) => self.toggle_details(cx),
            LayoutEvent::Changed => {
                Self::persist_layout(layout, cx);
                self.save_right_panel_width(cx);
            }
        }
    }

    /// The web keeps the right panel's width per thread (`t3code:preview-panel-width:<thread>`).
    fn right_panel_width_key(&self) -> Option<String> {
        self.shown_key().map(|key| format!("t3code:preview-panel-width:{}", scoped_key(&key)))
    }

    /// Gives the right panel the shown thread's saved width, or the web's default.
    fn restore_right_panel_width(&self, cx: &mut Context<Self>) {
        let Some(key) = self.right_panel_width_key() else { return };
        let width = crate::client_persistence_storage::LocalStorage::get::<f32>(cx, &key).unwrap_or(RIGHT_PANEL_DEFAULT_WIDTH);
        self.layout.update(cx, |layout, cx| layout.set_right_panel_width(px(width), cx));
    }

    /// Saves the right panel's width for the shown thread (after a drag, like the web).
    fn save_right_panel_width(&self, cx: &mut Context<Self>) {
        let Some(key) = self.right_panel_width_key() else { return };
        let width = f32::from(self.layout.read(cx).right_panel_width());
        let saved = crate::client_persistence_storage::LocalStorage::get::<f32>(cx, &key);
        if saved.unwrap_or(RIGHT_PANEL_DEFAULT_WIDTH) != width {
            crate::client_persistence_storage::LocalStorage::set(cx, &key, Some(&width));
        }
    }

    /// Saves widths and the sidebar's open state; unchanged values write nothing.
    fn persist_layout(layout: &Entity<AppSidebarLayout>, cx: &mut App) {
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

    fn on_data_event(&mut self, _: &Entity<DataEvents>, event: &DataEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let DataEvent::OpenThread(key) = event {
            return self.navigate(Route::Thread(key.clone()), cx);
        }
        if let DataEvent::OpenSideChat { parent, side_chat } = event {
            let side_chat_id = side_chat.thread_id.0.to_string();
            self.right_panels.update(cx, |store, cx| {
                store.open_side_chat(&scoped_key(parent), &side_chat_id);
                cx.notify();
            });
            self.navigate(Route::Thread(parent.clone()), cx);
            return self.panels_changed(window, cx);
        }
        if let DataEvent::SnapshotChanged = event {
            // The sidebar takes every snapshot, including the one that lets a restored route show.
            let active = self.shown_key();
            let mut snapshot = self.data.snapshot(active.as_ref(), cx);
            // Drafts ride along so one snapshot rebuilds the sidebar once.
            (snapshot.drafts, snapshot.active_draft) = self.sidebar_drafts(cx);
            self.sidebar.update(cx, |sidebar, cx| sidebar.set_snapshot(snapshot, cx));
            let route = self.router.read(cx).current().clone();
            if self.shown.is_none()
                && let Some(target) = route.thread_target()
                && self.data.thread_exists(&target, cx)
            {
                return self.apply_route(route, window, cx);
            }
            // `_chat.index.tsx` starts a draft once a project is known.
            if route == Route::Index && self.data.most_recent_project(cx).is_some() {
                self.apply_route(route, window, cx);
            }
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

/// Which thread `thread.previous`, `thread.next` and `thread.jump.n` show.
#[derive(Clone, Copy)]
enum ThreadStep {
    Previous,
    Next,
    /// Zero-based row among the sidebar's thread rows.
    Jump(usize),
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("app-root")
            .track_focus(&self.focus)
            .size_full()
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.layout.update(cx, |l, cx| l.toggle_sidebar(cx))))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| this.toggle_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleRightPanel, window, cx| {
                this.edit_right_panel(window, cx, |store, key| store.toggle_visibility(key))
            }))
            .on_action(cx.listener(|this, _: &ToggleRightPanelMaximized, window, cx| {
                this.edit_right_panel(window, cx, |store, key| {
                    let maximized = store.state(key).maximized;
                    store.set_maximized(key, !maximized);
                })
            }))
            .on_action(cx.listener(|this, _: &NewRightPanelTab, window, cx| this.edit_right_panel(window, cx, |store, key| store.show(key))))
            .on_action(cx.listener(|this, _: &CloseRightPanelTab, window, cx| {
                this.edit_right_panel(window, cx, |store, key| store.close_active(key))
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedView, window, cx| {
                this.edit_right_panel(window, cx, |store, key| {
                    store.reopen_closed(key);
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleDiff, window, cx| {
                this.edit_right_panel(window, cx, |store, key| store.toggle(key, SingletonKind::Diff))
            }))
            .on_action(cx.listener(|this, _: &TogglePreview, window, cx| {
                this.edit_right_panel(window, cx, |store, key| store.toggle(key, SingletonKind::Preview))
            }))
            .on_action(cx.listener(|this, _: &NavigateBack, _, cx| this.router.update(cx, |r, cx| r.back(cx))))
            .on_action(cx.listener(|this, _: &NavigateForward, _, cx| this.router.update(cx, |r, cx| r.forward(cx))))
            .on_action(cx.listener(|this, _: &NewChat, window, cx| this.new_thread(None, window, cx)))
            .on_action(cx.listener(|this, _: &NewLocalChat, window, cx| this.new_thread(None, window, cx)))
            .on_action(cx.listener(|this, _: &NewChatWithoutProject, window, cx| this.new_thread(None, window, cx)))
            .on_action(cx.listener(|this, _: &OpenUsage, _, cx| this.navigate(Route::Usage, cx)))
            .on_action(cx.listener(|this, _: &PreviousThread, _, cx| this.step_thread(ThreadStep::Previous, cx)))
            .on_action(cx.listener(|this, _: &NextThread, _, cx| this.step_thread(ThreadStep::Next, cx)))
            .on_action(cx.listener(|this, _: &JumpToThread1, _, cx| this.step_thread(ThreadStep::Jump(0), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread2, _, cx| this.step_thread(ThreadStep::Jump(1), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread3, _, cx| this.step_thread(ThreadStep::Jump(2), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread4, _, cx| this.step_thread(ThreadStep::Jump(3), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread5, _, cx| this.step_thread(ThreadStep::Jump(4), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread6, _, cx| this.step_thread(ThreadStep::Jump(5), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread7, _, cx| this.step_thread(ThreadStep::Jump(6), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread8, _, cx| this.step_thread(ThreadStep::Jump(7), cx)))
            .on_action(cx.listener(|this, _: &JumpToThread9, _, cx| this.step_thread(ThreadStep::Jump(8), cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(cx.listener(|this, _: &ToggleThreadPanel, _, cx| this.toggle_details(cx)))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.shown.is_some() && this.main.read(cx).page.is_none() {
                    let chat = this.thread_view.read(cx).chat().clone();
                    chat.update(cx, |chat, cx| chat.on_key_down(event, window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &FindInThread, window, cx| {
                let chat = this.thread_view.read(cx).chat().clone();
                if this.shown.is_some() {
                    chat.update(cx, |chat, cx| chat.open_find(window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &CopyThreadReference, _, cx| this.copy_thread_reference(cx)))
            .on_action(cx.listener(|this, _: &ToggleFilePicker, window, cx| {
                this.right_panel.update(cx, |panel, cx| panel.open_file_picker(window, cx))
            }))
            .on_action(cx.listener(|this, _: &ToggleProjectSearch, window, cx| {
                this.right_panel.update(cx, |panel, cx| panel.open_content_search(window, cx))
            }))
            .on_action(cx.listener(|this, _: &OpenFavoriteEditor, _, cx| {
                this.thread_view.update(cx, |view, cx| view.open_favorite_editor(cx))
            }))
            .on_action(cx.listener(|this, _: &FocusNextWorkspaceArea, window, cx| this.focus_area(true, window, cx)))
            .on_action(cx.listener(|this, _: &FocusPreviousWorkspaceArea, window, cx| this.focus_area(false, window, cx)))
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .on_action(|_: &crate::shortcuts::commands::ToggleThemeEditor, window, cx| crate::components::settings::theme_editor_panel::ThemeEditorStore::toggle(window, cx))
            .child(self.layout.clone())
            .children(self.usage.as_ref().map(|(usage, _)| {
                let router = self.router.downgrade();
                Modal::dialog("usage-route", usage.clone(), move |_, cx| {
                    _ = router.update(cx, |router, cx| router.back(cx));
                })
            }))
            .children(self.expanded.as_ref().map(|(dialog, _)| dialog.clone()))
            .children(self.link_dialog.as_ref().map(|(dialog, _)| dialog.clone()))
            .children(crate::components::settings::theme_editor_panel::ThemeEditorStore::layer(cx))
    }
}

/// `src/a.rs:12` or `src/a.rs#L12` into the path and the line.
fn split_line_suffix(target: &str) -> (&str, Option<u32>) {
    if let Some((path, line)) = target.rsplit_once("#L")
        && let Ok(line) = line.parse()
    {
        return (path, Some(line));
    }
    if let Some((path, line)) = target.rsplit_once(':')
        && let Ok(line) = line.parse()
    {
        return (path, Some(line));
    }
    (target, None)
}
