//! The right panel: its tab strip in the titlebar row and the active
//! surface's body. Ports `RightPanelTabs.tsx`. Tabs activate on click, close
//! on the X or a middle click, reorder by drag, and have a context menu
//! (Close, Close others, Close to the right, Close all). The `+` menu and the
//! empty panel's launcher open new surfaces.
//!
//! Bodies: the diff panel (one view, fed the shown thread's patch the frame
//! after the thread shows, so switching threads never waits on it), the
//! Files panel (one per project), terminal tabs (one session per tab), and a
//! pull request, issue or browser tab's view on the thread's environment.
//! Surfaces not built yet (device, linked items, side chat) draw a labelled
//! empty state. Only the active tab's body renders,
//! and a closed panel renders nothing.

use gpui_kit::rems;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use t3code_contracts::ProjectScript;

use crate::components::diff_panel::{DiffPanel, DiffPanelEvent};
use crate::components::diff_panel_shell::DiffPanelLoadingState;
use crate::components::files::file_preview_panel::{FilesPanel, FilesPanelEvent};
use crate::components::thread_terminal_drawer::{BackendFactory, TerminalLaunch, next_terminal_id};
use crate::diff_panel_store::{DiffPanelSelection, DiffPanelStore};
use crate::file_source::SharedWorkspaceFiles;
use crate::router::ThreadTarget;
use crate::state::app_data::SharedAppData;
use crate::terminal::backend::TerminalBackend;
use crate::terminal::surface::{TerminalSurface, TerminalSurfaceEvent};
use crate::terminal_ui_state_store::ThreadKey as TerminalThreadKey;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::app_sidebar_layout::AppSidebarLayout;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::empty::Empty;
use crate::components::ui::menu::{ContextMenu, MenuItem, MenuPopup, dropdown_menu};
use crate::components::ui::tooltip::tooltip;
use crate::right_panel_store::{RightPanelKind, RightPanelStore, RightPanelSurface, SingletonKind};
use crate::i18n::t;

/// The diff loads after the frame that shows the thread; this is the
/// longest it waits when no frame comes.
const DIFF_LOAD_FALLBACK: std::time::Duration = std::time::Duration::from_millis(50);

/// What the panel asks the app to do beyond editing the store.
#[derive(Clone, Debug)]
pub enum RightPanelEvent {
    /// The store changed; persist it.
    Changed,
    /// "Add to chat" in the Files tab: insert this mention in the composer.
    InsertReference(String),
}

/// Starts a project script in a new terminal session: the launch and the command.
pub type ScriptBackendFactory = Rc<dyn Fn(&TerminalLaunch, &str, &mut App) -> Box<dyn TerminalBackend>>;

/// How the panel starts terminal sessions.
#[derive(Clone)]
pub struct TerminalBackends {
    pub shell: BackendFactory,
    pub script: ScriptBackendFactory,
}

/// A project's workspace as the Files tab and the media viewer read it.
#[derive(Clone)]
pub struct Workspace {
    pub key: SharedString,
    pub name: SharedString,
    pub files: SharedWorkspaceFiles,
}

/// A Files tab request waiting for the panel to exist (it needs the window).
enum FilesRequest {
    Open { path: SharedString, line: Option<u32> },
    Picker,
    Search,
}

/// A tab being dragged to a new position.
#[derive(Clone)]
struct DraggedTab {
    id: SharedString,
    title: SharedString,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        div()
            .h(rems(1.5))
            .px(rems(0.5))
            .flex()
            .items_center()
            .rounded(rems(0.375))
            .bg(theme.colors.accent_surface)
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(theme.colors.text)
            .child(self.title.clone())
    }
}

