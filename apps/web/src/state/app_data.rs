//! The window's one data source. [`AppRoot`](crate::AppRoot), the thread route view and the
//! right panel read threads, run commands and reach each panel's data through [`AppData`];
//! [`FixtureData`](crate::fixtures::FixtureData) answers from the in-memory fixtures (checks,
//! scenes, the parity tool) and [`LiveData`](crate::live::LiveData) from the environments'
//! stores and RPC sessions. Changes reach the views as [`DataEvent`]s on [`AppData::events`].

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Entity, EventEmitter, SharedString, Task, WeakEntity};
use t3code_contracts::{EditorId, EnvironmentId, ProjectScript};

use crate::components::issue::issues_page::EnvironmentIssueSource;
use crate::components::preview::preview_source::PreviewSource;
use crate::components::pull_request::pull_request_source::PullRequestDetailSource;
use crate::components::pull_request::pull_requests_page::EnvironmentListSource;
use crate::components::settings::server_settings_data::ServerPanelsModel;
use crate::components::usage::usage_source::UsageSource;

use crate::components::chat::composer::{ChatComposer, ComposerEvent, ComposerState};
use crate::components::chat::thread_details_panel::{ThreadDetailsEvent, ThreadDetailsInput};
use crate::components::chat::composer_command_menu::ComposerMenuSource;
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::chat::timeline_entries::{MessageId, PendingQuestion, TimelineInput};
use crate::components::command_palette_lookups::CommandPaletteSource;
use crate::components::diff_panel::DiffPanelInput;
use crate::components::branch_toolbar::{BranchToolbarEvent, BranchToolbarProps};
use crate::components::git_actions_control::GitActionsProps;
use crate::components::git_actions_control_source::GitActionsSource;
use crate::remote_open::RemoteOpenState;
use crate::components::right_panel_tabs::{TerminalBackends, Workspace};
use crate::components::sidebar::context_menu::MenuCommand;
use crate::components::sidebar::model::{ProjectId, ProjectRef, SidebarSnapshot, ThreadKey};
use crate::composer_draft_store::ComposerDraftStore;
use crate::diff_panel_store::DiffPanelSelection;
use crate::router::{DraftId, ThreadTarget};

/// What the chat view needs to show one thread.
#[derive(Clone)]
pub struct ThreadView {
    /// `environmentId:threadId` for server threads, `draft:<id>` for drafts.
    pub key: SharedString,
    pub project: SharedString,
    pub title: SharedString,
    pub cwd: SharedString,
    pub timeline: TimelineInput,
    pub is_draft: bool,
    pub running: bool,
    /// Every question of the pending request; `timeline.pending_question` is the first.
    pub questions: Arc<[PendingQuestion]>,
    /// The branch the thread works on.
    pub branch: Option<SharedString>,
    /// The project, for per-project panels (Files) and the git controls.
    pub project_id: Option<ProjectId>,
    /// The project's icon override and fetched favicon, for the header breadcrumb.
    pub project_icon: Option<crate::components::project_favicon::ProjectIconSource>,
}

/// A change the views apply.
#[derive(Clone, Debug)]
pub enum DataEvent {
    /// The sidebar rows changed; re-read [`AppData::snapshot`].
    SnapshotChanged,
    /// A thread's timeline or state changed; re-read [`AppData::thread_view`].
    ThreadChanged(ThreadTarget),
    /// Streamed message text grew: `(message, text, still streaming)`.
    Text { thread: ThreadKey, changes: Arc<[(MessageId, SharedString, bool)]> },
    /// A draft got its first message and is now a server thread.
    DraftPromoted { draft: DraftId, thread: ThreadKey },
    /// The server refused the launch `DraftPromoted` announced: the draft is back.
    DraftLaunchFailed { draft: DraftId, thread: ThreadKey },
    /// Show this thread (a fork or side chat that just arrived).
    OpenThread(ThreadKey),
}

/// The emitter every [`AppData`] publishes on. It also owns the source's subscriptions.
#[derive(Default)]
pub struct DataEvents {
    pub(crate) _subscriptions: Vec<gpui_kit::Subscription>,
    pub(crate) _tasks: Vec<Task<()>>,
}

impl EventEmitter<DataEvent> for DataEvents {}

