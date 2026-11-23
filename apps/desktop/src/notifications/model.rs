//! The notification inbox model. Ports `apps/desktop/src/notifications/model.ts`
//! as is: it reconciles each thread's approval, input, failed and completed
//! signals against what it last observed, with a silent first-observation
//! baseline, and keeps up to 200 read entries.

use std::collections::{BTreeMap, HashSet};

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};
use t3code_web::desktop_notifications::{
    DesktopNotification, DesktopNotificationCommand, DesktopNotificationKind as Kind,
    DesktopNotificationSnapshot, DesktopNotificationThread, ReadState, TurnState,
};

/// Read entries kept beyond the unread ones.
const HISTORY_LIMIT: usize = 200;

/// One thread's signals when last observed. Each is the turn (or turn and
/// completion time) it belongs to, so a new turn reads as a new signal.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signals {
    pub approval: Option<SharedString>,
    pub input: Option<SharedString>,
    pub failed: Option<SharedString>,
    pub completed: Option<SharedString>,
    /// Absent in state saved before read states were observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_state: Option<ReadState>,
}

impl Signals {
    fn get(&self, kind: Kind) -> Option<&SharedString> {
        match kind {
            Kind::Approval => self.approval.as_ref(),
            Kind::Input => self.input.as_ref(),
            Kind::Failed => self.failed.as_ref(),
            Kind::Completed => self.completed.as_ref(),
        }
    }

    fn of(thread: &DesktopNotificationThread) -> Self {
        let turn = thread.turn_id.clone().unwrap_or_else(|| "session".into());
        let live = !thread.archived;
        Self {
            approval: (live && thread.approval).then(|| turn.clone()),
            input: (live && thread.input).then(|| turn.clone()),
            failed: (live && thread.turn_state == Some(TurnState::Error)).then(|| turn.clone()),
            completed: match &thread.completed_at {
                Some(at) if live && thread.turn_state == Some(TurnState::Completed) => {
                    Some(format!("{turn}:{at}").into())
                }
                _ => None,
            },
            read_state: Some(thread.read_state),
        }
    }
}

/// The persisted inbox: `notifications.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NotificationState {
    pub sequence: u64,
    /// Signals by `JSON.stringify([environmentId, threadId])`.
    pub observed: BTreeMap<String, Signals>,
    pub items: Vec<DesktopNotification>,
}

/// What a reconcile produced: the next state, and the entries to announce.
pub struct Reconciled {
    pub state: NotificationState,
    /// New unread entries for a thread seen before: these get a native notification.
    pub added: Vec<DesktopNotification>,
    /// Whether `state` differs from the input (`next === state` in the TypeScript).
    pub changed: bool,
}

fn thread_key(environment_id: &str, thread_id: &str) -> String {
    serde_json::to_string(&[environment_id, thread_id]).unwrap_or_default()
}

/// A turn end any client has seen is read here too. Requests still wait for an answer.
fn seen_turn_end(thread: &DesktopNotificationThread, kind: Kind) -> bool {
    thread.read_state == ReadState::Seen && kind.is_turn_end()
}

/// Marked unread on some client: the same turn end became unread. Only that change
/// counts, and state saved before read states existed records one first.
fn marked_unread(previous: Option<&Signals>, next: &Signals, kind: Kind) -> bool {
    let Some(previous) = previous else { return false };
    kind.is_turn_end()
        && next.get(kind).is_some()
        && next.get(kind) == previous.get(kind)
        && previous.read_state.is_some_and(|state| state != ReadState::Unread)
        && next.read_state == Some(ReadState::Unread)
}

