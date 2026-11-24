//! The thread data the timeline derives its rows from: messages, work-log
//! entries, plans and lifecycle events in timeline order, plus the run state.
//! Mirrors `TimelineEntry` / `WorkLogEntry` of `session-logic.ts` with a typed
//! payload per tool kind. The client runtime's thread projection maps onto
//! [`TimelineInput`]; fixtures build it directly.

use std::sync::Arc;

use gpui_kit::SharedString;

macro_rules! id_type {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub SharedString);

        impl $name {
            pub fn new(id: impl Into<SharedString>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    )*};
}

id_type!(
    /// A timeline entry.
    EntryId,
    /// A run (one turn of the agent).
    RunId,
    /// A chat message.
    MessageId,
    /// One attempt within a run; a steer supersedes the attempt it cut off.
    AttemptId,
    /// A pending approval or user-input request.
    RequestId,
);

/// Milliseconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    pub fn millis_since(self, earlier: Timestamp) -> i64 {
        (self.0 - earlier.0).max(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

/// Why a user message was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InputIntent {
    #[default]
    Prompt,
    /// Sent while the agent worked, to redirect it.
    Steer,
    /// A queued message promoted to a steer.
    PromotedQueuedToSteer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Image,
    Video,
    File,
}

/// A file sent with a user message.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    pub name: SharedString,
    pub kind: AttachmentKind,
    /// An image source GPUI can load (path or URL).
    pub source: SharedString,
}

impl Attachment {
    /// What `img` loads: a file for an absolute path (live attachments in the image cache), the
    /// bundled asset or URL otherwise.
    pub fn image_source(&self) -> gpui_kit::ImageSource {
        let path = std::path::Path::new(self.source.as_ref());
        if path.is_absolute() { path.to_path_buf().into() } else { self.source.clone().into() }
    }
}

/// A user or assistant message.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub id: MessageId,
    pub role: MessageRole,
    pub text: SharedString,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// The reply is still streaming in.
    pub streaming: bool,
    pub run_id: Option<RunId>,
    pub attachments: Arc<[Attachment]>,
    pub input_intent: InputIntent,
    /// "Sent by T3 Code" (automations, scheduled tasks).
    pub sent_by_app: bool,
    /// A guest author's name; their messages do not group with yours.
    pub guest: Option<SharedString>,
    /// Sent by another agent (`createdBy: "agent"` from a thread, not the
    /// server): the message folds to one "Agent" line.
    pub from_agent: Option<AgentSender>,
}

/// The agent behind a message it sent into this thread.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSender {
    /// The sending thread's title (`linkedSender.title`); `None` shows "Agent".
    pub title: Option<SharedString>,
}

/// Lifecycle of a tool call (`toolLifecycleStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
    Stopped,
}

/// One file a change touched.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    pub path: SharedString,
    pub additions: u32,
    pub deletions: u32,
    /// `create`, `update`, `delete`, `rename`.
    pub operation: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    pub path: SharedString,
    pub line: u32,
    pub column: u32,
    pub preview: SharedString,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebResult {
    pub title: SharedString,
    pub url: SharedString,
    pub snippet: SharedString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoState {
    Pending,
    Running,
    Completed,
}

/// What kind of tool a dynamic (MCP or app) call is, for its icon and summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynamicToolKind {
    Mcp,
    Browser,
    Device,
    PullRequest,
    ThreadCreate,
    JsRepl,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalKind {
    Command,
    FileChange,
    FileRead,
    Permission,
    AppAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionStatus {
    Running,
    Completed,
    Failed,
}

/// The typed payload of a work-log entry, one variant per tool kind.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkItem {
    Command { command: SharedString, output: Option<SharedString>, exit_code: Option<i32> },
    FileChange { changes: Arc<[FileChange]>, raw_diff: Option<SharedString> },
    FileRead { paths: Arc<[SharedString]>, output: Option<SharedString> },
    FileSearch { pattern: SharedString, matches: Arc<[SearchMatch]> },
    WebSearch { queries: Arc<[SharedString]>, results: Arc<[WebResult]> },
    DynamicTool {
        kind: DynamicToolKind,
        tool: SharedString,
        title: SharedString,
        args: Arc<[(SharedString, SharedString)]>,
        output: Option<SharedString>,
    },
    Reasoning { text: SharedString },
    TodoList { steps: Arc<[(TodoState, SharedString)]> },
    Approval { request_id: RequestId, kind: ApprovalKind, detail: SharedString, reason: Option<SharedString> },
    QuestionAnswer { pairs: Arc<[(SharedString, SharedString)]> },
    SystemNotice { info: bool, text: SharedString },
    Notification { text: SharedString },
    Error { message: SharedString, usage_limit_retry: Option<SharedString>, retryable: bool },
    Compaction { status: CompactionStatus, tokens_before: Option<u64>, tokens_after: Option<u64> },
    ViewedImage { path: SharedString },
}

/// One work-log entry: a tool call, thought, notice or error.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkLogEntry {
    pub id: EntryId,
    pub run_id: Option<RunId>,
    pub created_at: Timestamp,
    pub status: ToolStatus,
    /// The provider's label, used when nothing better is derived.
    pub label: SharedString,
    pub item: WorkItem,
    /// Set when a steer superseded the attempt this entry belongs to.
    pub superseded_attempt: Option<AttemptId>,
    /// The web's presentation of the turn item this entry came from (labels, action,
    /// failure, summaries). Live entries carry it; fixtures leave it `None`.
    pub source: Option<std::sync::Arc<crate::work_entry_labels::WorkSource>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentStatus {
    Queued,
    Running,
    Waiting,
    Completed,
    Failed,
    Idle,
    Stopped,
}

impl SubagentStatus {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Waiting)
    }
}

