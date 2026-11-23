//! The sidebar's view model: the typed data the sidebar renders. The runtime's
//! shell state maps onto these structs (one [`SidebarThread`] per
//! `EnvironmentThreadShell`, one [`SidebarProject`] per environment project);
//! everything derived from them lives in `sidebar_logic`.

use std::sync::Arc;

use gpui_kit::SharedString;

/// Milliseconds since the Unix epoch. The shell's ISO strings parse to this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimestampMs(pub i64);

impl TimestampMs {
    pub const SECOND: i64 = 1_000;
    pub const MINUTE: i64 = 60_000;
    pub const HOUR: i64 = 3_600_000;
    pub const DAY: i64 = 86_400_000;

    /// `self` moved back by `ms`.
    pub fn minus(self, ms: i64) -> Self {
        Self(self.0 - ms)
    }
}

/// One T3 server. Ids are opaque strings.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnvironmentId(pub SharedString);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(pub SharedString);

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadId(pub SharedString);

/// A thread's identity across environments (ids repeat between servers).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ThreadKey {
    pub environment_id: EnvironmentId,
    pub thread_id: ThreadId,
}

/// A project's identity across environments.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectRef {
    pub environment_id: EnvironmentId,
    pub project_id: ProjectId,
}

/// `OrchestrationV2RunStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Preparing,
    Queued,
    Starting,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
    Cancelled,
    RolledBack,
}

/// The shell runtime summary's status: `idle` (background work holds
/// completion) or a run status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeStatus {
    Idle,
    Run(RunStatus),
}

/// Why the last run failed, as far as the sidebar cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    UsageLimit,
    Other,
}

/// `ThreadRuntimeSummary`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadRuntime {
    pub status: RuntimeStatus,
    pub last_error_class: Option<ErrorClass>,
}

/// `ThreadRunSummary`: the latest run's status and times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatestRun {
    pub status: RunStatus,
    pub requested_at: Option<TimestampMs>,
    pub started_at: Option<TimestampMs>,
    pub completed_at: Option<TimestampMs>,
}

/// A request rolled up from a child thread (`pendingSubagentRequest`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentRequest {
    Approval,
    UserInput,
}

/// `settledOverride`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettledOverride {
    Settled,
    Active,
}

/// The pull request linked to a thread, as the row badge shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequestBadge {
    pub number: u32,
    pub state: PullRequestState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullRequestState {
    Open,
    Draft,
    Merged,
    Closed,
}

/// Lines added and removed by the latest run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffStat {
    pub insertions: u32,
    pub deletions: u32,
}

/// One sidebar thread row's data (`SidebarThreadSummary`).
#[derive(Clone, Debug)]
pub struct SidebarThread {
    pub key: ThreadKey,
    pub project_id: ProjectId,
    pub title: SharedString,
    pub branch: Option<SharedString>,
    /// The thread runs in a git worktree rather than the project checkout.
    pub in_worktree: bool,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    pub latest_user_message_at: Option<TimestampMs>,
    pub latest_user_authored_message_at: Option<TimestampMs>,
    pub runtime: Option<ThreadRuntime>,
    pub latest_run: Option<LatestRun>,
    pub blocked_on_thread_wait: bool,
    pub has_pending_approvals: bool,
    pub has_pending_user_input: bool,
    pub pending_subagent_request: Option<SubagentRequest>,
    pub pending_guest_message_count: u32,
    /// The run is working toward a goal ("Goal" instead of "Working").
    pub goal_active: bool,
    /// Resolved visit watermark: the server value, or the local fallback on
    /// servers that predate visit tracking (`resolveThreadLastVisitedAt`).
    pub last_visited_at: Option<TimestampMs>,
    /// A desktop completion notification for this thread is unread.
    pub has_unread_completion_notification: bool,
    pub settled_override: Option<SettledOverride>,
    pub settled_at: Option<TimestampMs>,
    pub unsettled_at: Option<TimestampMs>,
    pub snoozed_until: Option<TimestampMs>,
    /// When a snoozed thread woke (timer or raised hand), if it has.
    pub woke_at: Option<TimestampMs>,
    pub pinned_at: Option<TimestampMs>,
    pub pin_order_key: Option<SharedString>,
    pub active_order_key: Option<SharedString>,
    pub archived: bool,
    pub has_unsent_draft: bool,
    pub diff: Option<DiffStat>,
    pub pull_request: Option<PullRequestBadge>,
    pub issue_key: Option<SharedString>,
    pub running_terminal_count: u32,
    /// The thread lives on a machine other than this one.
    pub remote_machine: bool,
}