/// The right panel for the thread on screen.
pub struct RightPanel {
    store: Entity<RightPanelStore>,
    /// The scoped thread key whose panel shows.
    thread: Option<SharedString>,
    target: Option<ThreadTarget>,
    data: SharedAppData,
    diff: Option<(Entity<DiffPanel>, Subscription)>,
    /// The thread and scope the diff view holds; anything else is stale.
    diff_shown: Option<(SharedString, DiffPanelSelection)>,
    diff_load_scheduled: bool,
    diff_fallback: Option<Task<()>>,
    /// The scope being loaded and its request; a newer request replaces it.
    diff_requested: Option<(SharedString, DiffPanelSelection)>,
    diff_loading: Option<Task<()>>,
    /// A file to reveal once the diff loads.
    pending_reveal: Option<SharedString>,
    files: HashMap<SharedString, (Entity<FilesPanel>, Subscription)>,
    files_requests: Vec<FilesRequest>,
    backends: TerminalBackends,
    /// Terminal tab sessions by thread and terminal id.
    terminals: HashMap<(SharedString, String), Entity<TerminalSurface>>,
    /// Scripts to start in the terminal tab of that id when it first renders.
    pending_scripts: HashMap<(SharedString, String), ProjectScript>,
    /// Pull request, issue and browser tab views, by thread and surface id.
    surface_views: HashMap<(SharedString, String), AnyView>,
    /// Read for the titlebar row's height and the insets around the fixed toggles.
    layout: WeakEntity<AppSidebarLayout>,
    topbar_height: Pixels,
    focus: FocusHandle,
    _observe: Subscription,
}

impl Focusable for RightPanel {
    /// The panel's area, for focus next/previous area.
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<RightPanelEvent> for RightPanel {}

/// The tab's label (`surfaceTitle`).
pub fn surface_title(surface: &RightPanelSurface) -> SharedString {
    match surface {
        RightPanelSurface::Diff { .. } => t!("Diff"),
        RightPanelSurface::Files { .. } => t!("Files"),
        RightPanelSurface::File { relative_path, .. } => {
            relative_path.rsplit(['/', '\\']).next().unwrap_or(relative_path).to_string().into()
        }
        RightPanelSurface::Terminal { active_terminal_id, .. } => {
            crate::components::thread_terminal_drawer::terminal_label(active_terminal_id)
        }
        RightPanelSurface::PullRequest { number, .. } => format!("#{number}").into(),
        RightPanelSurface::PullRequests { .. } => t!("Linked items"),
        RightPanelSurface::Issue { key, .. } => {
            if key.chars().all(|c| c.is_ascii_digit()) { format!("#{key}").into() } else { key.clone().into() }
        }
        RightPanelSurface::Device { title, .. } => title.clone().map(Into::into).unwrap_or_else(|| t!("Device")),
        RightPanelSurface::SideChat { .. } => t!("Side chat"),
        RightPanelSurface::Preview { .. } => t!("Browser"),
    }
}

/// The tab's 12px glyph (`SurfaceIcon`).
fn surface_icon(kind: RightPanelKind) -> IconName {
    match kind {
        RightPanelKind::Diff => IconName::FileDiff,
        RightPanelKind::Files => IconName::Files,
        RightPanelKind::File => IconName::File,
        RightPanelKind::Preview => IconName::Globe,
        RightPanelKind::Device => IconName::Smartphone,
        RightPanelKind::Terminal => IconName::SquareTerminal,
        RightPanelKind::PullRequest => IconName::GitPullRequestArrow,
        RightPanelKind::PullRequests => IconName::Link,
        RightPanelKind::Issue => IconName::CircleDot,
        RightPanelKind::SideChat => IconName::MessagesSquare,
    }
}

/// One launcher entry: a kind the `+` menu and the empty panel offer.
#[derive(Clone, Copy)]
enum Launch {
    Surface(SingletonKind),
    Terminal,
}

impl Launch {
    const ALL: [Launch; 6] = [
        Launch::Surface(SingletonKind::Preview),
        Launch::Terminal,
        Launch::Surface(SingletonKind::Files),
        Launch::Surface(SingletonKind::Diff),
        Launch::Surface(SingletonKind::PullRequests),
        Launch::Surface(SingletonKind::Device),
    ];

