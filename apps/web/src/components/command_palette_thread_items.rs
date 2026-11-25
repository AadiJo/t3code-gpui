//! Thread rows. Ports `buildThreadActionItems` and the recent-threads list
//! from `CommandPalette.logic.ts` / `CommandPaletteThreadItems.tsx`.

use std::rc::Rc;
use std::sync::Arc;

use t3code_shared::palette_search::thread_recent_use;

use super::command_palette_logic::{PaletteEvent, PaletteItem, PaletteItemKind, PaletteRun, PaletteTitle, RECENT_THREAD_LIMIT};
use super::command_palette_lookups::{PaletteProject, PaletteThread};
use super::icons::IconName;
use super::sidebar::model::ThreadKey;
use crate::i18n::t;

/// The value a thread row is known by.
pub fn thread_value(key: &ThreadKey) -> String {
    format!("thread:{}:{}", key.environment_id.0, key.thread_id.0)
}

/// One row per thread, in the order given. The open thread is listed last
/// in its group, since picking it does nothing.
pub fn build_thread_items(
    threads: &[Arc<PaletteThread>],
    projects: &[PaletteProject],
    active: Option<&ThreadKey>,
    now_ms: i64,
) -> Vec<Rc<PaletteItem>> {
    threads
        .iter()
        .map(|thread| {
            let project_title = thread
                .project
                .as_ref()
                .and_then(|project| projects.iter().find(|p| &p.project_ref == project))
                .map(|p| p.title.clone());
            let is_current = active == Some(&thread.key);
            let mut parts: Vec<String> = Vec::with_capacity(3);
            if let Some(title) = project_title {
                parts.push(title.to_string());
            }
            if let Some(branch) = &thread.branch {
                parts.push(format!("#{branch}"));
            }
            if is_current {
                parts.push(t!("Current thread").to_string());
            }
            Rc::new(PaletteItem {
                value: thread_value(&thread.key).into(),
                title: PaletteTitle::Text(thread.title.clone()),
                terms: Rc::from([]),
                title_is_first_term: true,
                description: (!parts.is_empty()).then(|| parts.join(" · ").into()),
                icon: IconName::MessageSquare,
                disabled: false,
                unavailable: false,
                shortcut: None,
                secondary: is_current,
                timestamp: Some(thread.timestamp(now_ms)),
                thread: Some(thread.clone()),
                trailing: None,
                kind: PaletteItemKind::Action {
                    run: PaletteRun::Event(PaletteEvent::OpenThread { thread: thread.key.clone(), query: Default::default() }),
                    keep_open: false,
                },
                hide_chevron: false,
                brand: None,
                project: None,
            })
        })
        .collect()
}

/// The threads used most recently, the open one left out.
pub fn recent_thread_items(items: &[Rc<PaletteItem>], active: Option<&ThreadKey>) -> Vec<Rc<PaletteItem>> {
    let mut recent: Vec<&Rc<PaletteItem>> = items
        .iter()
        .filter(|item| item.thread.as_ref().is_some_and(|thread| Some(&thread.key) != active))
        .collect();
    recent.sort_by_key(|item| std::cmp::Reverse(item.thread.as_ref().map_or(0, |t| thread_recent_use(&t.subject))));
    recent.into_iter().take(RECENT_THREAD_LIMIT).cloned().collect()
}
