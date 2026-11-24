//! How a thread relates to others (`threadRelationships.ts`): its parent, forks, side chats,
//! subagents and context transfers, as a graph built from shell rows and the open thread's
//! projection. The thread details card's Lineage and Side chats sections read it.

use std::collections::HashMap;
use std::sync::Arc;

use t3code_contracts::{
    OrchestrationV2AppThreadLineage, OrchestrationV2AppThreadLineageRelationshipToParent as Relationship,
    OrchestrationV2ThreadShell, OrchestrationV2ThreadShellForkedFrom as ForkedFrom, ThreadId,
};
use t3code_shared::thread_projection::ThreadProjection;

use super::models::parse_ms;

/// What an edge records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThreadRelationshipKind {
    Fork,
    Subagent,
    Transfer,
}

/// A related thread's status as the rows label it (`threadRelationshipStatusLabel`'s inputs):
/// shell, run, subagent and transfer statuses share one vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelationshipStatus {
    Preparing,
    Starting,
    Running,
    InProgress,
    Pending,
    Queued,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Error,
    Cancelled,
    Interrupted,
    RolledBack,
    ResolvedNative,
    ResolvedPortable,
    Consumed,
    Superseded,
    Idle,
    /// A value this client does not know.
    Unknown,
}

impl RelationshipStatus {
    /// Reads a wire status.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "preparing" => Self::Preparing,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "in_progress" => Self::InProgress,
            "pending" => Self::Pending,
            "queued" => Self::Queued,
            "waiting" => Self::Waiting,
            "blocked" => Self::Blocked,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            "rolled_back" => Self::RolledBack,
            "resolved_native" => Self::ResolvedNative,
            "resolved_portable" => Self::ResolvedPortable,
            "consumed" => Self::Consumed,
            "superseded" => Self::Superseded,
            "idle" => Self::Idle,
            _ => Self::Unknown,
        }
    }

    /// `failed` or `error`.
    pub fn is_failed(self) -> bool {
        matches!(self, Self::Failed | Self::Error)
    }

    /// Settled agent work (`isFinishedAgentStatus`).
    pub fn is_finished_agent(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Error | Self::Cancelled | Self::Interrupted | Self::Idle)
    }

    /// The status dot's working colour (`running`, `in_progress`, `pending`, `waiting`).
    pub fn is_working(self) -> bool {
        matches!(self, Self::Running | Self::InProgress | Self::Pending | Self::Waiting)
    }
}

/// A thread in the graph; `missing` when only an edge names it (`thread` is then `None`).
#[derive(Clone, Debug)]
pub struct ThreadRelationshipNode {
    pub thread_id: ThreadId,
    pub thread: Option<Arc<OrchestrationV2ThreadShell>>,
    pub missing: bool,
}

/// One relationship, from parent (or transfer source) to child (or target).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRelationshipEdge {
    pub source_thread_id: ThreadId,
    pub target_thread_id: ThreadId,
    pub kind: ThreadRelationshipKind,
    pub status: Option<RelationshipStatus>,
}

/// Every known thread and relationship.
#[derive(Clone, Debug, Default)]
pub struct ThreadRelationshipGraph {
    pub nodes: HashMap<ThreadId, ThreadRelationshipNode>,
    pub edges: Vec<ThreadRelationshipEdge>,
}

/// A thread one edge away from the one shown.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadRelationshipWalkRow {
    pub thread_id: ThreadId,
    pub from_thread_id: ThreadId,
    pub depth: usize,
    pub edge: ThreadRelationshipEdge,
}

/// The thread a fork was made from: the recorded fork source, else the lineage parent.
fn parent_thread_id<'a>(forked_from: Option<&'a ForkedFrom>, lineage: &'a OrchestrationV2AppThreadLineage) -> Option<&'a ThreadId> {
    match forked_from {
        Some(ForkedFrom::Run(run)) => Some(&run.thread_id),
        _ => lineage.parent_thread_id.as_ref(),
    }
}

/// Where "Merge back" sends a fork's latest run (`resolveMergeBackTargetThreadId`); `None` for
/// anything but a fork.
pub fn resolve_merge_back_target_thread_id(projection: Option<&ThreadProjection>) -> Option<ThreadId> {
    let thread = &projection?.thread;
    if thread.lineage.relationship_to_parent != Some(Relationship::Fork) {
        return None;
    }
    parent_thread_id(thread.forked_from.as_ref(), &thread.lineage).cloned()
}