impl SidebarThread {
    pub fn project_ref(&self) -> ProjectRef {
        ProjectRef {
            environment_id: self.key.environment_id.clone(),
            project_id: self.project_id.clone(),
        }
    }
}

/// How projects that share a repository combine into one sidebar group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectGroupingMode {
    #[default]
    Repository,
    RepositoryPath,
    Separate,
}

/// How project groups are ordered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProjectSortOrder {
    #[default]
    UpdatedAt,
    CreatedAt,
    Manual,
}

/// One environment project (`EnvironmentProject`).
#[derive(Clone, Debug)]
pub struct SidebarProject {
    pub project_ref: ProjectRef,
    pub title: SharedString,
    pub workspace_root: SharedString,
    /// Canonical repository identity, once the server has resolved it.
    pub repository_key: Option<SharedString>,
    /// The project's root relative to its repository root, if known.
    pub repository_relative_path: Option<SharedString>,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    pub pinned: bool,
    pub archived: bool,
}

/// Sidebar-affecting client settings.
#[derive(Clone, Copy, Debug)]
pub struct SidebarSettings {
    /// "Boring sidebar": active threads sit in project folders.
    pub project_folders: bool,
    /// Working threads move to a Working shelf at the bottom.
    pub working_shelf: bool,
    pub grouping_mode: ProjectGroupingMode,
    pub project_sort_order: ProjectSortOrder,
    /// Fork: show the bell and notification inbox in the footer.
    pub notification_inbox: bool,
    /// The environment supports settle / snooze / pin.
    pub settlement_supported: bool,
    pub snooze_supported: bool,
    pub pinning_supported: bool,
    /// Settled rows shown before "Show N more".
    pub settled_page_size: usize,
}

impl Default for SidebarSettings {
    fn default() -> Self {
        Self {
            project_folders: false,
            working_shelf: true,
            grouping_mode: ProjectGroupingMode::Repository,
            project_sort_order: ProjectSortOrder::UpdatedAt,
            notification_inbox: true,
            settlement_supported: true,
            snooze_supported: true,
            pinning_supported: true,
            settled_page_size: 10,
        }
    }
}

/// What a notification-inbox entry is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    Approval,
    Input,
    Failed,
    Completed,
}

/// One entry in the fork's notification inbox.
#[derive(Clone, Debug)]
pub struct SidebarNotification {
    pub id: SharedString,
    pub kind: NotificationKind,
    pub thread: ThreadKey,
    pub title: SharedString,
    pub project_label: SharedString,
    pub environment_label: SharedString,
    pub resolved: bool,
    pub unread: bool,
    pub at: TimestampMs,
}

/// Everything the sidebar renders. Rebuild it when the shell changes; the
/// sidebar derives rows from it once per change, not per frame.
#[derive(Clone, Debug, Default)]
pub struct SidebarSnapshot {
    pub projects: Vec<SidebarProject>,
    pub threads: Vec<Arc<SidebarThread>>,
    pub notifications: Vec<SidebarNotification>,
    pub settings: SidebarSettings,
    /// The thread the main pane shows.
    pub active_thread: Option<ThreadKey>,
    /// The environment label pill in the header, when that mode is on.
    pub environment_pill: Option<SharedString>,
    /// The clock the snapshot was taken against.
    pub now: TimestampMs,
}
