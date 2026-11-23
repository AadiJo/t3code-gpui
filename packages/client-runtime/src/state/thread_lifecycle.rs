//! Optimistic shell row updates (`threadLifecycle.ts`). A command's update applies on top of the
//! server rows as soon as it is sent; after the server accepts it at sequence `n` it stays until
//! a shell snapshot reaches `n`, then retires. A failed command removes it at once.

use std::collections::HashMap;
use std::sync::Arc;

use t3code_contracts::{OrchestrationV2ThreadShell, ThreadId};

use super::shell::ShellState;

/// What an optimistic update does to a row. `accepted` is true once the server replied.
pub type ApplyUpdate = Arc<dyn Fn(&mut OrchestrationV2ThreadShell, bool) + Send + Sync>;

/// Identifies a pending update so its command can confirm or remove it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UpdateId(u64);

struct PendingUpdate {
    id: UpdateId,
    thread_id: ThreadId,
    apply: ApplyUpdate,
    /// The reply's sequence, once the server accepted the command.
    sequence: Option<u64>,
}

/// The optimistic layer of one environment's shell.
#[derive(Default)]
pub struct PendingUpdates {
    next_id: u64,
    updates: Vec<PendingUpdate>,
    /// The last view built, reused while neither the source nor the updates change.
    memo: Option<(Arc<ShellState>, u64, Arc<ShellState>)>,
    revision: u64,
}

impl PendingUpdates {
    /// Starts applying `apply` to `thread_id`.
    pub fn push(&mut self, thread_id: ThreadId, apply: ApplyUpdate) -> UpdateId {
        self.next_id += 1;
        self.revision += 1;
        let id = UpdateId(self.next_id);
        self.updates.push(PendingUpdate { id, thread_id, apply, sequence: None });
        id
    }

    /// Records the server's acceptance; the update stays until the shell reaches `sequence`.
    pub fn confirm(&mut self, id: UpdateId, sequence: u64) {
        if let Some(update) = self.updates.iter_mut().find(|update| update.id == id) {
            update.sequence = Some(sequence);
            self.revision += 1;
        }
    }

    /// Drops an update whose command failed.
    pub fn remove(&mut self, id: UpdateId) {
        let before = self.updates.len();
        self.updates.retain(|update| update.id != id);
        if self.updates.len() != before {
            self.revision += 1;
        }
    }

    /// Threads with an update still applied.
    pub fn thread_ids(&self) -> impl Iterator<Item = &ThreadId> {
        self.updates.iter().map(|update| &update.thread_id)
    }

    /// Retires accepted updates the shell has caught up with.
    pub(crate) fn retire(&mut self, snapshot_sequence: Option<u64>) {
        let before = self.updates.len();
        self.updates.retain(|update| match (update.sequence, snapshot_sequence) {
            (Some(sequence), Some(current)) => current < sequence,
            (Some(_), None) => false,
            (None, _) => true,
        });
        if self.updates.len() != before {
            self.revision += 1;
        }
    }

    /// The source rows with every pending update applied in order. Only touched rows get a new
    /// `Arc`.
    pub(crate) fn apply(&mut self, source: &Arc<ShellState>) -> Arc<ShellState> {
        if let Some((memo_source, revision, view)) = &self.memo
            && Arc::ptr_eq(memo_source, source)
            && *revision == self.revision
        {
            return Arc::clone(view);
        }
        let view = self.build(source);
        self.memo = Some((Arc::clone(source), self.revision, Arc::clone(&view)));
        view
    }

    fn build(&self, source: &Arc<ShellState>) -> Arc<ShellState> {
        let Some(snapshot) = &source.snapshot else { return Arc::clone(source) };
        if self.updates.is_empty() {
            return Arc::clone(source);
        }
        let mut by_thread: HashMap<&ThreadId, Vec<&PendingUpdate>> = HashMap::new();
        for update in &self.updates {
            by_thread.entry(&update.thread_id).or_default().push(update);
        }
        let threads = snapshot
            .threads
            .iter()
            .map(|row| match by_thread.get(&row.id) {
                None => Arc::clone(row),
                Some(updates) => {
                    let mut next = (**row).clone();
                    for update in updates {
                        (update.apply)(&mut next, update.sequence.is_some());
                    }
                    Arc::new(next)
                }
            })
            .collect();
        let mut snapshot = (**snapshot).clone();
        snapshot.threads = Arc::new(threads);
        Arc::new(ShellState { snapshot: Some(Arc::new(snapshot)), ..(**source).clone() })
    }
}