/// A child thread the agent launched.
#[derive(Clone, Debug, PartialEq)]
pub struct Subagent {
    pub id: SharedString,
    pub run_id: Option<RunId>,
    pub provider_turn_id: Option<SharedString>,
    pub title: SharedString,
    pub detail: Option<SharedString>,
    pub status: SubagentStatus,
    /// Model label drawn in the avatar.
    pub model: SharedString,
    pub elapsed_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DividerKind {
    RunInterrupted,
    Forked,
    ParentLink,
    NewSession,
    Handoff,
}

/// A system divider across the timeline (`TimelineSystemDivider`).
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub kind: DividerKind,
    pub run_id: Option<RunId>,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// A clickable divider (open the parent or fork) shows as a bordered pill.
    pub action: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TimelineEvent {
    Subagent(Subagent),
    Divider(Divider),
}

impl TimelineEvent {
    pub fn run_id(&self) -> Option<&RunId> {
        match self {
            Self::Subagent(s) => s.run_id.as_ref(),
            Self::Divider(d) => d.run_id.as_ref(),
        }
    }
}

/// A plan the agent proposed.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposedPlan {
    pub run_id: Option<RunId>,
    pub title: SharedString,
    pub markdown: SharedString,
}

/// Setup of a new worktree before the agent starts (`WorktreeSetupSnapshot`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorktreeSetup {
    pub phase: SetupPhase,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub branch: Option<SharedString>,
    pub base_ref: Option<SharedString>,
    pub worktree_path: Option<SharedString>,
    pub setup_script: Option<SetupScript>,
    pub stages: Arc<[SetupStage]>,
    /// Why setup failed, shown under a failed setup.
    pub error: Option<SharedString>,
    /// The agent started and the run is under way: the row folds into one summary line
    /// (`setupHandedOff`). Set by the timeline when it places the row.
    pub embedded: bool,
}

/// `WorktreeSetupPhase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPhase {
    Running,
    Done,
    Failed,
    Cancelled,
}

/// `WorktreeSetupStageStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupStageStatus {
    Pending,
    Running,
    Done,
    Skipped,
    Warning,
    Failed,
}

/// `WorktreeSetupStageId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupStageId {
    Fetch,
    Checkout,
    Submodules,
    SetupScript,
    Agent,
}

/// One setup stage. `label` is already the script's name for the setup script stage.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupStage {
    pub id: SetupStageId,
    pub label: SharedString,
    pub status: SetupStageStatus,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub percent: Option<u64>,
    pub detail: Option<SharedString>,
    /// The script's last output lines.
    pub tail: Arc<[SharedString]>,
}

/// The project's setup script the setup runs.
#[derive(Clone, Debug, PartialEq)]
pub struct SetupScript {
    pub name: SharedString,
    pub command: SharedString,
    pub terminal_id: SharedString,
}

