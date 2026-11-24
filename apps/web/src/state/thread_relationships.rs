//! The thread details card's Lineage and Side chats rows (`ThreadRelationshipsPanel` in
//! `ThreadRelationshipsControl.tsx`): the relationship graph of the shown thread grouped into
//! related threads, active and previous agents and side chats, with the merge-back target, the
//! disconnect action and the section headings' running counts.

use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_client_runtime::state::subagent_runtime::{derive_workflow_groups, format_subagent_display_title, projected_subagents_to_runtime};
use t3code_client_runtime::state::thread_relationships::{
    RelationshipStatus, ThreadRelationshipKind, ThreadRelationshipWalkRow, derive_thread_relationship_graph,
    immediate_thread_relationships, is_parent_thread_relationship, order_web_thread_lineage_rows,
    resolve_merge_back_target_thread_id, thread_relationship_row_status,
};
use t3code_client_runtime::state::thread_workflows::{can_detach_thread_provider_session, resolve_latest_merge_back_run};
use t3code_contracts::{OrchestrationV2DelegatedFrom, OrchestrationV2ThreadShell, RunId, ThreadId};
use t3code_shared::thread_projection::ThreadProjection;

use crate::i18n::t;

/// The row's fallback icon (`RelationshipIcon`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipIcon {
    /// `CornerLeftUpIcon`: the parent.
    Parent,
    /// `BotIcon`: a subagent.
    Agent,
    /// `MessagesSquareIcon`: a side chat.
    SideChat,
    /// `GitForkIcon`: a fork or transfer.
    Fork,
}

/// One related thread.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipRow {
    pub thread_id: ThreadId,
    pub title: SharedString,
    pub icon: RelationshipIcon,
    pub status: Option<RelationshipStatus>,
    /// What a click does, under the title in the tooltip (`relationshipHint`).
    pub hint: SharedString,
    /// The fork's merge-back target: the row splits into open, merge and its status.
    pub merge_target: bool,
}

/// A collapsible group of agent rows (`ThreadLineageGroup`); `label` is `None` for the
/// headingless groups.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipGroup {
    pub id: &'static str,
    pub label: Option<SharedString>,
    pub rows: Vec<RelationshipRow>,
    pub expanded: bool,
}

/// Merge back from the shown fork into its source.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeBack {
    pub target: ThreadId,
    /// The target's title, unless it is hidden or unknown.
    pub parent_title: Option<SharedString>,
    /// The run merged; `None` until a run in this fork completes.
    pub run_id: Option<RunId>,
}

/// The thread on a linked environment whose agent delegated this one.
#[derive(Clone, Debug, PartialEq)]
pub struct DelegatedFromRow {
    pub from: OrchestrationV2DelegatedFrom,
    pub title: SharedString,
    /// The environment's label (connected, or as recorded at link time).
    pub label: Option<SharedString>,
    /// This client is connected to that environment.
    pub connected: bool,
}

/// What the Lineage and Side chats sections show; `None` hides both.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipsInput {
    pub lineage_visible: bool,
    pub side_chats_visible: bool,
    pub groups: Vec<RelationshipGroup>,
    pub side_chats: Vec<RelationshipRow>,
    pub running_count: usize,
    pub running_side_chats: usize,
    /// "Lineage" heading shows (a lone "Previous agents" group names itself).
    pub show_heading: bool,
    pub can_detach: bool,
    pub merge: Option<MergeBack>,
    pub delegated_from: Option<DelegatedFromRow>,
    /// A rel row's status dot sits above the icon when any provider instance shows a badge.
    pub any_instance_badge: bool,
}

/// What the derivation reads.
pub struct RelationshipSources<'a> {
    pub thread_id: &'a ThreadId,
    /// Live shells first, then archived ones.
    pub shells: Vec<&'a Arc<OrchestrationV2ThreadShell>>,
    pub projection: Option<&'a ThreadProjection>,
    /// Threads hidden by the environment's hiding rules.
    pub is_hidden: &'a dyn Fn(&ThreadId) -> bool,
    pub hiding_anywhere: bool,
    pub show_lineage: bool,
    pub show_side_chats: bool,
    pub any_instance_badge: bool,
    /// The delegating environment's label when this client is connected to it.
    pub connected_label: &'a dyn Fn(&t3code_contracts::EnvironmentId) -> Option<SharedString>,
}