/// What the route view does after a timeline button ran.
#[derive(Clone, Debug)]
pub enum ActionOutcome {
    Done,
    /// Show this thread (a fork was created).
    OpenThread(ThreadKey),
    /// Put this text in the composer (edit from here, edit a queued message).
    LoadComposer(SharedString),
    /// Ask "Edit from here?" before rewinding to `turn_count`; `restore_files` says whether
    /// "Revert files too" is offered.
    ConfirmRevert { message: MessageId, turn_count: usize, restore_files: bool },
}

/// What the route view does after a thread details card event ran.
#[derive(Clone, Debug)]
pub enum DetailsOutcome {
    Done,
    /// Show this browser tab (a server opened in the preview).
    OpenBrowser(SharedString),
    /// Put this task in the composer (Resolve, Fix).
    LoadComposer(SharedString),
}

/// What "Edit from here" puts back in the composer: the message's text and fresh copies of its
/// attachments (`rewindToMessage`).
pub struct RevertedDraft {
    pub prompt: SharedString,
    pub attachments: Vec<crate::composer_draft_store::ComposerAttachment>,
}

/// What a sidebar thread command left for the app to do.
#[derive(Clone, Debug)]
pub enum CommandOutcome {
    Done,
    /// Ask for a new title.
    Rename(ThreadKey),
    /// Start a new thread in this project.
    NewThread(Option<ProjectRef>),
}

/// What a thread's source control controls are built from.
pub struct GitControlsSetup {
    pub source: Arc<dyn GitActionsSource>,
    pub git: GitActionsProps,
    pub scripts: Vec<ProjectScript>,
    pub editors: Vec<EditorId>,
    pub open_state: RemoteOpenState,
    /// The checkout is a git repository the session may run git in.
    pub is_git_repo: bool,
}

