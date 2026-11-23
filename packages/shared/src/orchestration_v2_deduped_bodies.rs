//! Restores turn item bodies the server blanked because their record already carries them
//! (`orchestrationV2DedupedBodies.ts`). A client opts in with `acceptDedupedBodies` /
//! `dedupeBodies=1`; the server blanks a body only when it equals the record's, so the restore is
//! exact. A marked body whose record is missing, or is itself still marked, stays blank and marked.

use std::collections::HashMap;
use std::sync::Arc;

use t3code_contracts::{
    CheckpointId, MessageId, NodeId, OrchestrationV2CheckpointFileSummary, OrchestrationV2Subagent,
    OrchestrationV2TurnItem,
};

use crate::thread_projection::{ThreadProjection, VisibleTurnItem};

/// Where marked bodies are copied back from.
pub trait DedupedBodySource {
    fn message_text(&self, id: &MessageId) -> Option<&str>;
    fn checkpoint_files(&self, id: &CheckpointId) -> Option<&[OrchestrationV2CheckpointFileSummary]>;
    fn subagent(&self, id: &NodeId) -> Option<&OrchestrationV2Subagent>;
}

/// Looks records up newest first: a deduped stream item repeats a record the stream just sent.
pub struct LatestBodies<'a>(pub &'a ThreadProjection);

impl DedupedBodySource for LatestBodies<'_> {
    fn message_text(&self, id: &MessageId) -> Option<&str> {
        self.0.messages.iter().rev().find(|message| &message.id == id).map(|message| message.text.as_str())
    }
    fn checkpoint_files(&self, id: &CheckpointId) -> Option<&[OrchestrationV2CheckpointFileSummary]> {
        self.0.checkpoints.iter().rev().find(|checkpoint| &checkpoint.id == id).map(|checkpoint| checkpoint.files.as_slice())
    }
    fn subagent(&self, id: &NodeId) -> Option<&OrchestrationV2Subagent> {
        self.0.subagents.iter().rev().find(|subagent| &subagent.id == id).map(|subagent| &**subagent)
    }
}

/// Indexes a snapshot's records once, for restoring many items.
struct SnapshotBodies<'a> {
    messages: HashMap<&'a MessageId, &'a str>,
    checkpoints: HashMap<&'a CheckpointId, &'a [OrchestrationV2CheckpointFileSummary]>,
    subagents: HashMap<&'a NodeId, &'a OrchestrationV2Subagent>,
}

impl<'a> SnapshotBodies<'a> {
    fn new(projection: &'a ThreadProjection) -> Self {
        Self {
            messages: projection.messages.iter().map(|message| (&message.id, message.text.as_str())).collect(),
            checkpoints: projection.checkpoints.iter().map(|checkpoint| (&checkpoint.id, checkpoint.files.as_slice())).collect(),
            subagents: projection.subagents.iter().map(|subagent| (&subagent.id, &**subagent)).collect(),
        }
    }
}

impl DedupedBodySource for SnapshotBodies<'_> {
    fn message_text(&self, id: &MessageId) -> Option<&str> {
        self.messages.get(id).copied()
    }
    fn checkpoint_files(&self, id: &CheckpointId) -> Option<&[OrchestrationV2CheckpointFileSummary]> {
        self.checkpoints.get(id).copied()
    }
    fn subagent(&self, id: &NodeId) -> Option<&OrchestrationV2Subagent> {
        self.subagents.get(id).copied()
    }
}

/// Fields shared by subagent records and subagent items.
pub trait SubagentBodies {
    fn prompt_mut(&mut self) -> (&mut String, &mut Option<t3code_contracts::wire::True>);
    fn result_mut(&mut self) -> (&mut Option<String>, &mut Option<t3code_contracts::wire::True>);
    fn is_marked(&self) -> bool;
}

macro_rules! subagent_bodies {
    ($($ty:ty),*) => {$(
        impl SubagentBodies for $ty {
            fn prompt_mut(&mut self) -> (&mut String, &mut Option<t3code_contracts::wire::True>) {
                (&mut self.prompt, &mut self.prompt_omitted)
            }
            fn result_mut(&mut self) -> (&mut Option<String>, &mut Option<t3code_contracts::wire::True>) {
                (&mut self.result, &mut self.result_omitted)
            }
            fn is_marked(&self) -> bool {
                self.prompt_omitted.is_some() || self.result_omitted.is_some()
            }
        }
    )*};
}
subagent_bodies!(OrchestrationV2Subagent, t3code_contracts::OrchestrationV2TurnItemSubagent);