/// `relationshipHint`.
fn hint(kind: ThreadRelationshipKind, source: &ThreadId, current: &ThreadId, side_chat: bool) -> SharedString {
    if side_chat {
        return t!("Open side chat in this chat");
    }
    match kind {
        ThreadRelationshipKind::Transfer => t!("Open context transfer in this chat"),
        ThreadRelationshipKind::Subagent if source == current => t!("Open subagent in this chat"),
        ThreadRelationshipKind::Subagent => t!("Open parent agent in this chat"),
        _ if source == current => t!("Open fork in this chat"),
        _ => t!("Open parent thread in this chat"),
    }
}

/// Derives the sections' rows, or `None` when neither section has anything to show.
pub fn derive(sources: RelationshipSources<'_>) -> Option<RelationshipsInput> {
    let current = sources.thread_id;
    let projection = sources.projection;
    let graph = derive_thread_relationship_graph(sources.shells.iter().copied(), projection);
    let current_shell = sources.shells.iter().find(|s| &s.id == current).copied();
    let merge_target = resolve_merge_back_target_thread_id(projection);
    let latest_merge_run = projection.and_then(|p| resolve_latest_merge_back_run(&p.runs)).map(|r| r.id.clone());
    let runtime = projection.map(|p| projected_subagents_to_runtime(p.subagents.iter().map(|a| &**a))).unwrap_or_default();
    let workflows = derive_workflow_groups(&runtime, &|n| t!("Phase {number}", number = n as i64).to_string());
    let agent_by_thread = |thread: &ThreadId| projection.and_then(|p| p.subagents.iter().find(|s| s.child_thread_id.as_ref() == Some(thread)));
    let side_chat_of = |thread: &ThreadId| graph.nodes.get(thread).and_then(|n| n.thread.as_ref()).is_some_and(|t| t.lineage.side_chat == Some(true));

    let ordered = order_web_thread_lineage_rows(&graph, &immediate_thread_relationships(&graph, current), current, merge_target.as_ref());
    let rows: Vec<ThreadRelationshipWalkRow> = ordered
        .into_iter()
        // The graph rebuilds missing parents from lineage, so hidden ones are filtered here.
        .filter(|row| !(sources.is_hidden)(&row.thread_id))
        .filter(|row| {
            let missing = graph.nodes.get(&row.thread_id).is_some_and(|n| n.missing);
            let unloaded_child = row.edge.kind == ThreadRelationshipKind::Subagent && !is_parent_thread_relationship(&row.edge, current);
            !(sources.hiding_anywhere && missing && !unloaded_child)
        })
        .map(|mut row| {
            // A workflow coordinator's row shows the workflow's status.
            if row.edge.kind == ThreadRelationshipKind::Subagent && !is_parent_thread_relationship(&row.edge, current)
                && let Some(agent) = agent_by_thread(&row.thread_id)
                && let Some(group) = workflows.iter().find(|g| g.workflow.id == agent.id.as_str())
            {
                row.edge.status = Some(RelationshipStatus::from_wire(group.workflow.status.as_str()));
            }
            row
        })
        .collect();

    let to_row = |row: &ThreadRelationshipWalkRow| {
        let node = graph.nodes.get(&row.thread_id);
        let shell = node.and_then(|n| n.thread.as_ref());
        let is_parent = is_parent_thread_relationship(&row.edge, current);
        let is_subagent = row.edge.kind == ThreadRelationshipKind::Subagent;
        let side_chat = shell.is_some_and(|t| t.lineage.side_chat == Some(true));
        let agent = (is_subagent && !is_parent).then(|| agent_by_thread(&row.thread_id)).flatten();
        let workflow = agent.and_then(|a| workflows.iter().find(|g| g.workflow.id == a.id.as_str()));
        let title = workflow
            .and_then(|g| g.workflow.workflow_name.clone())
            .or_else(|| shell.map(|t| t.title.clone()))
            .or_else(|| agent.and_then(|a| a.title.clone()))
            .unwrap_or_else(|| row.thread_id.to_string());
        let title = if is_subagent { format_subagent_display_title(&title) } else { title };
        let icon = if is_parent {
            RelationshipIcon::Parent
        } else if is_subagent {
            RelationshipIcon::Agent
        } else if side_chat {
            RelationshipIcon::SideChat
        } else {
            RelationshipIcon::Fork
        };
        RelationshipRow {
            thread_id: row.thread_id.clone(),
            title: title.into(),
            icon,
            status: thread_relationship_row_status(&graph, row),
            hint: hint(row.edge.kind, &row.edge.source_thread_id, current, side_chat && !is_parent),
            merge_target: Some(&row.thread_id) == merge_target.as_ref(),
        }
    };

    let (mut related, mut active, mut previous, mut side_chats) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut running = 0;
    for row in &rows {
        let is_parent = is_parent_thread_relationship(&row.edge, current);
        if !is_parent && side_chat_of(&row.thread_id) {
            side_chats.push(to_row(row));
        } else if row.edge.kind != ThreadRelationshipKind::Subagent || is_parent {
            related.push(to_row(row));
        } else if row.edge.status.is_some_and(RelationshipStatus::is_finished_agent) {
            previous.push(to_row(row));
        } else {
            if row.edge.status == Some(RelationshipStatus::Running) {
                running += 1;
            }
            active.push(to_row(row));
        }
    }
    let running_count = running
        + projection.map_or(0, |p| {
            p.subagents.iter().filter(|a| a.child_thread_id.is_none() && a.status.as_str() == "running").count()
        });
    let can_detach = projection.is_some_and(can_detach_thread_provider_session);
    let delegated_from = current_shell.and_then(|s| s.delegated_from.clone()).map(|from| {
        let connected = (sources.connected_label)(&from.environment_id);
        let fallback = current_shell.and_then(|s| s.link_origin.as_ref()).map(|o| SharedString::from(o.label.clone()));
        DelegatedFromRow {
            // The title it had at delegation; the parent lives in the other environment's shell.
            title: from.title.clone().into(),
            label: connected.clone().or(fallback),
            connected: connected.is_some(),
            from,
        }
    });
    let lineage_visible = sources.show_lineage
        && (!related.is_empty()
            || !active.is_empty()
            || !previous.is_empty()
            || running_count > 0
            || delegated_from.is_some()
            // Disconnect lives in this heading, so a thread with only side chats still shows it.
            || (can_detach && !side_chats.is_empty()));
    let side_chats_visible = sources.show_side_chats && !side_chats.is_empty();
    if !lineage_visible && !side_chats_visible {
        return None;
    }
    let running_side_chats = side_chats.iter().filter(|r| r.status == Some(RelationshipStatus::Running)).count();
    let show_heading = running_count > 0 || can_detach || !related.is_empty() || !active.is_empty() || delegated_from.is_some();
    let merge = merge_target.map(|target| MergeBack {
        parent_title: (!(sources.is_hidden)(&target))
            .then(|| graph.nodes.get(&target).and_then(|n| n.thread.as_ref()).map(|t| SharedString::from(t.title.clone())))
            .flatten(),
        target,
        run_id: latest_merge_run,
    });
    let groups = vec![
        RelationshipGroup { id: "related", label: None, rows: std::mem::take(&mut related), expanded: true },
        RelationshipGroup { id: "active", label: None, rows: std::mem::take(&mut active), expanded: true },
        RelationshipGroup { id: "previous", label: Some(t!("Previous agents")), rows: std::mem::take(&mut previous), expanded: false },
    ];
    Some(RelationshipsInput {
        lineage_visible,
        side_chats_visible,
        groups,
        running_side_chats,
        side_chats: std::mem::take(&mut side_chats),
        running_count,
        show_heading,
        can_detach,
        merge,
        delegated_from,
        any_instance_badge: sources.any_instance_badge,
    })
}
