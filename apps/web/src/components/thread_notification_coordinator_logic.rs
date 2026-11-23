//! When a thread's status change notifies. Ports
//! `ThreadNotificationCoordinator.logic.ts`, with the clock passed in: a grace
//! timer is a deadline that [`NotificationGate::due`] releases, so the
//! coordinator and the fixture driver decide the same way.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use gpui_kit::SharedString;

use crate::components::sidebar::model::TimestampMs;

/// What a notification is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadNotificationKind {
    /// The thread needs attention: approval, input, guest request, failure, limit.
    Input,
    Completion,
}

/// What the gate needs from one thread (`ThreadNotificationObservation`).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadNotificationObservation {
    /// Status and request key, or run and status without a request, while the thread needs attention.
    pub attention: Option<SharedString>,
    /// The pending request's id, when it has one; `None` whenever `attention` is.
    pub request_id: Option<SharedString>,
    /// The thread is idle and its latest run completed.
    pub settled: bool,
    /// When the latest run completed, if it did.
    pub completed_at: Option<TimestampMs>,
    /// The latest run answers a background-work progress wake, like a Monitor line.
    pub background_wake: bool,
}

/// Why a thread may not notify right now.
#[derive(Clone, Copy, Debug, Default)]
pub struct ObserveOptions {
    /// Archived or settled.
    pub archived: bool,
    /// A stop was requested, or the thread waits on background work.
    pub quiet: bool,
    pub snoozed: bool,
}

/// Matches the push relay's completion confirmation: a final wake can start right after a progress wake.
pub const RELEASED_COMPLETION_GRACE_MS: i64 = 5_000;

/// The snooze fields notifications read (`ThreadSnoozeShell`, reduced).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SnoozeState {
    pub snoozed_until: Option<TimestampMs>,
    pub snoozed_at: Option<TimestampMs>,
    /// `threadRaisedHandWhileSnoozed`: a request, guest message or failure woke it early.
    pub hand_raised: bool,
}

impl SnoozeState {
    /// `effectiveSnoozed`: hidden until a wake time still ahead, unless a hand went up.
    pub fn effective(&self, now: TimestampMs) -> bool {
        self.snoozed_until.is_some_and(|until| until > now) && !self.hand_raised
    }
}

/// Whether snooze suppresses notifications (`isSnoozedForNotification`). A
/// completion that landed during the snooze stays suppressed after the wake.
pub fn is_snoozed_for_notification(
    snooze: &SnoozeState,
    completed_at: Option<TimestampMs>,
    now: TimestampMs,
) -> bool {
    if snooze.effective(now) {
        return true;
    }
    let Some(completed_at) = completed_at else { return false };
    if snooze.snoozed_at.is_some_and(|at| completed_at < at) {
        return false;
    }
    snooze.effective(completed_at)
}

#[derive(Clone, Copy, Debug)]
struct Release {
    at: TimestampMs,
    /// When the grace ends; `None` while paused.
    deadline: Option<TimestampMs>,
    /// Start order, so graces ending together release in the order they began.
    order: u64,
}

/// The per-environment notification gate (`createThreadNotificationGate`).
pub struct NotificationGate<K> {
    /// The newest completion already notified, or deliberately skipped.
    handled: HashMap<K, TimestampMs>,
    /// Wake completions waiting out the grace.
    releases: HashMap<K, Release>,
    /// Attention keys with a request id seen since the thread last stopped needing attention.
    attended: HashMap<K, HashSet<SharedString>>,
    started: u64,
}

impl<K> Default for NotificationGate<K> {
    fn default() -> Self {
        Self { handled: HashMap::new(), releases: HashMap::new(), attended: HashMap::new(), started: 0 }
    }
}

impl<K: Clone + Eq + Hash> NotificationGate<K> {
    fn release(&mut self, key: K, at: TimestampMs, now: TimestampMs) {
        let deadline = Some(TimestampMs(now.0 + RELEASED_COMPLETION_GRACE_MS));
        self.started += 1;
        self.releases.insert(key, Release { at, deadline, order: self.started });
    }

