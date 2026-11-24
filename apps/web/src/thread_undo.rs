//! The thread undo notice (`showThreadUndoNotice.ts`). Settling, snoozing, unpinning or
//! archiving a thread offers an undo for 5 s; consecutive actions of one kind share one notice
//! and one undo. `thread.undo` and the notice's button run the latest group.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{App, Global, SharedString};

use crate::components::sidebar::model::ThreadKey;
use crate::i18n::t;

/// How long a notice stays.
const NOTICE_TIMEOUT: Duration = Duration::from_secs(5);

/// What the notice says was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoAction {
    Settled,
    Snoozed,
    Unpinned,
    Archived,
}

impl UndoAction {
    /// The notice's short summary (`SHORT_SUMMARY` in `SidebarThreadUndoNotice.tsx`).
    pub fn short_summary(self, count: usize) -> SharedString {
        match (self, count) {
            (Self::Settled, 1) => t!("Settled,"),
            (Self::Snoozed, 1) => t!("Snoozed,"),
            (Self::Unpinned, 1) => t!("Unpinned,"),
            (Self::Archived, 1) => t!("Archived,"),
            (Self::Settled, n) => t!("Settled {count},", count = n),
            (Self::Snoozed, n) => t!("Snoozed {count},", count = n),
            (Self::Unpinned, n) => t!("Unpinned {count},", count = n),
            (Self::Archived, n) => t!("Archived {count},", count = n),
        }
    }
}

/// Restores one thread; failures toast on their own.
pub type UndoFn = Rc<dyn Fn(&mut App)>;

struct Entry {
    action: UndoAction,
    key: ThreadKey,
    undo: UndoFn,
}

/// The undoable actions of the last 5 s.
#[derive(Default)]
pub struct ThreadUndo {
    entries: Vec<Entry>,
    /// Bumped on every new action, so only the last one's timer clears the notice.
    generation: u64,
}

impl Global for ThreadUndo {}

impl ThreadUndo {
    /// Offers an undo for `action` on `key`. A newer action on the same thread replaces its
    /// older one (`ThreadUndo.begin` claims).
    pub fn show(action: UndoAction, key: ThreadKey, undo: UndoFn, cx: &mut App) {
        let generation = {
            let state = cx.default_global::<Self>();
            state.entries.retain(|entry| entry.key != key);
            state.entries.push(Entry { action, key, undo });
            state.generation += 1;
            state.generation
        };
        cx.refresh_windows();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(NOTICE_TIMEOUT).await;
            cx.update(|cx| {
                let state = cx.default_global::<Self>();
                if state.generation == generation {
                    state.entries.clear();
                    cx.refresh_windows();
                }
            });
        })
        .detach();
    }

    /// The notice to show: the latest action and how many threads it covers.
    pub fn notice(cx: &App) -> Option<(UndoAction, usize)> {
        let entries = &cx.try_global::<Self>()?.entries;
        let latest = entries.last()?.action;
        Some((latest, entries.iter().rev().take_while(|entry| entry.action == latest).count()))
    }

    /// Undoes the latest group (`undoLatestThreadAction`). False when there is nothing to undo.
    pub fn undo_latest(cx: &mut App) -> bool {
        let Some((latest, count)) = Self::notice(cx) else { return false };
        let group: Vec<UndoFn> = {
            let state = cx.default_global::<Self>();
            let split = state.entries.len() - count;
            state.entries.drain(split..).filter(|entry| entry.action == latest).map(|entry| entry.undo).collect()
        };
        cx.refresh_windows();
        for undo in group {
            undo(cx);
        }
        true
    }
}
