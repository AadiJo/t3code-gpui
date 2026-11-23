//! One open thread on the GPUI side (`threadSync.ts`): follows a runtime [`ThreadHandle`] and
//! keeps the timeline's view model in step.
//!
//! Updates arrive at most once per frame. While only assistant or reasoning text streams
//! ([`QuietThread`]), the thread emits [`LiveThreadEvent::Text`] with the changed messages so
//! the timeline patches one row; any other change emits [`LiveThreadEvent::Structure`] and the
//! timeline takes the whole input.

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{Context, EventEmitter, SharedString, Task};
use t3code_client_runtime::state::models::now_ms;
use t3code_client_runtime::state::optimistic_messages::{OptimisticMessage, OptimisticMessages};
use t3code_client_runtime::state::thread_detail::QuietThread;
use t3code_client_runtime::state::threads::LoadEarlierResult;
use t3code_client_runtime::state::{ThreadHandle, ThreadState, ThreadStatus};
use t3code_contracts::{MessageId as RuntimeMessageId, TurnItemId};
use t3code_shared::thread_projection::ThreadProjection;

use crate::components::chat::timeline_entries::{MessageId, TimelineInput};
use crate::components::sidebar::model::ThreadKey;
use crate::session_logic::{TimelineMapper, changed_message_text, timeline_input};
use crate::state::follow_watch;

/// What changed in a [`LiveThread`].
#[derive(Clone, Debug)]
pub enum LiveThreadEvent {
    /// The rows changed; read [`LiveThread::input`].
    Structure,
    /// Only these messages' text (or streaming flag) changed.
    Text(Arc<[(MessageId, SharedString, bool)]>),
    /// The sync status changed (cached, live, deleted, error) without new rows.
    Status,
}

/// The mapped view model of a thread, kept while the thread is closed so a revisit does not
/// map again.
pub struct MappedThread {
    pub projection: Option<Arc<ThreadProjection>>,
    pub input: TimelineInput,
    pub mapper: TimelineMapper,
}

/// Counters for measurements.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadUpdateStats {
    pub structure: u64,
    pub text: u64,
    pub status: u64,
    /// Time spent mapping, in microseconds.
    pub map_micros: u64,
}

/// An open thread. Dropping it unmounts the runtime handle (the runtime keeps the state in its
/// resume cache).
pub struct LiveThread {
    key: ThreadKey,
    handle: Arc<ThreadHandle>,
    state: Arc<ThreadState>,
    quiet: QuietThread,
    mapped: MappedThread,
    optimistic: OptimisticMessages,
    stats: ThreadUpdateStats,
    _follow: Task<()>,
}

impl EventEmitter<LiveThreadEvent> for LiveThread {}

impl LiveThread {
    /// Opens `handle`. The first value is mapped now (or taken from `cached` when it still
    /// describes the same projection), so the caller can paint it in this frame.
    pub fn new(key: ThreadKey, handle: ThreadHandle, cached: Option<MappedThread>, cx: &mut Context<Self>) -> Self {
        let state = handle.state();
        let mut mapped = cached.unwrap_or_else(|| MappedThread { projection: None, input: TimelineInput::default(), mapper: TimelineMapper::default() });
        let mut quiet = QuietThread::default();
        let mut stats = ThreadUpdateStats::default();
        let same = match (&mapped.projection, &state.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            let started = Instant::now();
            mapped.input = state.data.as_deref().map(|data| timeline_input(&mut mapped.mapper, data, &[])).unwrap_or_default();
            mapped.projection = state.data.clone();
            stats.map_micros += started.elapsed().as_micros() as u64;
        }
        if let Some(data) = &state.data {
            quiet.update(data);
        }
        let handle = Arc::new(handle);
        let weak = cx.weak_entity();
        let follow = follow_watch(handle.receiver(), cx, move |state, cx| weak.update(cx, |this, cx| this.apply(state, cx)).is_ok());
        Self { key, handle, state, quiet, mapped, optimistic: OptimisticMessages::default(), stats, _follow: follow }
    }

