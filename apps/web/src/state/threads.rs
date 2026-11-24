//! The open thread and the mapped view models of recently shown ones (`state/threads.ts`).
//!
//! Opening a thread mounts it in the runtime, whose first value is already the best
//! projection held (live, the 5-minute resume cache, or the disk cache read synchronously).
//! The mapped [`TimelineInput`] of a closed thread stays here under the runtime's own caps
//! ([`RESUME_CACHE_LIMIT`] threads, [`RESUME_TTL`]), so a revisit whose projection did not
//! change hands the timeline its rows without mapping again; one that did change reuses every
//! unchanged row's mapped entry.

use std::collections::VecDeque;
use std::time::Instant;

use gpui_kit::{App, AppContext as _, Context, Entity, Subscription};
use t3code_client_runtime::state::environment::{RESUME_CACHE_LIMIT, RESUME_TTL};
use t3code_client_runtime::state::models::now_ms;
use t3code_client_runtime::state::drafts::scoped_thread_key;

use super::StateBridge;
use super::shell::runtime_ids;
use crate::components::chat::timeline_entries::TimelineInput;
use crate::components::sidebar::model::ThreadKey;
use crate::thread_sync::{LiveThread, MappedThread};

struct CachedView {
    key: ThreadKey,
    mapped: MappedThread,
    closed_at: Instant,
}

/// How the last switch was served, for measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchSource {
    /// The mapped view model was reused as is.
    MappedCache,
    /// Rows were mapped from the runtime's first value (live, resume or disk cache).
    Mapped,
    /// Nothing was held; the timeline waits for the network.
    Empty,
}

/// The open thread and the cache of closed ones. Notifies when the open thread changes.
/// A side chat shown in the right panel streams beside it in its own slot.
#[derive(Default)]
pub struct ThreadViews {
    active: Option<Entity<LiveThread>>,
    side: Option<Entity<LiveThread>>,
    cache: VecDeque<CachedView>,
    last_switch: Option<(SwitchSource, std::time::Duration)>,
    _subscription: Option<Subscription>,
    /// The open thread's visit tracker hears here whether the window is in the background; it
    /// stops when this is dropped.
    viewing: Option<tokio::sync::watch::Sender<bool>>,
    /// The window went to the background (`ThreadVisitWatermark` `windowFocusedAtom`).
    window_inactive: bool,
}

impl ThreadViews {
    pub fn active(&self) -> Option<&Entity<LiveThread>> {
        self.active.as_ref()
    }

    /// How the last [`Self::open`] was served and how long it took.
    pub fn last_switch(&self) -> Option<(SwitchSource, std::time::Duration)> {
        self.last_switch
    }

    /// Threads whose mapped view model is held.
    pub fn cached_keys(&self) -> impl Iterator<Item = &ThreadKey> {
        self.cache.iter().map(|view| &view.key)
    }

    fn sweep(&mut self) {
        let now = Instant::now();
        self.cache.retain(|view| now.duration_since(view.closed_at) < RESUME_TTL);
        while self.cache.len() > RESUME_CACHE_LIMIT {
            self.cache.pop_front();
        }
    }

