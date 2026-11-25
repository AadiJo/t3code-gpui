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
use crate::components::side_chat_pane::{SideChatPane, SideChatPaneEvent};
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
    /// "Add to chat" on a side chat: a thread chip for the main thread's next message.
    AddThreadContext(String),
    /// A review comment from the Diff tab (`addReviewComment`): its chip goes in the composer at
    /// the caret unless it is there already.
    AddReviewComment(String),
    /// A review comment was deleted (`removeReviewComment`): its chip leaves the composer.
    RemoveReviewComment(String),
    /// A side chat's image or link, for the window to open.
    SideChatAction(crate::components::chat::messages_timeline::TimelineAction),
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
    /// The shown git diff's checkout and refs, for `review.getDiffFileContents`.
    diff_source: Option<DiffSource>,
    /// Diff files whose full contents arrived, for checks.
    contents_loaded: Vec<SharedString>,
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
    /// The side chat tab on screen; one side chat streams at a time.
    side_chat: Option<(crate::components::sidebar::model::ThreadKey, Entity<SideChatPane>, Subscription)>,
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
        RightPanelKind::Preview => IconName::Earth,
        RightPanelKind::Device => IconName::Smartphone,
        RightPanelKind::Terminal => IconName::SquareTerminal,
        RightPanelKind::PullRequest => IconName::GitPullRequestArrow,
        RightPanelKind::PullRequests => IconName::Link2,
        RightPanelKind::Issue => IconName::CircleDot,
        RightPanelKind::SideChat => IconName::MessagesSquare,
    }
}

