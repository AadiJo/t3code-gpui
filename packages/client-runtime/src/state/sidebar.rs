//! Sidebar status, sections and unread state (`Sidebar.logic.ts`, the partition in
//! `Sidebar.tsx`). Pure functions of presented rows and the current time.

use t3code_contracts::OrchestrationV2ProviderFailureClass;
use t3code_contracts::OrchestrationV2ShellThreadStatus as ShellStatus;
use t3code_contracts::OrchestrationV2ThreadShellSettledOverride as Settled;

use super::models::{EpochMs, PendingSubagentRequest, ThreadRow};
use super::thread_inbox::InboxTracker;
use super::thread_settled::{effective_snoozed, has_unseen_run_failure};
use super::thread_sort::{sort_active, sort_by_return, sort_by_send, sort_pinned, sort_settled, sort_snoozed};

/// What a thread row says it needs (`SidebarThreadStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadStatus {
    Approval,
    Input,
    /// A guest message waits for the owner.
    Request,
    Waiting,
    Preparing,
    Working,
    Failed,
    Limited,
    Ready,
}

/// The status, by precedence (`resolveSidebarThreadStatus`).
pub fn thread_status(row: &ThreadRow) -> ThreadStatus {
    if row.has_pending_approvals || row.pending_subagent_request == Some(PendingSubagentRequest::Approval) {
        return ThreadStatus::Approval;
    }
    if row.has_pending_user_input || row.pending_subagent_request == Some(PendingSubagentRequest::UserInput) {
        return ThreadStatus::Input;
    }
    if row.source.pending_guest_message_count.unwrap_or(0) > 0 {
        return ThreadStatus::Request;
    }
    let Some(runtime) = &row.runtime else { return ThreadStatus::Ready };
    match runtime.status {
        ShellStatus::Running if row.source.blocked_on_thread_wait == Some(true) => ThreadStatus::Waiting,
        ShellStatus::Preparing => ThreadStatus::Preparing,
        ShellStatus::Queued | ShellStatus::Starting | ShellStatus::Running | ShellStatus::Waiting => ThreadStatus::Working,
        ShellStatus::Idle => ThreadStatus::Waiting,
        ShellStatus::Failed if runtime.last_error_class == Some(OrchestrationV2ProviderFailureClass::UsageLimit) => ThreadStatus::Limited,
        ShellStatus::Failed => ThreadStatus::Failed,
        _ => ThreadStatus::Ready,
    }
}

/// The row's status: a failure the user already opened settles to ready
/// (`resolveSidebarRowStatus`).
pub fn row_status(row: &ThreadRow) -> ThreadStatus {
    let status = thread_status(row);
    if status != ThreadStatus::Failed || row.latest_run.as_ref().is_none_or(|run| run.status != ShellStatus::Failed) {
        return status;
    }
    if has_unseen_run_failure(row) { status } else { ThreadStatus::Ready }
}

/// The server's visit watermark, or the local fallback for servers without one
/// (`resolveThreadLastVisitedAt`).
pub fn last_visited_at(row: &ThreadRow, local: Option<EpochMs>) -> Option<EpochMs> {
    row.last_visited_at.or(local)
}

/// A completed run the user has not seen (`hasUnseenCompletion`). A thread never visited reads
/// as seen.
pub fn has_unseen_completion(row: &ThreadRow, last_visited_at: Option<EpochMs>) -> bool {
    let Some(run) = &row.latest_run else { return false };
    let Some(completed_at) = run.completed_at else { return false };
    if matches!(run.status, ShellStatus::Interrupted | ShellStatus::Cancelled) {
        return false;
    }
    last_visited_at.is_some_and(|visited| completed_at > visited)
}

/// The dot on a closed folder: attention, then failed, unread, working
/// (`resolveSidebarFolderDot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderDot {
    Attention,
    Failed,
    Unread,
    Working,
}