    pub fn key(&self) -> &ThreadKey {
        &self.key
    }

    /// The timeline's view model as of the last update.
    pub fn input(&self) -> &TimelineInput {
        &self.mapped.input
    }

    pub fn state(&self) -> &Arc<ThreadState> {
        &self.state
    }

    pub fn status(&self) -> ThreadStatus {
        self.state.status
    }

    pub fn projection(&self) -> Option<&Arc<ThreadProjection>> {
        self.state.data.as_ref()
    }

    pub fn stats(&self) -> ThreadUpdateStats {
        self.stats
    }

    pub fn handle(&self) -> &Arc<ThreadHandle> {
        &self.handle
    }

    /// Takes the mapped view model, leaving an empty one; for the cache on close.
    pub fn take_mapped(&mut self) -> MappedThread {
        std::mem::replace(
            &mut self.mapped,
            MappedThread { projection: None, input: TimelineInput::default(), mapper: TimelineMapper::default() },
        )
    }

    fn remap(&mut self) {
        let started = Instant::now();
        if let Some(data) = &self.state.data {
            self.mapped.input = timeline_input(&mut self.mapped.mapper, data, self.optimistic.messages());
        } else {
            self.mapped.input = TimelineInput::default();
        }
        self.mapped.projection = self.state.data.clone();
        self.stats.map_micros += started.elapsed().as_micros() as u64;
    }

    fn apply(&mut self, state: Arc<ThreadState>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&state, &self.state) {
            return;
        }
        let previous = std::mem::replace(&mut self.state, state);
        let same_data = match (&previous.data, &self.state.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same_data {
            self.stats.status += 1;
            cx.emit(LiveThreadEvent::Status);
            cx.notify();
            return;
        }
        let thread_id = self.handle.thread_id().clone();
        let reconciled = self.optimistic.reconcile(&thread_id, self.state.data.as_deref());
        let structure_changed = match &self.state.data {
            Some(data) => self.quiet.update(data),
            None => true,
        };
        let before = Arc::clone(&self.mapped.input.entries);
        self.remap();
        if structure_changed || reconciled {
            self.stats.structure += 1;
            cx.emit(LiveThreadEvent::Structure);
        } else {
            let changes = changed_message_text(&before, &self.mapped.input.entries);
            self.stats.text += 1;
            cx.emit(LiveThreadEvent::Text(changes.into()));
        }
        cx.notify();
    }

    /// Shows a sent message at once, until the server's copy arrives.
    pub fn push_optimistic(&mut self, message_id: RuntimeMessageId, text: String, cx: &mut Context<Self>) {
        let thread_id = self.handle.thread_id().clone();
        self.optimistic.push(&thread_id, OptimisticMessage { message_id, text, attachments: Vec::new(), created_at: now_ms() });
        self.remap();
        self.stats.structure += 1;
        cx.emit(LiveThreadEvent::Structure);
        cx.notify();
    }

    /// Drops a local message whose send failed.
    pub fn remove_optimistic(&mut self, message_id: &RuntimeMessageId, cx: &mut Context<Self>) {
        self.optimistic.remove(message_id);
        self.remap();
        cx.emit(LiveThreadEvent::Structure);
        cx.notify();
    }

    /// Pages older history (`loadEarlier`); the new rows arrive as a structure update.
    pub fn load_earlier(&self, cx: &mut Context<Self>) -> Task<LoadEarlierResult> {
        let handle = Arc::clone(&self.handle);
        cx.background_executor().spawn(async move { handle.load_earlier().await })
    }

    /// Loads the hidden rows of a collapsed work group.
    pub fn expand_work_group(&self, anchor: TurnItemId, cx: &mut Context<Self>) -> Task<LoadEarlierResult> {
        let handle = Arc::clone(&self.handle);
        cx.background_executor().spawn(async move { handle.expand_work_group(anchor).await })
    }
}
