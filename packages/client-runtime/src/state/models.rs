//! Shell rows as the client presents them (`models.ts`): the run and runtime summaries and the
//! pending-request flags every sidebar derivation reads. Timestamps are parsed once here, to
//! epoch milliseconds, so sorts and status checks never re-parse ISO strings.

use std::sync::Arc;

use t3code_contracts::{
    EnvironmentId, OrchestrationProjectShell, OrchestrationV2AppThreadLineageRelationshipToParent as Relationship,
    OrchestrationV2PendingRuntimeRequestSummaryKind as RequestKind, OrchestrationV2ProviderFailureClass,
    OrchestrationV2ShellThreadStatus as ShellStatus, OrchestrationV2ThreadShell, RunId, ThreadId,
};
use t3code_shared::orchestration_v2_pending_background_work::background_work_holds_completion;

/// Milliseconds since the Unix epoch.
pub type EpochMs = i64;

/// Parses an ISO 8601 timestamp; `None` for anything else (`toSortableTimestamp`).
pub fn parse_ms(iso: &str) -> Option<EpochMs> {
    iso.parse::<jiff::Timestamp>().ok().map(|time| time.as_millisecond())
}

/// Parses an optional ISO timestamp.
pub fn parse_opt(iso: Option<&String>) -> Option<EpochMs> {
    iso.and_then(|value| parse_ms(value))
}

/// Formats epoch milliseconds as the ISO string the server expects.
pub fn format_ms(ms: EpochMs) -> String {
    jiff::Timestamp::from_millisecond(ms).map(|time| time.to_string()).unwrap_or_default()
}

/// The current time in epoch milliseconds.
pub fn now_ms() -> EpochMs {
    jiff::Timestamp::now().as_millisecond()
}

/// The latest run as the sidebar sees it (`ThreadRunSummary`). An idle thread's run reads
/// `completed`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    pub run_id: RunId,
    pub status: ShellStatus,
    pub requested_at: Option<EpochMs>,
    pub started_at: Option<EpochMs>,
    pub completed_at: Option<EpochMs>,
}

/// What the thread is doing now (`ThreadRuntimeSummary`). `Idle` means background work holds
/// the run's completion.
#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeSummary {
    pub status: ShellStatus,
    pub active_run_id: Option<RunId>,
    pub activity_started_at: Option<EpochMs>,
    pub last_error_class: Option<OrchestrationV2ProviderFailureClass>,
    pub updated_at: Option<EpochMs>,
}

/// A request rolled up from child threads (`PendingSubagentRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingSubagentRequest {
    Approval,
    UserInput,
}

/// One shell row with its derived summaries (`EnvironmentThreadShell`). `source` is the row the
/// server sent; compare it by `Arc::ptr_eq` to skip unchanged rows.
#[derive(Debug, Clone)]
pub struct ThreadRow {
    pub environment_id: EnvironmentId,
    pub source: Arc<OrchestrationV2ThreadShell>,
    pub latest_run: Option<RunSummary>,
    pub runtime: Option<RuntimeSummary>,
    pub has_pending_approvals: bool,
    pub has_pending_user_input: bool,
    pub pending_user_input_at: Option<EpochMs>,
    pub pending_subagent_request: Option<PendingSubagentRequest>,
    pub created_at: Option<EpochMs>,
    pub updated_at: Option<EpochMs>,
    pub unsettled_at: Option<EpochMs>,
    pub settled_at: Option<EpochMs>,
    pub snoozed_until: Option<EpochMs>,
    pub snoozed_at: Option<EpochMs>,
    pub latest_user_message_at: Option<EpochMs>,
    /// When the user last wrote. The generated row folds an absent field into `None`, so a
    /// server predating the field reads as "never wrote" and sorts by creation time.
    pub latest_user_authored_message_at: Option<EpochMs>,
    /// The server's visit watermark. The generated row folds an absent field into `None`, so
    /// servers predating visit tracking read as never visited; the local fallback map in
    /// `ui_state` covers them.
    pub last_visited_at: Option<EpochMs>,
}

impl ThreadRow {
    pub fn id(&self) -> &ThreadId {
        &self.source.id
    }

