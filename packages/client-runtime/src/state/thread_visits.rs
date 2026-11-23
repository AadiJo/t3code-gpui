//! The visit tracker of an open thread (`threadVisits.ts`). While the view is mounted and
//! visible it records one open, then advances the server's visit watermark as the shell row
//! changes, at most once per 10 s while a run streams, and at once for an unseen completion or a
//! wake. A failed visit retries only on a new session.
//!
//! The tracker is a state machine: feed it events, and it answers what to send and when to
//! look again. [`run_visit_tracker`] drives it against a store.

use std::time::Duration;

use t3code_contracts::{OrchestrationV2ThreadShell, ThreadId};

use super::models::{EpochMs, format_ms, now_ms, parse_ms};
use crate::operations::EnvironmentCommands;

/// At most one visit per this long while the row keeps changing.
pub const VISIT_DISPATCH_THROTTLE_MS: i64 = 10_000;

/// What the tracker reads from a shell row.
#[derive(Debug, Clone)]
pub struct VisitedShell {
    pub updated_at: EpochMs,
    pub updated_at_iso: String,
    pub last_visited_at: Option<EpochMs>,
    pub latest_run_completed_at: Option<EpochMs>,
    /// When a wake the user is looking at took effect, if later than `updated_at`.
    pub woke_at: Option<(EpochMs, String)>,
}

impl VisitedShell {
    pub fn from_row(row: &OrchestrationV2ThreadShell, woke_at: Option<EpochMs>) -> Option<Self> {
        Some(Self {
            updated_at: parse_ms(&row.updated_at)?,
            updated_at_iso: row.updated_at.clone(),
            last_visited_at: row.last_visited_at.as_deref().and_then(parse_ms),
            latest_run_completed_at: row.latest_run_completed_at.as_deref().and_then(parse_ms),
            woke_at: woke_at.map(|at| (at, format_ms(at))),
        })
    }
}

/// A visit to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Visit {
    pub visited_at: String,
    pub opened: bool,
}

/// What the tracker wants next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisitAction {
    Idle,
    Send(Visit),
    /// Look again at this time.
    WakeAt(EpochMs),
}

/// One open thread's visit state.
#[derive(Debug)]
pub struct VisitTracker {
    open_pending: bool,
    last_dispatched: Option<EpochMs>,
    last_accepted: Option<EpochMs>,
    retry_pending: bool,
    last_dispatch_at: EpochMs,
    dispatch_generation: u64,
    pending: Option<Visit>,
}

impl VisitTracker {
    /// A tracker for a thread the user just opened, on session `generation`.
    pub fn new(generation: u64) -> Self {
        Self {
            open_pending: true,
            last_dispatched: None,
            last_accepted: None,
            retry_pending: false,
            last_dispatch_at: i64::MIN / 2,
            dispatch_generation: generation,
            pending: None,
        }
    }

    fn accept(&mut self, visited_at: EpochMs) {
        if self.last_accepted.is_none_or(|accepted| visited_at > accepted) {
            self.last_accepted = Some(visited_at);
        }
    }

    fn dispatch(&mut self, visited_at: (EpochMs, &str), opened: bool, now: EpochMs, generation: u64) -> VisitAction {
        self.retry_pending = false;
        self.last_dispatched = Some(visited_at.0);
        self.last_dispatch_at = now;
        self.dispatch_generation = generation;
        let visit = Visit { visited_at: visited_at.1.to_owned(), opened };
        self.pending = Some(visit.clone());
        VisitAction::Send(visit)
    }

    /// The server took the visit.
    pub fn on_accepted(&mut self, visit: &Visit) {
        self.pending = None;
        if let Some(at) = parse_ms(&visit.visited_at) {
            self.accept(at);
        }
    }

    /// The visit failed; it is retried once the session changes.
    pub fn on_failed(&mut self, visit: &Visit) {
        self.pending = None;
        let at = parse_ms(&visit.visited_at);
        if self.last_accepted.zip(at).is_some_and(|(accepted, at)| accepted >= at) || self.last_dispatched != at {
            return;
        }
        if visit.opened {
            self.open_pending = true;
        }
        self.retry_pending = true;
    }