/// A shell's live status: its activity run, else its own status.
fn shell_status(thread: &OrchestrationV2ThreadShell) -> RelationshipStatus {
    RelationshipStatus::from_wire(thread.activity_run_status.map(|s| s.as_str()).unwrap_or(thread.status.as_str()))
}

/// Edges in first-insertion order, replaced in place when the same edge is added again (a JS
/// `Map` keyed by source, target and kind).
#[derive(Default)]
struct EdgeSet {
    edges: Vec<ThreadRelationshipEdge>,
    index: HashMap<(ThreadId, ThreadId, ThreadRelationshipKind), usize>,
}

impl EdgeSet {
    fn insert(&mut self, mut edge: ThreadRelationshipEdge) {
        if edge.kind == ThreadRelationshipKind::Subagent && edge.status == Some(RelationshipStatus::RolledBack) {
            edge.status = Some(RelationshipStatus::Interrupted);
        }
        let key = (edge.source_thread_id.clone(), edge.target_thread_id.clone(), edge.kind);
        match self.index.get(&key) {
            Some(&at) => self.edges[at] = edge,
            None => {
                self.index.insert(key, self.edges.len());
                self.edges.push(edge);
            }
        }
    }
}

/// Builds the graph (`deriveThreadRelationshipGraph`). `threads` run from most to least
/// authoritative: live shells before archived snapshots, which may hold a stale copy.
pub fn derive_thread_relationship_graph<'a>(
    threads: impl IntoIterator<Item = &'a Arc<OrchestrationV2ThreadShell>>,
    projection: Option<&ThreadProjection>,
) -> ThreadRelationshipGraph {
    let mut ordered: Vec<&Arc<OrchestrationV2ThreadShell>> = Vec::new();
    let mut nodes: HashMap<ThreadId, ThreadRelationshipNode> = HashMap::new();
    for thread in threads {
        if !nodes.contains_key(&thread.id) {
            nodes.insert(thread.id.clone(), ThreadRelationshipNode { thread_id: thread.id.clone(), thread: Some(Arc::clone(thread)), missing: false });
            ordered.push(thread);
        }
    }
    let mut edges = EdgeSet::default();
    let mut add = |nodes: &mut HashMap<ThreadId, ThreadRelationshipNode>, edge: ThreadRelationshipEdge| {
        for id in [&edge.source_thread_id, &edge.target_thread_id] {
            nodes.entry(id.clone()).or_insert_with(|| ThreadRelationshipNode { thread_id: id.clone(), thread: None, missing: true });
        }
        edges.insert(edge);
    };
    let lineage_edge = |id: &ThreadId, parent: &ThreadId, relationship: Option<Relationship>, status| ThreadRelationshipEdge {
        source_thread_id: parent.clone(),
        target_thread_id: id.clone(),
        kind: if relationship == Some(Relationship::Subagent) { ThreadRelationshipKind::Subagent } else { ThreadRelationshipKind::Fork },
        status,
    };
    for thread in &ordered {
        if let Some(parent) = parent_thread_id(thread.forked_from.as_ref(), &thread.lineage) {
            let edge = lineage_edge(&thread.id, parent, thread.lineage.relationship_to_parent, Some(shell_status(thread)));
            add(&mut nodes, edge);
        }
    }
    // The open thread may be left out of the shell (an idle child); its own lineage still counts,
    // with no status.
    if let Some(selected) = projection.map(|p| &p.thread).filter(|t| !ordered.iter().any(|s| s.id == t.id))
        && let Some(parent) = parent_thread_id(selected.forked_from.as_ref(), &selected.lineage)
    {
        let edge = lineage_edge(&selected.id, parent, selected.lineage.relationship_to_parent, None);
        add(&mut nodes, edge);
    }
    if let Some(projection) = projection {
        let owner = &projection.thread.id;
        let shells: HashMap<&ThreadId, &Arc<OrchestrationV2ThreadShell>> = ordered.iter().map(|t| (&t.id, *t)).collect();
        for subagent in projection.subagents.iter() {
            let Some(child_id) = &subagent.child_thread_id else { continue };
            let child = shells.get(child_id);
            let work_active = matches!(subagent.status.as_str(), "pending" | "running" | "waiting");
            let status = match child {
                Some(child) if child.activity_run_status.is_some() => shell_status(child),
                Some(child) if child.latest_run_id.is_some() && !work_active => RelationshipStatus::from_wire(child.status.as_str()),
                _ => RelationshipStatus::from_wire(subagent.status.as_str()),
            };
            let edge = ThreadRelationshipEdge {
                source_thread_id: owner.clone(),
                target_thread_id: child_id.clone(),
                kind: ThreadRelationshipKind::Subagent,
                status: Some(status),
            };
            add(&mut nodes, edge);
        }
        for transfer in projection.context_transfers.iter() {
            if transfer.source_thread_id == transfer.target_thread_id {
                continue;
            }
            let edge = ThreadRelationshipEdge {
                source_thread_id: transfer.source_thread_id.clone(),
                target_thread_id: transfer.target_thread_id.clone(),
                kind: ThreadRelationshipKind::Transfer,
                status: Some(RelationshipStatus::from_wire(transfer.status.as_str())),
            };
            add(&mut nodes, edge);
        }
    }
    ThreadRelationshipGraph { nodes, edges: edges.edges }
}

