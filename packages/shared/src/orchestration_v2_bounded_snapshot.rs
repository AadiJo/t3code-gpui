//! Undoes the opt-in snapshot compactions (`orchestrationV2BoundedSnapshot.ts`).
//!
//! With `turnItemsOmitLocalVisible` the server leaves out of `turnItems` the leading items that
//! the timeline already carries as local rows; deduped bodies carry their own markers.

use std::sync::Arc;

use crate::orchestration_v2_deduped_bodies::restore_snapshot_bodies;
use crate::thread_projection::ThreadProjection;

/// Puts the local timeline rows back at the front of `turn_items`
/// (`restoreLocalVisibleTurnItems`). The rows and the items share their `Arc`s.
pub fn restore_local_visible_turn_items(projection: &mut ThreadProjection) {
    let thread_id = projection.thread.id.clone();
    let mut items: Vec<_> = projection
        .visible_turn_items
        .iter()
        .filter(|row| row.is_local_to(&thread_id))
        .map(|row| Arc::clone(&row.item))
        .collect();
    if items.is_empty() {
        return;
    }
    items.extend(projection.turn_items.iter().cloned());
    projection.turn_items = Arc::new(items);
}

/// Drops the leading `turn_items` that local timeline rows hold by the same `Arc`, for a compact
/// cache record (`omitLocalVisibleTurnItems`). `None` when there is nothing to omit.
pub fn omit_local_visible_turn_items(projection: &ThreadProjection) -> Option<ThreadProjection> {
    let thread_id = &projection.thread.id;
    let mut omitted = 0;
    for row in projection.visible_turn_items.iter().filter(|row| row.is_local_to(thread_id)) {
        if !projection.turn_items.get(omitted).is_some_and(|item| Arc::ptr_eq(item, &row.item)) {
            return None;
        }
        omitted += 1;
    }
    (omitted > 0).then(|| ThreadProjection {
        turn_items: Arc::new(projection.turn_items[omitted..].to_vec()),
        ..projection.clone()
    })
}

/// Restores a decoded snapshot before reducers or caches see it (`boundedSnapshotProjection`):
/// omitted local items when the snapshot says so, then deduped bodies, then shared `Arc`s.
pub fn bounded_snapshot_projection(projection: &mut ThreadProjection, turn_items_omit_local_visible: bool) {
    if turn_items_omit_local_visible {
        restore_local_visible_turn_items(projection);
    } else {
        projection.share_local_items();
    }
    restore_snapshot_bodies(projection);
}