impl WorktreeSetup {
    /// `worktreeSetupAgentStarted`: the agent stage is done.
    pub fn agent_started(&self) -> bool {
        self.stages.iter().any(|stage| stage.id == SetupStageId::Agent && stage.status == SetupStageStatus::Done)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryKind {
    Message(ChatMessage),
    Work(WorkLogEntry),
    ProposedPlan(ProposedPlan),
    Event(TimelineEvent),
    /// Agent `send_to_user` text.
    SentMessage { run_id: Option<RunId>, text: SharedString },
    /// A sandboxed HTML render the agent produced.
    HtmlRender { run_id: Option<RunId>, title: SharedString },
    /// An MCP app frame.
    McpApp { run_id: Option<RunId>, title: SharedString },
}

/// One entry of the thread timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineEntry {
    pub id: EntryId,
    pub created_at: Timestamp,
    pub kind: EntryKind,
}

impl TimelineEntry {
    /// The run an entry belongs to (`timelineEntryRunId`).
    pub fn run_id(&self) -> Option<&RunId> {
        match &self.kind {
            EntryKind::Message(m) if m.role == MessageRole::Assistant => m.run_id.as_ref(),
            EntryKind::Message(_) => None,
            EntryKind::Work(w) => w.run_id.as_ref(),
            EntryKind::ProposedPlan(p) => p.run_id.as_ref(),
            EntryKind::SentMessage { run_id, .. }
            | EntryKind::HtmlRender { run_id, .. }
            | EntryKind::McpApp { run_id, .. } => run_id.as_ref(),
            EntryKind::Event(_) => None,
        }
    }

    /// When the entry ended: a message's last update, else its creation.
    pub fn end(&self) -> Timestamp {
        match &self.kind {
            EntryKind::Message(m) => m.updated_at,
            _ => self.created_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Interrupted,
}

/// The latest run of the thread.
#[derive(Clone, Debug, PartialEq)]
pub struct LatestRun {
    pub run_id: RunId,
    pub status: RunStatus,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
    pub turn_ended: bool,
}

/// Files a turn changed, shown as the changed-files card under its reply.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnDiffSummary {
    pub assistant_message_id: MessageId,
    pub files: Arc<[FileChange]>,
}

/// A message waiting to send after the current turn.
#[derive(Clone, Debug, PartialEq)]
pub struct QueuedMessage {
    pub id: SharedString,
    pub text: SharedString,
    pub created_at: Timestamp,
    pub image_count: usize,
    pub guest: bool,
    pub editing: bool,
}

/// A question the agent is waiting on, shown as the composer's top drawer.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingQuestion {
    pub request_id: RequestId,
    pub header: SharedString,
    pub question: SharedString,
    pub options: Arc<[(SharedString, SharedString)]>,
    pub multi_select: bool,
    pub index: usize,
    pub count: usize,
}

/// Everything row derivation reads (`deriveMessagesTimelineRows` input).
#[derive(Clone, Debug, Default)]
pub struct TimelineInput {
    pub entries: Arc<Vec<TimelineEntry>>,
    pub pending_approval_ids: Vec<RequestId>,
    pub latest_run: Option<LatestRun>,
    pub is_working: bool,
    pub active_turn_started_at: Option<Timestamp>,
    pub turn_diff_summaries: Arc<[TurnDiffSummary]>,
    pub supports_rollback: bool,
    /// Live data: the turn count each user message's "Edit from here" rewinds to, by message
    /// id; empty means every user message gets one (fixtures).
    pub revert_turn_counts: Arc<std::collections::HashMap<SharedString, usize>>,
    pub worktree_setup: Option<WorktreeSetup>,
    pub queued: Arc<[QueuedMessage]>,
    pub pending_question: Option<PendingQuestion>,
    /// The turn item behind each live entry, by entry id; row rules that read item fields
    /// (an assistant message's phase) use it. Empty for fixtures.
    pub item_sources: Arc<std::collections::HashMap<SharedString, Arc<t3code_contracts::OrchestrationV2TurnItem>>>,
    /// The superseded attempt of each live entry that belongs to one, by entry id.
    pub superseded_attempts: Arc<std::collections::HashMap<SharedString, AttemptId>>,
}