/// Each thread one edge from `thread_id`, once, in edge order (`immediateThreadRelationships`).
pub fn immediate_thread_relationships(graph: &ThreadRelationshipGraph, thread_id: &ThreadId) -> Vec<ThreadRelationshipWalkRow> {
    let mut seen = std::collections::HashSet::new();
    graph
        .edges
        .iter()
        .filter_map(|edge| {
            let related = if &edge.source_thread_id == thread_id {
                &edge.target_thread_id
            } else if &edge.target_thread_id == thread_id {
                &edge.source_thread_id
            } else {
                return None;
            };
            seen.insert(related.clone()).then(|| ThreadRelationshipWalkRow {
                thread_id: related.clone(),
                from_thread_id: thread_id.clone(),
                depth: 1,
                edge: edge.clone(),
            })
        })
        .collect()
}

/// Whether the edge points at `current` from its parent (`isParentThreadRelationship`).
pub fn is_parent_thread_relationship(edge: &ThreadRelationshipEdge, current: &ThreadId) -> bool {
    edge.kind != ThreadRelationshipKind::Transfer && &edge.target_thread_id == current
}

/// The status a row shows (`threadRelationshipRowStatus`): a child's edge status, else the
/// related thread's own.
pub fn thread_relationship_row_status(graph: &ThreadRelationshipGraph, row: &ThreadRelationshipWalkRow) -> Option<RelationshipStatus> {
    if row.edge.kind == ThreadRelationshipKind::Transfer || row.thread_id == row.edge.target_thread_id {
        return row.edge.status;
    }
    graph.nodes.get(&row.thread_id)?.thread.as_deref().map(shell_status)
}

fn created_at_ms(node: Option<&ThreadRelationshipNode>) -> Option<i64> {
    parse_ms(&node?.thread.as_ref()?.created_at)
}

