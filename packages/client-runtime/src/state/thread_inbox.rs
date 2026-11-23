//! The working shelf (`threadInbox.ts`): which threads read as working, with a 2 s grace after
//! a run ends so an orchestrator's next run does not bounce the thread between sections.

use std::collections::HashMap;

use t3code_contracts::{EnvironmentId, OrchestrationV2ShellThreadStatus as ShellStatus, ProviderInteractionMode, ThreadId};

use super::models::{EpochMs, ThreadRow, is_active_status};
use super::thread_settled::effective_snoozed;

/// A thread that stopped working keeps reading as working this long.
pub const WORKING_EXIT_GRACE_MS: i64 = 2_000;
/// A run that ended this long ago gets no grace.
const STALE_EXIT_MS: i64 = 60_000;

/// Whether the thread is doing work the user is not needed for (`isThreadWorking`).
pub fn is_thread_working(row: &ThreadRow) -> bool {
    if row.has_pending_approvals || row.has_pending_user_input || row.pending_subagent_request.is_some() {
        return false;
    }
    let runtime = row.runtime.as_ref();
    if !runtime.is_some_and(|runtime| is_active_status(runtime.status) || runtime.status == ShellStatus::Idle) {
        return false;
    }
    let run_settled = row.latest_run.as_ref().is_some_and(|run| {
        !is_active_status(run.status) && runtime.and_then(|runtime| runtime.active_run_id.as_ref()) != Some(&run.run_id)
    });
    !(row.source.interaction_mode == ProviderInteractionMode::Plan && row.source.has_actionable_proposed_plan && run_settled)
}

type Key = (EnvironmentId, ThreadId);

struct Tracked {
    working: bool,
    deadline: Option<EpochMs>,
    placement: Option<u64>,
}

/// Holds threads in the working shelf through the exit grace and remembers when each returned
/// (`createInboxReturnTracker`). Call [`InboxTracker::observe`] on each shell change and at
/// [`InboxTracker::next_deadline`].
#[derive(Default)]
pub struct InboxTracker {
    states: HashMap<Key, Tracked>,
    returns: HashMap<Key, EpochMs>,
}

fn placement(row: &ThreadRow) -> u64 {
    use std::hash::{Hash, Hasher};
    let source = &row.source;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&source.archived_at, &source.deleted_at, &source.pinned_at, source.settled_override.map(|s| s.as_str()), &source.settled_at, &source.snoozed_at, &source.snoozed_until, &source.active_order_key).hash(&mut hasher);
    hasher.finish()
}

impl InboxTracker {
    /// Updates placements from the current rows at time `now`.
    pub fn observe(&mut self, rows: &[&ThreadRow], now: EpochMs) {
        let mut present = std::collections::HashSet::new();
        for row in rows {
            let key = (row.environment_id.clone(), row.id().clone());
            let previous = self.states.get(&key);
            let actual = is_thread_working(row);
            let previously_working = previous.is_some_and(|state| state.working);
            let placement = (actual || previously_working).then(|| placement(row));
            let actionable = row.has_pending_approvals
                || row.has_pending_user_input
                || row.pending_subagent_request.is_some()
                || row.source.pending_guest_message_count.unwrap_or(0) > 0
                || (row.source.interaction_mode == ProviderInteractionMode::Plan && row.source.has_actionable_proposed_plan)
                || row.source.last_error.is_some()
                || row.latest_run.as_ref().is_some_and(|run| run.status == ShellStatus::Failed);
            let moved = previous.is_some_and(|state| state.placement != placement);
            let excluded = row.source.archived_at.is_some()
                || row.source.deleted_at.is_some()
                || row.source.pinned_at.is_some()
                || row.source.settled_override == Some(t3code_contracts::OrchestrationV2ThreadShellSettledOverride::Settled)
                || effective_snoozed(row, now);
            let ended_long_ago = row.latest_run.as_ref().and_then(|run| run.completed_at).is_some_and(|at| at <= now - STALE_EXIT_MS);
            let deadline = (!actual && previously_working && !actionable && !moved && !excluded && !ended_long_ago)
                .then(|| previous.and_then(|state| state.deadline).unwrap_or(now + WORKING_EXIT_GRACE_MS));
            let working = actual || deadline.is_some_and(|deadline| deadline > now);
            if previously_working && !working {
                self.returns.insert(key.clone(), now);
            }
            self.states.insert(key.clone(), Tracked { working, deadline: deadline.filter(|_| working), placement });
            present.insert(key);
        }
        self.states.retain(|key, _| present.contains(key));
        self.returns.retain(|key, _| present.contains(key));
    }

    /// When a grace runs out; observe again then.
    pub fn next_deadline(&self) -> Option<EpochMs> {
        self.states.values().filter_map(|state| state.deadline).min()
    }

    /// Whether the row reads as working, grace included.
    pub fn is_working(&self, row: &ThreadRow) -> bool {
        self.states.get(&(row.environment_id.clone(), row.id().clone())).map_or_else(|| is_thread_working(row), |state| state.working)
    }

    /// When a thread left the working shelf, for the return sort.
    pub fn returned_at(&self, row: &ThreadRow) -> Option<EpochMs> {
        self.returns.get(&(row.environment_id.clone(), row.id().clone())).copied()
    }
}
