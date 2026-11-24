//! Rows the shell leaves to on-demand reads (`threadFamilies.ts`). Shells with
//! `omitIdleSubagents` or `omitOlderSettled` leave out idle subagents and older settled threads;
//! a thread's family (every active thread sharing its `lineage.rootThreadId`), the Settled
//! shelf's older pages and search rows are read when shown and mounted into the shell under a
//! key. Mounted rows merge behind the shell rows, which win (deltas carry every change), and are
//! never persisted.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use t3code_contracts::{OrchestrationV2ThreadShell, ThreadId};
use t3code_shared::thread_projection::Rows;

use super::shell::ShellState;
use super::shell_reducer::ShellSnapshot;

/// One family or page read: its rows and the server sequence they are current to.
#[derive(Debug, PartialEq)]
pub struct FamilyRows {
    pub snapshot_sequence: u64,
    pub threads: Rows<OrchestrationV2ThreadShell>,
}

impl FamilyRows {
    pub fn new(snapshot_sequence: u64, threads: Vec<OrchestrationV2ThreadShell>) -> Arc<Self> {
        Arc::new(Self { snapshot_sequence, threads: Arc::new(threads.into_iter().map(Arc::new).collect()) })
    }
}

/// Family rows the shell does not have (`threadFamilyRowsMissingFromShell`). A shell row is at
/// least as new as any family copy; a family copy read before the shell removed that thread is
/// stale.
pub fn rows_missing_from_shell(
    shell_threads: &[Arc<OrchestrationV2ThreadShell>],
    families: &[Arc<FamilyRows>],
    removed_at: &HashMap<ThreadId, u64>,
) -> Vec<Arc<OrchestrationV2ThreadShell>> {
    let mut present: HashSet<&ThreadId> = shell_threads.iter().map(|thread| &thread.id).collect();
    let mut missing = Vec::new();
    for family in families {
        for thread in family.threads.iter() {
            if present.contains(&thread.id) {
                continue;
            }
            if removed_at.get(&thread.id).is_some_and(|&sequence| sequence > family.snapshot_sequence) {
                continue;
            }
            present.insert(&thread.id);
            missing.push(Arc::clone(thread));
        }
    }
    missing
}

/// Mounted reads by key, and the last merge, so a shell update that only moves the cursor keeps
/// the merged rows (`snapshotAtom`).
#[derive(Default)]
pub(crate) struct Mounts {
    reads: BTreeMap<String, Vec<Arc<FamilyRows>>>,
    revision: u64,
    memo: Option<Memo>,
}

struct Memo {
    source: Arc<ShellState>,
    revision: u64,
    shell_threads: Rows<OrchestrationV2ThreadShell>,
    removed_at: Arc<HashMap<ThreadId, u64>>,
    families: Vec<Arc<FamilyRows>>,
    threads: Rows<OrchestrationV2ThreadShell>,
    merged: Arc<ShellState>,
}

impl Mounts {
    /// Replaces the reads under `key`. Returns whether anything changed.
    pub(crate) fn set(&mut self, key: &str, reads: Vec<Arc<FamilyRows>>) -> bool {
        if self.reads.get(key).is_some_and(|current| current.len() == reads.len() && current.iter().zip(&reads).all(|(a, b)| Arc::ptr_eq(a, b))) {
            return false;
        }
        self.reads.insert(key.to_owned(), reads);
        self.revision += 1;
        true
    }

    /// Drops the reads under `key`. Returns whether anything changed.
    pub(crate) fn remove(&mut self, key: &str) -> bool {
        let removed = self.reads.remove(key).is_some();
        if removed {
            self.revision += 1;
        }
        removed
    }

    /// `source` with every mounted row the shell lacks appended. Reads older than the latest
    /// authoritative snapshot's cut wait for new ones.
    pub(crate) fn merge(&mut self, source: &Arc<ShellState>) -> Arc<ShellState> {
        let Some(snapshot) = source.snapshot.as_ref().filter(|_| !self.reads.is_empty()) else {
            self.memo = None;
            return Arc::clone(source);
        };
        if let Some(memo) = &self.memo
            && Arc::ptr_eq(&memo.source, source)
            && memo.revision == self.revision
        {
            return Arc::clone(&memo.merged);
        }
        let cut = snapshot.older_settled.as_ref().map_or(0, |older| older.as_of_sequence);
        let families: Vec<Arc<FamilyRows>> =
            self.reads.values().flatten().filter(|read| read.snapshot_sequence >= cut).cloned().collect();
        let reuse = self.memo.as_ref().filter(|memo| {
            Arc::ptr_eq(&memo.shell_threads, &snapshot.threads)
                && Arc::ptr_eq(&memo.removed_at, &source.removed_at)
                && memo.families.len() == families.len()
                && memo.families.iter().zip(&families).all(|(a, b)| Arc::ptr_eq(a, b))
        });
        let threads = match reuse {
            Some(memo) => Arc::clone(&memo.threads),
            None => {
                let extras = rows_missing_from_shell(&snapshot.threads, &families, &source.removed_at);
                if extras.is_empty() {
                    Arc::clone(&snapshot.threads)
                } else {
                    let mut threads = Vec::with_capacity(snapshot.threads.len() + extras.len());
                    threads.extend(snapshot.threads.iter().cloned());
                    threads.extend(extras);
                    Arc::new(threads)
                }
            }
        };
        let merged = if Arc::ptr_eq(&threads, &snapshot.threads) {
            Arc::clone(source)
        } else {
            let snapshot = ShellSnapshot { threads: Arc::clone(&threads), ..(**snapshot).clone() };
            Arc::new(ShellState { snapshot: Some(Arc::new(snapshot)), ..(**source).clone() })
        };
        self.memo = Some(Memo {
            source: Arc::clone(source),
            revision: self.revision,
            shell_threads: Arc::clone(&snapshot.threads),
            removed_at: Arc::clone(&source.removed_at),
            families,
            threads,
            merged: Arc::clone(&merged),
        });
        merged
    }
}