/// The web card's order (`orderWebThreadLineageRows`): the parent, then a distinct merge-back
/// target, then newest-created first; missing and undated threads sink, ties go by id.
pub fn order_web_thread_lineage_rows(
    graph: &ThreadRelationshipGraph,
    rows: &[ThreadRelationshipWalkRow],
    current: &ThreadId,
    merge_target: Option<&ThreadId>,
) -> Vec<ThreadRelationshipWalkRow> {
    let pin = |row: &ThreadRelationshipWalkRow| {
        if is_parent_thread_relationship(&row.edge, current) {
            0
        } else if Some(&row.thread_id) == merge_target {
            1
        } else {
            2
        }
    };
    let mut rows = rows.to_vec();
    rows.sort_by(|left, right| {
        pin(left).cmp(&pin(right)).then_with(|| {
            let (l, r) = (created_at_ms(graph.nodes.get(&left.thread_id)), created_at_ms(graph.nodes.get(&right.thread_id)));
            match (l, r) {
                (Some(l), Some(r)) if l != r => r.cmp(&l),
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                _ => left.thread_id.as_str().cmp(right.thread_id.as_str()),
            }
        })
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_fixtures::{app_thread, projection, shell, subagent, transfer};
    use serde_json::{Value, json};

    fn id(value: &str) -> ThreadId {
        ThreadId::new(value)
    }

    fn shells(values: Vec<Value>) -> Vec<Arc<OrchestrationV2ThreadShell>> {
        values.into_iter().map(|v| Arc::new(shell(v))).collect()
    }

    fn lineage(parent: Option<&str>, relationship: Option<&str>) -> Value {
        json!({"rootThreadId": parent.unwrap_or("root"), "parentThreadId": parent, "relationshipToParent": relationship})
    }

    #[test]
    fn keeps_parent_status_independent_of_child() {
        for (parent_status, child_status) in [("running", "completed"), ("completed", "running")] {
            let threads = shells(vec![
                json!({"id": "parent", "activityRunStatus": (parent_status == "running").then_some("running"), "status": if parent_status == "running" { "cancelled" } else { parent_status }, "lineage": lineage(None, None)}),
                json!({"id": "child", "activityRunStatus": (child_status == "running").then_some("running"), "status": if child_status == "running" { "cancelled" } else { child_status }, "lineage": lineage(Some("parent"), Some("fork"))}),
            ]);
            let graph = derive_thread_relationship_graph(&threads, None);
            let child_row = &immediate_thread_relationships(&graph, &id("child"))[0];
            assert_eq!(thread_relationship_row_status(&graph, child_row), Some(RelationshipStatus::from_wire(parent_status)));
            let parent_row = &immediate_thread_relationships(&graph, &id("parent"))[0];
            assert_eq!(thread_relationship_row_status(&graph, parent_row), Some(RelationshipStatus::from_wire(child_status)));
        }
    }

    #[test]
    fn does_not_label_a_missing_parent_with_its_childs_running_status() {
        let threads = shells(vec![json!({"id": "child", "status": "running", "lineage": lineage(Some("missing-parent"), Some("subagent"))})]);
        let graph = derive_thread_relationship_graph(&threads, None);
        let row = &immediate_thread_relationships(&graph, &id("child"))[0];
        assert_eq!(thread_relationship_row_status(&graph, row), None);
    }

    #[test]
    fn keeps_the_parent_edge_of_an_opened_child_omitted_from_the_shell() {
        for relationship in ["fork", "subagent"] {
            let forked_from = (relationship == "fork").then(|| json!({"type": "run", "threadId": "parent", "runId": "source-run"}));
            let projection = projection(
                app_thread(json!({"id": "omitted-child", "forkedFrom": forked_from, "lineage": {"rootThreadId": "parent", "parentThreadId": "parent", "relationshipToParent": relationship}})),
                json!({}),
            );
            let graph = derive_thread_relationship_graph(&[], Some(&projection));
            let rows = immediate_thread_relationships(&graph, &id("omitted-child"));
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].thread_id, id("parent"));
            assert_eq!(rows[0].edge.source_thread_id, id("parent"));
            assert_eq!(rows[0].edge.target_thread_id, id("omitted-child"));
            let kind = if relationship == "fork" { ThreadRelationshipKind::Fork } else { ThreadRelationshipKind::Subagent };
            assert_eq!(rows[0].edge.kind, kind);
            assert_eq!(resolve_merge_back_target_thread_id(Some(&projection)), (relationship == "fork").then(|| id("parent")));
        }
    }

    #[test]
    fn keeps_an_older_activity_run_visible_over_a_newer_cancelled_run() {
        let threads = shells(vec![json!({
            "id": "thread-child", "title": "Active child", "activityRunStatus": "running", "status": "cancelled",
            "forkedFrom": {"type": "run", "threadId": "thread-parent", "runId": "run-parent"},
            "lineage": {"rootThreadId": "thread-parent", "parentThreadId": "thread-parent", "relationshipToParent": "subagent"},
        })]);
        let graph = derive_thread_relationship_graph(&threads, None);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].target_thread_id, id("thread-child"));
        assert_eq!(graph.edges[0].status, Some(RelationshipStatus::Running));
    }

    #[test]
    fn keeps_missing_parents_and_cycles_navigable_without_recursive_traversal() {
        let threads = shells(vec![
            json!({"id": "thread-root", "title": "Root", "status": "completed", "forkedFrom": {"type": "run", "threadId": "thread-child", "runId": "run-cycle"}, "lineage": {"rootThreadId": "thread-root", "parentThreadId": "thread-child", "relationshipToParent": "fork"}}),
            json!({"id": "thread-child", "title": "Child", "status": "completed", "forkedFrom": {"type": "run", "threadId": "thread-missing", "runId": "run-missing"}, "lineage": {"rootThreadId": "thread-root", "parentThreadId": "thread-missing", "relationshipToParent": "fork"}}),
        ]);
        let graph = derive_thread_relationship_graph(&threads, None);
        assert!(graph.nodes[&id("thread-missing")].missing);
        let ids: Vec<_> = immediate_thread_relationships(&graph, &id("thread-root")).into_iter().map(|r| r.thread_id).collect();
        assert_eq!(ids, vec![id("thread-child")]);
    }

    #[test]
    fn combines_subagent_and_transfer_edges_with_archived_shell_state() {
        let threads = shells(vec![
            json!({"id": "thread-parent", "title": "Parent", "status": "completed", "lineage": {"rootThreadId": "thread-parent", "parentThreadId": null, "relationshipToParent": null}}),
            json!({"id": "thread-child", "title": "Subagent", "status": "completed", "archivedAt": "2026-06-24T00:00:00.000Z", "lineage": {"rootThreadId": "thread-parent", "parentThreadId": "thread-parent", "relationshipToParent": "subagent"}}),
        ]);
        let projection = projection(
            app_thread(json!({"id": "thread-parent"})),
            json!({
                "subagents": [subagent(json!({"childThreadId": "thread-child", "status": "completed"}))],
                "contextTransfers": [transfer("thread-child", "thread-transfer", "consumed")],
            }),
        );
        let graph = derive_thread_relationship_graph(&threads, Some(&projection));
        assert!(graph.nodes[&id("thread-child")].thread.as_ref().unwrap().archived_at.is_some());
        assert!(graph.nodes[&id("thread-transfer")].missing);
        assert!(graph.edges.iter().any(|e| e.source_thread_id == id("thread-parent") && e.target_thread_id == id("thread-child") && e.kind == ThreadRelationshipKind::Subagent));
        assert!(graph.edges.iter().any(|e| e.source_thread_id == id("thread-child") && e.target_thread_id == id("thread-transfer") && e.kind == ThreadRelationshipKind::Transfer));
    }

    #[test]
    fn keeps_the_live_shell_when_an_archived_snapshot_contains_the_same_thread_id() {
        let live = json!({"id": "thread-child", "title": "Live child", "status": "running", "archivedAt": null,
            "forkedFrom": {"type": "run", "threadId": "thread-parent", "runId": "run-live"},
            "lineage": {"rootThreadId": "thread-parent", "parentThreadId": "thread-parent", "relationshipToParent": "fork"}});
        let stale = json!({"id": "thread-child", "title": "Stale archived child", "status": "completed", "archivedAt": "2026-06-24T00:00:00.000Z",
            "forkedFrom": {"type": "run", "threadId": "thread-stale-parent", "runId": "run-stale"},
            "lineage": {"rootThreadId": "thread-stale-parent", "parentThreadId": "thread-stale-parent", "relationshipToParent": "fork"}});
        let threads = shells(vec![live, stale]);
        let graph = derive_thread_relationship_graph(&threads, None);
        let child = graph.nodes[&id("thread-child")].thread.clone().unwrap();
        assert_eq!(child.title, "Live child");
        assert_eq!(child.status.as_str(), "running");
        assert!(child.archived_at.is_none());
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].source_thread_id, id("thread-parent"));
        assert!(!graph.nodes.contains_key(&id("thread-stale-parent")));
    }

    #[test]
    fn resolves_merge_back_only_for_forks_and_prefers_the_recorded_fork_source() {
        let make = |forked_from: Value, relationship: &str| {
            projection(
                app_thread(json!({"id": "thread-fork", "forkedFrom": forked_from,
                    "lineage": {"rootThreadId": "thread-source", "parentThreadId": "thread-parent", "relationshipToParent": relationship}})),
                json!({}),
            )
        };
        let recorded = make(json!({"type": "run", "threadId": "thread-source", "runId": "run-source"}), "fork");
        assert_eq!(resolve_merge_back_target_thread_id(Some(&recorded)), Some(id("thread-source")));
        assert_eq!(resolve_merge_back_target_thread_id(Some(&make(Value::Null, "fork"))), Some(id("thread-parent")));
        assert_eq!(resolve_merge_back_target_thread_id(Some(&make(Value::Null, "subagent"))), None);
    }

    fn fork_shell(thread: &str, parent: Option<&str>, created_at: Option<&str>, updated_at: Option<&str>, status: Option<&str>) -> Value {
        json!({
            "id": thread, "title": thread, "status": status.unwrap_or("completed"), "archivedAt": null,
            "forkedFrom": parent.map(|p| json!({"type": "run", "threadId": p, "runId": format!("run-{thread}")})),
            "lineage": {"rootThreadId": parent.unwrap_or(thread), "parentThreadId": parent, "relationshipToParent": parent.map(|_| "fork")},
            // The fork's test leaves createdAt undecoded; an unparseable value is the same here.
            "createdAt": created_at.unwrap_or("undated"),
            "updatedAt": updated_at.unwrap_or("2026-06-20T00:00:00.000Z"),
        })
    }

    fn ordered_lineage_ids(threads: Vec<Value>, merge_target: Option<&str>, projection: Option<ThreadProjection>) -> Vec<ThreadId> {
        let threads = shells(threads);
        let graph = derive_thread_relationship_graph(&threads, projection.as_ref());
        let current = id("thread-current");
        let rows = immediate_thread_relationships(&graph, &current);
        order_web_thread_lineage_rows(&graph, &rows, &current, merge_target.map(id).as_ref()).into_iter().map(|r| r.thread_id).collect()
    }

    #[test]
    fn does_not_reorder_when_related_thread_activity_arrives() {
        let c = "thread-current";
        let before = ordered_lineage_ids(
            vec![
                fork_shell(c, None, Some("2026-06-01T00:00:00.000Z"), None, None),
                fork_shell("thread-fork-a", Some(c), Some("2026-06-02T00:00:00.000Z"), None, None),
                fork_shell("thread-fork-b", Some(c), Some("2026-06-03T00:00:00.000Z"), None, None),
                fork_shell("thread-fork-c", Some(c), Some("2026-06-04T00:00:00.000Z"), None, None),
            ],
            None,
            None,
        );
        let after = ordered_lineage_ids(
            vec![
                fork_shell("thread-fork-b", Some(c), Some("2026-06-03T00:00:00.000Z"), Some("2026-07-30T00:00:00.000Z"), Some("running")),
                fork_shell("thread-fork-c", Some(c), Some("2026-06-04T00:00:00.000Z"), None, None),
                fork_shell(c, None, Some("2026-06-01T00:00:00.000Z"), None, None),
                fork_shell("thread-fork-a", Some(c), Some("2026-06-02T00:00:00.000Z"), Some("2026-07-31T00:00:00.000Z"), Some("failed")),
            ],
            None,
            None,
        );
        assert_eq!(before, vec![id("thread-fork-c"), id("thread-fork-b"), id("thread-fork-a")]);
        assert_eq!(after, before);
    }

    #[test]
    fn pins_the_parent_first_and_a_distinct_merge_back_target_second() {
        let c = "thread-current";
        let ids = ordered_lineage_ids(
            vec![
                fork_shell("thread-parent", None, Some("2026-06-01T00:00:00.000Z"), None, None),
                fork_shell(c, Some("thread-parent"), Some("2026-06-02T00:00:00.000Z"), None, None),
                fork_shell("thread-merge-target", Some(c), Some("2026-06-03T00:00:00.000Z"), None, None),
                fork_shell("thread-newest-fork", Some(c), Some("2026-06-09T00:00:00.000Z"), None, None),
            ],
            Some("thread-merge-target"),
            None,
        );
        assert_eq!(ids, vec![id("thread-parent"), id("thread-merge-target"), id("thread-newest-fork")]);
    }

    #[test]
    fn sinks_missing_nodes_and_shells_without_a_decoded_created_at() {
        let c = "thread-current";
        let projection = projection(
            app_thread(json!({"id": c})),
            json!({"contextTransfers": [transfer(c, "thread-missing-transfer", "consumed")]}),
        );
        let ids = ordered_lineage_ids(
            vec![
                fork_shell(c, None, Some("2026-06-01T00:00:00.000Z"), None, None),
                fork_shell("thread-undated-fork", Some(c), None, None, None),
                fork_shell("thread-dated-fork", Some(c), Some("2026-06-02T00:00:00.000Z"), None, None),
            ],
            None,
            Some(projection),
        );
        assert_eq!(ids, vec![id("thread-dated-fork"), id("thread-missing-transfer"), id("thread-undated-fork")]);
    }
}
