//! One thread's projection as the client holds it (`OrchestrationV2ThreadProjection`), with every
//! record behind an `Arc`. An event that changes one record copies only the pointer list of its
//! collection, so readers holding the previous projection keep it, and views skip rows whose
//! `Arc` did not change.
//!
//! The JSON shape is the wire shape, so snapshots, stream items and cache files decode straight
//! into this type.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use t3code_contracts::{
    OrchestrationV2AppThread, OrchestrationV2Checkpoint, OrchestrationV2CheckpointScope, OrchestrationV2ContextHandoff,
    OrchestrationV2ContextTransfer, OrchestrationV2ConversationMessage, OrchestrationV2ExecutionNode,
    OrchestrationV2PlanArtifact, OrchestrationV2ProviderSession, OrchestrationV2ProviderThread,
    OrchestrationV2ProviderTurn, OrchestrationV2Run, OrchestrationV2RunAttempt, OrchestrationV2RuntimeRequest,
    OrchestrationV2Subagent, OrchestrationV2ThreadProjectionVisibleTurnItemsItemVisibility as Visibility,
    OrchestrationV2TurnItem, ThreadId, TurnItemId,
};

use crate::turn_item::TurnItemExt;

/// A collection of shared records. Edit it with [`Arc::make_mut`]; untouched records keep
/// their `Arc`.
pub type Rows<T> = Arc<Vec<Arc<T>>>;

/// One row of the rendered timeline (`OrchestrationV2ProjectedTurnItem`). Rows inherited from
/// a fork ancestor belong to another thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleTurnItem {
    pub position: u64,
    pub visibility: Visibility,
    pub source_thread_id: ThreadId,
    pub source_item_id: TurnItemId,
    pub item: Arc<OrchestrationV2TurnItem>,
}

impl VisibleTurnItem {
    /// A row of the projection's own thread (`isLocalTimelineRow`).
    pub fn is_local_to(&self, thread_id: &ThreadId) -> bool {
        self.visibility == Visibility::Local || &self.source_thread_id == thread_id
    }
}

/// The full projection of one thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadProjection {
    pub thread: Arc<OrchestrationV2AppThread>,
    pub runs: Rows<OrchestrationV2Run>,
    pub attempts: Rows<OrchestrationV2RunAttempt>,
    pub nodes: Rows<OrchestrationV2ExecutionNode>,
    pub subagents: Rows<OrchestrationV2Subagent>,
    pub provider_sessions: Rows<OrchestrationV2ProviderSession>,
    pub provider_threads: Rows<OrchestrationV2ProviderThread>,
    pub provider_turns: Rows<OrchestrationV2ProviderTurn>,
    pub runtime_requests: Rows<OrchestrationV2RuntimeRequest>,
    pub messages: Rows<OrchestrationV2ConversationMessage>,
    pub plans: Rows<OrchestrationV2PlanArtifact>,
    pub turn_items: Rows<OrchestrationV2TurnItem>,
    pub checkpoint_scopes: Rows<OrchestrationV2CheckpointScope>,
    pub checkpoints: Rows<OrchestrationV2Checkpoint>,
    pub context_handoffs: Rows<OrchestrationV2ContextHandoff>,
    pub context_transfers: Rows<OrchestrationV2ContextTransfer>,
    pub visible_turn_items: Arc<Vec<VisibleTurnItem>>,
    pub updated_at: String,
}

impl ThreadProjection {
    /// Makes local timeline rows and `turn_items` share one `Arc` per item when their values
    /// match. JSON carries each item twice; sharing halves the memory of a decoded timeline.
    pub fn share_local_items(&mut self) {
        let thread_id = self.thread.id.clone();
        let by_id: std::collections::HashMap<TurnItemId, &Arc<OrchestrationV2TurnItem>> =
            self.turn_items.iter().map(|item| (item.item_id(), item)).collect();
        let mut changed = false;
        let rows: Vec<VisibleTurnItem> = self
            .visible_turn_items
            .iter()
            .map(|row| {
                if row.is_local_to(&thread_id)
                    && let Some(shared) = by_id.get(&row.source_item_id)
                    && !Arc::ptr_eq(shared, &row.item)
                    && ***shared == *row.item
                {
                    changed = true;
                    return VisibleTurnItem { item: Arc::clone(shared), ..row.clone() };
                }
                row.clone()
            })
            .collect();
        if changed {
            self.visible_turn_items = Arc::new(rows);
        }
    }

    /// Bytes of JSON this projection encodes to, for cache size caps.
    pub fn encoded_len(&self) -> usize {
        struct Counter(usize);
        impl std::io::Write for Counter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0 += buf.len();
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut counter = Counter(0);
        _ = serde_json::to_writer(&mut counter, self);
        counter.0
    }
}

/// A record kept in a projection collection, upserted by its id.
pub trait Keyed {
    type Key: PartialEq + ?Sized;
    fn key(&self) -> &Self::Key;
}

macro_rules! keyed {
    ($($ty:ty => $key:ty),* $(,)?) => {
        $(impl Keyed for $ty {
            type Key = $key;
            fn key(&self) -> &$key {
                &self.id
            }
        })*
    };
}

keyed!(
    OrchestrationV2Run => t3code_contracts::RunId,
    OrchestrationV2RunAttempt => t3code_contracts::RunAttemptId,
    OrchestrationV2ExecutionNode => t3code_contracts::NodeId,
    OrchestrationV2Subagent => t3code_contracts::NodeId,
    OrchestrationV2ProviderSession => t3code_contracts::ProviderSessionId,
    OrchestrationV2ProviderThread => t3code_contracts::ProviderThreadId,
    OrchestrationV2ProviderTurn => t3code_contracts::ProviderTurnId,
    OrchestrationV2RuntimeRequest => t3code_contracts::RuntimeRequestId,
    OrchestrationV2ConversationMessage => t3code_contracts::MessageId,
    OrchestrationV2CheckpointScope => t3code_contracts::CheckpointScopeId,
    OrchestrationV2Checkpoint => t3code_contracts::CheckpointId,
    OrchestrationV2ContextHandoff => t3code_contracts::ContextHandoffId,
    OrchestrationV2ContextTransfer => t3code_contracts::ContextTransferId,
);

impl Keyed for OrchestrationV2PlanArtifact {
    type Key = str;
    fn key(&self) -> &str {
        match self {
            Self::ProposedPlan(plan) => plan.id.as_str(),
            Self::TodoList(plan) => plan.id.as_str(),
            Self::Unknown(value) => value.get("id").and_then(serde_json::Value::as_str).unwrap_or_default(),
        }
    }
}

/// Replaces the record with the same id, or appends it (`upsertEntity`).
pub fn upsert<T: Keyed>(rows: &mut Rows<T>, record: T) {
    let rows = Arc::make_mut(rows);
    match rows.iter().position(|row| row.key() == record.key()) {
        Some(index) => rows[index] = Arc::new(record),
        None => rows.push(Arc::new(record)),
    }
}
