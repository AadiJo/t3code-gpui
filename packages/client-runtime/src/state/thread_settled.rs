//! Snooze, wake and unseen-failure rules (`threadSettled.ts`). Pure functions of a row and the
//! current time.

use t3code_contracts::OrchestrationV2PendingRuntimeRequestSummaryKind as RequestKind;
use t3code_contracts::OrchestrationV2ShellThreadStatus as ShellStatus;

use super::models::{EpochMs, ThreadRow, parse_ms};

/// Failed starts can leave user messages unadopted; the pending window is bounded.
pub const QUEUED_TURN_START_GRACE_MS: i64 = 2 * 60 * 1000;

/// Whether a run is about to start (`hasQueuedTurnStart`).
pub fn has_queued_turn_start(row: &ThreadRow, now: EpochMs) -> bool {
    if row.runtime.as_ref().is_some_and(|runtime| matches!(runtime.status, ShellStatus::Preparing | ShellStatus::Queued | ShellStatus::Starting)) {
        return true;
    }
    let Some(message_at) = row.latest_user_message_at else { return false };
    // Bounded both ways: the message time comes from whichever device sent it.
    if (now - message_at).abs() > QUEUED_TURN_START_GRACE_MS {
        return false;
    }
    match &row.latest_run {
        None => true,
        Some(run) => [run.requested_at, run.started_at, run.completed_at].iter().all(|at| at.is_none_or(|at| at < message_at)),
    }
}

fn pending_question_at(row: &ThreadRow) -> Option<EpochMs> {
    row.pending_user_input_at
}

/// When the latest failure happened: `None` when the runtime has not failed, `Some(None)` when
/// it failed with no date.
fn failure_edge_at(row: &ThreadRow) -> Option<Option<EpochMs>> {
    let runtime = row.runtime.as_ref()?;
    if runtime.status != ShellStatus::Failed {
        return None;
    }
    match &row.latest_run {
        Some(run) if run.status == ShellStatus::Failed => Some(run.completed_at),
        _ => Some(runtime.updated_at),
    }
}

/// Only a request for the user or a fresh failure ends a snooze early
/// (`threadRaisedHandWhileSnoozed`). Snoozing an existing question means "ask me later".
pub fn raised_hand_while_snoozed(row: &ThreadRow) -> bool {
    let request = row.source.pending_runtime_request.as_ref();
    if row.has_pending_approvals || request.is_some_and(|request| request.kind != RequestKind::UserInput) {
        return true;
    }
    if row.has_pending_user_input {
        let fresh = match (row.snoozed_at, pending_question_at(row)) {
            (Some(snoozed_at), Some(asked_at)) => asked_at > snoozed_at,
            _ => true,
        };
        if fresh {
            return true;
        }
    }
    if row.pending_subagent_request.is_some() || row.source.pending_guest_message_count.unwrap_or(0) > 0 {
        return true;
    }
    match failure_edge_at(row) {
        None => false,
        Some(failed_at) => match row.snoozed_at {
            None => true,
            Some(snoozed_at) => failed_at.is_some_and(|at| at > snoozed_at),
        },
    }
}

/// Whether snoozing is offered (`canSnooze`).
pub fn can_snooze(row: &ThreadRow, now: EpochMs) -> bool {
    !row.has_pending_approvals
        && !row.source.pending_runtime_request.as_ref().is_some_and(|request| request.kind != RequestKind::UserInput)
        && row.pending_subagent_request.is_none()
        && row.source.pending_guest_message_count.unwrap_or(0) == 0
        && !has_queued_turn_start(row, now)
}

/// Snoozed with the timer still running and no hand raised (`effectiveSnoozed`).
pub fn effective_snoozed(row: &ThreadRow, now: EpochMs) -> bool {
    row.snoozed_until.is_some_and(|wake| wake > now) && !raised_hand_while_snoozed(row)
}

/// When a snoozed thread woke, by timer or raised hand (`threadWokeAt`).
pub fn thread_woke_at(row: &ThreadRow, now: EpochMs) -> Option<EpochMs> {
    let wake = row.snoozed_until?;
    if raised_hand_while_snoozed(row) {
        let failed_at = failure_edge_at(row).flatten();
        let question_at = pending_question_at(row);
        let fresh_failure = failed_at.filter(|at| row.snoozed_at.is_none_or(|snoozed| *at > snoozed));
        let fresh_question = question_at.filter(|at| row.snoozed_at.is_some_and(|snoozed| *at > snoozed));
        if let Some(failed) = fresh_failure
            && fresh_question.is_none_or(|asked| failed >= asked)
        {
            return Some(failed);
        }
        if fresh_question.is_some() {
            return fresh_question;
        }
        return row.runtime.as_ref().and_then(|runtime| runtime.updated_at).or(row.snoozed_at);
    }
    (wake <= now).then_some(wake)
}

/// Whether a woken thread woke after the user last saw it (`isThreadWokeUnseen`). A run that
/// completed after the last visit counts as seeing it.
pub fn is_woke_unseen(woke_at: Option<EpochMs>, settled: bool, last_visited_at: Option<EpochMs>, latest_run_completed_at: Option<EpochMs>) -> bool {
    let Some(woke_at) = woke_at.filter(|_| !settled) else { return false };
    [last_visited_at, latest_run_completed_at].iter().flatten().all(|seen| *seen < woke_at)
}

/// Whether the latest run failed after the user last saw the thread (`hasUnseenRunFailure`).
pub fn has_unseen_run_failure(row: &ThreadRow) -> bool {
    let Some(run) = row.latest_run.as_ref().filter(|run| run.status == ShellStatus::Failed) else { return false };
    let Some(visited) = row.last_visited_at else { return true };
    match run.completed_at.or(run.started_at).or(run.requested_at) {
        Some(failed_at) => failed_at > visited,
        None => true,
    }
}

/// Parses a snooze-until value the server sent, for display.
pub fn snoozed_until(row: &ThreadRow) -> Option<EpochMs> {
    row.source.snoozed_until.as_deref().and_then(parse_ms)
}