    fn skip(&mut self, key: &K, at: Option<TimestampMs>) {
        if let Some(at) = at
            && self.handled.get(key).is_none_or(|handled| at > *handled)
        {
            self.handled.insert(key.clone(), at);
        }
    }

    fn attend(
        &mut self,
        key: &K,
        prior: Option<&ThreadNotificationObservation>,
        next: &ThreadNotificationObservation,
    ) -> bool {
        let Some(attention) = &next.attention else {
            self.attended.remove(key);
            return false;
        };
        if next.request_id.is_none() {
            return prior.is_none_or(|prior| prior.attention.as_ref() != Some(attention));
        }
        self.attended.entry(key.clone()).or_default().insert(attention.clone())
    }

    /// Observes one changed thread and returns the notification it fires now, if any.
    pub fn observe(
        &mut self,
        key: &K,
        prior: Option<&ThreadNotificationObservation>,
        next: &ThreadNotificationObservation,
        options: ObserveOptions,
        now: TimestampMs,
    ) -> Option<ThreadNotificationKind> {
        let silenced = options.archived || options.quiet || options.snoozed;
        // The grace survives updates that keep the wake's completion; anything else cancels it.
        if let Some(pending) = self.releases.get(key).copied()
            && Some(pending.at) == next.completed_at
            && !silenced
        {
            if pending.deadline.is_none() {
                self.release(key.clone(), pending.at, now);
            }
            return None;
        }
        self.releases.remove(key);
        // The first sighting, archived or snoozed threads, and stopped or silent runs never notify completion.
        if prior.is_none() || silenced {
            self.skip(key, next.completed_at);
        }
        let fresh = self.attend(key, prior, next);
        if prior.is_none() || options.archived {
            return None;
        }
        if fresh {
            return Some(ThreadNotificationKind::Input);
        }
        let at = next.completed_at?;
        if !next.settled || self.handled.get(key).is_some_and(|handled| at <= *handled) {
            return None;
        }
        if next.background_wake {
            self.release(key.clone(), at, now);
            return None;
        }
        self.handled.insert(key.clone(), at);
        Some(ThreadNotificationKind::Completion)
    }

    /// Whether a wake completion for `key` is waiting out its grace.
    pub fn is_releasing(&self, key: &K) -> bool {
        self.releases.contains_key(key)
    }

    /// Releases every grace that ended by `now`, as completions to notify.
    pub fn due(&mut self, now: TimestampMs) -> Vec<K> {
        let mut due: Vec<(K, TimestampMs, (Option<TimestampMs>, u64))> = self
            .releases
            .iter()
            .filter(|(_, release)| release.deadline.is_some_and(|deadline| deadline <= now))
            .map(|(key, release)| (key.clone(), release.at, (release.deadline, release.order)))
            .collect();
        due.sort_by_key(|(.., order)| *order);
        for (key, at, _) in &due {
            self.releases.remove(key);
            self.handled.insert(key.clone(), *at);
        }
        due.into_iter().map(|(key, ..)| key).collect()
    }

    /// The earliest running grace deadline, to schedule a wake-up for.
    pub fn next_deadline(&self) -> Option<TimestampMs> {
        self.releases.values().filter_map(|release| release.deadline).min()
    }

    /// Stops grace timers while the snapshot is stale; the next observation restarts or drops them.
    pub fn pause(&mut self) {
        for release in self.releases.values_mut() {
            release.deadline = None;
        }
    }

    /// Forgets threads that left the snapshot.
    pub fn retain(&mut self, keys: &HashSet<K>) {
        self.handled.retain(|key, _| keys.contains(key));
        self.releases.retain(|key, _| keys.contains(key));
        self.attended.retain(|key, _| keys.contains(key));
    }
}