    /// Decides what to do for the current row. `viewing` is false while the window is hidden;
    /// the watermark then holds, so a turn that finishes stays unread.
    pub fn on_shell(&mut self, shell: Option<&VisitedShell>, viewing: bool, generation: u64, now: EpochMs) -> VisitAction {
        let Some(shell) = shell.filter(|_| viewing) else { return VisitAction::Idle };
        if self.pending.is_some() {
            return VisitAction::Idle;
        }
        let wake_is_latest = shell.woke_at.as_ref().is_some_and(|(at, _)| *at > shell.updated_at);
        let (updated_at, visited_iso) = match &shell.woke_at {
            Some((at, iso)) if wake_is_latest => (*at, iso.as_str()),
            _ => (shell.updated_at, shell.updated_at_iso.as_str()),
        };
        if let Some(visited) = shell.last_visited_at {
            self.accept(visited);
        }
        if self.retry_pending && self.last_accepted.zip(self.last_dispatched).is_some_and(|(accepted, dispatched)| accepted >= dispatched) {
            self.retry_pending = false;
            self.open_pending = false;
        }
        // A rejected visit waits for a new session, even if streaming changes the row.
        if self.retry_pending && generation == self.dispatch_generation {
            return VisitAction::Idle;
        }
        let elapsed = now - self.last_dispatch_at;
        if self.retry_pending && elapsed < VISIT_DISPATCH_THROTTLE_MS {
            return VisitAction::WakeAt(self.last_dispatch_at + VISIT_DISPATCH_THROTTLE_MS);
        }
        // The open rides on the first visit the server hears, however late the row arrives.
        if self.open_pending {
            self.open_pending = false;
            return self.dispatch((updated_at, visited_iso), true, now, generation);
        }
        if shell.last_visited_at.is_some_and(|visited| visited >= updated_at) {
            // Already seen, so a later mark-unread stays sticky.
            if self.last_dispatched.is_none_or(|dispatched| dispatched < updated_at) {
                self.last_dispatched = Some(updated_at);
            }
            return VisitAction::Idle;
        }
        if !self.retry_pending && self.last_dispatched.is_some_and(|dispatched| dispatched >= updated_at) {
            return VisitAction::Idle;
        }
        let unseen_completion = shell
            .latest_run_completed_at
            .is_some_and(|completed| shell.last_visited_at.is_none_or(|visited| completed > visited));
        if unseen_completion || wake_is_latest || elapsed >= VISIT_DISPATCH_THROTTLE_MS {
            return self.dispatch((updated_at, visited_iso), false, now, generation);
        }
        VisitAction::WakeAt(self.last_dispatch_at + VISIT_DISPATCH_THROTTLE_MS)
    }
}

/// Runs a tracker for one open thread until `viewing` closes. `woke_at` gives the row's wake
/// time when the thread was snoozed.
pub async fn run_visit_tracker(
    commands: EnvironmentCommands,
    thread_id: ThreadId,
    mut viewing: tokio::sync::watch::Receiver<bool>,
    woke_at: impl Fn(&OrchestrationV2ThreadShell) -> Option<EpochMs> + Send + 'static,
) {
    let store = commands.store().clone();
    let mut shell = store.shell().subscribe();
    let mut status = store.context().supervisor.status.clone();
    let generation = |status: &tokio::sync::watch::Receiver<crate::connection::ConnectionStatus>| status.borrow().generation;
    let mut tracker = VisitTracker::new(generation(&status));
    let (results, mut result_rx) = tokio::sync::mpsc::unbounded_channel::<(Visit, bool)>();
    loop {
        let row = shell.borrow_and_update().thread(&thread_id).cloned();
        let visited = row.as_deref().and_then(|row| VisitedShell::from_row(row, woke_at(row)));
        let wake = match tracker.on_shell(visited.as_ref(), *viewing.borrow_and_update(), generation(&status), now_ms()) {
            VisitAction::Send(visit) => {
                let commands = commands.clone();
                let thread_id = thread_id.clone();
                let results = results.clone();
                tokio::spawn(async move {
                    let ok = commands.visit(&thread_id, visit.visited_at.clone(), visit.opened, None).await.is_ok();
                    _ = results.send((visit, ok));
                });
                None
            }
            VisitAction::WakeAt(at) => Some(at),
            VisitAction::Idle => None,
        };
        let sleep = async {
            match wake {
                Some(at) => tokio::time::sleep(Duration::from_millis((at - now_ms()).max(0) as u64)).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            changed = shell.changed() => if changed.is_err() { return },
            changed = viewing.changed() => if changed.is_err() { return },
            changed = status.changed() => if changed.is_err() { return },
            () = sleep => {}
            Some((visit, ok)) = result_rx.recv() => {
                if ok { tracker.on_accepted(&visit) } else { tracker.on_failed(&visit) }
            }
        }
    }
}
