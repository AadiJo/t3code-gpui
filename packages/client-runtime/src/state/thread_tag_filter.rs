//! Which threads a server's tag filter hides (`threadTagFilter.ts`, `hiddenThreadTagsAtom`).
//! Children (subagents, side chats, delegated tasks) follow their parent; while any environment
//! hides tags, a child whose parent is not loaded stays hidden.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use t3code_contracts::{EnvironmentId, ServerSettings, ThreadId};

use super::models::ThreadRow;

/// One thread's scoped identity.
pub type ThreadRef = (EnvironmentId, ThreadId);

/// The tags an environment hides, or `None` while its filter is off (`hiddenThreadTagsAtom`).
/// Tags are trimmed, so a hand-edited " private" still matches.
pub fn hidden_tags(settings: Option<&ServerSettings>) -> Option<Vec<String>> {
    let settings = settings?;
    if settings.hide_tagged_threads != Some(true) {
        return None;
    }
    let tags: Vec<String> = settings.hidden_thread_tags.iter().flatten().map(|tag| tag.trim()).filter(|tag| !tag.is_empty()).map(str::to_owned).collect();
    (!tags.is_empty()).then_some(tags)
}

/// The thread a child takes its visibility from (`visibilityParentKey`).
fn visibility_parent(row: &ThreadRow) -> Option<ThreadRef> {
    let lineage = &row.source.lineage;
    if !lineage.is_listed()
        && let Some(parent) = &lineage.parent_thread_id
    {
        return Some((row.environment_id.clone(), parent.clone()));
    }
    row.source.delegated_from.as_ref().map(|from| (from.environment_id.clone(), from.thread_id.clone()))
}

/// Keys of the threads the tag filters hide (`hiddenThreadKeys`). `rows` are every enabled
/// environment's shell rows; `filters` the hidden tags by environment (absent means off).
pub fn hidden_thread_keys(rows: &[Arc<ThreadRow>], filters: &HashMap<EnvironmentId, Vec<String>>) -> HashSet<ThreadRef> {
    if filters.is_empty() {
        return HashSet::new();
    }
    let by_key: HashMap<ThreadRef, &ThreadRow> = rows.iter().map(|row| ((row.environment_id.clone(), row.id().clone()), &**row)).collect();
    let mut known: HashMap<ThreadRef, bool> = HashMap::new();
    fn is_hidden(key: &ThreadRef, by_key: &HashMap<ThreadRef, &ThreadRow>, filters: &HashMap<EnvironmentId, Vec<String>>, known: &mut HashMap<ThreadRef, bool>) -> bool {
        if let Some(hidden) = known.get(key) {
            return *hidden;
        }
        let Some(row) = by_key.get(key) else { return false };
        let hidden = match visibility_parent(row) {
            // Seeded first so a malformed cycle ends visible.
            Some(parent) if by_key.contains_key(&parent) => {
                known.insert(key.clone(), false);
                is_hidden(&parent, by_key, filters, known)
            }
            // The parent is not loaded, and some environment hides tags.
            Some(_) => true,
            None => {
                let tags = row.source.tags.as_deref().unwrap_or_default();
                filters.get(&row.environment_id).is_some_and(|hidden| hidden.iter().any(|tag| tags.contains(tag)))
            }
        };
        known.insert(key.clone(), hidden);
        hidden
    }
    by_key.keys().filter(|key| is_hidden(key, &by_key, filters, &mut known)).cloned().collect()
}
