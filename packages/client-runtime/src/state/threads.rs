//! The detail stream of one mounted thread (`threads.ts`): seed from a cache, load a bounded
//! snapshot, follow `orchestration.subscribeThread`, page older history, and save idle projections.
//!
//! One task owns every write to the thread's state, so stream batches, history merges and stop
//! reconciliation never overwrite each other (the TypeScript's apply lock).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use t3code_contracts::rpc::methods::{OrchestrationGetTurnItem, OrchestrationSubscribeThread};
use t3code_contracts::{
    OrchestrationV2DomainEvent, OrchestrationV2GetTurnItemInput, OrchestrationV2RunStatus,
    OrchestrationV2SubscribeThreadInput, OrchestrationV2TurnItem, ThreadId, TurnItemId,
};
use t3code_shared::orchestration_v2_pending_background_work::pending_background_turn_items;
use t3code_shared::thread_projection::ThreadProjection;
use t3code_shared::turn_item::{TurnItemExt, starts_turn};
use tokio::sync::{mpsc, oneshot, watch};

use super::cache_persistence::Persister;
use super::cache_store::{CachedThread, Ticket};
use super::environment::{ConnectionProjection, EnvironmentContext, connection_phase};
use super::environment_http::{endpoint_path, get_bytes, is_not_found};
use super::orchestration_v2_projection::{PartialWindow, apply_projection_event, is_thread_deleted};
use super::raw_stream::RawStream;
use super::shell::retry_delay;
use super::thread_history_merge::{HistoryMeta, WorkPage, merge_older_history, merge_older_subagents};
use super::thread_state::{
    HistoryPage, ThreadSnapshot, ThreadState, ThreadStatus, ThreadStreamItem, decode_bounded_snapshot, decode_history_page,
    decode_stream_item,
};
use crate::rpc::RpcSession;

const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(6);
const HISTORY_TIMEOUT: Duration = Duration::from_secs(6);
const TURN_ITEM_TIMEOUT: Duration = Duration::from_secs(10);
const RECONCILE_CONCURRENCY: usize = 4;

/// How a "load earlier" request ended (`ThreadHistoryLoadEarlierResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadEarlierResult {
    Loaded,
    Noop,
    Busy,
    Error(String),
}

/// Requests a mounted thread's task serves.
pub(crate) enum ThreadCommand {
    /// Load older history. `work_anchor` expands that collapsed work group instead; with
    /// `through_entry_id` pages continue until that entry is loaded.
    LoadEarlier {
        through_entry_id: Option<String>,
        work_anchor: Option<TurnItemId>,
        reply: oneshot::Sender<LoadEarlierResult>,
    },
    /// After a stop: ask the server about locally active background items and settle those it
    /// says ended.
    ReconcilePendingWork { reply: oneshot::Sender<()> },
    /// A fetched history page, merged by the task if its cursor is still current.
    Page { request: PageRequest, page: HistoryPage, keep_paging: bool, reply: oneshot::Sender<bool> },
    /// A history load failed; recorded on the history meta if its cursor is still current.
    PageFailed { cursor: String, message: String },
    /// Server copies of items reconciliation asked about.
    Settled { items: HashMap<TurnItemId, OrchestrationV2TurnItem>, reply: oneshot::Sender<()> },
}

#[derive(Clone)]
pub(crate) struct PageRequest {
    cursor: String,
    work_anchor: Option<TurnItemId>,
    /// Whether this page completes a conversation load (not the first half of a `through` pair).
    complete: bool,
}

/// The sync task of one thread.
pub(crate) struct ThreadSync {
    pub context: Arc<EnvironmentContext>,
    pub thread_id: ThreadId,
    pub state: watch::Sender<Arc<ThreadState>>,
    pub commands: mpsc::UnboundedReceiver<ThreadCommand>,
    pub command_sender: mpsc::UnboundedSender<ThreadCommand>,
    pub persister: Persister<(CachedThread, Ticket)>,
    awaiting_completion: bool,
    socket_open_tried: bool,
    http_misses: u32,
    failures: u32,
    /// Live status kept from a warm resume until the first subscription replaces it.
    resuming_live: bool,
}

