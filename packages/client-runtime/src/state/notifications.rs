//! When to notify about a thread (`ThreadNotificationCoordinator.logic.ts` and the observe loop
//! of `ThreadNotificationCoordinator.tsx`). Observe an environment's rows only while its shell
//! is live; call [`NotificationGate::pause`] otherwise.
//!
//! - "input": the row starts needing the user (input, request, approval, failed, limited),
//!   once per request id, or once per run and status without one.
//! - "completion": the row is ready, its run completed after the last one handled, and no
//!   monitor holds the wake. Background-wake completions wait 5 s in case a final wake follows.
//! - Never on first sighting, for archived or settled threads, when a stop was requested or the
//!   thread waits, or while snoozed; children shown under a parent are skipped.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use t3code_contracts::{OrchestrationV2ShellThreadStatus as ShellStatus, OrchestrationV2ThreadShell, OrchestrationV2ThreadShellPendingBackgroundTasksItem as Task, OrchestrationV2ThreadShellSettledOverride as Settled, ThreadId};

use super::models::{EpochMs, PendingSubagentRequest, ThreadRow};
use super::sidebar::{ThreadStatus, thread_status};
use super::thread_settled::effective_snoozed;

/// A push relay's completion grace: a final wake can start right after a progress wake.
pub const RELEASED_COMPLETION_GRACE_MS: i64 = 5_000;

/// Which notification to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Input,
    Completion,
}

/// What the gate remembers about a row between observations (`ThreadNotificationObservation`).
#[derive(Debug, Clone)]
pub struct Observation {
    source: Arc<OrchestrationV2ThreadShell>,
    subagent_request: Option<PendingSubagentRequest>,
    pub status: ThreadStatus,
    attention: Option<String>,
    request_id: Option<String>,
    settled: bool,
    completed_at: Option<EpochMs>,
    background_wake: bool,
    waiting: bool,
}

/// A monitor wakes the agent per line, so a progress wake's completion waits for it to end.
fn monitor_holds_wake_completion(thread: &OrchestrationV2ThreadShell) -> bool {
    thread.latest_run_background_wake == Some(true)
        && thread
            .pending_background_tasks
            .iter()
            .flatten()
            .any(|task| matches!(task, Task::Monitor(monitor) if !monitor.task_id.starts_with("pull-request-watch:")))
}

fn observe_row(row: &ThreadRow) -> Observation {
    let mut status = thread_status(row);
    if status == ThreadStatus::Ready && row.latest_run.as_ref().is_some_and(|run| run.status == ShellStatus::Failed) {
        status = ThreadStatus::Failed;
    }
    let needs_attention = matches!(status, ThreadStatus::Input | ThreadStatus::Request | ThreadStatus::Approval | ThreadStatus::Failed | ThreadStatus::Limited);
    let own_request = row.source.pending_runtime_request.as_ref().map(|request| request.id.as_str().to_owned());
    let request_id = needs_attention
        .then(|| if row.has_pending_approvals || row.has_pending_user_input { own_request } else { None })
        .flatten();
    let attention = needs_attention.then(|| match &request_id {
        Some(id) => format!("{status:?}:{id}"),
        None => format!("{}:{status:?}", row.latest_run.as_ref().map_or("", |run| run.run_id.as_str())),
    });
    let completed = row.latest_run.as_ref().is_some_and(|run| run.status == ShellStatus::Completed);
    let settled = status == ThreadStatus::Ready && completed && !monitor_holds_wake_completion(&row.source);
    let waiting = status == ThreadStatus::Waiting;
    let completed_at = row.latest_run.as_ref().and_then(|run| run.completed_at);
    Observation {
        source: Arc::clone(&row.source),
        subagent_request: row.pending_subagent_request,
        status,
        attention,
        request_id,
        settled,
        completed_at: if settled || (waiting && completed) { completed_at } else { None },
        background_wake: row.source.latest_run_background_wake == Some(true),
        waiting,
    }
}

/// Whether snooze silences a completion, including one that landed during the snooze but is
/// processed after the wake (`isSnoozedForNotification`).
fn snoozed_for_notification(row: &ThreadRow, completed_at: Option<EpochMs>, now: EpochMs) -> bool {
    if effective_snoozed(row, now) {
        return true;
    }
    let Some(completed_at) = completed_at else { return false };
    if row.snoozed_at.is_some_and(|snoozed| completed_at < snoozed) {
        return false;
    }
    effective_snoozed(row, completed_at)
}

/// The notification state of one environment's threads.
#[derive(Default)]
pub struct NotificationGate {
    previous: HashMap<ThreadId, Observation>,
    handled: HashMap<ThreadId, EpochMs>,
    /// Wake completions waiting out the grace: completion time and when to release.
    releases: HashMap<ThreadId, (EpochMs, Option<EpochMs>)>,
    attended: HashMap<ThreadId, HashSet<String>>,
    reconnecting: bool,
}

