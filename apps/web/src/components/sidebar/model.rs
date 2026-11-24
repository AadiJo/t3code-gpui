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
    /// The provider instance the thread runs on, when the client knows it.
    pub provider: Option<SidebarProvider>,
}

/// The provider mark a sidebar row shows (`SidebarProviderStack`).
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarProvider {
    pub mark: SidebarProviderMark,
    /// `providerInstanceInitials(displayName)`, drawn in the instance badge.
    pub initials: SharedString,
    /// `shouldShowInstanceBadge`: another instance shares this driver.
    pub show_badge: bool,
}

/// Which glyph a provider instance draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarProviderMark {
    /// A bundled provider or model mark.
    Brand(crate::model_selection::ProviderIcon),
    /// An ACP registry agent whose icon has not loaded: the registry's own mark.
    AcpRegistry,
}

impl SidebarThread {
    /// Whether the row offers Snooze (`canSnooze`). The row does not carry the pending
    /// runtime request kind or a queued turn start, so those two gates are not applied.
    pub fn can_snooze(&self) -> bool {
        !self.has_pending_approvals && self.pending_subagent_request.is_none() && self.pending_guest_message_count == 0
    }

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
    /// `repositoryGroupingDisplayNameOf`: the origin's display name (or key), else the identity's.
    pub repository_display_name: Option<SharedString>,
    /// The repository identity's `name`.
    pub repository_name: Option<SharedString>,
    /// The user renamed the project, so a group keeps its title.
    pub has_custom_title: bool,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    pub pinned: bool,
    pub archived: bool,
    /// The name its group shows when the data source derives it (`deriveProjectGroupLabel`).
    pub group_label: Option<SharedString>,
    /// The project's icon override (`projectIcon`).
    pub icon: Option<t3code_contracts::OrchestrationProjectShellProjectIcon>,
    /// The project's fetched favicon image on disk, once loaded.
    pub favicon: Option<std::sync::Arc<std::path::Path>>,
}

/// Sidebar-affecting client settings.
#[derive(Clone, Copy, Debug)]
pub struct SidebarSettings {
    /// "Boring sidebar": active threads sit in project folders.
    pub project_folders: bool,
    /// Folder rows add the card's branch and PR line (`boringSidebarMultilineRows`).
    pub folder_rows_multiline: bool,
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
            folder_rows_multiline: false,
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

/// One unsent new-thread draft: a draft session whose composer has content.
#[derive(Clone, Debug, PartialEq)]
pub struct SidebarDraft {
    pub id: SharedString,
    pub project: ProjectRef,
    /// First line of the prompt, or "N attachments".
    pub preview: SharedString,
    pub created_at: TimestampMs,
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
    /// Threads whose auto-settle is off (`autoSettleDisabledAt` set), for the
    /// "Auto-settle behavior" menu. Empty means every thread follows the settings.
    pub auto_settle_disabled: std::collections::HashSet<ThreadKey>,
    /// Unsent new-thread drafts (`SidebarDraftBlock`), newest first or in any order.
    pub drafts: Vec<SidebarDraft>,
    /// The draft the main pane shows.
    pub active_draft: Option<SharedString>,
    /// Per-thread facts for the hover card (environment, model, hand-offs).
    /// A thread without an entry shows only what its row carries.
    pub hover_details: std::collections::HashMap<ThreadKey, crate::components::thread_hover_card::ThreadHoverDetails>,
    /// Threads the server found by their messages for a search (`useThreadSearch`), in its order.
    pub search_content_matches: Option<SearchContentMatches>,
    /// Each environment's snooze menu settings (`ServerSettings.snoozePresets`); an environment
    /// without an entry uses the defaults. Resolve a menu with
    /// `state::sidebar_actions::resolve_snooze_presets` when it opens.
    pub snooze_presets: Arc<std::collections::HashMap<EnvironmentId, Arc<[t3code_contracts::SnoozePresetSetting]>>>,
    /// Older settled threads the servers leave out of the shell (`useOlderSettledShelf`).
    pub older_settled: OlderSettledShelf,
}

/// Where the Settled shelf stands when servers leave older settled threads out: the shelf lists
/// a row only at or above `frontier` (rows below it could sort under rows not loaded yet), and
/// its header counts what is not loaded on top of the listed rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OlderSettledShelf {
    pub frontier: Option<SettledFrontier>,
    /// Per project: the server's count of older settled threads minus the older rows held.
    pub unloaded: std::collections::HashMap<ProjectRef, i64>,
    /// A requested page is loading ("Loading").
    pub loading: bool,
    /// A requested page failed ("Could not load older threads. Retry").
    pub failed: bool,
}

/// A position in the servers' order of settled threads (settle time desc, id asc).
#[derive(Clone, Debug, PartialEq)]
pub struct SettledFrontier {
    pub settled_at: TimestampMs,
    pub thread_id: Option<SharedString>,
}

impl OlderSettledShelf {
    /// Whether the shelf may list `thread` (`isListedSettledThread`).
    pub fn is_listed(&self, thread: &SidebarThread) -> bool {
        let Some(frontier) = &self.frontier else { return true };
        let Some(settled_at) = thread.settled_at else { return true };
        !(settled_at < frontier.settled_at
            || frontier.thread_id.as_ref().is_some_and(|id| settled_at == frontier.settled_at && thread.key.thread_id.0.as_ref() > id.as_ref()))
    }

    /// Older settled threads not held, in `projects` (all when `None`) (`unloadedOlderSettledCount`).
    pub fn unloaded_count(&self, projects: Option<&std::collections::HashSet<ProjectRef>>) -> usize {
        let total: i64 = self.unloaded.iter().filter(|(project, _)| projects.is_none_or(|set| set.contains(*project))).map(|(_, n)| n).sum();
        total.max(0) as usize
    }
}

/// What the Settled shelf shows, so the data reads older pages until it lists `visible_count`
/// rows (`useOlderSettledShelf` inputs). `retry` reads a failed page again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettledShelfRequest {
    pub expanded: bool,
    pub visible_count: usize,
    /// The project filter's projects; `None` for all.
    pub projects: Option<Vec<ProjectRef>>,
    pub retry: bool,
}

/// Server message matches for one search query.
#[derive(Clone, Debug, Default)]
pub struct SearchContentMatches {
    pub query: SharedString,
    pub threads: std::sync::Arc<Vec<ThreadKey>>,
}