/// Should a projection be saved: never while a run streams or after older pages were merged.
pub fn should_persist(projection: &ThreadProjection, history: &HistoryMeta) -> bool {
    !history.expanded
        && !projection.runs.iter().any(|run| {
            matches!(run.status, OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting | OrchestrationV2RunStatus::Running)
        })
}

enum Followed {
    SessionChanged,
    Failed(String),
    /// Deleted by an event; stay parked until the session changes.
    Parked,
    Retry,
}

impl ThreadSync {
    pub fn new(
        context: Arc<EnvironmentContext>,
        thread_id: ThreadId,
        state: watch::Sender<Arc<ThreadState>>,
        commands: mpsc::UnboundedReceiver<ThreadCommand>,
        command_sender: mpsc::UnboundedSender<ThreadCommand>,
        seed_live: bool,
    ) -> Self {
        let cache = context.cache.clone();
        let environment_id = context.environment_id.clone();
        let persister = Persister::spawn(move |(thread, ticket): (CachedThread, Ticket)| {
            cache.save_thread(&environment_id, &thread, ticket);
        });
        Self {
            context,
            thread_id,
            state,
            commands,
            command_sender,
            persister,
            awaiting_completion: false,
            socket_open_tried: false,
            http_misses: 0,
            failures: 0,
            resuming_live: seed_live,
        }
    }

    fn current(&self) -> Arc<ThreadState> {
        self.state.borrow().clone()
    }

    fn set(&self, next: ThreadState) {
        self.state.send_replace(Arc::new(next));
    }

    fn update(&self, change: impl FnOnce(&ThreadState) -> Option<ThreadState>) {
        let current = self.current();
        if let Some(next) = change(&current) {
            self.set(next);
        }
    }

    fn offer_persist(&self, projection: &Arc<ThreadProjection>, history: &HistoryMeta, sequence: u64) {
        if !should_persist(projection, history) {
            return;
        }
        let thread = CachedThread {
            snapshot_sequence: sequence,
            projection: Arc::clone(projection),
            history_cursor: history.history_cursor.clone(),
            has_more_history: history.has_more_history,
            latest_local_turn_ordinal: history.latest_local_turn_ordinal,
        };
        self.persister.offer((thread, self.context.cache.ticket()));
    }

    /// Installs a snapshot with its paging meta in one update, so a bounded window never saves
    /// as a complete one.
    fn set_snapshot(&self, snapshot: ThreadSnapshot) {
        let projection = Arc::new(snapshot.projection);
        let history = snapshot.history.unwrap_or_default();
        let current = self.current();
        let status = if current.error.is_some() {
            ThreadStatus::Cached
        } else if self.awaiting_completion {
            ThreadStatus::Synchronizing
        } else {
            ThreadStatus::Live
        };
        self.offer_persist(&projection, &history, snapshot.snapshot_sequence);
        self.set(ThreadState {
            data: Some(projection),
            status,
            error: current.error.clone(),
            history,
            sequence: snapshot.snapshot_sequence,
        });
    }

    fn set_deleted(&mut self) {
        self.awaiting_completion = false;
        let sequence = self.current().sequence;
        self.set(ThreadState { status: ThreadStatus::Deleted, sequence, ..ThreadState::empty() });
        self.persister.cancel();
        self.context.cache.remove_thread(&self.context.environment_id, &self.thread_id, self.context.cache.ticket());
    }

    fn set_stream_error(&mut self, message: &str) {
        self.awaiting_completion = false;
        self.update(|current| Some(ThreadState { status: current.offline_status(), error: Some(message.into()), ..current.clone() }));
    }

    fn mark_synchronizing(&self) {
        if self.resuming_live {
            return;
        }
        self.update(|current| {
            (current.status != ThreadStatus::Deleted).then(|| ThreadState { status: ThreadStatus::Synchronizing, error: None, ..current.clone() })
        });
    }

    fn on_connection(&self, phase: ConnectionProjection) {
        self.update(|current| match phase {
            ConnectionProjection::Synchronizing | ConnectionProjection::Ready
                if current.status == ThreadStatus::Deleted || current.error.is_some() || current.status == ThreadStatus::Live =>
            {
                None
            }
            ConnectionProjection::Synchronizing | ConnectionProjection::Ready => {
                Some(ThreadState { status: ThreadStatus::Synchronizing, error: None, ..current.clone() })
            }
            ConnectionProjection::Disconnected => {
                let status = current.offline_status();
                (status != current.status).then(|| ThreadState { status, ..current.clone() })
            }
        });
    }

