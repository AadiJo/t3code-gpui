//! Thread rows across environments (`threadShell.ts`): child request roll-up, which rows the
//! sidebar lists, and presented rows reused while their source row is unchanged.
//!
//! Lists concatenate enabled environments in order. A missing row never means a deleted thread:
//! servers leave idle subagents and older settled threads out of the shell.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use t3code_contracts::{
    EnvironmentId, OrchestrationV2AppThreadLineageRelationshipToParent as Relationship,
    OrchestrationV2CreationSource, OrchestrationV2PendingRuntimeRequestSummaryKind as RequestKind,
    OrchestrationV2ShellThreadStatus as ShellStatus, OrchestrationV2ThreadShell, ThreadId,
};

use super::models::{PendingSubagentRequest, ThreadRow, is_active_status, present_thread_shell};

type Threads = [Arc<OrchestrationV2ThreadShell>];

/// A user-only question keeps its mark through roll-ups; views see it as user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RolledUp {
    Approval,
    UserInput,
    UserOnlyInput,
}

impl From<RolledUp> for PendingSubagentRequest {
    fn from(request: RolledUp) -> Self {
        match request {
            RolledUp::Approval => Self::Approval,
            RolledUp::UserInput | RolledUp::UserOnlyInput => Self::UserInput,
        }
    }
}

fn is_child_run_live(thread: &OrchestrationV2ThreadShell) -> bool {
    match thread.activity_run_status {
        Some(_) => true,
        None => matches!(thread.status, ShellStatus::Preparing | ShellStatus::Starting | ShellStatus::Running | ShellStatus::Waiting),
    }
}

/// What a subagent or delegated child asks; provider-native subagents ask on their parent.
fn subagent_request(thread: &OrchestrationV2ThreadShell) -> Option<RolledUp> {
    let kind = thread.pending_runtime_request.as_ref()?.kind;
    let child = thread.lineage.relationship_to_parent == Some(Relationship::Subagent) || thread.delegated_from.is_some();
    let provider_native =
        thread.lineage.relationship_to_parent == Some(Relationship::Subagent) && thread.creation_source == OrchestrationV2CreationSource::Provider;
    if kind == RequestKind::AuthRefresh || !child || provider_native || !is_child_run_live(thread) {
        return None;
    }
    Some(match kind {
        RequestKind::UserInput if thread.pending_runtime_request.as_ref().and_then(|r| r.user_only) == Some(true) => RolledUp::UserOnlyInput,
        RequestKind::UserInput => RolledUp::UserInput,
        _ => RolledUp::Approval,
    })
}

/// A side chat asks its user directly, so questions roll up as well as approvals.
fn side_chat_request(thread: &OrchestrationV2ThreadShell) -> Option<RolledUp> {
    let kind = thread.pending_runtime_request.as_ref()?.kind;
    if kind == RequestKind::AuthRefresh || thread.lineage.side_chat != Some(true) {
        return None;
    }
    Some(if kind == RequestKind::UserInput { RolledUp::UserInput } else { RolledUp::Approval })
}

/// The threads to flag for children's requests, up the subagent and side-chat chain
/// (`pendingSubagentRequestsByThread`). The coordinator answers subagent questions, so only
/// approvals and user-only questions climb.
fn requests_by_thread(threads: &Threads, delegated: &[(ThreadId, RolledUp)]) -> HashMap<ThreadId, RolledUp> {
    let mut requests: HashMap<ThreadId, RolledUp> = HashMap::new();
    let by_id: HashMap<&ThreadId, &Arc<OrchestrationV2ThreadShell>> = threads.iter().map(|thread| (&thread.id, thread)).collect();
    let flag = |requests: &mut HashMap<ThreadId, RolledUp>, first: Option<&ThreadId>, request: RolledUp, mut seen: HashSet<ThreadId>| {
        if request == RolledUp::UserInput {
            return;
        }
        let mut parent_id = first.cloned();
        while let Some(id) = parent_id.filter(|id| !seen.contains(id)) {
            seen.insert(id.clone());
            if requests.get(&id) != Some(&RolledUp::Approval) {
                requests.insert(id.clone(), request);
            }
            let parent = by_id.get(&id);
            parent_id = parent
                .filter(|parent| parent.lineage.relationship_to_parent == Some(Relationship::Subagent) || parent.lineage.side_chat == Some(true))
                .and_then(|parent| parent.lineage.parent_thread_id.clone());
        }
    };
    for child in threads {
        let parent_id = child.lineage.parent_thread_id.as_ref();
        if child.lineage.side_chat == Some(true) {
            if let (Some(parent_id), Some(request)) = (parent_id, side_chat_request(child))
                && requests.get(parent_id) != Some(&RolledUp::Approval)
            {
                requests.insert(parent_id.clone(), request);
            }
            continue;
        }
        if child.lineage.relationship_to_parent != Some(Relationship::Subagent) {
            continue;
        }
        if let Some(request) = subagent_request(child) {
            flag(&mut requests, parent_id, request, HashSet::from([child.id.clone()]));
        }
    }
    for (parent_id, request) in delegated {
        flag(&mut requests, Some(parent_id), *request, HashSet::new());
    }
    requests
}

