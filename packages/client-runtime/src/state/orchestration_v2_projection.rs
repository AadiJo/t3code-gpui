//! Applies thread stream events to a projection (`orchestrationV2Projection.ts`). Each event
//! replaces only the records it names; every other record keeps its `Arc`.

use std::sync::Arc;

use t3code_contracts::{
    OrchestrationV2DomainEvent as Event, OrchestrationV2DomainEventThreadChangedType as Changed, OrchestrationV2Run,
    OrchestrationV2RunStatus, OrchestrationV2ThreadProjectionVisibleTurnItemsItemVisibility as Visibility,
    OrchestrationV2TurnItem, OrchestrationV2UserMessageInputIntent, ProviderThreadId, RunId, ThreadPullRequestLink,
};
use t3code_shared::orchestration_v2_deduped_bodies::{LatestBodies, restore_subagent_bodies, restore_turn_item_bodies};
use t3code_shared::orchestration_v2_timeline::{TurnItemVisibility, is_turn_item_visible};
use t3code_shared::thread_projection::{ThreadProjection, VisibleTurnItem, upsert};
use t3code_shared::turn_item::TurnItemExt;

/// Set while the projection holds a bounded window with older history not loaded.
#[derive(Debug, Clone, Copy, Default)]
pub struct PartialWindow {
    pub latest_local_turn_ordinal: Option<u64>,
}

/// The thread and time of any event.
pub fn event_thread_and_time(event: &Event) -> Option<(&t3code_contracts::ThreadId, &str)> {
    macro_rules! head {
        ($($variant:ident),*) => {
            match event {
                $(Event::$variant(event) => Some((&event.thread_id, event.occurred_at.as_str())),)*
                Event::Unknown(_) => None,
            }
        };
    }
    head!(
        ThreadCreated, ThreadChanged, ThreadPullRequestLinkUpdated, ThreadVisitRecorded, RunCreated, RunUpdated,
        RunBackgroundWorkCancelled, RunAttemptCreated, RunAttemptUpdated, NodeUpdated, SubagentUpdated,
        ProviderSessionAttachedOrProviderSessionUpdated, ProviderSessionDetached, ProviderThreadUpdated,
        ProviderTurnUpdated, RuntimeRequestUpdated, MessageUpdated, TurnItemUpdated, PlanUpdated, CheckpointScopeCreated,
        CheckpointCaptured, CheckpointRollbackRequested, ContextHandoffUpdated, ContextTransferCreated, ContextTransferUpdated
    )
}

/// Whether this client can apply the event. Unknown events, and turn item updates of an
/// unknown item type, still advance the cursor but change nothing.
pub fn is_known_event(event: &Event) -> bool {
    match event {
        Event::Unknown(_) => false,
        Event::TurnItemUpdated(update) => !matches!(update.payload, OrchestrationV2TurnItem::Unknown(_)),
        _ => true,
    }
}

