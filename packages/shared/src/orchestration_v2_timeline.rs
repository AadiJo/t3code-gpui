//! Which turn items the timeline shows (`orchestrationV2Timeline.ts`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use t3code_contracts::{
    NodeId, OrchestrationV2Run, OrchestrationV2RunAttempt, OrchestrationV2RunAttemptStatus, OrchestrationV2RunStatus,
    OrchestrationV2TurnItem, OrchestrationV2UserMessageInputIntent, RunId,
};

use crate::turn_item::TurnItemExt;

fn is_queued_turn(item: &OrchestrationV2TurnItem) -> bool {
    matches!(item, OrchestrationV2TurnItem::UserMessage(message) if message.input_intent == OrchestrationV2UserMessageInputIntent::QueuedTurn)
}

/// Whether one item shows: not in a rolled-back run, not a queued input while its run waits or
/// was cancelled, and not the interrupt result of a superseded attempt without a request.
pub fn is_turn_item_visible(
    item: &OrchestrationV2TurnItem,
    runs: &[Arc<OrchestrationV2Run>],
    attempts: &[Arc<OrchestrationV2RunAttempt>],
    items: &[Arc<OrchestrationV2TurnItem>],
) -> bool {
    let run_id = item.item_run_id();
    let run_status = |status: OrchestrationV2RunStatus| {
        run_id.is_some_and(|id| runs.iter().any(|run| &run.id == id && run.status == status))
    };
    if run_status(OrchestrationV2RunStatus::RolledBack) {
        return false;
    }
    if is_queued_turn(item) && (run_status(OrchestrationV2RunStatus::Queued) || run_status(OrchestrationV2RunStatus::Cancelled)) {
        return false;
    }
    !is_superseded_interrupt(item, attempts, items)
}

fn is_superseded_interrupt(
    item: &OrchestrationV2TurnItem,
    attempts: &[Arc<OrchestrationV2RunAttempt>],
    items: &[Arc<OrchestrationV2TurnItem>],
) -> bool {
    let OrchestrationV2TurnItem::RunInterruptResult(result) = item else { return false };
    let (Some(run_id), Some(node_id)) = (&result.run_id, &result.node_id) else { return false };
    let superseded = attempts.iter().any(|attempt| {
        &attempt.run_id == run_id
            && &attempt.root_node_id == node_id
            && attempt.status == OrchestrationV2RunAttemptStatus::Superseded
    });
    superseded
        && !items.iter().any(|candidate| {
            matches!(&**candidate, OrchestrationV2TurnItem::RunInterruptRequest(request) if request.run_id.as_ref() == Some(run_id))
        })
}

/// A visibility check built once for many items (`createOrchestrationV2TurnItemVisibility`).
pub struct TurnItemVisibility {
    statuses: HashMap<RunId, OrchestrationV2RunStatus>,
    superseded_roots: HashMap<RunId, HashSet<NodeId>>,
    interrupt_runs: HashSet<RunId>,
}

impl TurnItemVisibility {
    pub fn new(
        runs: &[Arc<OrchestrationV2Run>],
        attempts: &[Arc<OrchestrationV2RunAttempt>],
        items: &[Arc<OrchestrationV2TurnItem>],
    ) -> Self {
        let mut statuses = HashMap::new();
        for run in runs {
            if matches!(
                run.status,
                OrchestrationV2RunStatus::RolledBack | OrchestrationV2RunStatus::Cancelled | OrchestrationV2RunStatus::Queued
            ) {
                statuses.insert(run.id.clone(), run.status);
            } else {
                statuses.remove(&run.id);
            }
        }
        let mut superseded_roots: HashMap<RunId, HashSet<NodeId>> = HashMap::new();
        for attempt in attempts {
            if attempt.status == OrchestrationV2RunAttemptStatus::Superseded {
                superseded_roots.entry(attempt.run_id.clone()).or_default().insert(attempt.root_node_id.clone());
            }
        }
        let mut interrupt_runs = HashSet::new();
        if !superseded_roots.is_empty() {
            for item in items {
                if let OrchestrationV2TurnItem::RunInterruptRequest(request) = &**item
                    && let Some(run_id) = &request.run_id
                {
                    interrupt_runs.insert(run_id.clone());
                }
            }
        }
        Self { statuses, superseded_roots, interrupt_runs }
    }

    /// Whether nothing can be hidden, so callers can skip the pass.
    pub fn shows_everything(&self) -> bool {
        self.statuses.is_empty() && self.superseded_roots.is_empty()
    }

    pub fn is_visible(&self, item: &OrchestrationV2TurnItem) -> bool {
        let status = item.item_run_id().and_then(|id| self.statuses.get(id));
        if status == Some(&OrchestrationV2RunStatus::RolledBack) {
            return false;
        }
        if matches!(status, Some(OrchestrationV2RunStatus::Queued | OrchestrationV2RunStatus::Cancelled))
            && is_queued_turn(item)
        {
            return false;
        }
        if let OrchestrationV2TurnItem::RunInterruptResult(result) = item
            && let (Some(run_id), Some(node_id)) = (&result.run_id, &result.node_id)
        {
            return !(self.superseded_roots.get(run_id).is_some_and(|roots| roots.contains(node_id))
                && !self.interrupt_runs.contains(run_id));
        }
        true
    }
}
