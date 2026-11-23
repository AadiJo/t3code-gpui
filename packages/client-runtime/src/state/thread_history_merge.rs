//! Merging older history pages into a bounded projection (`threadHistoryMerge.ts`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use t3code_contracts::{
    OrchestrationV2Subagent, OrchestrationV2ThreadProjectionVisibleTurnItemsItemVisibility as Visibility,
    OrchestrationV2TurnItem, ThreadId, TurnItemId,
};
use t3code_shared::orchestration_v2_timeline::is_turn_item_visible;
use t3code_shared::thread_projection::{ThreadProjection, VisibleTurnItem};
use t3code_shared::turn_item::TurnItemExt;

/// Paging state of a thread's timeline (`ThreadHistoryMeta`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryMeta {
    pub history_cursor: Option<String>,
    pub has_more_history: bool,
    pub loading: bool,
    pub error: Option<String>,
    /// Older pages were merged in. Stays true after the last page, so the projection is never
    /// cached as a complete window.
    pub expanded: bool,
    pub latest_local_turn_ordinal: Option<u64>,
}

impl HistoryMeta {
    /// Whether the projection is a bounded window with older rows not loaded.
    pub fn is_partial(&self) -> bool {
        self.has_more_history || self.history_cursor.is_some()
    }

    /// Whether the timeline shows "load earlier" (`shouldShowLoadEarlierControl`).
    pub fn shows_load_earlier(&self) -> bool {
        self.has_more_history || self.error.is_some()
    }

    /// The meta after a conversation page (`applyHistoryPageMeta`).
    pub fn after_page(&self, next_cursor: Option<String>, has_more_history: bool) -> Self {
        Self {
            history_cursor: next_cursor,
            has_more_history,
            loading: false,
            error: None,
            expanded: true,
            latest_local_turn_ordinal: self.latest_local_turn_ordinal,
        }
    }
}

type RowKey = (ThreadId, TurnItemId);

fn key(row: &VisibleTurnItem) -> RowKey {
    (row.source_thread_id.clone(), row.source_item_id.clone())
}

/// A page is a contiguous range: current rows before its first shared row are older than the
/// page start, and new page rows sit just before the next row both lists share.
fn interleave(current: &[VisibleTurnItem], page: Vec<VisibleTurnItem>, page_keys: &HashSet<RowKey>) -> Vec<VisibleTurnItem> {
    let index: HashMap<RowKey, usize> = current.iter().enumerate().map(|(i, row)| (key(row), i)).collect();
    let mut merged = Vec::with_capacity(current.len() + page.len());
    let mut pending = Vec::new();
    let mut next = 0;
    let emit = |merged: &mut Vec<VisibleTurnItem>, next: &mut usize, end: usize| {
        while *next < end {
            let row = &current[*next];
            if !page_keys.contains(&key(row)) {
                merged.push(row.clone());
            }
            *next += 1;
        }
    };
    for row in page {
        match index.get(&key(&row)) {
            None => pending.push(row),
            Some(&at) => {
                emit(&mut merged, &mut next, at + 1);
                merged.append(&mut pending);
                merged.push(row);
            }
        }
    }
    merged.append(&mut pending);
    emit(&mut merged, &mut next, current.len());
    merged
}

/// Where a collapsed work group's page goes: the anchor row and the cursor after this page.
pub struct WorkPage<'a> {
    pub anchor_id: &'a TurnItemId,
    pub next_cursor: Option<String>,
}