/// Whether the event deletes the thread.
pub fn is_thread_deleted(event: &Event) -> bool {
    matches!(event, Event::ThreadChanged(change) if change.r#type == Changed::ThreadDeleted)
}

fn renumber(rows: &mut [VisibleTurnItem]) {
    for (position, row) in rows.iter_mut().enumerate() {
        row.position = position as u64;
    }
}

fn remove_visible(rows: &mut Arc<Vec<VisibleTurnItem>>, id: &t3code_contracts::TurnItemId) {
    if let Some(index) = rows.iter().position(|row| &row.source_item_id == id) {
        let rows = Arc::make_mut(rows);
        rows.remove(index);
        renumber(rows);
    }
}

/// Drops local rows that are no longer visible (`activeVisibleTurnItems`).
fn refresh_visibility(projection: &mut ThreadProjection) {
    let visibility = TurnItemVisibility::new(&projection.runs, &projection.attempts, &projection.turn_items);
    if visibility.shows_everything() {
        return;
    }
    let hidden = |row: &VisibleTurnItem| row.visibility == Visibility::Local && !visibility.is_visible(&row.item);
    if projection.visible_turn_items.iter().any(hidden) {
        let rows = Arc::make_mut(&mut projection.visible_turn_items);
        rows.retain(|row| !hidden(row));
        renumber(rows);
    }
}

fn oldest_local_ordinal(rows: &[VisibleTurnItem]) -> Option<u64> {
    rows.iter().filter(|row| row.visibility == Visibility::Local).map(|row| row.item.item_ordinal()).min()
}

fn is_unfinished(status: OrchestrationV2RunStatus) -> bool {
    matches!(
        status,
        OrchestrationV2RunStatus::Preparing
            | OrchestrationV2RunStatus::Queued
            | OrchestrationV2RunStatus::Starting
            | OrchestrationV2RunStatus::Running
            | OrchestrationV2RunStatus::Waiting
    )
}

fn below_partial_window(projection: &ThreadProjection, item: &OrchestrationV2TurnItem, window: PartialWindow) -> bool {
    let ordinal = item.item_ordinal();
    if window.latest_local_turn_ordinal.is_some_and(|latest| ordinal <= latest) {
        return true;
    }
    oldest_local_ordinal(&projection.visible_turn_items).is_some_and(|oldest| ordinal < oldest)
}

/// Whether an item missing from a bounded window is unloaded history rather than new
/// (`shouldDropMissingPartialTurnItem`). Items of runs still going are always new.
fn drop_missing_partial_item(projection: &ThreadProjection, item: &OrchestrationV2TurnItem, window: PartialWindow) -> bool {
    let id = item.item_id();
    if projection.visible_turn_items.iter().any(|row| row.source_item_id == id) {
        return false;
    }
    let Some(run_id) = item.item_run_id() else { return below_partial_window(projection, item, window) };
    if projection.runs.iter().any(|run| &run.id == run_id && is_unfinished(run.status)) {
        return false;
    }
    let ordinal = item.item_ordinal();
    if projection.visible_turn_items.iter().any(|row| {
        row.visibility == Visibility::Local && row.item.item_run_id() == Some(run_id) && row.item.item_ordinal() <= ordinal
    }) {
        return false;
    }
    below_partial_window(projection, item, window)
}

/// Inserts or replaces the local row of `item` (`upsertVisibleTurnItem`). A new row goes before
/// the first later local row by `(ordinal, id)`.
fn upsert_visible(projection: &mut ThreadProjection, item: &Arc<OrchestrationV2TurnItem>, partial: Option<PartialWindow>) {
    let id = item.item_id();
    let thread_id = item.item_thread_id().unwrap_or_else(|| projection.thread.id.clone());
    let rows = &projection.visible_turn_items;
    // Streamed updates land on the newest rows; ids are unique, so search from the end.
    let index = rows.iter().rposition(|row| row.source_item_id == id);
    if let Some(index) = index {
        let previous = &rows[index];
        if previous.visibility == Visibility::Local && previous.source_thread_id == thread_id {
            if Arc::ptr_eq(&previous.item, item) {
                return;
            }
            if previous.item.item_ordinal() == item.item_ordinal() {
                Arc::make_mut(&mut projection.visible_turn_items)[index].item = Arc::clone(item);
                return;
            }
        }
    }
    if index.is_none() && partial.is_some_and(|window| drop_missing_partial_item(projection, item, window)) {
        return;
    }
    let ordinal = item.item_ordinal();
    let row = VisibleTurnItem {
        position: 0,
        visibility: Visibility::Local,
        source_thread_id: thread_id,
        source_item_id: id.clone(),
        item: Arc::clone(item),
    };
    let rows = Arc::make_mut(&mut projection.visible_turn_items);
    if let Some(index) = index {
        rows.remove(index);
    }
    let insertion = rows
        .iter()
        .position(|candidate| {
            candidate.visibility == Visibility::Local && {
                let candidate_ordinal = candidate.item.item_ordinal();
                candidate_ordinal > ordinal
                    || (candidate_ordinal == ordinal && candidate.item.item_id().as_str() > id.as_str())
            }
        })
        .unwrap_or(rows.len());
    rows.insert(insertion, row);
    renumber(rows);
}

/// Shows the retained `queued_turn` user items of a run that left the queue
/// (`restoreQueuedUserItems`). Items held were loaded or streamed, so they skip the window check.
fn restore_queued_user_items(projection: &mut ThreadProjection, run_id: &RunId) {
    let items: Vec<_> = projection
        .turn_items
        .iter()
        .filter(|item| {
            item.item_run_id() == Some(run_id)
                && matches!(&***item, OrchestrationV2TurnItem::UserMessage(message) if message.input_intent == OrchestrationV2UserMessageInputIntent::QueuedTurn)
        })
        .cloned()
        .collect();
    for item in items {
        if is_turn_item_visible(&item, &projection.runs, &projection.attempts, &projection.turn_items) {
            upsert_visible(projection, &item, None);
        }
    }
}

fn is_queued_placeholder(thread: &t3code_contracts::OrchestrationV2ProviderThread) -> bool {
    thread.status == t3code_contracts::OrchestrationV2ProviderThreadStatus::NotLoaded
        && thread.first_run_ordinal.is_none()
        && thread.native_thread_ref.is_none()
        && thread.provider_session_id.is_none()
}

fn same_pull_request(left: &ThreadPullRequestLink, right: &ThreadPullRequestLink) -> bool {
    left.number == right.number
        && left.host.eq_ignore_ascii_case(&right.host)
        && left.repository.eq_ignore_ascii_case(&right.repository)
}

fn edit_thread(projection: &mut ThreadProjection, edit: impl FnOnce(&mut t3code_contracts::OrchestrationV2AppThread)) {
    edit(Arc::make_mut(&mut projection.thread));
}

/// Applies one event (`applyOrchestrationV2ProjectionEvent`). Returns whether anything changed.
/// `partial` is set while older history is not loaded, so items below the window are dropped.
pub fn apply_projection_event(projection: &mut ThreadProjection, event: &Event, partial: Option<PartialWindow>) -> bool {
    let Some((thread_id, occurred_at)) = event_thread_and_time(event) else { return false };
    if thread_id != &projection.thread.id || !is_known_event(event) {
        return false;
    }
    let bump = |projection: &mut ThreadProjection| projection.updated_at = occurred_at.to_owned();
    match event {
        Event::ThreadPullRequestLinkUpdated(event) => {
            bump(projection);
            edit_thread(projection, |thread| {
                if let Some(links) = thread.pull_requests.as_mut() {
                    for link in links.iter_mut().filter(|link| same_pull_request(link, &event.payload)) {
                        *link = event.payload.clone();
                    }
                }
            });
        }
        Event::ThreadCreated(event) => {
            bump(projection);
            projection.thread = Arc::new(event.payload.clone());
        }
        Event::ThreadChanged(event) => {
            // Visits and tags are not activity: no updatedAt bump.
            if !matches!(event.r#type, Changed::ThreadVisited | Changed::ThreadTagsUpdated | Changed::ThreadMarkedUnread) {
                bump(projection);
            }
            projection.thread = Arc::new(event.payload.clone());
        }
        Event::ThreadVisitRecorded(event) => edit_thread(projection, |thread| {
            thread.last_visited_at = event.payload.last_visited_at.clone();
            if let Some(opens) = &event.payload.opens {
                thread.opens = Some(opens.clone());
            }
        }),
        Event::RunCreated(t3code_contracts::OrchestrationV2DomainEventRunCreated { payload, .. })
        | Event::RunUpdated(t3code_contracts::OrchestrationV2DomainEventRunUpdated { payload, .. }) => {
            apply_run(projection, payload);
            bump(projection);
        }
        Event::RunBackgroundWorkCancelled(event) => {
            bump(projection);
            if let Some(index) = projection.runs.iter().position(|run| run.id == event.payload.run_id) {
                let runs = Arc::make_mut(&mut projection.runs);
                Arc::make_mut(&mut runs[index]).restart_cancelled_background_work =
                    Some(event.payload.restart_cancelled_background_work.clone());
            }
        }
        Event::RunAttemptCreated(t3code_contracts::OrchestrationV2DomainEventRunAttemptCreated { payload, .. })
        | Event::RunAttemptUpdated(t3code_contracts::OrchestrationV2DomainEventRunAttemptUpdated { payload, .. }) => {
            bump(projection);
            upsert(&mut projection.attempts, payload.clone());
            refresh_visibility(projection);
        }
        Event::NodeUpdated(event) => {
            bump(projection);
            upsert(&mut projection.nodes, event.payload.clone());
        }
        Event::SubagentUpdated(event) => {
            bump(projection);
            let mut subagent = event.payload.clone();
            let previous = projection.subagents.iter().find(|agent| agent.id == subagent.id).cloned();
            restore_subagent_bodies(&mut subagent, previous.as_deref());
            upsert(&mut projection.subagents, subagent);
        }
        Event::ProviderSessionAttachedOrProviderSessionUpdated(event) => {
            bump(projection);
            upsert(&mut projection.provider_sessions, event.payload.clone());
        }
        Event::ProviderSessionDetached(event) => {
            bump(projection);
            let id = &event.payload.provider_session_id;
            if projection.provider_sessions.iter().any(|session| &session.id == id) {
                Arc::make_mut(&mut projection.provider_sessions).retain(|session| &session.id != id);
            }
        }
        Event::ProviderThreadUpdated(event) => {
            bump(projection);
            let payload = &event.payload;
            if payload.app_thread_id.as_ref() == Some(&projection.thread.id) && !is_queued_placeholder(payload) {
                let id: ProviderThreadId = payload.id.clone();
                edit_thread(projection, |thread| thread.active_provider_thread_id = Some(id));
            }
            upsert(&mut projection.provider_threads, payload.clone());
        }
        Event::ProviderTurnUpdated(event) => {
            bump(projection);
            let mut turn = event.payload.clone();
            if turn.token_usage.is_none() {
                turn.token_usage = projection.provider_turns.iter().find(|held| held.id == turn.id).and_then(|held| held.token_usage.clone());
            }
            upsert(&mut projection.provider_turns, turn);
        }
        Event::RuntimeRequestUpdated(event) => {
            bump(projection);
            upsert(&mut projection.runtime_requests, event.payload.clone());
        }
        Event::MessageUpdated(event) => {
            bump(projection);
            upsert(&mut projection.messages, event.payload.clone());
        }
        Event::PlanUpdated(event) => {
            bump(projection);
            upsert(&mut projection.plans, event.payload.clone());
        }
        Event::TurnItemUpdated(event) => return apply_turn_item(projection, &event.payload, occurred_at, partial),
        Event::CheckpointScopeCreated(event) => {
            bump(projection);
            upsert(&mut projection.checkpoint_scopes, event.payload.clone());
        }
        Event::CheckpointCaptured(event) => {
            bump(projection);
            upsert(&mut projection.checkpoints, event.payload.clone());
        }
        Event::CheckpointRollbackRequested(_) => bump(projection),
        Event::ContextHandoffUpdated(event) => {
            bump(projection);
            upsert(&mut projection.context_handoffs, event.payload.clone());
        }
        Event::ContextTransferCreated(t3code_contracts::OrchestrationV2DomainEventContextTransferCreated { payload, .. })
        | Event::ContextTransferUpdated(t3code_contracts::OrchestrationV2DomainEventContextTransferUpdated { payload, .. }) => {
            bump(projection);
            upsert(&mut projection.context_transfers, payload.clone());
        }
        Event::Unknown(_) => return false,
    }
    true
}

fn apply_run(projection: &mut ThreadProjection, run: &OrchestrationV2Run) {
    let was_queued = projection.runs.iter().any(|held| held.id == run.id && held.status == OrchestrationV2RunStatus::Queued);
    upsert(&mut projection.runs, run.clone());
    refresh_visibility(projection);
    if was_queued && run.status != OrchestrationV2RunStatus::Queued {
        restore_queued_user_items(projection, &run.id);
    }
}

fn apply_turn_item(
    projection: &mut ThreadProjection,
    payload: &OrchestrationV2TurnItem,
    occurred_at: &str,
    partial: Option<PartialWindow>,
) -> bool {
    let mut item = restore_turn_item_bodies(payload, &LatestBodies(projection)).unwrap_or_else(|| payload.clone());
    let id = item.item_id();
    // Streamed updates land on the newest items, so search from the end.
    let index = projection.turn_items.iter().rposition(|held| held.item_id() == id);
    let previous = index.map(|index| Arc::clone(&projection.turn_items[index]));
    if let Some(previous) = &previous {
        item.keep_deferred_work_from(previous);
    }
    if index.is_none() && partial.is_some_and(|window| drop_missing_partial_item(projection, &item, window)) {
        return false;
    }
    let item = Arc::new(item);
    projection.updated_at = occurred_at.to_owned();
    {
        let items = Arc::make_mut(&mut projection.turn_items);
        match index {
            Some(index) => items[index] = Arc::clone(&item),
            None => items.push(Arc::clone(&item)),
        }
    }
    // Only interrupt requests change another item's visibility; streaming text must not recheck
    // every row against every run.
    let interrupt = |item: &OrchestrationV2TurnItem| matches!(item, OrchestrationV2TurnItem::RunInterruptRequest(_));
    if interrupt(&item) || previous.as_deref().is_some_and(interrupt) {
        refresh_visibility(projection);
    }
    if is_turn_item_visible(&item, &projection.runs, &projection.attempts, &projection.turn_items) {
        upsert_visible(projection, &item, partial);
    } else {
        remove_visible(&mut projection.visible_turn_items, &id);
    }
    true
}
