//! Applying shell snapshots and deltas (`shellReducer.ts`). Unchanged rows keep their `Arc`, and
//! a batch of deltas builds at most one new list per collection.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use t3code_contracts::{
    OrchestrationProjectShell, OrchestrationV2OlderSettledThreads, OrchestrationV2ShellSnapshot,
    OrchestrationV2ShellStreamItem as Item, OrchestrationV2ShellStreamItemThreadUpdatedLocation as Location,
    OrchestrationV2ThreadShell,
};
use t3code_shared::thread_projection::Rows;

/// The rows of one environment's shell (`OrchestrationV2ShellSnapshot`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellSnapshot {
    pub schema_version: u64,
    pub snapshot_sequence: u64,
    pub threads: Rows<OrchestrationV2ThreadShell>,
    /// Legacy: current servers send the archive through its own query.
    #[serde(default)]
    pub archived_threads: Rows<OrchestrationV2ThreadShell>,
    pub projects: Rows<OrchestrationProjectShell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_settled: Option<OrchestrationV2OlderSettledThreads>,
}

fn rows<T>(items: Vec<T>) -> Rows<T> {
    Arc::new(items.into_iter().map(Arc::new).collect())
}

impl From<OrchestrationV2ShellSnapshot> for ShellSnapshot {
    fn from(snapshot: OrchestrationV2ShellSnapshot) -> Self {
        Self {
            schema_version: snapshot.schema_version,
            snapshot_sequence: snapshot.snapshot_sequence,
            threads: rows(snapshot.threads),
            archived_threads: rows(snapshot.archived_threads),
            projects: rows(snapshot.projects),
            older_settled: snapshot.older_settled,
        }
    }
}

/// Keeps the previous `Arc` of every row that did not change (`reuseUnchangedThreadShells`).
pub fn reuse_unchanged_threads(previous: Option<&ShellSnapshot>, next: &mut ShellSnapshot) {
    let Some(previous) = previous.filter(|previous| !previous.threads.is_empty()) else { return };
    let by_id: HashMap<_, _> = previous.threads.iter().map(|thread| (&thread.id, thread)).collect();
    for row in Arc::make_mut(&mut next.threads) {
        if let Some(prior) = by_id.get(&row.id)
            && !Arc::ptr_eq(prior, row)
            && ***prior == **row
        {
            *row = Arc::clone(prior);
        }
    }
}

/// A project mutation can land with a null identity while an earlier snapshot already resolved
/// it; keep the prior identity for the same workspace root so multi-environment grouping holds.
fn retain_repository_identity(
    previous: Option<&Arc<OrchestrationProjectShell>>,
    mut next: OrchestrationProjectShell,
) -> OrchestrationProjectShell {
    if next.repository_identity.is_none()
        && let Some(previous) = previous
        && previous.repository_identity.is_some()
        && previous.workspace_root == next.workspace_root
    {
        next.repository_identity = previous.repository_identity.clone();
    }
    next
}

/// Merges a snapshot's projects with the held ones (`mergeShellSnapshotProjects`).
/// `resolved_roots` marks an enrichment refresh: rows and the cursor stay, and only listed
/// roots take the new repository identity.
pub fn merge_snapshot_projects(
    previous: Option<&ShellSnapshot>,
    mut next: ShellSnapshot,
    resolved_roots: Option<&[String]>,
) -> ShellSnapshot {
    let Some(previous) = previous else { return next };
    let by_id: HashMap<_, _> = previous.projects.iter().map(|project| (&project.id, project)).collect();
    if let Some(roots) = resolved_roots {
        let next_by_id: HashMap<_, _> = next.projects.iter().map(|project| (&project.id, project)).collect();
        let mut changed = false;
        let projects: Vec<_> = previous
            .projects
            .iter()
            .map(|project| {
                let Some(candidate) = next_by_id.get(&project.id).filter(|c| c.workspace_root == project.workspace_root) else {
                    return Arc::clone(project);
                };
                let take = if roots.contains(&project.workspace_root) {
                    project.repository_identity != candidate.repository_identity
                } else {
                    project.repository_identity.is_none() && candidate.repository_identity.is_some()
                };
                if !take {
                    return Arc::clone(project);
                }
                changed = true;
                let mut updated = (**project).clone();
                updated.repository_identity = candidate.repository_identity.clone();
                Arc::new(updated)
            })
            .collect();
        return if changed { ShellSnapshot { projects: Arc::new(projects), ..previous.clone() } } else { previous.clone() };
    }
    let projects: Vec<_> = next
        .projects
        .iter()
        .map(|project| {
            let prior = by_id.get(&project.id).copied();
            let merged = retain_repository_identity(prior, (**project).clone());
            match prior {
                Some(prior) if **prior == merged => Arc::clone(prior),
                _ => Arc::new(merged),
            }
        })
        .collect();
    let unchanged = projects.len() == previous.projects.len()
        && projects.iter().zip(previous.projects.iter()).all(|(a, b)| Arc::ptr_eq(a, b));
    next.projects = if unchanged { Arc::clone(&previous.projects) } else { Arc::new(projects) };
    next
}