/// Child requests rolled up to each environment's threads, following tasks delegated across
/// environments to a fixed point.
pub fn pending_subagent_requests(environments: &[(EnvironmentId, &Threads)]) -> HashMap<EnvironmentId, HashMap<ThreadId, PendingSubagentRequest>> {
    let local: HashMap<&EnvironmentId, HashMap<ThreadId, RolledUp>> =
        environments.iter().map(|(id, threads)| (id, requests_by_thread(threads, &[]))).collect();
    let mut tasks_by_delegator: HashMap<&EnvironmentId, Vec<(&EnvironmentId, &Arc<OrchestrationV2ThreadShell>)>> = HashMap::new();
    for (runs_in, threads) in environments {
        for thread in threads.iter() {
            if let Some(from) = &thread.delegated_from
                && &from.environment_id != runs_in
            {
                tasks_by_delegator.entry(&from.environment_id).or_default().push((runs_in, thread));
            }
        }
    }
    let mut flags: HashMap<EnvironmentId, HashMap<ThreadId, RolledUp>> = HashMap::new();
    let mut changed = !tasks_by_delegator.is_empty();
    while changed {
        changed = false;
        let mut next = flags.clone();
        for (environment_id, tasks) in &tasks_by_delegator {
            let requests_of = |id: &EnvironmentId| flags.get(id).or_else(|| local.get(id)).cloned().unwrap_or_default();
            let delegated: Vec<(ThreadId, RolledUp)> = tasks
                .iter()
                .filter_map(|(runs_in, thread)| {
                    let own = subagent_request(thread);
                    let nested = requests_of(runs_in).get(&thread.id).copied();
                    let request = if own == Some(RolledUp::Approval) { own } else { nested.or(own) };
                    Some((thread.delegated_from.as_ref()?.thread_id.clone(), request?))
                })
                .collect();
            if delegated.is_empty() {
                continue;
            }
            let Some(threads) = environments.iter().find(|(id, _)| id == *environment_id).map(|(_, threads)| *threads) else { continue };
            let requests = requests_by_thread(threads, &delegated);
            if requests_of(environment_id) == requests {
                continue;
            }
            next.insert((*environment_id).clone(), requests);
            changed = true;
        }
        flags = next;
    }
    environments
        .iter()
        .map(|(id, _)| {
            let rolled = flags.get(id).or_else(|| local.get(id)).cloned().unwrap_or_default();
            (id.clone(), rolled.into_iter().map(|(thread, request)| (thread, request.into())).collect())
        })
        .collect()
}

/// Presents rows across environments and keeps each presented row while its source row and
/// roll-up are unchanged, so views can compare by `Arc::ptr_eq`.
#[derive(Default)]
pub struct ShellRows {
    memo: HashMap<(EnvironmentId, ThreadId), Arc<ThreadRow>>,
}

impl ShellRows {
    /// Every row of the given shells, presented, in environment order.
    pub fn present(&mut self, environments: &[(EnvironmentId, &Threads)]) -> Vec<Arc<ThreadRow>> {
        let requests = pending_subagent_requests(environments);
        let mut next = HashMap::with_capacity(self.memo.len());
        let mut rows = Vec::new();
        for (environment_id, threads) in environments {
            let rolled = requests.get(environment_id);
            for thread in threads.iter() {
                let request = rolled.and_then(|requests| requests.get(&thread.id)).copied();
                let key = (environment_id.clone(), thread.id.clone());
                let row = match self.memo.remove(&key) {
                    Some(row) if Arc::ptr_eq(&row.source, thread) && row.pending_subagent_request == request => row,
                    _ => Arc::new(present_thread_shell(environment_id, thread, request)),
                };
                next.insert(key, Arc::clone(&row));
                rows.push(row);
            }
        }
        self.memo = next;
        rows
    }
}