    fn label(self) -> SharedString {
        match self {
            Launch::Surface(SingletonKind::Preview) => t!("Browser"),
            Launch::Terminal => t!("Terminal"),
            Launch::Surface(SingletonKind::Files) => t!("Files"),
            Launch::Surface(SingletonKind::Diff) => t!("Diff"),
            Launch::Surface(SingletonKind::PullRequests) => t!("Linked items"),
            Launch::Surface(SingletonKind::Device) => t!("Device"),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Launch::Surface(kind) => surface_icon(kind.kind()),
            Launch::Terminal => IconName::SquareTerminal,
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            Launch::Surface(SingletonKind::Preview) => "B",
            Launch::Terminal => "T",
            Launch::Surface(SingletonKind::Files) => "F",
            Launch::Surface(SingletonKind::Diff) => "D",
            Launch::Surface(SingletonKind::PullRequests) => "L",
            Launch::Surface(SingletonKind::Device) => "M",
        }
    }
}

impl RightPanel {
    pub fn new(
        store: Entity<RightPanelStore>,
        layout: WeakEntity<AppSidebarLayout>,
        data: SharedAppData,
        backends: TerminalBackends,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.default_global::<DiffPanelStore>();
        let observe = cx.observe(&store, |_, _, cx| cx.notify());
        Self {
            store,
            thread: None,
            target: None,
            data,
            diff: None,
            diff_shown: None,
            diff_load_scheduled: false,
            diff_fallback: None,
            diff_requested: None,
            diff_loading: None,
            pending_reveal: None,
            files: HashMap::new(),
            files_requests: Vec::new(),
            backends,
            terminals: HashMap::new(),
            pending_scripts: HashMap::new(),
            surface_views: HashMap::new(),
            layout,
            topbar_height: px(40.),
            focus: cx.focus_handle(),
            _observe: observe,
        }
    }

    /// Shows `target`'s panel. Nothing loads here: the active tab's body
    /// loads when it renders.
    pub fn set_thread(&mut self, thread: Option<SharedString>, target: Option<ThreadTarget>, cx: &mut Context<Self>) {
        if self.thread != thread {
            self.thread = thread;
            self.target = target;
            self.files_requests.clear();
            cx.notify();
        }
    }

    pub fn diff_panel(&self) -> Option<&Entity<DiffPanel>> {
        self.diff.as_ref().map(|(diff, _)| diff)
    }

    /// The Files panel of the shown thread's project, once built.
    pub fn files_panel(&mut self, cx: &mut App) -> Option<Entity<FilesPanel>> {
        let key = self.workspace(cx)?.key;
        self.files.get(&key).map(|(panel, _)| panel.clone())
    }

    /// The terminal session of a tab, once started.
    pub fn terminal(&self, terminal_id: &str) -> Option<&Entity<TerminalSurface>> {
        let thread = self.thread.clone()?;
        self.terminals.get(&(thread, terminal_id.to_string()))
    }

    /// Whether the diff view holds the shown thread's current scope.
    pub fn diff_loaded(&self, cx: &App) -> bool {
        self.diff.is_some() && !self.diff_stale(cx)
    }

    /// The shown thread's project workspace.
    pub fn workspace(&mut self, cx: &mut App) -> Option<Workspace> {
        let target = self.target.clone()?;
        self.data.workspace(&target, cx)
    }

    /// Opens the Diff tab on a turn (its reply id) or the branch, revealing `file`.
    pub fn open_diff(&mut self, turn: Option<SharedString>, file: Option<SharedString>, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        let store = cx.global_mut::<DiffPanelStore>();
        match &turn {
            Some(turn) => store.select_turn(&thread, turn, file.as_deref()),
            None if file.is_some() => store.select_git_scope(&thread, crate::diff_panel_store::GitScope::Branch),
            None => {}
        }
        self.pending_reveal = file;
        if self.diff_loaded(cx)
            && let Some(path) = self.pending_reveal.take()
            && let Some((diff, _)) = &self.diff
        {
            diff.update(cx, |diff, cx| diff.reveal_path(&path, cx));
        }
        self.edit(cx, |store, thread| store.open(thread, SingletonKind::Diff));
    }

    /// Opens `path` in the Files tab, at `line`.
    pub fn open_file(&mut self, path: SharedString, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        self.files_request(FilesRequest::Open { path, line }, window, cx);
    }

    /// Go to file (Mod+P): the Files tab with its picker open.
    pub fn open_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files_request(FilesRequest::Picker, window, cx);
    }

