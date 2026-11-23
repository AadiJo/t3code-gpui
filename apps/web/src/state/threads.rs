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
use t3code_client_runtime::state::models::{format_ms, now_ms};
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
#[derive(Default)]
pub struct ThreadViews {
    active: Option<Entity<LiveThread>>,
    cache: VecDeque<CachedView>,
    last_switch: Option<(SwitchSource, std::time::Duration)>,
    _subscription: Option<Subscription>,
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
        cx.notify();
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
        let cached = self.cache.iter().position(|view| view.key == key).and_then(|ix| self.cache.remove(ix)).map(|view| view.mapped);
        let had_projection = cached.as_ref().and_then(|mapped| mapped.projection.clone());
        let (handle, commands) = {
            let env = environment.read(cx);
            (env.store.open_thread(&thread_id), env.commands.clone())
        };
        let thread = cx.new(|cx| LiveThread::new(key.clone(), handle, cached, cx));
        let source = match (&had_projection, thread.read(cx).projection()) {
            (Some(previous), Some(current)) if std::sync::Arc::ptr_eq(previous, current) => SwitchSource::MappedCache,
            (_, Some(_)) => SwitchSource::Mapped,
            (_, None) => SwitchSource::Empty,
        };
        self.last_switch = Some((source, started.elapsed()));
        self._subscription = Some(cx.observe(&thread, |_, _, cx| cx.notify()));
        self.active = Some(thread.clone());
        // The visit watermark: the server's for unread state, the local one for old servers.
        let now = now_ms();
        StateBridge::ui_state(cx).update(|ui| ui.mark_local_visit(scoped_thread_key(&environment_id, &thread_id), now));
        StateBridge::spawn(async move { commands.visit(&thread_id, format_ms(now), true, None).await }, cx).detach();
        StateBridge::shell(cx).update(cx, |shell, cx| shell.set_active_thread(Some(key), cx));
        cx.notify();
        Some(thread)
    }

    /// The view model of the open thread, if any.
    pub fn active_input(&self, cx: &App) -> Option<TimelineInput> {
        self.active.as_ref().map(|thread| thread.read(cx).input().clone())
    }
}