    /// Applies one delivered chunk: consecutive events in one state update, so a batch that
    /// changes nothing keeps the state object.
    fn apply_items(&mut self, items: Vec<ThreadStreamItem>) {
        let mut events: Vec<(u64, Option<OrchestrationV2DomainEvent>)> = Vec::new();
        for item in items {
            match item {
                ThreadStreamItem::Event { sequence, event } if !event.as_ref().is_some_and(is_thread_deleted) => {
                    events.push((sequence, event));
                }
                ThreadStreamItem::Event { sequence, event: Some(_) } => {
                    self.apply_events(std::mem::take(&mut events));
                    if sequence > self.current().sequence {
                        self.set_deleted();
                    }
                }
                ThreadStreamItem::Event { .. } => unreachable!("covered by the first arm"),
                ThreadStreamItem::Snapshot(snapshot) => {
                    self.apply_events(std::mem::take(&mut events));
                    self.set_snapshot(*snapshot);
                }
                ThreadStreamItem::Synchronized => {
                    self.apply_events(std::mem::take(&mut events));
                    self.awaiting_completion = false;
                    self.update(|current| {
                        (current.data.is_some() && current.status != ThreadStatus::Deleted && current.error.is_none() && current.status != ThreadStatus::Live)
                            .then(|| ThreadState { status: ThreadStatus::Live, ..current.clone() })
                    });
                }
                ThreadStreamItem::Ignored => {}
            }
        }
        self.apply_events(events);
    }

    fn apply_events(&self, events: Vec<(u64, Option<OrchestrationV2DomainEvent>)>) {
        let current = self.current();
        let mut sequence = current.sequence;
        let fresh: Vec<_> = events
            .into_iter()
            .filter(|(item_sequence, _)| {
                let fresh = *item_sequence > sequence;
                if fresh {
                    sequence = *item_sequence;
                }
                fresh
            })
            .filter_map(|(_, event)| event)
            .collect();
        if sequence == current.sequence {
            return;
        }
        let (Some(data), false) = (&current.data, current.status == ThreadStatus::Deleted) else {
            // No projection to apply to; only the cursor moves.
            self.set(ThreadState { sequence, ..(*current).clone() });
            return;
        };
        let mut projection = (**data).clone();
        let mut history = current.history.clone();
        let mut changed = false;
        for event in &fresh {
            let partial = history.is_partial().then_some(PartialWindow { latest_local_turn_ordinal: history.latest_local_turn_ordinal });
            if apply_projection_event(&mut projection, event, partial) {
                changed = true;
                if partial.is_some()
                    && let OrchestrationV2DomainEvent::TurnItemUpdated(update) = event
                {
                    let ordinal = update.payload.item_ordinal();
                    if history.latest_local_turn_ordinal.is_none_or(|watermark| ordinal > watermark) {
                        history.latest_local_turn_ordinal = Some(ordinal);
                    }
                }
            }
        }
        if !changed {
            self.set(ThreadState { sequence, ..(*current).clone() });
            return;
        }
        let projection = Arc::new(projection);
        self.offer_persist(&projection, &history, sequence);
        self.set(ThreadState {
            data: Some(projection),
            status: if self.awaiting_completion { ThreadStatus::Synchronizing } else { ThreadStatus::Live },
            error: None,
            history,
            sequence,
        });
    }