/// One collection being edited by a batch: indexed on first use, copied on first change.
struct Batch<'a, K, T> {
    source: &'a Rows<T>,
    rows: Option<Vec<Option<Arc<T>>>>,
    index: Option<HashMap<K, usize>>,
    key: fn(&T) -> K,
}

impl<'a, K: Hash + Eq + Clone, T> Batch<'a, K, T> {
    fn new(source: &'a Rows<T>, key: fn(&T) -> K) -> Self {
        Self { source, rows: None, index: None, key }
    }

    fn index(&mut self) -> &mut HashMap<K, usize> {
        let key = self.key;
        let source = self.source;
        self.index.get_or_insert_with(|| source.iter().enumerate().map(|(i, row)| (key(row), i)).collect())
    }

    fn get(&mut self, id: &K) -> Option<Arc<T>> {
        let position = *self.index().get(id)?;
        match &self.rows {
            Some(rows) => rows[position].clone(),
            None => Some(Arc::clone(&self.source[position])),
        }
    }

    fn rows(&mut self) -> &mut Vec<Option<Arc<T>>> {
        let source = self.source;
        self.rows.get_or_insert_with(|| source.iter().cloned().map(Some).collect())
    }

    fn set(&mut self, row: T) {
        let id = (self.key)(&row);
        let position = self.index().get(&id).copied();
        let rows = self.rows();
        match position {
            Some(position) => rows[position] = Some(Arc::new(row)),
            None => {
                rows.push(Some(Arc::new(row)));
                let last = rows.len() - 1;
                self.index().insert(id, last);
            }
        }
    }

    fn remove(&mut self, id: &K) {
        if let Some(position) = self.index().remove(id) {
            self.rows()[position] = None;
        }
    }

    fn finish(self) -> Rows<T> {
        match self.rows {
            Some(rows) => Arc::new(rows.into_iter().flatten().collect()),
            None => Arc::clone(self.source),
        }
    }
}

/// Applies ordered deltas (`applyShellStreamEvents`). Items at or below the cursor are skipped;
/// a `thread.updated` equal to the held row keeps it. Returns `None` when nothing moved.
pub fn apply_shell_deltas(snapshot: &ShellSnapshot, items: &[Item]) -> Option<ShellSnapshot> {
    let mut projects = Batch::new(&snapshot.projects, |project: &OrchestrationProjectShell| project.id.clone());
    let mut threads = Batch::new(&snapshot.threads, |thread: &OrchestrationV2ThreadShell| thread.id.clone());
    let mut archived = Batch::new(&snapshot.archived_threads, |thread: &OrchestrationV2ThreadShell| thread.id.clone());
    let mut sequence = snapshot.snapshot_sequence;
    for item in items {
        match item {
            Item::ProjectUpdated(delta) if delta.sequence > sequence => {
                let previous = projects.get(&delta.project.id);
                projects.set(retain_repository_identity(previous.as_ref(), delta.project.clone()));
                sequence = delta.sequence;
            }
            Item::ProjectRemoved(delta) if delta.sequence > sequence => {
                projects.remove(&delta.project_id);
                sequence = delta.sequence;
            }
            Item::ThreadUpdated(delta) if delta.sequence > sequence => {
                if delta.location == Location::Active {
                    if threads.get(&delta.thread.id).is_none_or(|previous| *previous != delta.thread) {
                        threads.set(delta.thread.clone());
                    }
                } else {
                    threads.remove(&delta.thread.id);
                }
                // Legacy archive deltas leave the shell; the archive is queried on its own.
                archived.remove(&delta.thread.id);
                sequence = delta.sequence;
            }
            Item::ThreadRemoved(delta) if delta.sequence > sequence => {
                threads.remove(&delta.thread_id);
                archived.remove(&delta.thread_id);
                sequence = delta.sequence;
            }
            _ => {}
        }
    }
    (sequence != snapshot.snapshot_sequence).then(|| ShellSnapshot {
        snapshot_sequence: sequence,
        projects: projects.finish(),
        threads: threads.finish(),
        archived_threads: archived.finish(),
        ..snapshot.clone()
    })
}