/// Reconciles current thread conditions (`reconcileNotifications`). `now` is the
/// ISO time new entries are stamped with.
pub fn reconcile_notifications(
    state: &NotificationState,
    snapshot: &DesktopNotificationSnapshot,
    now: &SharedString,
) -> Reconciled {
    let mut observed = state.observed.clone();
    let mut sequence = state.sequence;
    let mut changed = false;
    let mut added = Vec::new();
    let mut created: Vec<DesktopNotification> = Vec::new();
    let environments: HashSet<&SharedString> = snapshot.environment_ids.iter().collect();
    let mut current: Vec<(String, &DesktopNotificationThread, Signals)> = Vec::new();
    for thread in &snapshot.threads {
        if !environments.contains(&thread.environment_id) {
            continue;
        }
        let key = thread_key(&thread.environment_id, &thread.thread_id);
        let signals = Signals::of(thread);
        match current.iter_mut().find(|(existing, ..)| *existing == key) {
            Some(entry) => *entry = (key, thread, signals),
            None => current.push((key, thread, signals)),
        }
    }
    let mut create = |thread: &DesktopNotificationThread, kind: Kind, read: bool| {
        sequence += 1;
        let item = DesktopNotification {
            id: format!("t3-work-{sequence}").into(),
            environment_id: thread.environment_id.clone(),
            thread_id: thread.thread_id.clone(),
            kind,
            thread_title: thread.thread_title.clone(),
            project_title: thread.project_title.clone(),
            environment_title: thread.environment_title.clone(),
            created_at: now.clone(),
            read,
            resolved: false,
        };
        created.insert(0, item.clone());
        item
    };
    for (key, thread, next) in &current {
        let previous = observed.get(key).cloned();
        if previous.as_ref().is_none_or(|previous| {
            next.read_state != previous.read_state
                || Kind::ALL.iter().any(|kind| next.get(*kind) != previous.get(*kind))
        }) {
            changed = true;
        }
        observed.insert(key.clone(), next.clone());
        // Keep the baseline while settled so reopening cannot replay old completions.
        if thread.settled {
            continue;
        }
        for kind in Kind::ALL {
            let Some(signal) = next.get(kind) else { continue };
            if previous.as_ref().and_then(|p| p.get(kind)) == Some(signal) {
                continue;
            }
            // Initial completions are history. Pending questions still belong in the inbox.
            if previous.is_none() && kind.is_turn_end() {
                continue;
            }
            // A main turn that ends while subagents run is not the finish.
            if kind == Kind::Completed && thread.background_working {
                continue;
            }
            let read = thread.on_screen || seen_turn_end(thread, kind);
            let item = create(thread, kind, read);
            if previous.is_some() && !read {
                added.push(item);
            }
        }
        // A mark-unread without a matching entry (pruned history, or a turn that
        // ended before this app ran) gets a silent entry that carries the badge.
        let turn_end = if next.completed.is_some() {
            Some(Kind::Completed)
        } else if next.failed.is_some() {
            Some(Kind::Failed)
        } else {
            None
        };
        if let Some(turn_end) = turn_end
            && !thread.snoozed
            && marked_unread(previous.as_ref(), next, turn_end)
            && !state.items.iter().any(|item| {
                !item.resolved
                    && item.kind == turn_end
                    && item.environment_id == thread.environment_id
                    && item.thread_id == thread.thread_id
            })
        {
            create(thread, turn_end, false);
        }
    }
    let mut items: Vec<DesktopNotification> = state
        .items
        .iter()
        .map(|item| {
            if !environments.contains(&item.environment_id) {
                return item.clone();
            }
            let key = thread_key(&item.environment_id, &item.thread_id);
            let entry = current.iter().find(|(k, ..)| *k == key);
            let previous = state.observed.get(&key);
            // Settling or snoozing is setting the thread aside: its entries are read.
            let resolved = item.resolved
                || entry.is_none_or(|(_, thread, signals)| {
                    thread.settled
                        || thread.snoozed
                        || signals.get(item.kind).is_none()
                        || previous.is_some_and(|p| signals.get(item.kind) != p.get(item.kind))
                });
            // The badge returns; the banner does not, because nothing is added.
            let unread = !resolved
                && entry.is_some_and(|(_, _, signals)| marked_unread(previous, signals, item.kind));
            let read = !unread
                && (item.read
                    || resolved
                    || entry.is_some_and(|(_, thread, _)| seen_turn_end(thread, item.kind)));
            let (thread_title, project_title, environment_title) = match entry {
                Some((_, thread, _)) => (
                    thread.thread_title.clone(),
                    thread.project_title.clone(),
                    thread.environment_title.clone(),
                ),
                None => (
                    item.thread_title.clone(),
                    item.project_title.clone(),
                    item.environment_title.clone(),
                ),
            };
            let next = DesktopNotification {
                thread_title,
                project_title,
                environment_title,
                resolved,
                read,
                ..item.clone()
            };
            if next != *item {
                changed = true;
            }
            next
        })
        .collect();
    let created_count = created.len();
    created.append(&mut items);
    // Bound retained history, without dropping unread attention items.
    let mut history = 0;
    created.retain(|item| {
        if !item.read {
            return true;
        }
        history += 1;
        history <= HISTORY_LIMIT
    });
    let changed = changed || created_count > 0 || created.len() != state.items.len();
    Reconciled {
        state: if changed {
            NotificationState { sequence, observed, items: created }
        } else {
            state.clone()
        },
        added,
        changed,
    }
}

/// Marks entries read (`readNotifications`). Returns `None` when nothing changed.
pub fn read_notifications(
    state: &NotificationState,
    command: &DesktopNotificationCommand,
) -> Option<NotificationState> {
    let mut changed = false;
    let items = state
        .items
        .iter()
        .map(|item| {
            let matches = match command {
                DesktopNotificationCommand::Read(ids) => ids.contains(&item.id),
                DesktopNotificationCommand::ReadThread(key) => item.is_for(key),
            };
            if !matches || item.read {
                return item.clone();
            }
            changed = true;
            DesktopNotification { read: true, ..item.clone() }
        })
        .collect();
    changed.then(|| NotificationState { items, ..state.clone() })
}
