//! Derivations of one thread's projection that the timeline and composer read
//! (`threadDetail.ts`, `threadRequests.ts`, `threadWorkflows.ts`).
//!
//! [`same_thread_structure`] is the main render-cost control while text streams: a view keeps
//! its row layout when only assistant or reasoning text grew.

use std::sync::Arc;

use t3code_contracts::{
    OrchestrationV2ConversationMessage, OrchestrationV2Run, OrchestrationV2RunStatus, OrchestrationV2RuntimeRequest,
    OrchestrationV2RuntimeRequestStatus, OrchestrationV2TurnItem,
};
use t3code_shared::thread_projection::{ThreadProjection, VisibleTurnItem};

/// A pending approval or question with its card, when the window holds it.
#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub request: Arc<OrchestrationV2RuntimeRequest>,
    pub card: Option<Arc<OrchestrationV2TurnItem>>,
}

/// Pending requests joined to their `approval_request` / `user_input_request` items
/// (`derivePendingThreadRequests`).
pub fn pending_requests(projection: &ThreadProjection) -> Vec<PendingRequest> {
    projection
        .runtime_requests
        .iter()
        .filter(|request| request.status == OrchestrationV2RuntimeRequestStatus::Pending)
        .map(|request| PendingRequest {
            request: Arc::clone(request),
            card: projection
                .turn_items
                .iter()
                .rev()
                .find(|item| match &***item {
                    OrchestrationV2TurnItem::ApprovalRequest(card) => card.request_id == request.id,
                    OrchestrationV2TurnItem::UserInputRequest(card) => card.request_id == request.id,
                    _ => false,
                })
                .cloned(),
        })
        .collect()
}

/// The latest run still going (`resolveActiveThreadRun`).
pub fn active_run(projection: &ThreadProjection) -> Option<&Arc<OrchestrationV2Run>> {
    projection.runs.iter().rev().find(|run| {
        matches!(
            run.status,
            OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting | OrchestrationV2RunStatus::Running | OrchestrationV2RunStatus::Waiting
        )
    })
}

/// Messages the user queued, in queue order (`getUserQueuedThreadRuns` sorted by
/// `queuePosition ?? ordinal`, then `ordinal`). Automatic notifications and delegated
/// completions are not user queued.
pub fn user_queued_runs(projection: &ThreadProjection) -> Vec<(Arc<OrchestrationV2Run>, Option<Arc<OrchestrationV2ConversationMessage>>)> {
    let mut runs: Vec<_> = projection
        .runs
        .iter()
        .filter(|run| run.status == OrchestrationV2RunStatus::Queued)
        .filter_map(|run| {
            let message = projection.messages.iter().find(|message| message.id == run.user_message_id).cloned();
            let automatic = message.as_ref().is_some_and(|message| message.delegated_completion.is_some() || message.notification.is_some());
            (!automatic).then(|| (Arc::clone(run), message))
        })
        .collect();
    runs.sort_by_key(|(run, _)| (run.queue_position.unwrap_or(run.ordinal), run.ordinal));
    runs
}

/// Whether the queue is held (`run.interrupt { holdQueue }` paused it).
pub fn queue_held(projection: &ThreadProjection) -> bool {
    projection.runs.iter().any(|run| run.status == OrchestrationV2RunStatus::Queued && run.queue_held == Some(true))
}

fn same_rows<T>(a: &Arc<Vec<Arc<T>>>, b: &Arc<Vec<Arc<T>>>, same: impl Fn(&T, &T) -> bool) -> bool {
    Arc::ptr_eq(a, b) || (a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| Arc::ptr_eq(x, y) || same(x, y)))
}

fn same_message_structure(a: &OrchestrationV2ConversationMessage, b: &OrchestrationV2ConversationMessage) -> bool {
    a.id == b.id && a.streaming == b.streaming && a.role == b.role && a.attachments.len() == b.attachments.len()
}