impl NotificationGate {
    /// Observes the listed rows of a live shell at `now` and returns what to notify. `rows` are
    /// the environment's rows not shown under a parent.
    pub fn observe(&mut self, rows: &[Arc<ThreadRow>], now: EpochMs) -> Vec<(ThreadId, NotificationKind)> {
        let mut notify = Vec::new();
        let mut next = HashMap::with_capacity(rows.len());
        let mut changed = Vec::new();
        for row in rows {
            let id = row.id().clone();
            let prior = self.previous.remove(&id);
            if let Some(prior) = prior.as_ref().filter(|prior| {
                !self.reconnecting && Arc::ptr_eq(&prior.source, &row.source) && prior.subagent_request == row.pending_subagent_request
            }) {
                next.insert(id, prior.clone());
                continue;
            }
            let state = observe_row(row);
            next.insert(id.clone(), state.clone());
            changed.push((row, prior, state));
        }
        self.reconnecting = false;
        let present: HashSet<ThreadId> = next.keys().cloned().collect();
        self.previous = next;
        self.handled.retain(|id, _| present.contains(id));
        self.releases.retain(|id, _| present.contains(id));
        self.attended.retain(|id, _| present.contains(id));
        for (row, prior, state) in changed {
            let archived = row.source.archived_at.is_some() || row.source.settled_override == Some(Settled::Settled);
            let quiet = row.source.latest_run_stop_requested == Some(true) || state.waiting;
            let snoozed = snoozed_for_notification(row, state.completed_at, now);
            if let Some(kind) = self.gate(row.id(), prior.as_ref(), &state, archived, quiet, snoozed, now) {
                notify.push((row.id().clone(), kind));
            }
        }
        notify
    }

    #[allow(clippy::too_many_arguments)]
    fn gate(&mut self, id: &ThreadId, prior: Option<&Observation>, next: &Observation, archived: bool, quiet: bool, snoozed: bool, now: EpochMs) -> Option<NotificationKind> {
        let silenced = archived || quiet || snoozed;
        // The grace survives updates that keep the wake's completion; anything else cancels it.
        if let Some((at, release)) = self.releases.get_mut(id)
            && Some(*at) == next.completed_at
            && !silenced
        {
            release.get_or_insert(now + RELEASED_COMPLETION_GRACE_MS);
            return None;
        }
        self.releases.remove(id);
        if prior.is_none() || silenced {
            self.skip(id, next.completed_at);
        }
        let fresh = self.attend(id, prior, next);
        if prior.is_none() || archived {
            return None;
        }
        if fresh {
            return Some(NotificationKind::Input);
        }
        let at = next.completed_at?;
        if !next.settled || self.handled.get(id).is_some_and(|handled| at <= *handled) {
            return None;
        }
        if next.background_wake {
            self.releases.insert(id.clone(), (at, Some(now + RELEASED_COMPLETION_GRACE_MS)));
            return None;
        }
        self.handled.insert(id.clone(), at);
        Some(NotificationKind::Completion)
    }

    fn skip(&mut self, id: &ThreadId, at: Option<EpochMs>) {
        if let Some(at) = at
            && self.handled.get(id).is_none_or(|handled| at > *handled)
        {
            self.handled.insert(id.clone(), at);
        }
    }

    fn attend(&mut self, id: &ThreadId, prior: Option<&Observation>, next: &Observation) -> bool {
        let Some(attention) = &next.attention else {
            self.attended.remove(id);
            return false;
        };
        if next.request_id.is_none() {
            return prior.is_none_or(|prior| prior.attention.as_ref() != Some(attention));
        }
        self.attended.entry(id.clone()).or_default().insert(attention.clone())
    }

    /// Completions whose grace ran out by `now`; notify them.
    pub fn release_due(&mut self, now: EpochMs) -> Vec<(ThreadId, NotificationKind)> {
        let due: Vec<(ThreadId, EpochMs)> = self
            .releases
            .iter()
            .filter(|(_, (_, release))| release.is_some_and(|release| release <= now))
            .map(|(id, (at, _))| (id.clone(), *at))
            .collect();
        due.into_iter()
            .map(|(id, at)| {
                self.releases.remove(&id);
                self.handled.insert(id.clone(), at);
                (id, NotificationKind::Completion)
            })
            .collect()
    }

    /// When the next grace runs out.
    pub fn next_release(&self) -> Option<EpochMs> {
        self.releases.values().filter_map(|(_, release)| *release).min()
    }

    /// Stops grace timers while the shell is not live; the next observation restarts or drops
    /// them and re-reads every row.
    pub fn pause(&mut self) {
        for (_, release) in self.releases.values_mut() {
            *release = None;
        }
        self.reconnecting = true;
    }
}