/// Keys of delegated tasks whose parent is listed in an enabled environment and not archived;
/// those show under the parent (`delegatedUnderParentAtom`).
pub fn delegated_under_parent(rows: &[Arc<ThreadRow>]) -> HashSet<(EnvironmentId, ThreadId)> {
    let index: HashMap<(&EnvironmentId, &ThreadId), &Arc<ThreadRow>> = rows.iter().map(|row| ((&row.environment_id, row.id()), row)).collect();
    rows.iter()
        .filter(|row| {
            row.source.delegated_from.as_ref().is_some_and(|from| {
                index.get(&(&from.environment_id, &from.thread_id)).is_some_and(|parent| parent.source.archived_at.is_none())
            })
        })
        .map(|row| (row.environment_id.clone(), row.id().clone()))
        .collect()
}

/// The rows the sidebar lists: not archived, not shown under a parent, not hidden by a tag
/// filter (`navigationThreadShellsAtom`).
pub fn navigation_rows(rows: &[Arc<ThreadRow>], hidden: &HashSet<(EnvironmentId, ThreadId)>) -> Vec<Arc<ThreadRow>> {
    let under_parent = delegated_under_parent(rows);
    rows.iter()
        .filter(|row| {
            let key = (row.environment_id.clone(), row.id().clone());
            row.source.archived_at.is_none()
                && row.source.lineage.is_listed()
                && !under_parent.contains(&key)
                && !hidden.contains(&key)
        })
        .cloned()
        .collect()
}

/// Whether two presentations of a row differ only by what a streaming run ticks: `updatedAt`,
/// item counts past zero, and the latest message (`sameApartFromStreaming`). A "quiet" view
/// keeps the previous row then, which is the main render-cost control while text streams.
pub fn same_apart_from_streaming(previous: &ThreadRow, next: &ThreadRow) -> bool {
    if previous.environment_id != next.environment_id
        || previous.pending_subagent_request != next.pending_subagent_request
        || !next.runtime.as_ref().is_some_and(|runtime| is_active_status(runtime.status))
    {
        return false;
    }
    let (before, after) = (&*previous.source, &*next.source);
    if (before.item_count > 0) != (after.item_count > 0) || (before.visible_item_count > 0) != (after.visible_item_count > 0) {
        return false;
    }
    let mut patched = before.clone();
    patched.updated_at.clone_from(&after.updated_at);
    patched.item_count = after.item_count;
    patched.visible_item_count = after.visible_item_count;
    patched.latest_visible_message.clone_from(&after.latest_visible_message);
    patched == *after
}

/// The nearest listed ancestor of a thread that has no row of its own (subagent, side chat,
/// delegated task), for highlighting the row that represents it
/// (`resolveListedAncestorThreadRef`).
pub fn listed_ancestor(rows: &[Arc<ThreadRow>], environment_id: &EnvironmentId, thread_id: &ThreadId) -> (EnvironmentId, ThreadId) {
    let find = |environment: &EnvironmentId, thread: &ThreadId| rows.iter().find(|row| &row.environment_id == environment && row.id() == thread);
    let start = (environment_id.clone(), thread_id.clone());
    let mut current = start.clone();
    let mut visited = HashSet::new();
    while let Some(row) = find(&current.0, &current.1) {
        let parent = if !row.source.lineage.is_listed() {
            match &row.source.lineage.parent_thread_id {
                Some(parent) => (row.environment_id.clone(), parent.clone()),
                None => return start,
            }
        } else if let Some(from) = &row.source.delegated_from {
            let parent = (from.environment_id.clone(), from.thread_id.clone());
            if find(&parent.0, &parent.1).is_none_or(|parent| parent.source.archived_at.is_some()) {
                return current;
            }
            parent
        } else {
            return current;
        };
        if !visited.insert(current.clone()) {
            return start;
        }
        current = parent;
    }
    start
}