    /// Opens the thread over HTTP when there is nothing to resume from.
    async fn load_bounded(&mut self, session: &RpcSession) -> Result<(), Followed> {
        let path = endpoint_path::<t3code_contracts::http::endpoints::OrchestrationThreadBoundedSnapshot>(&[(
            "threadId",
            self.thread_id.as_str(),
        )]);
        let query = [
            ("compactTurnItems", "1".to_owned()),
            ("dedupeBodies", "1".to_owned()),
            ("compactIds", "1".to_owned()),
            ("deferWork", "1".to_owned()),
        ];
        match get_bytes(&self.context, session, &path, &query, SNAPSHOT_TIMEOUT).await {
            Ok(bytes) => {
                let decoded = tokio::task::spawn_blocking(move || {
                    std::str::from_utf8(&bytes).map_err(|error| error.to_string()).and_then(|text| decode_bounded_snapshot(text).map_err(|error| error.to_string()))
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|decoded| decoded);
                match decoded {
                    Ok(snapshot) => {
                        self.http_misses = 0;
                        self.set_snapshot(snapshot);
                    }
                    Err(error) => tracing::warn!(thread_id = %self.thread_id, %error, "bounded snapshot did not decode; using the socket"),
                }
                Ok(())
            }
            Err(error) if is_not_found(&error) => {
                // A 404 also means "not created yet", so keep asking while mounted.
                if self.current().status != ThreadStatus::Deleted {
                    self.set_deleted();
                }
                let delay = retry_delay(self.http_misses);
                self.http_misses += 1;
                tokio::time::sleep(delay).await;
                Err(Followed::Retry)
            }
            Err(error) => {
                tracing::warn!(thread_id = %self.thread_id, %error, "bounded snapshot unavailable; using the socket");
                Ok(())
            }
        }
    }

    async fn follow(&mut self, session: &Arc<RpcSession>, sessions: &mut watch::Receiver<Option<Arc<RpcSession>>>) -> Followed {
        let current = self.current();
        if current.status == ThreadStatus::Deleted && self.http_misses == 0 && current.data.is_none() && self.socket_open_tried {
            return Followed::Parked;
        }
        self.awaiting_completion = true;
        self.mark_synchronizing();
        self.resuming_live = false;

        // A server that pages snapshots sends the same bounded window over the socket, so the
        // first cold open skips HTTP; a later cold attempt asks HTTP, which tells a missing
        // thread apart from a failure.
        let pagination = session.initial_config.thread_snapshot_pagination == Some(true);
        let socket_only = current.data.is_none() && pagination && !self.socket_open_tried;
        self.socket_open_tried = true;
        if current.data.is_none() && !socket_only {
            let load = self.load_bounded(session);
            let outcome = tokio::select! {
                outcome = load => Some(outcome),
                () = super::environment::wait_for_new_session(sessions, session) => None,
            };
            match outcome {
                None => return Followed::SessionChanged,
                Some(Err(followed)) => return followed,
                Some(Ok(())) => {}
            }
        }

        let current = self.current();
        let input = OrchestrationV2SubscribeThreadInput {
            thread_id: self.thread_id.clone(),
            after_sequence: current.data.as_ref().map(|_| current.sequence),
            request_completion_marker: Some(true),
            accept_bounded_snapshot: Some(true),
            accept_deferred_work: Some(true),
            accept_compact_turn_items: Some(true),
            accept_compact_ids: Some(true),
            accept_deduped_bodies: Some(true),
        };
        let mut stream = session.client.subscribe::<RawStream<OrchestrationSubscribeThread>>(input);
        let mut status = self.context.supervisor.status.clone();
        loop {
            tokio::select! {
                chunk = stream.next() => match chunk {
                    Some(Ok(values)) => {
                        self.failures = 0;
                        let items: Vec<_> = values.iter().map(|value| decode_stream_item(value)).collect();
                        self.apply_items(items);
                    }
                    Some(Err(error)) => {
                        if socket_only && self.current().data.is_none() {
                            // Retry over HTTP before showing an error.
                            return Followed::Retry;
                        }
                        return Followed::Failed(error.to_string());
                    }
                    None => return Followed::Failed("the thread stream ended".to_owned()),
                },
                command = self.commands.recv() => match command {
                    Some(command) => self.handle(command, Some(session)),
                    None => return Followed::Parked,
                },
                () = super::environment::wait_for_new_session(sessions, session) => return Followed::SessionChanged,
                changed = status.changed() => {
                    if changed.is_ok() {
                        let phase = connection_phase(&status.borrow_and_update());
                        self.on_connection(phase);
                    }
                }
            }
        }
    }

    /// Runs until the entry is unmounted (the task is aborted, or its command channel closes).
    pub async fn run(mut self) {
        let mut sessions = self.context.supervisor.session.clone();
        loop {
            let session = sessions.borrow_and_update().clone();
            let Some(session) = session else {
                self.on_connection(connection_phase(&self.context.supervisor.status.borrow()));
                tokio::select! {
                    changed = sessions.changed() => if changed.is_err() { return },
                    command = self.commands.recv() => match command {
                        Some(command) => self.handle(command, None),
                        None => return,
                    },
                }
                continue;
            };
            match self.follow(&session, &mut sessions).await {
                Followed::SessionChanged | Followed::Retry => {}
                Followed::Parked => {
                    tokio::select! {
                        () = super::environment::wait_for_new_session(&mut sessions, &session) => {}
                        command = self.commands.recv() => match command {
                            Some(command) => self.handle(command, Some(&session)),
                            None => return,
                        },
                    }
                }
                Followed::Failed(message) => {
                    tracing::warn!(thread_id = %self.thread_id, %message, "thread subscription failed");
                    self.set_stream_error("Could not synchronize the thread.");
                    let delay = retry_delay(self.failures);
                    self.failures += 1;
                    let sleep = tokio::time::sleep(delay);
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            () = &mut sleep => break,
                            () = super::environment::wait_for_new_session(&mut sessions, &session) => break,
                            command = self.commands.recv() => match command {
                                Some(command) => self.handle(command, Some(&session)),
                                None => return,
                            },
                        }
                    }
                    // A new attempt clears the error once it synchronizes.
                    self.update(|current| Some(ThreadState { error: None, ..current.clone() }));
                }
            }
        }
    }

    fn handle(&mut self, command: ThreadCommand, session: Option<&Arc<RpcSession>>) {
        match command {
            ThreadCommand::LoadEarlier { through_entry_id, work_anchor, reply } => {
                self.start_load_earlier(through_entry_id, work_anchor, session, reply);
            }
            ThreadCommand::Page { request, page, keep_paging, reply } => {
                _ = reply.send(self.merge_page(&request, page, keep_paging));
            }
            ThreadCommand::PageFailed { cursor, message } => self.fail_load(&cursor, message),
            ThreadCommand::ReconcilePendingWork { reply } => self.start_reconcile(session, reply),
            ThreadCommand::Settled { items, reply } => {
                self.settle(items);
                _ = reply.send(());
            }
        }
    }

    fn work_cursor(state: &ThreadState, anchor: &TurnItemId) -> Option<String> {
        state.data.as_ref()?.visible_turn_items.iter().find(|row| &row.source_item_id == anchor)?.item.deferred_work().map(|work| work.cursor.to_owned())
    }

    fn start_load_earlier(
        &mut self,
        through_entry_id: Option<String>,
        work_anchor: Option<TurnItemId>,
        session: Option<&Arc<RpcSession>>,
        reply: oneshot::Sender<LoadEarlierResult>,
    ) {
        let current = self.current();
        let cursor = match &work_anchor {
            None => current.history.history_cursor.clone(),
            Some(anchor) => Self::work_cursor(&current, anchor),
        };
        let (Some(cursor), false, Some(_)) = (cursor, current.status == ThreadStatus::Deleted, &current.data) else {
            _ = reply.send(LoadEarlierResult::Noop);
            return;
        };
        if work_anchor.is_none() && !current.history.has_more_history {
            _ = reply.send(LoadEarlierResult::Noop);
            return;
        }
        if work_anchor.is_none() && current.history.loading {
            _ = reply.send(LoadEarlierResult::Busy);
            return;
        }
        let Some(session) = session.cloned() else {
            _ = reply.send(LoadEarlierResult::Error("Environment is not connected.".to_owned()));
            return;
        };
        if work_anchor.is_none() {
            self.set(ThreadState { history: HistoryMeta { loading: true, error: None, ..current.history.clone() }, ..(*current).clone() });
        }
        let context = Arc::clone(&self.context);
        let thread_id = self.thread_id.clone();
        let commands = self.command_sender.clone();
        tokio::spawn(async move {
            let result = page_history(&context, &session, &thread_id, cursor, through_entry_id, work_anchor, &commands).await;
            _ = reply.send(result);
        });
    }

    /// Merges a page if its cursor is still the active one. Returns whether to continue.
    fn merge_page(&mut self, request: &PageRequest, page: HistoryPage, keep_paging: bool) -> bool {
        let current = self.current();
        let is_current = match &request.work_anchor {
            None => current.history.history_cursor.as_deref() == Some(request.cursor.as_str()),
            Some(anchor) => Self::work_cursor(&current, anchor).as_deref() == Some(request.cursor.as_str()),
        };
        if !is_current {
            return false;
        }
        let Some(data) = current.data.as_ref().filter(|_| current.status != ThreadStatus::Deleted) else {
            self.set(ThreadState { history: HistoryMeta::default(), ..(*current).clone() });
            return false;
        };
        let mut projection = (**data).clone();
        let work = request.work_anchor.as_ref().map(|anchor| WorkPage { anchor_id: anchor, next_cursor: page.next_cursor.clone() });
        let next_cursor = page.next_cursor.clone();
        let has_more = page.has_more_history;
        merge_older_history(&mut projection, page.items, work);
        merge_older_subagents(&mut projection, page.subagents);
        let history = if request.work_anchor.is_none() && request.complete {
            HistoryMeta { loading: keep_paging, ..current.history.after_page(next_cursor, has_more) }
        } else {
            HistoryMeta { expanded: true, ..current.history.clone() }
        };
        self.set(ThreadState { data: Some(Arc::new(projection)), history, ..(*current).clone() });
        true
    }

    fn fail_load(&self, cursor: &str, message: String) {
        self.update(|current| {
            (current.history.history_cursor.as_deref() == Some(cursor)).then(|| ThreadState {
                history: HistoryMeta { loading: false, error: Some(message), ..current.history.clone() },
                ..current.clone()
            })
        });
    }

    fn start_reconcile(&self, session: Option<&Arc<RpcSession>>, reply: oneshot::Sender<()>) {
        let current = self.current();
        let (Some(session), Some(data)) = (session.cloned(), current.data.clone()) else {
            _ = reply.send(());
            return;
        };
        let pending: Vec<TurnItemId> =
            pending_background_turn_items(&data.turn_items, &data.runs).iter().map(|item| item.item_id()).collect();
        if pending.is_empty() {
            _ = reply.send(());
            return;
        }
        let thread_id = self.thread_id.clone();
        let commands = self.command_sender.clone();
        tokio::spawn(async move {
            let fetched: Vec<Option<OrchestrationV2TurnItem>> = futures::stream::iter(pending)
                .map(|item_id| {
                    let session = Arc::clone(&session);
                    let thread_id = thread_id.clone();
                    async move {
                        let call = session.client.call::<OrchestrationGetTurnItem>(OrchestrationV2GetTurnItemInput { thread_id, item_id, revision: None });
                        tokio::time::timeout(TURN_ITEM_TIMEOUT, call).await.ok()?.ok()?.item
                    }
                })
                .buffer_unordered(RECONCILE_CONCURRENCY)
                .collect()
                .await;
            let items = fetched.into_iter().flatten().filter(|item| !item.is_work_active()).map(|item| (item.item_id(), item)).collect();
            let (done, wait) = oneshot::channel();
            if commands.send(ThreadCommand::Settled { items, reply: done }).is_ok() {
                _ = wait.await;
            }
            _ = reply.send(());
        });
    }

    /// Replaces still-active local copies with the server's terminal items (`settleTurnItems`).
    /// Events applied meanwhile win, and inherited rows belong to another thread.
    fn settle(&self, settled: HashMap<TurnItemId, OrchestrationV2TurnItem>) {
        let current = self.current();
        let Some(data) = current.data.as_ref().filter(|_| !settled.is_empty()) else { return };
        let mut changed = false;
        let mut replace = |item: &Arc<OrchestrationV2TurnItem>| -> Arc<OrchestrationV2TurnItem> {
            match settled.get(&item.item_id()) {
                Some(server) if item.is_work_active() => {
                    changed = true;
                    Arc::new(server.clone())
                }
                _ => Arc::clone(item),
            }
        };
        let turn_items: Vec<_> = data.turn_items.iter().map(&mut replace).collect();
        let by_id: HashMap<TurnItemId, Arc<OrchestrationV2TurnItem>> = turn_items.iter().map(|item| (item.item_id(), Arc::clone(item))).collect();
        let visible: Vec<_> = data
            .visible_turn_items
            .iter()
            .map(|row| {
                let mut row = row.clone();
                if row.source_thread_id == self.thread_id
                    && row.item.is_work_active()
                    && let Some(item) = by_id.get(&row.source_item_id).filter(|item| !item.is_work_active())
                {
                    row.item = Arc::clone(item);
                }
                row
            })
            .collect();
        if !changed {
            return;
        }
        let mut projection = (**data).clone();
        projection.turn_items = Arc::new(turn_items);
        projection.visible_turn_items = Arc::new(visible);
        let projection = Arc::new(projection);
        self.offer_persist(&projection, &current.history, current.sequence);
        self.set(ThreadState { data: Some(projection), ..(*current).clone() });
    }
}