    /// The row's title; subagent titles drop their provider prefix (`formatSubagentDisplayTitle`).
    pub fn title(&self) -> &str {
        let title = self.source.title.as_str();
        if self.source.lineage.relationship_to_parent == Some(Relationship::Subagent) {
            title.split_once(": ").map_or(title, |(_, rest)| rest)
        } else {
            title
        }
    }
}

fn terminal(status: ShellStatus) -> bool {
    matches!(
        status,
        ShellStatus::Completed | ShellStatus::Interrupted | ShellStatus::Failed | ShellStatus::Cancelled | ShellStatus::RolledBack
    )
}

/// Only work the agent waits on holds a run at `idle`; otherwise the activity run's status wins.
fn shell_runtime(thread: &OrchestrationV2ThreadShell) -> Option<RuntimeSummary> {
    let park_at_idle = background_work_holds_completion(thread.pending_background_tasks.as_deref().unwrap_or_default())
        && thread.status != ShellStatus::Failed;
    if thread.latest_run_id.is_none() && !park_at_idle {
        return None;
    }
    let status = if park_at_idle {
        ShellStatus::Idle
    } else {
        thread
            .activity_run_status
            .and_then(|status| ShellStatus::from_wire(status.as_str()))
            .unwrap_or(thread.status)
    };
    Some(RuntimeSummary {
        status,
        active_run_id: thread.active_run_id.clone(),
        activity_started_at: parse_opt(thread.activity_run_started_at.as_ref()),
        last_error_class: thread.last_error_class,
        updated_at: parse_ms(&thread.updated_at),
    })
}

/// Builds the presented row (`presentThreadShell`).
pub fn present_thread_shell(
    environment_id: &EnvironmentId,
    thread: &Arc<OrchestrationV2ThreadShell>,
    pending_subagent_request: Option<PendingSubagentRequest>,
) -> ThreadRow {
    let updated_at = parse_ms(&thread.updated_at);
    let latest_run = thread.latest_run_id.as_ref().map(|run_id| RunSummary {
        run_id: run_id.clone(),
        status: if thread.status == ShellStatus::Idle { ShellStatus::Completed } else { thread.status },
        requested_at: parse_opt(thread.latest_run_requested_at.as_ref()),
        started_at: parse_opt(thread.latest_run_started_at.as_ref()),
        completed_at: match &thread.latest_run_completed_at {
            Some(at) => parse_ms(at),
            None if thread.status == ShellStatus::Idle || terminal(thread.status) => updated_at,
            None => None,
        },
    });
    let request = thread.pending_runtime_request.as_ref();
    let user_input = request.filter(|request| request.kind == RequestKind::UserInput);
    ThreadRow {
        environment_id: environment_id.clone(),
        latest_run,
        runtime: shell_runtime(thread),
        has_pending_approvals: request
            .is_some_and(|request| !matches!(request.kind, RequestKind::UserInput | RequestKind::AuthRefresh)),
        has_pending_user_input: user_input.is_some(),
        pending_user_input_at: user_input.and_then(|request| parse_ms(&request.created_at)),
        pending_subagent_request,
        created_at: parse_ms(&thread.created_at),
        updated_at,
        unsettled_at: parse_opt(thread.unsettled_at.as_ref()),
        settled_at: parse_opt(thread.settled_at.as_ref()),
        snoozed_until: parse_opt(thread.snoozed_until.as_ref()),
        snoozed_at: parse_opt(thread.snoozed_at.as_ref()),
        latest_user_message_at: parse_opt(thread.latest_user_message_at.as_ref()),
        latest_user_authored_message_at: parse_opt(thread.latest_user_authored_message_at.as_ref()),
        last_visited_at: parse_opt(thread.last_visited_at.as_ref()),
        source: Arc::clone(thread),
    }
}

/// A project with its environment (`EnvironmentProject`).
#[derive(Debug, Clone)]
pub struct EnvironmentProject {
    pub environment_id: EnvironmentId,
    pub project: Arc<OrchestrationProjectShell>,
}

/// Run statuses the client treats as active (`threadRunStatusIsActive`).
pub fn is_active_status(status: ShellStatus) -> bool {
    matches!(
        status,
        ShellStatus::Preparing | ShellStatus::Queued | ShellStatus::Starting | ShellStatus::Running | ShellStatus::Waiting
    )
}