/// One launcher entry: a kind the `+` menu and the empty panel offer.
#[derive(Clone, Copy, PartialEq, Eq)]
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
            diff_source: None,
            contents_loaded: Vec::new(),
            diff_loading: None,
            pending_reveal: None,
            files: HashMap::new(),
            files_requests: Vec::new(),
            backends,
            terminals: HashMap::new(),
            pending_scripts: HashMap::new(),
            surface_views: HashMap::new(),
            side_chat: None,
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

    /// Diff files whose full contents arrived from the server.
    pub fn contents_loaded(&self) -> &[SharedString] {
        &self.contents_loaded
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
        let capabilities = crate::live::host_menus::target_file_menu(self.target.as_ref(), cx);
        let menus = crate::live::host_menus::right_panel_menus(self, self.target.clone().as_ref(), cx);
        panel.update(cx, |panel, cx| {
            panel.set_capabilities(capabilities, cx);
            panel.set_markdown_menus(menus);
        });
        let subscription = cx.subscribe(&panel, |this, _, event: &FilesPanelEvent, cx| match event {
            FilesPanelEvent::AddToChat(mention) => cx.emit(RightPanelEvent::InsertReference(mention.clone())),
            FilesPanelEvent::OpenInEditor { absolute_path, editor, reveal } => this.open_in_editor(absolute_path.clone(), *editor, *reveal, cx),
            FilesPanelEvent::OpenInPreview { absolute_path } => this.open_in_preview(absolute_path.clone(), cx),
            FilesPanelEvent::RunOnServer { .. } => crate::live::host_menus::on_files_event(this, this.target.clone(), event, cx),
        });
        self.files.insert(workspace.key.clone(), (panel.clone(), subscription));
        // `canWriteFiles`: the session's filesystem-write grant, asked of the server once.
        if let Some(ThreadTarget::Server(key)) = &self.target {
            let id = crate::state::shell::runtime_ids(key).0;
            if let Some(check) = crate::components::settings::connections_settings_access::check_session(&id, cx) {
                let weak = panel.downgrade();
                cx.spawn(async move |_, cx| {
                    let Ok(session) = check.await else { return };
                    let writable = crate::components::settings::connections_settings_access::grants(&session, t3code_contracts::AuthEnvironmentScope::FilesystemWrite);
                    weak.update(cx, |panel, cx| panel.set_read_only_connection(!writable, cx)).ok();
                })
                .detach();
            }
        }
        Some(panel)
    }

    /// The Files tab's "Open in" (`OpenInPicker` on a file): on a server reached over SSH, the
    /// editor's remote URL on this machine; otherwise `shell.openInEditor` on the host.
    fn open_in_editor(&mut self, absolute_path: String, editor: t3code_contracts::EditorId, reveal: bool, cx: &mut Context<Self>) {
        use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
        use crate::remote_open::{RemoteOpenState, build_remote_open_url};
        let Some(target) = self.target.clone() else { return };
        let Some(view) = self.data.thread_view(&target, cx) else { return };
        let setup = self.data.git_controls(&view, cx);
        match setup.open_state {
            RemoteOpenState::RemoteLinks(host) => {
                if let Some(url) = build_remote_open_url(editor, &host.host, host.username.as_deref(), &absolute_path, true) {
                    cx.open_url(&url);
                }
            }
            RemoteOpenState::RemoteUnavailable => {
                Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to open file")).description(t!("No SSH route to this machine")), cx);
            }
            RemoteOpenState::LocalExec => {
                let request = setup.source.open_in_editor(t3code_contracts::LaunchEditorInput {
                    cwd: absolute_path,
                    editor,
                    position: None,
                    project_root: None,
                    reveal: reveal.then_some(true),
                });
                cx.spawn(async move |_, cx| {
                    if let Err(error) = request.await {
                        cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to open file")).description(error.0), cx));
                    }
                })
                .detach();
            }
        }
    }

    /// "Open file in preview browser" (`handleOpenInBrowser`): the file in a browser tab.
    fn open_in_preview(&mut self, absolute_path: String, cx: &mut Context<Self>) {
        use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
        let Some(target) = self.target.clone() else { return };
        let request = self.data.open_file_in_preview(&target, absolute_path, cx);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| match result {
                Ok(tab) => this.open_browser_tab(&tab, cx),
                Err(error) => {
                    Toaster::push(ToastData::new(ToastKind::Error, t!("Unable to open file in browser")).description(error), cx);
                }
            })
            .ok();
        })
        .detach();
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

    pub(crate) fn add_terminal_tab(&mut self, id: String, cx: &mut Context<Self>) {
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
                this.diff_source = input.workspace_root.clone().map(|cwd| DiffSource {
                    cwd,
                    base_ref: input.base_ref.clone(),
                    head_ref: input.head_ref.clone(),
                });
                let capabilities = crate::live::host_menus::target_file_menu(this.target.as_ref(), cx);
                diff.update(cx, |diff, cx| {
                    diff.set_file_menu_capabilities(capabilities, cx);
                    diff.set_input(input, cx);
                });
                this.diff_shown = Some((thread, selection));
                if let Some(path) = this.pending_reveal.take() {
                    diff.update(cx, |diff, cx| diff.reveal_path(&path, cx));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Reads both sides of one file of a git diff so its context expands
    /// (`createGitDiffFileContentsLoader`). Turn diffs have no checkout to read from.
    fn load_file_contents(&mut self, key: SharedString, file: std::sync::Arc<crate::diff_patch::FileDiff>, cx: &mut Context<Self>) {
        use t3code_contracts::{ReviewDiffChangeType, ReviewDiffFileContentsInput, ReviewDiffPreviewSourceKind as Kind};
        let (Some((_, selection)), Some(target), Some(source)) = (self.diff_selection(cx), self.target.clone(), self.diff_source.clone()) else { return };
        let source_kind = match selection {
            DiffPanelSelection::Unstaged => Kind::WorkingTree,
            DiffPanelSelection::Branch { .. } => Kind::BranchRange,
            DiffPanelSelection::Commit { .. } => Kind::Commit,
            _ => {
                crate::components::ui::toast::Toaster::push(
                    crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, t!("Diff file contents are unavailable for this selection.")),
                    cx,
                );
                return;
            }
        };
        let (base_ref, head_ref) = match &selection {
            DiffPanelSelection::Commit { sha, .. } => (None, Some(sha.to_string())),
            _ => (source.base_ref.map(|r| r.to_string()), source.head_ref.map(|r| r.to_string())),
        };
        let input = ReviewDiffFileContentsInput {
            cwd: source.cwd.to_string(),
            source_kind,
            change_type: ReviewDiffChangeType::from_wire(file.change.as_str()).unwrap_or(ReviewDiffChangeType::Change),
            base_ref,
            head_ref,
            old_path: file.prev_path.clone().unwrap_or_else(|| file.path.clone()).to_string(),
            new_path: file.path.to_string(),
        };
        let request = self.data.diff_file_contents(&target, input, cx);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| match result {
                Ok(contents) => {
                    this.contents_loaded.push(key.clone());
                    if let Some((diff, _)) = &this.diff {
                        let contents = crate::diff_patch::FileContents::new(&contents.old_contents, &contents.new_contents);
                        diff.update(cx, |diff, cx| diff.set_file_contents(&key, contents, cx));
                    }
                }
                Err(error) => {
                    let toast = crate::components::ui::toast::ToastData::new(crate::components::ui::toast::ToastKind::Error, t!("Could not load the file contents")).description(error);
                    crate::components::ui::toast::Toaster::push(toast, cx);
                }
            })
            .ok();
        })
        .detach();
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
            DiffPanelEvent::RequestFileContents { key, file } => self.load_file_contents(key.clone(), file.clone(), cx),
            DiffPanelEvent::ReviewComment(comment) => cx.emit(RightPanelEvent::AddReviewComment(review_comment_chip(comment))),
            DiffPanelEvent::DeleteReviewComment(id) => {
                cx.emit(RightPanelEvent::RemoveReviewComment(crate::state::pull_request_mentions::review_comment_context_id(id)))
            }
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
            RightPanelSurface::SideChat { thread_id, .. } => match self.ensure_side_chat_pane(thread_id, window, cx) {
                Some(pane) => div().flex_1().min_h_0().child(pane).into_any_element(),
                None => self.render_empty(surface, cx),
            },
            RightPanelSurface::PullRequest { .. } | RightPanelSurface::PullRequests { .. } | RightPanelSurface::Issue { .. } | RightPanelSurface::Preview { .. } => {
                match self.surface_view(surface, window, cx) {
                    Some(view) => div().flex_1().min_h_0().child(view).into_any_element(),
                    None => self.render_empty(surface, cx),
                }
            }
            _ => self.render_empty(surface, cx),
        }
    }

    /// The side chat's key on the shown thread's environment.
    fn side_chat_key(&self, thread_id: &str) -> Option<crate::components::sidebar::model::ThreadKey> {
        let thread = self.thread.clone()?;
        let (environment, _) = thread.split_once(':')?;
        Some(crate::state::shell::thread_key(&t3code_contracts::EnvironmentId::new(environment), &t3code_contracts::ThreadId::new(thread_id)))
    }

    /// The pane of the side chat tab on screen, built when it first shows (`SideChatPane`).
    fn ensure_side_chat_pane(&mut self, thread_id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<SideChatPane>> {
        let key = self.side_chat_key(thread_id)?;
        if let Some((shown, pane, _)) = &self.side_chat
            && shown == &key
        {
            return Some(pane.clone());
        }
        self.drop_side_chat(cx);
        let data = self.data.clone();
        let pane = cx.new(|cx| SideChatPane::new(data, key.clone(), window, cx));
        let surface_id = format!("side-chat:{thread_id}");
        let subscription = cx.subscribe_in(&pane, window, move |this, _, event: &SideChatPaneEvent, _, cx| match event {
            SideChatPaneEvent::Close => this.close_surface(&surface_id, cx),
            SideChatPaneEvent::AddToChat(reference) => cx.emit(RightPanelEvent::AddThreadContext(reference.clone())),
            // Scripts and terminals open in this panel, the side chat's host (`rightPanelHostRef`).
            SideChatPaneEvent::Window(crate::components::chat::messages_timeline::TimelineAction::RunShellCommand(command)) => {
                this.run_script(chat_code_block(command), cx)
            }
            SideChatPaneEvent::Window(crate::components::chat::messages_timeline::TimelineAction::OpenTerminal(id)) => this.add_terminal_tab(id.to_string(), cx),
            SideChatPaneEvent::Window(action) => cx.emit(RightPanelEvent::SideChatAction(action.clone())),
        });
        self.side_chat = Some((key, pane.clone(), subscription));
        Some(pane)
    }

    /// The side chat tab's pane, while one is on screen.
    pub fn side_chat_pane(&self) -> Option<&Entity<SideChatPane>> {
        self.side_chat.as_ref().map(|(_, pane, _)| pane)
    }

    /// Unmounts the side chat's stream when its tab leaves the screen.
    fn drop_side_chat(&mut self, cx: &mut Context<Self>) {
        if let Some((key, _, _)) = self.side_chat.take() {
            self.data.close_side_chat(&key, cx);
        }
    }

    /// Closes a tab (`useRightPanelSurfaceClose`): a side chat with no run or message is deleted
    /// with it, unless the shell already shows it gone.
    pub fn close_surface(&mut self, surface_id: &str, cx: &mut Context<Self>) {
        let thread = self.thread.clone();
        let side = thread
            .as_ref()
            .map(|t| self.store.read(cx).state(t))
            .and_then(|state| state.surfaces.into_iter().find(|s| s.id() == surface_id))
            .and_then(|surface| match surface {
                RightPanelSurface::SideChat { thread_id, .. } => self.side_chat_key(&thread_id),
                _ => None,
            });
        let id = surface_id.to_owned();
        self.edit(cx, |store, thread| store.close_surface(thread, &id));
        if let Some(key) = side {
            if self.side_chat.as_ref().is_some_and(|(shown, _, _)| shown == &key) {
                self.drop_side_chat(cx);
            }
            if !matches!(self.data.side_chat(&key, cx), crate::state::app_data::SideChatLookup::Deleted) {
                self.data.delete_thread(&key, true, cx).detach();
            }
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
                let menus = crate::live::host_menus::right_panel_menus(self, self.target.clone().as_ref(), cx);
                cx.new(|cx| crate::components::pull_request::pull_request_detail_panel::PullRequestDetailPanel::new(source, reference, cx).with_markdown_menus(menus)).into()
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
                let menus = crate::live::host_menus::right_panel_menus(self, self.target.clone().as_ref(), cx);
                cx.new(|cx| crate::components::issue::issue_detail_panel::IssueDetailPanel::new(source, reference, window, cx).with_markdown_menus(menus)).into()
            }
            RightPanelSurface::PullRequests { .. } => crate::components::pull_request::thread_pull_requests_panel::thread_pull_requests_panel(crate::state::shell::thread_key(&environment, &t3code_contracts::ThreadId::new(thread_id)), self.store.clone(), cx),
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
        // A side chat tab names its side chat and dots its state (`SideChatTabIcon`).
        let side_chat = match surface {
            RightPanelSurface::SideChat { thread_id, .. } => self.side_chat_key(thread_id).map(|key| self.data.side_chat(&key, cx)),
            _ => None,
        };
        let side_facts = match &side_chat {
            Some(crate::state::app_data::SideChatLookup::Found(facts)) => Some(facts.clone()),
            _ => None,
        };
        let title = side_facts.as_ref().map(|f| f.title.clone()).unwrap_or_else(|| surface_title(surface));
        let side_dot = side_facts.as_ref().and_then(|f| f.state).map(|state| match state {
            crate::state::app_data::SideChatState::Running => c.muted_foreground,
            _ => theme.fixed.info,
        });
        let group: SharedString = format!("tab-{id}").into();
        let close = {
            let id = id.clone();
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.close_surface(&id, cx);
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
                    .relative()
                    .group_hover(group.clone(), |s| s.invisible())
                    .child(Icon::new(surface_icon(surface.kind())).size(IconSize::Xs))
                    .children(side_dot.map(|dot| {
                        div().absolute().top(theme.rem(-0.125)).right(theme.rem(-0.125)).size(theme.rem(0.375)).rounded_full().bg(dot)
                    })),
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
                cx.listener(move |this, _: &MouseUpEvent, _, cx| this.close_surface(&id, cx))
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
                .item({
                    let (weak, id) = (weak.clone(), id.clone());
                    MenuItem::new(t!("Close")).on_select(move |_, cx| {
                        weak.update(cx, |this, cx| this.close_surface(&id, cx)).ok();
                    })
                })
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
        // Linked items need a linked pull request or issue (`SURFACE_UNAVAILABLE_HINTS`).
        let linked_items = self.data.palette_source().active_thread_has_linked_items();
        let theme = cx.app_theme();
        let c = &theme.colors;
        let hover = c.accent_surface.opacity(0.6);
        let rows = Launch::ALL.into_iter().enumerate().map(|(index, launch)| {
            let available = launch != Launch::Surface(SingletonKind::PullRequests) || linked_items;
            div()
                .id(("launch", index))
                .h(rems(2.))
                .w_full()
                .flex()
                .items_center()
                .gap(rems(0.625))
                .px(rems(0.625))
                .rounded(theme.radii.control)
                .ui_text(theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.text)
                .child(Icon::new(launch.icon()).size(IconSize::Md).color(c.text))
                .child(div().flex_1().min_w_0().truncate().child(launch.label()))
                .child(crate::components::ui::kbd::Kbd::new(launch.shortcut()))
                .when(!available, |d| d.opacity(0.5))
                .when(available, |d| {
                    d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(move |this, _, _, cx| this.launch(launch, cx)))
                })
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
        // The pane (and its stream) lives while its tab is the active one, hidden panel or not.
        if !matches!(state.active(), Some(RightPanelSurface::SideChat { .. })) {
            self.drop_side_chat(cx);
        }
        let body = match state.active() {
            Some(surface) => self.render_body(surface, window, cx),
            None => self.render_launcher(cx).into_any_element(),
        };
        div().size_full().flex().flex_col().track_focus(&self.focus).child(tab_row).child(body)
    }
}

/// Where the shown git diff was read from.
#[derive(Clone, Debug)]
struct DiffSource {
    cwd: SharedString,
    base_ref: Option<SharedString>,
    head_ref: Option<SharedString>,
}

/// Registers a Diff tab comment for the draft and returns its chip (`addReviewComment`, with
/// `reviewCommentContextLabel` for the chip's label).
fn review_comment_chip(comment: &crate::components::diffs::review_comment::ReviewComment) -> String {
    let context = t3code_client_runtime::state::drafts::ReviewCommentContext {
        id: comment.id.to_string(),
        section_id: comment.section_id.to_string(),
        section_title: comment.section_title.to_string(),
        file_path: comment.file_path.to_string(),
        start_index: comment.start_index.into(),
        end_index: comment.end_index.into(),
        range_label: comment.range_label.to_string(),
        text: comment.text.to_string(),
        diff: comment.diff.to_string(),
        pull_request: None,
    };
    crate::state::pull_request_mentions::register_comment(context, &review_comment_label(&comment.file_path, &comment.range_label, &comment.section_id))
}

/// `reviewCommentContextLabel`: `<basename> L12`, `L12 to L15`, `(before)` for removed lines.
pub fn review_comment_label(file_path: &str, range_label: &str, section_id: &str) -> String {
    let parse = |part: &str| -> Option<(char, String)> {
        let sign = part.chars().next().filter(|c| *c == '+' || *c == '-')?;
        let digits = &part[1..];
        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then(|| (sign, digits.to_string()))
    };
    let range = match range_label.split_once(" to ") {
        Some((a, b)) => match (parse(a), parse(b)) {
            (Some((s1, start)), Some((s2, end))) if s1 == s2 => Some((s1, start, Some(end))),
            _ => None,
        },
        None => parse(range_label).map(|(s, start)| (s, start, None)),
    };
    let range: String = match range {
        Some(('-', start, None)) => t!("L{start} (before)", start = start, context = "line in the old version of a diff").to_string(),
        Some((_, start, None)) => format!("L{start}"),
        Some(('-', start, Some(end))) => t!("L{start} to L{end} (before)", start = start, end = end, context = "line range in the old version of a diff").to_string(),
        Some((_, start, Some(end))) => t!("L{start} to L{end}", start = start, end = end, context = "line range in a diff").to_string(),
        None => range_label.to_string(),
    };
    let prefix = section_id.strip_prefix("pull-request:").filter(|n| n.chars().all(|c| c.is_ascii_digit())).map(|n| format!("#{n} ")).unwrap_or_default();
    let base = file_path.rsplit(['/', '\\']).next().unwrap_or(file_path);
    format!("{prefix}{base} {range}")
}

/// `runShellCommand`: an ad-hoc "Chat code block" script.
fn chat_code_block(command: &SharedString) -> ProjectScript {
    ProjectScript {
        id: "chat-code-block".into(),
        name: t!("Chat code block").to_string(),
        command: command.to_string(),
        icon: t3code_contracts::ProjectScriptIcon::Play,
        run_on_worktree_create: false,
        run_on_settle: None,
        stop_on_settle: None,
        r#async: None,
        preview_url: None,
        auto_open_preview: None,
    }
}