/// The window's data. See the module docs.
pub trait AppData: 'static {
    /// Where [`DataEvent`]s arrive.
    fn events(&self) -> Entity<DataEvents>;

    /// Whether there is anything to show; without it the window shows the first-run state.
    fn has_data(&self, cx: &App) -> bool;

    /// The sidebar's rows with `active` marked.
    fn snapshot(&self, active: Option<&ThreadKey>, cx: &App) -> SidebarSnapshot;

    /// The most recently updated project (`_chat.index.tsx` starts a draft there).
    fn most_recent_project(&self, cx: &App) -> Option<ProjectRef>;

    /// Whether `target` names a thread or draft this source knows.
    fn thread_exists(&self, target: &ThreadTarget, cx: &App) -> bool;

    /// Everything the chat view shows for `target`. On live data the first call for a thread
    /// opens it (mounts its stream); later calls re-read it.
    fn thread_view(&self, target: &ThreadTarget, cx: &mut App) -> Option<ThreadView>;

    /// Starts a draft thread in `project`.
    fn create_draft(&self, project: Option<ProjectRef>, cx: &mut App) -> DraftId;

    /// Forgets a draft the sidebar discarded.
    fn discard_draft(&self, _key: &ThreadKey, _cx: &mut App) {}

    /// The composer's state for `view`, starting from the settings in `base`.
    fn composer_state(&self, view: &ThreadView, base: &ComposerState, cx: &mut App) -> ComposerState;

    /// The `@`, `$` and `/` menu for `view`'s workspace; it refreshes `composer` when results land.
    fn menu_source(&self, view: &ThreadView, composer: &Entity<ChatComposer>, cx: &mut App) -> Rc<dyn ComposerMenuSource>;

    /// Where composer drafts persist.
    fn draft_store(&self, cx: &mut App) -> Rc<RefCell<dyn ComposerDraftStore>>;

    /// Runs a composer event on `target`. Returns the server thread a draft became.
    fn composer_event(
        &self,
        target: &ThreadTarget,
        event: &ComposerEvent,
        composer: &WeakEntity<ChatComposer>,
        cx: &mut App,
    ) -> Option<ThreadKey>;

    /// Runs a timeline button (approvals, fork, edit from here, the queue) on `key`.
    fn timeline_action(&self, key: &ThreadKey, action: &TimelineAction, cx: &mut App) -> ActionOutcome;

    /// Rewinds to before `message` ("Edit from here" confirmed); resolves with the prompt and
    /// attachments to put back in the composer.
    fn revert(&self, _key: &ThreadKey, _message: &MessageId, _turn_count: usize, _restore_files: bool, _cx: &mut App) -> Task<Option<RevertedDraft>> {
        Task::ready(None)
    }

    /// Answers every question of the pending request: `answers[i]` is question `i`'s option.
    fn answer_questions(&self, key: &ThreadKey, answers: &[usize], cx: &mut App);

    /// The thread moved to `branch` (the branch toolbar or a git action).
    fn set_thread_branch(&self, key: &ThreadKey, branch: SharedString, cx: &mut App);

    /// A sidebar thread command (pin, rename, archive...).
    fn thread_command(&self, keys: &[ThreadKey], command: MenuCommand, cx: &mut App) -> CommandOutcome;

    /// The sidebar search changed; a live source asks the servers for message matches.
    fn search_threads(&self, _query: SharedString, _cx: &mut App) {}

    /// The Settled shelf's state: a live source reads older settled pages from servers that
    /// leave them out of the shell until the shelf lists `visible_count` rows, and retries a
    /// failed page when asked. The snapshot's `older_settled` reports where it stands.
    fn show_older_settled(&self, _request: crate::components::sidebar::model::SettledShelfRequest, _cx: &mut App) {}

    /// Renames a thread.
    fn rename_thread(&self, key: &ThreadKey, title: String, cx: &mut App);

    /// The Diff tab's input for `target` and the scope it shows.
    fn diff_input(&self, target: &ThreadTarget, selection: &DiffPanelSelection, cx: &mut App) -> Task<Option<DiffPanelInput>>;

    /// The project workspace the Files tab of `target` reads.
    fn workspace(&self, target: &ThreadTarget, cx: &mut App) -> Option<Workspace>;

    /// The git, branch, scripts and editor controls of `view`, built when it first shows.
    fn git_controls(&self, view: &ThreadView, cx: &mut App) -> GitControlsSetup;

    /// The branch toolbar's props for `view`; re-read whenever the thread changes.
    fn toolbar_props(&self, view: &ThreadView, cx: &mut App) -> BranchToolbarProps;

    /// The breadcrumb's project icon source for `target`; re-read when the snapshot changes, as
    /// a favicon can load after the thread shows.
    fn project_icon(&self, _target: &ThreadTarget, _cx: &mut App) -> Option<crate::components::project_favicon::ProjectIconSource> {
        None
    }

    /// A branch toolbar choice on a draft (workspace mode, base branch, start from origin): the
    /// draft keeps it for its first send and emits [`DataEvent::ThreadChanged`].
    fn draft_workspace_event(&self, _draft: &DraftId, _event: &BranchToolbarEvent, _cx: &mut App) {}

    /// The details card's server facts for `view` (pull request, issues, servers, automations,
    /// context window) on top of `base`; re-read whenever the thread changes.
    fn thread_details(&self, _view: &ThreadView, base: ThreadDetailsInput, _cx: &mut App) -> ThreadDetailsInput {
        base
    }

    /// Runs a details card event on `key` that needs the server. Navigation stays with the view.
    fn thread_details_event(&self, _key: &ThreadKey, _event: &ThreadDetailsEvent, _cx: &mut App) -> Task<DetailsOutcome> {
        Task::ready(DetailsOutcome::Done)
    }

    /// How terminal tabs and the drawer start sessions.
    fn terminal_backends(&self, cx: &mut App) -> TerminalBackends;

    /// The command palette's data.
    fn palette_source(&self) -> Rc<dyn CommandPaletteSource>;

    /// The main pane shows `active` now.
    fn set_active_thread(&self, active: Option<&ThreadKey>, cx: &mut App);

    /// The pull requests page's environments.
    fn pull_request_sources(&self, cx: &mut App) -> Vec<EnvironmentListSource>;

    /// One environment's pull request detail (the page's detail and the panel tab).
    fn pull_request_detail(&self, environment: &EnvironmentId, cx: &mut App) -> Option<Rc<dyn PullRequestDetailSource>>;

    /// The issues page's environments.
    fn issue_sources(&self, cx: &mut App) -> Vec<EnvironmentIssueSource>;

    /// The usage page's data.
    fn usage_source(&self, cx: &mut App) -> Rc<dyn UsageSource>;

    /// The server settings pages' model, for the environment the window shows.
    fn server_settings(&self, cx: &mut App) -> Option<Entity<ServerPanelsModel>>;

    /// One environment's streamed browser preview.
    fn preview_source(&self, environment: &EnvironmentId, cx: &mut App) -> Option<Rc<dyn PreviewSource>>;
}

/// The handle views hold.
pub type SharedAppData = Rc<dyn AppData>;