    /// Closes the open thread, keeping its mapped view model.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(active) = self.active.take() {
            let key = active.read(cx).key().clone();
            let mapped = active.update(cx, |thread, _| thread.take_mapped());
            self.cache.retain(|view| view.key != key);
            self.cache.push_back(CachedView { key, mapped, closed_at: Instant::now() });
            self.sweep();
        }
        self._subscription = None;
        self.viewing = None;
        cx.notify();
    }

    /// The window gained or lost focus. A background window does not count as a visit: a turn
    /// that finishes meanwhile stays unread until the window is in front again.
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_inactive = !active;
        if let Some(viewing) = &self.viewing {
            viewing.send_replace(active);
        }
        if active && let Some(thread) = &self.active {
            let (environment_id, thread_id) = runtime_ids(thread.read(cx).key());
            StateBridge::ui_state(cx).update(|ui| ui.mark_local_visit(scoped_thread_key(&environment_id, &thread_id), now_ms()));
        }
    }

    /// Opens a thread and makes it the active one. Returns `None` when its environment is not
    /// enabled. The returned thread's [`LiveThread::input`] is complete when the runtime or this
    /// cache held the thread.
    pub fn open(&mut self, key: ThreadKey, cx: &mut Context<Self>) -> Option<Entity<LiveThread>> {
        if let Some(active) = &self.active
            && active.read(cx).key() == &key
        {
            return Some(active.clone());
        }
        let started = Instant::now();
        let (environment_id, thread_id) = runtime_ids(&key);
        let environment = StateBridge::environment(&environment_id, cx)?;
        self.close(cx);
        let promoted = self.side.take_if(|side| side.read(cx).key() == &key);
        let cached = self.cache.iter().position(|view| view.key == key).and_then(|ix| self.cache.remove(ix)).map(|view| view.mapped);
        let had_projection = cached.as_ref().and_then(|mapped| mapped.projection.clone());
        let commands = environment.read(cx).commands.clone();
        // A side chat the main pane navigates to keeps its stream and mapped rows.
        let thread = match promoted {
            Some(thread) => thread,
            None => {
                let handle = environment.read(cx).store.open_thread(&thread_id);
                cx.new(|cx| LiveThread::new(key.clone(), handle, cached, cx))
            }
        };
        let source = match (&had_projection, thread.read(cx).projection()) {
            (Some(previous), Some(current)) if std::sync::Arc::ptr_eq(previous, current) => SwitchSource::MappedCache,
            (_, Some(_)) => SwitchSource::Mapped,
            (_, None) => SwitchSource::Empty,
        };
        self.last_switch = Some((source, started.elapsed()));
        self._subscription = Some(cx.observe(&thread, |_, _, cx| cx.notify()));
        self.active = Some(thread.clone());
        // The visit watermark: the server's for unread state, the local one for old servers.
        if !self.window_inactive {
            StateBridge::ui_state(cx).update(|ui| ui.mark_local_visit(scoped_thread_key(&environment_id, &thread_id), now_ms()));
        }
        // `createThreadVisitTracker`: one open, then the watermark follows the row while shown.
        let (viewing, watch) = tokio::sync::watch::channel(!self.window_inactive);
        self.viewing = Some(viewing);
        StateBridge::spawn(t3code_client_runtime::state::thread_visits::run_visit_tracker(commands, thread_id, watch, |_| None), cx).detach();
        StateBridge::shell(cx).update(cx, |shell, cx| shell.set_active_thread(Some(key), cx));
        cx.notify();
        Some(thread)
    }

    /// `key`'s open thread in either slot.
    pub fn find(&self, key: &ThreadKey, cx: &App) -> Option<&Entity<LiveThread>> {
        self.active.iter().chain(self.side.iter()).find(|t| t.read(cx).key() == key)
    }

    /// The side chat the right panel shows, if any.
    pub fn side(&self) -> Option<&Entity<LiveThread>> {
        self.side.as_ref()
    }

    /// Opens a side chat in its own slot, leaving the main pane's thread, its visit mark and
    /// the shell's active thread alone. A different side chat replaces the previous one.
    pub fn open_side(&mut self, key: ThreadKey, cx: &mut Context<Self>) -> Option<Entity<LiveThread>> {
        if let Some(side) = &self.side
            && side.read(cx).key() == &key
        {
            return Some(side.clone());
        }
        let (environment_id, thread_id) = runtime_ids(&key);
        let environment = StateBridge::environment(&environment_id, cx)?;
        let handle = environment.read(cx).store.open_thread(&thread_id);
        let thread = cx.new(|cx| LiveThread::new(key, handle, None, cx));
        self.side = Some(thread.clone());
        Some(thread)
    }

    /// Unmounts the side chat's stream when it is `key`'s.
    pub fn close_side(&mut self, key: &ThreadKey, cx: &mut Context<Self>) {
        if self.side.as_ref().is_some_and(|side| side.read(cx).key() == key) {
            self.side = None;
        }
    }

    /// The view model of the open thread, if any.
    pub fn active_input(&self, cx: &App) -> Option<TimelineInput> {
        self.active.as_ref().map(|thread| thread.read(cx).input().clone())
    }
}