/// Items equal apart from streamed text.
fn same_item_structure(a: &OrchestrationV2TurnItem, b: &OrchestrationV2TurnItem) -> bool {
    match (a, b) {
        (OrchestrationV2TurnItem::AssistantMessage(a), OrchestrationV2TurnItem::AssistantMessage(b)) => {
            let mut patched = a.clone();
            patched.text.clone_from(&b.text);
            patched.updated_at.clone_from(&b.updated_at);
            patched == *b
        }
        (OrchestrationV2TurnItem::Reasoning(a), OrchestrationV2TurnItem::Reasoning(b)) => {
            let mut patched = a.clone();
            patched.text.clone_from(&b.text);
            patched.updated_at.clone_from(&b.updated_at);
            patched == *b
        }
        _ => false,
    }
}

fn same_visible(a: &Arc<Vec<VisibleTurnItem>>, b: &Arc<Vec<VisibleTurnItem>>) -> bool {
    Arc::ptr_eq(a, b)
        || (a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(x, y)| {
                x.source_item_id == y.source_item_id && (Arc::ptr_eq(&x.item, &y.item) || same_item_structure(&x.item, &y.item))
            }))
}

/// Whether two projections differ only by text streamed into existing assistant and reasoning
/// items, or by a visit mark (`sameThreadStructure`).
pub fn same_thread_structure(previous: &ThreadProjection, next: &ThreadProjection) -> bool {
    let unchanged = Arc::ptr_eq(&previous.runs, &next.runs)
        && Arc::ptr_eq(&previous.attempts, &next.attempts)
        && Arc::ptr_eq(&previous.nodes, &next.nodes)
        && Arc::ptr_eq(&previous.subagents, &next.subagents)
        && Arc::ptr_eq(&previous.provider_sessions, &next.provider_sessions)
        && Arc::ptr_eq(&previous.provider_threads, &next.provider_threads)
        && Arc::ptr_eq(&previous.provider_turns, &next.provider_turns)
        && Arc::ptr_eq(&previous.runtime_requests, &next.runtime_requests)
        && Arc::ptr_eq(&previous.plans, &next.plans)
        && Arc::ptr_eq(&previous.checkpoint_scopes, &next.checkpoint_scopes)
        && Arc::ptr_eq(&previous.checkpoints, &next.checkpoints)
        && Arc::ptr_eq(&previous.context_handoffs, &next.context_handoffs)
        && Arc::ptr_eq(&previous.context_transfers, &next.context_transfers);
    if !unchanged {
        return false;
    }
    if !Arc::ptr_eq(&previous.thread, &next.thread) {
        let mut patched = (*previous.thread).clone();
        patched.last_visited_at.clone_from(&next.thread.last_visited_at);
        patched.opens.clone_from(&next.thread.opens);
        patched.updated_at.clone_from(&next.thread.updated_at);
        if patched != *next.thread {
            return false;
        }
    }
    same_rows(&previous.messages, &next.messages, same_message_structure)
        && same_rows(&previous.turn_items, &next.turn_items, same_item_structure)
        && same_visible(&previous.visible_turn_items, &next.visible_turn_items)
}

/// Holds a projection's structure while only text streams (`quietThread`). Rows read their text
/// from the latest projection; layout reads from [`QuietThread::structure`].
#[derive(Default)]
pub struct QuietThread {
    structure: Option<Arc<ThreadProjection>>,
}

impl QuietThread {
    /// Feeds the latest projection; returns whether the structure changed.
    pub fn update(&mut self, next: &Arc<ThreadProjection>) -> bool {
        match &self.structure {
            Some(previous) if Arc::ptr_eq(previous, next) || same_thread_structure(previous, next) => false,
            _ => {
                self.structure = Some(Arc::clone(next));
                true
            }
        }
    }

    pub fn structure(&self) -> Option<&Arc<ThreadProjection>> {
        self.structure.as_ref()
    }
}