/// Interleaves an older page into the timeline (`mergeOlderHistoryIntoProjection`). With `work`
/// the page fills the collapsed group after its anchor instead. Returns whether anything changed.
pub fn merge_older_history(projection: &mut ThreadProjection, older: Vec<VisibleTurnItem>, work: Option<WorkPage<'_>>) -> bool {
    if older.is_empty() && work.is_none() {
        return false;
    }
    let thread_id = projection.thread.id.clone();
    let start = work.as_ref().and_then(|work| projection.visible_turn_items.iter().position(|row| &row.source_item_id == work.anchor_id));
    let anchor = start.map(|start| projection.visible_turn_items[start].clone());
    let deferred_last = anchor.as_ref().and_then(|anchor| anchor.item.deferred_work().map(|work| work.last_ordinal));
    if work.is_some() && deferred_last.is_none() {
        return false;
    }
    let current: HashMap<RowKey, &VisibleTurnItem> = projection.visible_turn_items.iter().map(|row| (key(row), row)).collect();
    let mut items_by_id: HashMap<TurnItemId, Arc<OrchestrationV2TurnItem>> =
        projection.turn_items.iter().map(|item| (item.item_id(), Arc::clone(item))).collect();
    let mut item_order: Vec<TurnItemId> = projection.turn_items.iter().map(|item| item.item_id()).collect();
    let mut page_keys = HashSet::new();
    let mut page = Vec::new();
    for page_row in older {
        if let (Some(anchor), Some(last)) = (&anchor, deferred_last) {
            let ordinal = page_row.item.item_ordinal();
            if page_row.source_thread_id != anchor.source_thread_id || ordinal < anchor.item.item_ordinal() || ordinal > last {
                continue;
            }
        }
        let row_key = key(&page_row);
        if page_keys.contains(&row_key) {
            continue;
        }
        let mut row = current.get(&row_key).map_or(page_row, |held| (*held).clone());
        let local = row.visibility == Visibility::Local || row.source_thread_id == thread_id;
        if !current.contains_key(&row_key) && local
            && let Some(held) = items_by_id.get(&row.source_item_id)
        {
            // Hidden live items stay hidden. Snapshots also keep interrupt requests and pending
            // request cards outside the window; their history page shows them.
            let card = matches!(
                &**held,
                OrchestrationV2TurnItem::RunInterruptRequest(_)
                    | OrchestrationV2TurnItem::ApprovalRequest(_)
                    | OrchestrationV2TurnItem::UserInputRequest(_)
            );
            if !card || !is_turn_item_visible(held, &projection.runs, &projection.attempts, &projection.turn_items) {
                continue;
            }
            row.item = Arc::clone(held);
        }
        page_keys.insert(row_key);
        if local && !items_by_id.contains_key(&row.source_item_id) {
            items_by_id.insert(row.source_item_id.clone(), Arc::clone(&row.item));
            item_order.push(row.source_item_id.clone());
        }
        page.push(row);
    }
    if page.is_empty() && work.is_none() {
        return false;
    }
    let rows = &projection.visible_turn_items;
    let visible: Vec<VisibleTurnItem> = match (work, start, anchor, deferred_last) {
        (Some(work), Some(start), Some(anchor), Some(last)) => {
            let mut end = start + 1;
            while end < rows.len() {
                let row = &rows[end];
                if row.source_thread_id != anchor.source_thread_id
                    || row.visibility == Visibility::Synthetic
                    || row.item.item_ordinal() > last
                {
                    break;
                }
                end += 1;
            }
            let mut item = (*anchor.item).clone();
            item.set_deferred_work_cursor(work.next_cursor);
            let updated = VisibleTurnItem { item: Arc::new(item), ..anchor.clone() };
            if anchor.visibility == Visibility::Local {
                items_by_id.insert(anchor.source_item_id.clone(), Arc::clone(&updated.item));
            }
            let mut section: Vec<VisibleTurnItem> = page;
            section.extend(rows[start..end].iter().filter(|row| !page_keys.contains(&key(row))).cloned());
            for row in &mut section {
                if row.source_item_id == anchor.source_item_id {
                    *row = updated.clone();
                }
            }
            section.sort_by_key(|row| row.item.item_ordinal());
            let mut merged = rows[..start].to_vec();
            merged.extend(section);
            merged.extend(rows[end..].iter().cloned());
            merged
        }
        _ => interleave(rows, page, &page_keys),
    };
    let unchanged = visible.len() == rows.len()
        && visible.iter().zip(rows.iter()).all(|(a, b)| Arc::ptr_eq(&a.item, &b.item) && a.source_item_id == b.source_item_id);
    if unchanged {
        return false;
    }
    let mut visible = visible;
    for (position, row) in visible.iter_mut().enumerate() {
        row.position = position as u64;
    }
    projection.turn_items = Arc::new(item_order.iter().filter_map(|id| items_by_id.get(id).cloned()).collect());
    projection.visible_turn_items = Arc::new(visible);
    true
}

/// Adds older subagent records; a record already held is newer and stays
/// (`mergeOlderSubagentsIntoProjection`).
pub fn merge_older_subagents(projection: &mut ThreadProjection, older: Option<Vec<Arc<OrchestrationV2Subagent>>>) {
    let held: HashSet<_> = projection.subagents.iter().map(|agent| agent.id.clone()).collect();
    let mut added: Vec<_> = older.unwrap_or_default().into_iter().filter(|agent| !held.contains(&agent.id)).collect();
    if added.is_empty() {
        return;
    }
    added.extend(projection.subagents.iter().cloned());
    projection.subagents = Arc::new(added);
}