pub fn folder_dot<'a>(rows: impl IntoIterator<Item = &'a ThreadRow> + Clone) -> Option<FolderDot> {
    let statuses: Vec<ThreadStatus> = rows.clone().into_iter().map(row_status).collect();
    let has = |status: ThreadStatus| statuses.contains(&status);
    if has(ThreadStatus::Input) || has(ThreadStatus::Approval) || has(ThreadStatus::Request) || has(ThreadStatus::Limited) {
        return Some(FolderDot::Attention);
    }
    if has(ThreadStatus::Failed) {
        return Some(FolderDot::Failed);
    }
    if rows.into_iter().zip(&statuses).any(|(row, status)| *status == ThreadStatus::Ready && has_unseen_completion(row, row.last_visited_at)) {
        return Some(FolderDot::Unread);
    }
    (has(ThreadStatus::Working) || has(ThreadStatus::Preparing)).then_some(FolderDot::Working)
}

/// A sidebar section (`SidebarSection`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Pinned,
    Active,
    Working,
    Snoozed,
    Settled,
}

/// The section of a row (`resolveSidebarThreadSection`). Snoozed and settled need the
/// environment's config loaded; `working_shelf` sends working rows to their own section.
pub fn section(row: &ThreadRow, now: EpochMs, config_loaded: bool, working_shelf: Option<&InboxTracker>) -> Section {
    if config_loaded && effective_snoozed(row, now) {
        return Section::Snoozed;
    }
    if config_loaded && row.source.settled_override == Some(Settled::Settled) {
        return Section::Settled;
    }
    if row.source.pinned_at.is_some() {
        return Section::Pinned;
    }
    match working_shelf {
        Some(tracker) if tracker.is_working(row) => Section::Working,
        _ => Section::Active,
    }
}

/// Sidebar options that change the partition.
#[derive(Debug, Clone, Copy, Default)]
pub struct PartitionOptions {
    /// Working rows get their own shelf, and active rows sort by return time.
    pub working_shelf: bool,
    /// Folder ("boring") mode sorts active rows by the user's last send.
    pub folder_mode: bool,
}

/// The rows of each section, sorted.
#[derive(Debug, Default)]
pub struct SidebarSections<'a> {
    pub pinned: Vec<&'a ThreadRow>,
    pub working: Vec<&'a ThreadRow>,
    pub active: Vec<&'a ThreadRow>,
    pub snoozed: Vec<&'a ThreadRow>,
    pub settled: Vec<&'a ThreadRow>,
}

/// Splits navigation rows into sections and sorts each (`Sidebar.tsx` partition).
/// `config_loaded` answers per environment.
pub fn partition<'a>(
    rows: &[&'a ThreadRow],
    now: EpochMs,
    config_loaded: impl Fn(&t3code_contracts::EnvironmentId) -> bool,
    inbox: Option<&InboxTracker>,
    options: PartitionOptions,
) -> SidebarSections<'a> {
    let mut sections = SidebarSections::default();
    let shelf = if options.working_shelf { inbox } else { None };
    for row in rows {
        match section(row, now, config_loaded(&row.environment_id), shelf) {
            Section::Pinned => sections.pinned.push(*row),
            Section::Working => sections.working.push(*row),
            Section::Active => sections.active.push(*row),
            Section::Snoozed => sections.snoozed.push(*row),
            Section::Settled => sections.settled.push(*row),
        }
    }
    sort_pinned(&mut sections.pinned);
    sort_by_send(&mut sections.working);
    match (options.working_shelf, options.folder_mode) {
        (true, true) => sort_by_send(&mut sections.active),
        (true, false) => sort_by_return(&mut sections.active, |row| inbox.and_then(|tracker| tracker.returned_at(row))),
        _ => sort_active(&mut sections.active),
    }
    sort_snoozed(&mut sections.snoozed);
    sort_settled(&mut sections.settled);
    sections
}