/// Copies a marked prompt or result back from `previous` (`restoreSubagentBodies`). Returns
/// whether anything changed.
pub fn restore_subagent_bodies(value: &mut impl SubagentBodies, previous: Option<&OrchestrationV2Subagent>) -> bool {
    let Some(previous) = previous else { return false };
    let mut changed = false;
    let (prompt, prompt_omitted) = value.prompt_mut();
    if prompt_omitted.is_some() && previous.prompt_omitted.is_none() {
        *prompt = previous.prompt.clone();
        *prompt_omitted = None;
        changed = true;
    }
    let (result, result_omitted) = value.result_mut();
    if result_omitted.is_some() && previous.result_omitted.is_none() {
        *result = previous.result.clone();
        *result_omitted = None;
        changed = true;
    }
    changed
}

/// Whether `item` carries a marked body.
pub fn has_marked_body(item: &OrchestrationV2TurnItem) -> bool {
    match item {
        OrchestrationV2TurnItem::UserMessage(item) => item.text_omitted.is_some(),
        OrchestrationV2TurnItem::AssistantMessage(item) => item.text_omitted.is_some(),
        OrchestrationV2TurnItem::Checkpoint(item) => item.files_omitted.is_some(),
        OrchestrationV2TurnItem::Subagent(item) => item.is_marked(),
        _ => false,
    }
}

/// Restores the marked bodies of one item (`restoreTurnItemBodies`); `None` when nothing changed.
pub fn restore_turn_item_bodies(item: &OrchestrationV2TurnItem, source: &impl DedupedBodySource) -> Option<OrchestrationV2TurnItem> {
    if !has_marked_body(item) {
        return None;
    }
    let mut next = item.clone();
    let changed = match &mut next {
        OrchestrationV2TurnItem::UserMessage(item) => {
            source.message_text(&item.message_id).map(|text| {
                item.text = text.to_owned();
                item.text_omitted = None;
            }).is_some()
        }
        OrchestrationV2TurnItem::AssistantMessage(item) => {
            source.message_text(&item.message_id).map(|text| {
                item.text = text.to_owned();
                item.text_omitted = None;
            }).is_some()
        }
        OrchestrationV2TurnItem::Checkpoint(item) => {
            source.checkpoint_files(&item.checkpoint_id).map(|files| {
                item.files = files.to_vec();
                item.files_omitted = None;
            }).is_some()
        }
        OrchestrationV2TurnItem::Subagent(item) => {
            let previous = source.subagent(&item.subagent_id);
            restore_subagent_bodies(item, previous)
        }
        _ => false,
    };
    changed.then_some(next)
}

/// Restores every local item of a decoded snapshot (`restoreSnapshotBodies`). Each item is
/// mapped once, so an item shared by the timeline and `turn_items` stays one `Arc`.
pub fn restore_snapshot_bodies(projection: &mut ThreadProjection) {
    let marked = projection.turn_items.iter().any(|item| has_marked_body(item))
        || projection.visible_turn_items.iter().any(|row| has_marked_body(&row.item));
    if !marked {
        return;
    }
    let thread_id = projection.thread.id.clone();
    let mut mapped: HashMap<*const OrchestrationV2TurnItem, Arc<OrchestrationV2TurnItem>> = HashMap::new();
    let (turn_items, visible) = {
        let source = SnapshotBodies::new(projection);
        let mut once = |item: &Arc<OrchestrationV2TurnItem>| -> Arc<OrchestrationV2TurnItem> {
            mapped
                .entry(Arc::as_ptr(item))
                .or_insert_with(|| restore_turn_item_bodies(item, &source).map_or_else(|| Arc::clone(item), Arc::new))
                .clone()
        };
        let visible: Vec<VisibleTurnItem> = projection
            .visible_turn_items
            .iter()
            .map(|row| {
                if row.is_local_to(&thread_id) {
                    VisibleTurnItem { item: once(&row.item), ..row.clone() }
                } else {
                    row.clone()
                }
            })
            .collect();
        let turn_items: Vec<Arc<OrchestrationV2TurnItem>> = projection.turn_items.iter().map(&mut once).collect();
        (turn_items, visible)
    };
    projection.turn_items = Arc::new(turn_items);
    projection.visible_turn_items = Arc::new(visible);
}