    /// Search project contents (Mod+Shift+F) in the Files tab.
    pub fn open_content_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files_request(FilesRequest::Search, window, cx);
    }

    fn files_request(&mut self, request: FilesRequest, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread.is_none() {
            return;
        }
        self.edit(cx, |store, thread| store.open(thread, SingletonKind::Files));
        self.files_requests.push(request);
        if let Some(panel) = self.ensure_files(window, cx) {
            self.run_files_requests(&panel, window, cx);
        }
    }

    fn run_files_requests(&mut self, panel: &Entity<FilesPanel>, window: &mut Window, cx: &mut Context<Self>) {
        for request in std::mem::take(&mut self.files_requests) {
            panel.update(cx, |panel, cx| match request {
                FilesRequest::Open { path, line } => panel.open_file(path, line, window, cx),
                FilesRequest::Picker => panel.open_file_picker(window, cx),
                FilesRequest::Search => panel.open_content_search(window, cx),
            });
        }
    }

    /// The Files panel for the shown thread's project, built on first use.
    fn ensure_files(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<FilesPanel>> {
        let workspace = self.workspace(cx)?;
        if let Some((panel, _)) = self.files.get(&workspace.key) {
            return Some(panel.clone());
        }
        let panel = cx.new(|cx| FilesPanel::new(workspace.files.clone(), workspace.name.clone(), window, cx));
        let subscription = cx.subscribe(&panel, |_, _, event: &FilesPanelEvent, cx| match event {
            FilesPanelEvent::AddToChat(mention) => cx.emit(RightPanelEvent::InsertReference(mention.clone())),
            // Opening on the host needs the server (`shell.openInEditor`).
            FilesPanelEvent::OpenInEditor { .. } => {}
            // Opening in the preview browser needs the server (`assets.createUrl`, `preview.open`).
            FilesPanelEvent::OpenInPreview { .. } => {}
        });
        self.files.insert(workspace.key.clone(), (panel.clone(), subscription));
        Some(panel)
    }

    /// Opens a terminal tab running a project script.
    pub fn run_script(&mut self, script: ProjectScript, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        let id = self.next_terminal_id(&thread);
        self.pending_scripts.insert((thread, id.clone()), script);
        self.add_terminal_tab(id, cx);
    }

    /// Opens a new terminal tab with a shell.
    pub fn new_terminal_tab(&mut self, cx: &mut Context<Self>) {
        let Some(thread) = self.thread.clone() else { return };
        let id = self.next_terminal_id(&thread);
        self.add_terminal_tab(id, cx);
    }

    fn next_terminal_id(&self, thread: &SharedString) -> String {
        let used: Vec<String> = self.terminals.keys().filter(|(t, _)| t == thread).map(|(_, id)| id.clone()).chain(
            self.pending_scripts.keys().filter(|(t, _)| t == thread).map(|(_, id)| id.clone()),
        ).collect();
        next_terminal_id(&used)
    }

    fn add_terminal_tab(&mut self, id: String, cx: &mut Context<Self>) {
        let surface = RightPanelSurface::Terminal {
            id: format!("terminal:{id}"),
            resource_id: id.clone(),
            terminal_ids: vec![id.clone()],
            active_terminal_id: id,
        };
        self.edit(cx, |store, thread| store.open_surface(thread, surface));
    }

    /// The session behind a terminal tab, started when the tab first renders.
    fn terminal_surface(&mut self, terminal_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TerminalSurface>> {
        let thread = self.thread.clone()?;
        let key = (thread.clone(), terminal_id.to_string());
        if let Some(surface) = self.terminals.get(&key) {
            return Some(surface.clone());
        }
        let cwd = self.workspace(cx).map(|w| SharedString::from(w.files.root().to_string())).unwrap_or_default();
        let launch = TerminalLaunch {
            thread: TerminalThreadKey(thread.to_string()),
            terminal_id: terminal_id.to_string().into(),
            cwd,
            worktree_path: None,
        };
        let backend = match self.pending_scripts.remove(&key) {
            Some(script) => (self.backends.script)(&launch, &script.command, cx),
            None => (self.backends.shell)(&launch, cx),
        };
        let surface = cx.new(|cx| TerminalSurface::new(backend, window, cx));
        let tab_id = format!("terminal:{terminal_id}");
        cx.subscribe(&surface, move |this, _, event: &TerminalSurfaceEvent, cx| {
            if let TerminalSurfaceEvent::SessionEnded(_) = event {
                let id = tab_id.clone();
                this.edit(cx, |store, thread| store.close_surface(thread, &id));
            }
        })
        .detach();
        self.terminals.insert(key, surface.clone());
        Some(surface)
    }

    fn diff_selection(&self, cx: &App) -> Option<(SharedString, DiffPanelSelection)> {
        let thread = self.thread.clone()?;
        let selection = cx.global::<DiffPanelStore>().selection(&thread);
        Some((thread, selection))
    }

    fn diff_stale(&self, cx: &App) -> bool {
        self.diff_shown != self.diff_selection(cx)
    }

    /// Feeds the diff view the shown thread's patch for its scope.
    fn load_diff(&mut self, cx: &mut Context<Self>) {
        if !self.diff_load_scheduled {
            return;
        }
        self.diff_load_scheduled = false;
        self.diff_fallback = None;
        let (Some((thread, selection)), Some(target), Some((diff, _))) =
            (self.diff_selection(cx), self.target.clone(), self.diff.as_ref())
        else {
            return;
        };
        let diff = diff.downgrade();
        self.diff_requested = Some((thread.clone(), selection.clone()));
        let input = self.data.diff_input(&target, &selection, cx);
        self.diff_loading = Some(cx.spawn(async move |this, cx| {
            let Some(input) = input.await else { return };
            this.update(cx, |this, cx| {
                // A thread or scope switch while it loaded makes the reply stale.
                if this.diff_selection(cx).as_ref() != Some(&(thread.clone(), selection.clone())) {
                    return;
                }
                let Some(diff) = diff.upgrade() else { return };
                diff.update(cx, |diff, cx| diff.set_input(input, cx));
                this.diff_shown = Some((thread, selection));
                if let Some(path) = this.pending_reveal.take() {
                    diff.update(cx, |diff, cx| diff.reveal_path(&path, cx));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_diff_event(&mut self, event: &DiffPanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            DiffPanelEvent::SelectionChanged(_) | DiffPanelEvent::Refresh | DiffPanelEvent::IgnoreWhitespaceChanged(_) => {
                self.diff_shown = None;
                self.diff_requested = None;
                cx.notify();
            }
            DiffPanelEvent::OpenFile(path) => self.open_file(path.clone(), None, window, cx),
            DiffPanelEvent::FileMenuAction { action, path } => {
                if let Some(files) = self.ensure_files(window, cx) {
                    files.update(cx, |files, cx| files.run_file_action(action.clone(), path.clone(), cx));
                }
            }
            // Full contents and review comments need the server.
            DiffPanelEvent::RequestFileContents { .. }
            | DiffPanelEvent::ReviewComment(_)
            | DiffPanelEvent::DeleteReviewComment(_) => {}
        }
    }

    /// The active surface's body.
    fn render_body(&mut self, surface: &RightPanelSurface, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match surface {
            RightPanelSurface::Diff { .. } => {
                if self.diff.is_none() {
                    let diff = cx.new(|cx| DiffPanel::new(window, cx));
                    let subscription = cx.subscribe_in(&diff, window, |this, _, event, window, cx| this.on_diff_event(event, window, cx));
                    self.diff = Some((diff, subscription));
                }
                if self.diff_stale(cx) {
                    // The patch loads after this frame, so showing a thread never waits on it.
                    if !self.diff_load_scheduled && self.diff_requested != self.diff_selection(cx) {
                        self.diff_load_scheduled = true;
                        cx.on_next_frame(window, |this, _, cx| this.load_diff(cx));
                        // A window that draws no next frame (hidden, headless) still loads.
                        self.diff_fallback = Some(cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(DIFF_LOAD_FALLBACK).await;
                            this.update(cx, |this, cx| this.load_diff(cx)).ok();
                        }));
                    }
                    let label = match self.diff_selection(cx).map(|(_, selection)| selection) {
                        Some(DiffPanelSelection::ToolCall { .. }) => t!("Loading tool call diff..."),
                        Some(DiffPanelSelection::Turn { .. }) => t!("Loading checkpoint diff..."),
                        Some(DiffPanelSelection::Unstaged) => t!("Loading uncommitted changes..."),
                        Some(DiffPanelSelection::Commit { .. }) => t!("Loading commit..."),
                        _ => t!("Loading changes..."),
                    };
                    return div().flex_1().min_h_0().child(DiffPanelLoadingState::new(label)).into_any_element();
                }
                let (diff, _) = self.diff.as_ref().expect("diff built above");
                div().flex_1().min_h_0().child(diff.clone()).into_any_element()
            }
            RightPanelSurface::Files { .. } | RightPanelSurface::File { .. } => match self.ensure_files(window, cx) {
                Some(panel) => {
                    if let RightPanelSurface::File { relative_path, reveal_line, .. } = surface
                        && panel.read(cx).open_path().map(|p| p.as_ref()) != Some(relative_path.as_str())
                    {
                        let (path, line) = (SharedString::from(relative_path.clone()), *reveal_line);
                        panel.update(cx, |panel, cx| panel.open_file(path, line, window, cx));
                    }
                    if !self.files_requests.is_empty() {
                        self.run_files_requests(&panel, window, cx);
                    }
                    div().flex_1().min_h_0().child(panel).into_any_element()
                }
                None => self.render_empty(surface, cx),
            },
            RightPanelSurface::Terminal { active_terminal_id, .. } => match self.terminal_surface(active_terminal_id, window, cx) {
                Some(terminal) => div().flex_1().min_h_0().px_2().pb_2().child(terminal).into_any_element(),
                None => self.render_empty(surface, cx),
            },
            RightPanelSurface::PullRequest { .. } | RightPanelSurface::Issue { .. } | RightPanelSurface::Preview { .. } => {
                match self.surface_view(surface, window, cx) {
                    Some(view) => div().flex_1().min_h_0().child(view).into_any_element(),
                    None => self.render_empty(surface, cx),
                }
            }
            _ => self.render_empty(surface, cx),
        }
    }

    /// The view behind a pull request, issue or browser tab, built on first show over the
    /// shown thread's environment.
    fn surface_view(&mut self, surface: &RightPanelSurface, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyView> {
        let thread = self.thread.clone()?;
        let key = (thread.clone(), surface.id().to_string());
        if let Some(view) = self.surface_views.get(&key) {
            return Some(view.clone());
        }
        let (environment, thread_id) = thread.split_once(':')?;
        let environment = t3code_contracts::EnvironmentId::new(environment);
        let view: AnyView = match surface {
            RightPanelSurface::PullRequest { project_id, repository, number, .. } => {
                let source = self.data.pull_request_detail(&environment, cx)?;
                let reference = t3code_contracts::PullRequestRef {
                    project_id: t3code_contracts::ProjectId::new(project_id.as_str()),
                    host: None,
                    expected_account_id: None,
                    allow_stale: None,
                    repository: repository.clone(),
                    number: u64::from(*number),
                };
                cx.new(|cx| crate::components::pull_request::pull_request_detail_panel::PullRequestDetailPanel::new(source, reference, cx)).into()
            }
            RightPanelSurface::Issue { project_id, tracker, scope, key: issue, .. } => {
                let source = self.data.issue_sources(cx).into_iter().find(|s| s.environment_id == environment)?.source;
                let reference = t3code_contracts::IssueRef {
                    project_id: t3code_contracts::ProjectId::new(project_id.as_str()),
                    tracker: serde_json::from_value(serde_json::Value::String(tracker.clone())).ok()?,
                    host: None,
                    scope: scope.clone(),
                    key: issue.clone(),
                };
                cx.new(|cx| crate::components::issue::issue_detail_panel::IssueDetailPanel::new(source, reference, window, cx)).into()
            }
            RightPanelSurface::Preview { resource_id, .. } => {
                let source = self.data.preview_source(&environment, cx)?;
                let (thread_id, tab) = (thread_id.to_owned(), resource_id.clone().unwrap_or_default());
                cx.new(|cx| crate::browser::server_browser_surface::ServerBrowserSurface::new(source, thread_id, tab, cx)).into()
            }
            _ => return None,
        };
        self.surface_views.insert(key, view.clone());
        Some(view)
    }

    /// A surface whose view does not exist yet names itself.
    fn render_empty(&self, surface: &RightPanelSurface, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(Empty::new(surface_title(surface)).icon(surface_icon(surface.kind())))
            .text_color(theme.colors.text)
            .into_any_element()
    }

    fn edit(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut RightPanelStore, &str)) {
        let Some(thread) = self.thread.clone() else { return };
        self.store.update(cx, |store, cx| {
            edit(store, &thread);
            cx.notify();
        });
        cx.emit(RightPanelEvent::Changed);
    }

    /// Opens the Linked items tab (`open(threadRef, "pull-requests")`).
    pub fn open_linked_items(&mut self, cx: &mut Context<Self>) {
        self.launch(Launch::Surface(SingletonKind::PullRequests), cx);
    }

    /// Opens a server browser tab (`openBrowser(threadRef, tabId)`).
    pub fn open_browser_tab(&mut self, tab: &str, cx: &mut Context<Self>) {
        let surface = RightPanelSurface::Preview { id: format!("browser:{tab}"), resource_id: Some(tab.to_owned()) };
        self.edit(cx, |store, thread| store.open_surface(thread, surface));
    }

    fn launch(&mut self, launch: Launch, cx: &mut Context<Self>) {
        match launch {
            Launch::Surface(kind) => self.edit(cx, |store, thread| store.open(thread, kind)),
            Launch::Terminal => self.new_terminal_tab(cx),
        }
    }

    fn launch_menu(weak: WeakEntity<Self>) -> MenuPopup {
        Launch::ALL.into_iter().fold(MenuPopup::new(), |menu, launch| {
            let weak = weak.clone();
            menu.item(MenuItem::new(launch.label()).icon(launch.icon()).on_select(move |_, cx| {
                weak.update(cx, |this, cx| this.launch(launch, cx)).ok();
            }))
        })
    }

    fn render_tab(&self, index: usize, surface: &RightPanelSurface, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let id: SharedString = surface.id().to_string().into();
        let title = surface_title(surface);
        let group: SharedString = format!("tab-{id}").into();
        let close = {
            let id = id.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                let id = id.clone();
                this.edit(cx, |store, thread| store.close_surface(thread, &id));
                cx.stop_propagation();
            })
        };
        let close_button = div()
            .id(("tab-close", index))
            .size(px(16.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(rems(0.25))
            .hover(|s| s.bg(c.muted))
            .child(
                div()
                    .group_hover(group.clone(), |s| s.invisible())
                    .child(Icon::new(surface_icon(surface.kind())).size(IconSize::Xs)),
            )
            .child(
                div()
                    .absolute()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .child(Icon::new(IconName::X).size(IconSize::Xs)),
            )
            .on_click(close)
            .tooltip(tooltip(t!("Close {title}", title = title.clone())));
        let (fg, bg) = if active { (c.text, Some(c.accent_surface)) } else { (c.muted_foreground, None) };
        let hover_bg = c.accent_surface.opacity(0.6);
        let hover_fg = c.text;
        let tab = div()
            .id(("tab", index))
            .group(group)
            .occlude()
            .h(rems(1.5))
            .max_w(rems(9.))
            .flex_none()
            .flex()
            .items_center()
            .gap(rems(0.125))
            .pl(rems(0.375))
            .pr(rems(0.5))
            .rounded(rems(0.375))
            .cursor_pointer()
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(fg)
            .when_some(bg, |d, bg| d.bg(bg))
            .when(!active, |d| d.hover(move |s| s.bg(hover_bg).text_color(hover_fg)))
            .child(close_button)
            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(title.clone()))
            .on_click({
                let id = id.clone();
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let id = id.clone();
                    this.edit(cx, |store, thread| store.activate_surface(thread, &id));
                })
            })
            .on_mouse_up(MouseButton::Middle, {
                let id = id.clone();
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    let id = id.clone();
                    this.edit(cx, |store, thread| store.close_surface(thread, &id));
                })
            })
            .on_drag(DraggedTab { id: id.clone(), title: title.clone() }, |tab, _, _, cx| cx.new(|_| tab.clone()))
            .drag_over::<DraggedTab>(move |s, _, _, _| s.bg(hover_bg))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                let dragged = dragged.id.clone();
                this.edit(cx, |store, thread| store.move_surface(thread, &dragged, index));
            }));
        let weak = cx.weak_entity();
        ContextMenu::new(("tab-menu", index), tab, move |_, _| {
            let action = |label: SharedString, f: fn(&mut RightPanelStore, &str, &str)| {
                let (weak, id) = (weak.clone(), id.clone());
                MenuItem::new(label).on_select(move |_, cx| {
                    let id = id.clone();
                    weak.update(cx, |this, cx| this.edit(cx, |store, thread| f(store, thread, &id))).ok();
                })
            };
            MenuPopup::new()
                .item(action(t!("Close"), |s, t, id| s.close_surface(t, id)))
                .item(action(t!("Close others"), |s, t, id| s.close_other_surfaces(t, id)))
                .item(action(t!("Close to the right"), |s, t, id| s.close_surfaces_to_right(t, id)))
                .separator()
                .item(action(t!("Close all"), |s, t, _| {
                    for surface in s.state(t).surfaces {
                        s.close_surface(t, surface.id());
                    }
                }))
        })
        .into_any_element()
    }

    fn render_launcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let hover = c.accent_surface.opacity(0.6);
        let rows = Launch::ALL.into_iter().enumerate().map(|(index, launch)| {
            div()
                .id(("launch", index))
                .h(rems(2.))
                .w_full()
                .flex()
                .items_center()
                .gap(rems(0.625))
                .px(rems(0.625))
                .rounded(theme.radii.control)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.text)
                .child(Icon::new(launch.icon()).size(IconSize::Md).color(c.muted_foreground))
                .child(div().flex_1().child(launch.label()))
                .child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(launch.shortcut()))
                .on_click(cx.listener(move |this, _, _, cx| this.launch(launch, cx)))
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .px(rems(1.5))
            .pb(self.topbar_height)
            .child(
                div()
                    .w_full()
                    .max_w(rems(20.))
                    .py(rems(1.5))
                    .child(
                        div()
                            .mb(rems(0.75))
                            .text_center()
                            .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                            .text_color(c.text)
                            .child(t!("Open a surface")),
                    )
                    .child(div().flex().flex_col().gap(rems(0.125)).children(rows)),
            )
    }
}