/// Fetches history pages until one holds a turn-starting row or history ends, handing each to
/// the thread task to merge. A cursor that does not advance is an error.
async fn page_history(
    context: &EnvironmentContext,
    session: &RpcSession,
    thread_id: &ThreadId,
    mut cursor: String,
    through_entry_id: Option<String>,
    work_anchor: Option<TurnItemId>,
    commands: &mpsc::UnboundedSender<ThreadCommand>,
) -> LoadEarlierResult {
    let path = endpoint_path::<t3code_contracts::http::endpoints::OrchestrationThreadHistoryPage>(&[("threadId", thread_id.as_str())]);
    loop {
        let fail = |message: String| {
            tracing::warn!(%thread_id, %message, "loading earlier history failed");
            if work_anchor.is_none() {
                _ = commands.send(ThreadCommand::PageFailed { cursor: cursor.clone(), message: message.clone() });
            }
            LoadEarlierResult::Error(message)
        };
        let views: &[Option<&str>] = if work_anchor.is_some() {
            &[Some("work")]
        } else if through_entry_id.is_some() {
            &[Some("conversation"), None]
        } else {
            &[None]
        };
        let mut next_cursor = None;
        for view in views {
            let mut query = vec![("cursor", cursor.clone()), ("compactIds", "1".to_owned())];
            let through = work_anchor.as_ref().map(|anchor| anchor.as_str().to_owned()).or_else(|| through_entry_id.clone());
            if through.is_none() || work_anchor.is_some() {
                query.push(("deferWork", "1".to_owned()));
            }
            if let Some(view) = view {
                query.push(("view", (*view).to_owned()));
            }
            if let Some(through) = through {
                query.push(("throughEntryId", through));
            }
            let bytes = match get_bytes(context, session, &path, &query, HISTORY_TIMEOUT).await {
                Ok(bytes) => bytes,
                Err(error) => return fail(error.to_string()),
            };
            let page = match std::str::from_utf8(&bytes).map_err(|error| error.to_string()).and_then(|text| decode_history_page(text).map_err(|error| error.to_string())) {
                Ok(page) => page,
                Err(error) => return fail(error),
            };
            let complete = *view != Some("conversation");
            let keep_paging = complete
                && match &work_anchor {
                    Some(_) => page.next_cursor.is_some(),
                    None => through_entry_id.is_none() && page.has_more_history && !page.items.iter().any(|row| starts_turn(&row.item)),
                };
            if keep_paging && page.next_cursor.as_deref().is_none_or(|next| next == cursor) {
                let message = if work_anchor.is_none() { "History did not advance." } else { "Work history did not advance." };
                return fail(message.to_owned());
            }
            let page_next = page.next_cursor.clone();
            let request = PageRequest { cursor: cursor.clone(), work_anchor: work_anchor.clone(), complete };
            let (reply, merged) = oneshot::channel();
            if commands.send(ThreadCommand::Page { request, page, keep_paging, reply }).is_err() {
                return LoadEarlierResult::Noop;
            }
            if !merged.await.unwrap_or(false) {
                return if work_anchor.is_some() { LoadEarlierResult::Error("Work history changed. Retry loading it.".to_owned()) } else { LoadEarlierResult::Noop };
            }
            next_cursor = if keep_paging { page_next } else { None };
        }
        match next_cursor {
            None => return LoadEarlierResult::Loaded,
            Some(next) => cursor = next,
        }
    }
}

impl Drop for ThreadSync {
    fn drop(&mut self) {
        self.persister.flush();
    }
}
