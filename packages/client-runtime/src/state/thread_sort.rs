//! Sidebar sort orders (`threadSort.ts`, the sorts in `threadInbox.ts`). Each decorates rows
//! with parsed keys once, then sorts.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use t3code_shared::pin_order_key::{generate_spread_pin_order_keys, pin_order_key_between};

use super::models::{EpochMs, ThreadRow};

/// Ties break on id, then environment: ids are unique only within an environment.
fn identity(a: &ThreadRow, b: &ThreadRow) -> Ordering {
    a.id().as_str().cmp(b.id().as_str()).then_with(|| a.environment_id.as_str().cmp(b.environment_id.as_str()))
}

fn sort_by_key_desc<'a>(rows: &mut Vec<&'a ThreadRow>, key: impl Fn(&ThreadRow) -> i64) {
    let mut keyed: Vec<(i64, &'a ThreadRow)> = rows.iter().map(|row| (key(row), *row)).collect();
    keyed.sort_by(|(ka, a), (kb, b)| kb.cmp(ka).then_with(|| identity(a, b)));
    *rows = keyed.into_iter().map(|(_, row)| row).collect();
}

/// The latest of a thread's activity times (`resolveThreadActivityTimestamp`).
pub fn activity_timestamp(row: &ThreadRow) -> Option<EpochMs> {
    let run = row.latest_run.as_ref();
    [row.latest_user_message_at, run.and_then(|r| r.requested_at), run.and_then(|r| r.started_at), run.and_then(|r| r.completed_at)]
        .into_iter()
        .flatten()
        .max()
        .or(row.updated_at)
}

/// Pinned: by order key as a plain string, keyless after them newest first
/// (`sortPinnedThreadsByOrderKey`).
pub fn sort_pinned(rows: &mut Vec<&ThreadRow>) {
    let (mut keyed, mut keyless): (Vec<&ThreadRow>, Vec<&ThreadRow>) = rows.iter().partition(|row| row.source.pin_order_key.is_some());
    keyed.sort_by(|a, b| a.source.pin_order_key.cmp(&b.source.pin_order_key).then_with(|| identity(a, b)));
    sort_by_key_desc(&mut keyless, |row| row.created_at.unwrap_or(0));
    keyed.extend(keyless);
    *rows = keyed;
}

/// Active, default order: keyless rows first by `max(createdAt, unsettledAt)`, newest first,
/// then by `activeOrderKey` (`sortActiveThreadsByOrderKey`).
pub fn sort_active(rows: &mut [&ThreadRow]) {
    let anchor = |row: &ThreadRow| row.created_at.unwrap_or(0).max(row.unsettled_at.unwrap_or(0));
    rows.sort_by(|a, b| {
        let (ka, kb) = (&a.source.active_order_key, &b.source.active_order_key);
        match (ka, kb) {
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(ka), Some(kb)) => ka.cmp(kb),
            (None, None) => anchor(b).cmp(&anchor(a)),
        }
        .then_with(|| identity(a, b))
    });
}

/// Active with the working shelf on: by when the thread came back
/// (`sortInboxThreadsByReturn`). `observed_return` is the inbox tracker's return time.
pub fn sort_by_return(rows: &mut Vec<&ThreadRow>, observed_return: impl Fn(&ThreadRow) -> Option<EpochMs>) {
    sort_by_key_desc(rows, |row| {
        let run = row.latest_run.as_ref();
        [row.created_at, row.unsettled_at, run.and_then(|r| r.requested_at), run.and_then(|r| r.completed_at), observed_return(row)]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(0)
    });
}

/// Working shelf and folder mode: by the user's last send (`sortWorkingThreadsBySend`).
pub fn sort_by_send(rows: &mut Vec<&ThreadRow>) {
    sort_by_key_desc(rows, |row| {
        let sent = row.latest_user_authored_message_at.or(row.latest_run.as_ref().and_then(|run| run.requested_at));
        row.created_at.unwrap_or(0).max(sent.unwrap_or(0))
    });
}

/// Snoozed: soonest wake first.
pub fn sort_snoozed(rows: &mut [&ThreadRow]) {
    rows.sort_by(|a, b| a.snoozed_until.unwrap_or(i64::MAX).cmp(&b.snoozed_until.unwrap_or(i64::MAX)).then_with(|| identity(a, b)));
}

/// Settled: `settledAt`, else activity time, newest first; ties in plain id order as the
/// server pages them (`sortSettledThreads`).
pub fn sort_settled(rows: &mut [&ThreadRow]) {
    let key = |row: &ThreadRow| row.settled_at.or_else(|| activity_timestamp(row)).unwrap_or(0);
    rows.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| a.id().as_str().cmp(b.id().as_str())));
}

/// Order keys to write after `moved_id` was dropped among `ordered_ids` (`planPinnedReorder`).
/// `keys_by_id` includes hidden rows' keys, which are reserved; only `ordered_ids` get writes.
pub fn plan_pinned_reorder(ordered_ids: &[&str], keys_by_id: &HashMap<&str, Option<&str>>, moved_id: &str) -> Vec<(String, String)> {
    let visible: HashSet<&str> = ordered_ids.iter().copied().collect();
    let reserved: HashSet<&str> =
        keys_by_id.iter().filter(|(id, _)| !visible.contains(*id)).filter_map(|(_, key)| *key).collect();
    let Some(moved) = ordered_ids.iter().position(|id| *id == moved_id) else { return Vec::new() };
    let key_of = |index: usize| keys_by_id.get(ordered_ids[index]).copied().flatten();
    let before = (moved > 0).then(|| key_of(moved - 1));
    let after = (moved + 1 < ordered_ids.len()).then(|| key_of(moved + 1));
    let usable = |neighbour: Option<Option<&str>>| neighbour.is_none_or(|key| key.is_some());
    if usable(before) && usable(after) {
        let after_key = after.flatten();
        let mut key = pin_order_key_between(before.flatten(), after_key);
        while let Some(candidate) = key.as_deref().filter(|candidate| reserved.contains(candidate)) {
            key = pin_order_key_between(Some(candidate), after_key);
        }
        if let Some(key) = key {
            return vec![(moved_id.to_owned(), key)];
        }
    }
    let keys: Vec<String> =
        generate_spread_pin_order_keys(ordered_ids.len() + reserved.len()).into_iter().filter(|key| !reserved.contains(key.as_str())).take(ordered_ids.len()).collect();
    ordered_ids
        .iter()
        .zip(keys)
        .filter(|(id, key)| keys_by_id.get(*id).copied().flatten() != Some(key.as_str()))
        .map(|(id, key)| ((*id).to_owned(), key))
        .collect()
}