impl Render for RightPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (topbar, leading, trailing) = self
            .layout
            .read_with(cx, |layout, _| {
                (layout.platform().topbar_height(), layout.right_panel_leading_inset(), layout.right_panel_trailing_inset(window))
            })
            .unwrap_or((px(40.), px(0.), px(112.)));
        self.topbar_height = topbar;
        let state = self.thread.as_ref().map(|t| self.store.read(cx).state(t)).unwrap_or_default();
        let active_id = state.active_surface_id.clone();
        let tabs: Vec<_> = state
            .surfaces
            .iter()
            .enumerate()
            .map(|(index, surface)| self.render_tab(index, surface, active_id.as_deref() == Some(surface.id()), cx))
            .collect();
        let weak = cx.weak_entity();
        let add = dropdown_menu(
            "right-panel-add",
            Button::new("right-panel-add-trigger")
                .icon(IconName::Plus)
                .size(ButtonSize::IconXs)
                .variant(ButtonVariant::GhostMuted),
            move |_, _| Self::launch_menu(weak.clone()),
        );
        let tab_row = div()
            .id("right-panel-tabbar")
            .flex_none()
            .h(topbar)
            .flex()
            .items_center()
            .gap_1()
            .pl(cx.app_theme().rem(0.5) + leading)
            .pr(trailing)
            .child(
                div()
                    .id("right-panel-tab-list")
                    .min_w_0()
                    .flex_1()
                    .h_full()
                    .overflow_x_scroll()
                    .child(div().h_full().flex().items_center().gap_1().children(tabs).child(add)),
            );
        let body = match state.active() {
            Some(surface) => self.render_body(surface, window, cx),
            None => self.render_launcher(cx).into_any_element(),
        };
        div().size_full().flex().flex_col().track_focus(&self.focus).child(tab_row).child(body)
    }
}
