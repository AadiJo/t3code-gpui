//! Everything the client holds for one environment: server config, the shell, and the mounted,
//! kept-alive and recently closed threads (`environmentStreams.ts`, `threadRetention.ts`, the
//! web's keep-alive in `web/state/threads.ts`).
//!
//! Memory caps, chosen for a desktop client:
//! - Resume cache: closed threads keep their full state for 5 minutes, at most
//!   [`RESUME_CACHE_LIMIT`] of them; the oldest goes first. Reopening one is instant and resumes
//!   the stream from its cursor.
//! - Keep-alive: up to [`KEEP_ALIVE_LIMIT`] running threads keep their detail stream open, so
//!   switching to a running thread is instant; the most recently started win.
//! - Disk: idle threads only, each under 4 MiB of JSON.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use t3code_contracts::{EnvironmentId, OrchestrationV2ShellThreadStatus as ShellStatus, ServerConfig, ServerConfigStreamEvent, ThreadId, TurnItemId};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::cache_store::CacheStore;
use super::older_settled_threads::{OlderSettledSync, OlderSettledThreads};
use super::shell::{ShellStore, ShellSync, shell_persister};
use super::thread_history_merge::HistoryMeta;
use super::thread_state::{ThreadState, ThreadStatus, has_missing_pending_question_card};
use super::threads::{LoadEarlierResult, ThreadCommand, ThreadSync};
use crate::authorization::RemoteAuthorization;
use crate::connection::{ConnectionPhase, ConnectionStatus, SupervisorHandle};
use crate::rpc::RpcSession;

/// Closed threads are kept in memory this long (`THREAD_SNAPSHOT_IDLE_TTL_MS`).
pub const RESUME_TTL: Duration = Duration::from_secs(5 * 60);
/// At most this many closed threads stay in memory.
pub const RESUME_CACHE_LIMIT: usize = 24;
/// At most this many running threads keep their detail stream open while not shown.
pub const KEEP_ALIVE_LIMIT: usize = 8;

/// Where a synced store stands (`EnvironmentShellStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Empty,
    Cached,
    Synchronizing,
    Live,
}

/// The connection state as stores see it (`connectionProjectionPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionProjection {
    Synchronizing,
    Ready,
    Disconnected,
}

pub fn connection_phase(status: &ConnectionStatus) -> ConnectionProjection {
    match status.phase {
        ConnectionPhase::Connecting => ConnectionProjection::Synchronizing,
        ConnectionPhase::Connected => ConnectionProjection::Ready,
        ConnectionPhase::Available | ConnectionPhase::Offline | ConnectionPhase::Backoff | ConnectionPhase::Blocked => {
            ConnectionProjection::Disconnected
        }
    }
}

/// Resolves once the session differs from `current` (replaced or gone).
pub async fn wait_for_new_session(sessions: &mut watch::Receiver<Option<Arc<RpcSession>>>, current: &Arc<RpcSession>) {
    loop {
        if !sessions.borrow_and_update().as_ref().is_some_and(|session| Arc::ptr_eq(session, current)) {
            return;
        }
        if sessions.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// What an environment's sync tasks share.
pub struct EnvironmentContext {
    pub environment_id: EnvironmentId,
    pub supervisor: SupervisorHandle,
    /// Holds the DPoP key HTTP requests are signed with.
    pub auth: Arc<RemoteAuthorization>,
    pub cache: CacheStore,
    /// The runtime sync tasks run on; stores can be opened from any thread.
    pub runtime: tokio::runtime::Handle,
}

struct LiveThread {
    state: watch::Sender<Arc<ThreadState>>,
    commands: mpsc::UnboundedSender<ThreadCommand>,
    task: tokio::task::AbortHandle,
    mounts: usize,
    keep_alive: bool,
}

struct ResumeEntry {
    state: Arc<ThreadState>,
    closed_at: Instant,
}

#[derive(Default)]
struct ThreadTable {
    live: HashMap<ThreadId, LiveThread>,
    resume: HashMap<ThreadId, ResumeEntry>,
}

impl ThreadTable {
    fn sweep(&mut self, now: Instant) {
        self.resume.retain(|_, entry| now.duration_since(entry.closed_at) < RESUME_TTL);
        while self.resume.len() > RESUME_CACHE_LIMIT {
            let oldest = self.resume.iter().min_by_key(|(_, entry)| entry.closed_at).map(|(id, _)| id.clone());
            match oldest {
                Some(id) => self.resume.remove(&id),
                None => break,
            };
        }
    }
}

struct Inner {
    context: Arc<EnvironmentContext>,
    config: watch::Sender<Option<Arc<ServerConfig>>>,
    config_events: broadcast::Sender<Arc<ServerConfigStreamEvent>>,
    shell: ShellStore,
    older_settled: OlderSettledThreads,
    threads: Mutex<ThreadTable>,
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        for task in self.tasks.get_mut().expect("tasks lock").drain(..) {
            task.abort();
        }
        for (_, live) in self.threads.get_mut().expect("threads lock").live.drain() {
            live.task.abort();
        }
    }
}

/// One environment's state. Cheap to clone; dropping the last clone stops its tasks.
#[derive(Clone)]
pub struct EnvironmentStore {
    inner: Arc<Inner>,
}

impl EnvironmentStore {
    /// Loads the shell and config from the disk cache and starts syncing.
    pub fn open(context: EnvironmentContext) -> Self {
        let context = Arc::new(context);
        let _runtime = context.runtime.enter();
        let shell = ShellStore::load(&context);
        let cached_config = context.cache.load_config(&context.environment_id).map(Arc::new);
        let (older_settled_sync, older_settled) = OlderSettledSync::new(Arc::clone(&context), shell.clone());
        let inner = Arc::new(Inner {
            older_settled,
            config: watch::Sender::new(cached_config),
            config_events: broadcast::channel(64).0,
            shell: shell.clone(),
            threads: Mutex::default(),
            tasks: Mutex::default(),
            context: Arc::clone(&context),
        });
        let sync = ShellSync {
            context: Arc::clone(&context),
            store: shell.clone(),
            persister: shell_persister(context.cache.clone(), context.environment_id.clone()),
        };
        let tasks = vec![
            tokio::spawn(sync.run()).abort_handle(),
            tokio::spawn(older_settled_sync.run()).abort_handle(),
            tokio::spawn(ShellSync::track_connection(shell, context.supervisor.status.clone())).abort_handle(),
            tokio::spawn(follow_config(Arc::downgrade(&inner))).abort_handle(),
            tokio::spawn(keep_running_threads(Arc::downgrade(&inner))).abort_handle(),
        ];
        *inner.tasks.lock().expect("tasks lock") = tasks;
        Self { inner }
    }

    pub fn environment_id(&self) -> &EnvironmentId {
        &self.inner.context.environment_id
    }

    pub fn context(&self) -> &Arc<EnvironmentContext> {
        &self.inner.context
    }

    /// The shell store: rows and projects with optimistic updates applied.
    pub fn shell(&self) -> &ShellStore {
        &self.inner.shell
    }

    /// Older settled pages for the Settled shelf and the families of mounted threads.
    pub fn older_settled(&self) -> &OlderSettledThreads {
        &self.inner.older_settled
    }

    /// The server config of the live session, or the last one seen.
    pub fn config(&self) -> watch::Receiver<Option<Arc<ServerConfig>>> {
        self.inner.config.subscribe()
    }

    /// Config events after each session's initial snapshot (provider statuses, keybindings).
    pub fn config_events(&self) -> broadcast::Receiver<Arc<ServerConfigStreamEvent>> {
        self.inner.config_events.subscribe()
    }

    /// Mounts a thread's detail. The handle's first value is already the best available
    /// projection: the live entry, the 5-minute resume cache, or the disk cache, read
    /// synchronously. Dropping the handle unmounts it.
    pub fn open_thread(&self, thread_id: &ThreadId) -> ThreadHandle {
        let mut table = self.inner.threads.lock().expect("threads lock");
        if let Some(live) = table.live.get_mut(thread_id) {
            live.mounts += 1;
            return self.handle(thread_id, live.state.subscribe(), live.commands.clone());
        }
        table.sweep(Instant::now());
        let resumed = table
            .resume
            .remove(thread_id)
            .map(|entry| entry.state)
            .filter(|state| !state.data.as_deref().is_some_and(has_missing_pending_question_card));
        let (seed, live_seed) = match resumed {
            Some(state) => {
                let seed = state.as_cached();
                let live = seed.status == ThreadStatus::Live;
                (seed, live)
            }
            None => (self.load_cached_thread(thread_id), false),
        };
        let (live, receiver) = self.spawn_thread(thread_id, seed, live_seed, 1, false);
        let commands = live.commands.clone();
        table.live.insert(thread_id.clone(), live);
        self.handle(thread_id, receiver, commands)
    }

    fn load_cached_thread(&self, thread_id: &ThreadId) -> ThreadState {
        let context = &self.inner.context;
        let Some(cached) = context.cache.load_thread(&context.environment_id, thread_id) else { return ThreadState::empty() };
        if has_missing_pending_question_card(&cached.projection) {
            return ThreadState::empty();
        }
        ThreadState {
            data: Some(cached.projection),
            status: ThreadStatus::Cached,
            error: None,
            history: HistoryMeta {
                history_cursor: cached.history_cursor,
                has_more_history: cached.has_more_history,
                latest_local_turn_ordinal: cached.latest_local_turn_ordinal,
                ..HistoryMeta::default()
            },
            sequence: cached.snapshot_sequence,
        }
    }

    fn spawn_thread(
        &self,
        thread_id: &ThreadId,
        seed: ThreadState,
        live_seed: bool,
        mounts: usize,
        keep_alive: bool,
    ) -> (LiveThread, watch::Receiver<Arc<ThreadState>>) {
        let context = &self.inner.context;
        let _runtime = context.runtime.enter();
        let state = watch::Sender::new(Arc::new(seed));
        let receiver = state.subscribe();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let sync = ThreadSync::new(Arc::clone(context), thread_id.clone(), state.clone(), command_rx, commands.clone(), live_seed);
        let task = tokio::spawn(sync.run()).abort_handle();
        (LiveThread { state, commands, task, mounts, keep_alive }, receiver)
    }

    fn handle(&self, thread_id: &ThreadId, state: watch::Receiver<Arc<ThreadState>>, commands: mpsc::UnboundedSender<ThreadCommand>) -> ThreadHandle {
        ThreadHandle { store: Arc::downgrade(&self.inner), thread_id: thread_id.clone(), state, commands }
    }

    /// Threads whose detail is held in memory: mounted or kept alive, then recently closed.
    pub fn held_threads(&self) -> (Vec<ThreadId>, Vec<ThreadId>) {
        let table = self.inner.threads.lock().expect("threads lock");
        (table.live.keys().cloned().collect(), table.resume.keys().cloned().collect())
    }

    /// The state of a mounted, kept-alive or recently closed thread, without mounting it.
    pub fn peek_thread(&self, thread_id: &ThreadId) -> Option<Arc<ThreadState>> {
        let table = self.inner.threads.lock().expect("threads lock");
        table
            .live
            .get(thread_id)
            .map(|live| live.state.borrow().clone())
            .or_else(|| table.resume.get(thread_id).map(|entry| Arc::clone(&entry.state)))
    }

    /// Asks a mounted or kept-alive thread to settle background items the server says ended.
    pub async fn reconcile_pending_work(&self, thread_id: &ThreadId) {
        let commands = {
            let table = self.inner.threads.lock().expect("threads lock");
            table.live.get(thread_id).map(|live| live.commands.clone())
        };
        if let Some(commands) = commands {
            let (reply, done) = oneshot::channel();
            if commands.send(ThreadCommand::ReconcilePendingWork { reply }).is_ok() {
                _ = done.await;
            }
        }
    }

    /// The current session, if connected.
    pub fn session(&self) -> Option<Arc<RpcSession>> {
        self.inner.context.supervisor.session.borrow().clone()
    }
}

impl Inner {
    /// Drops one mount; the last one stops the stream and keeps the state for resuming.
    fn unmount(&self, thread_id: &ThreadId, release_keep_alive: bool) {
        let mut table = self.threads.lock().expect("threads lock");
        let Some(live) = table.live.get_mut(thread_id) else { return };
        if release_keep_alive {
            live.keep_alive = false;
        } else {
            live.mounts = live.mounts.saturating_sub(1);
        }
        if live.mounts > 0 || live.keep_alive {
            return;
        }
        let live = table.live.remove(thread_id).expect("entry present");
        live.task.abort();
        let state = live.state.borrow().clone();
        // A thread whose older history was loaded gives it back: the next open starts from the
        // disk cache's bounded window and the subscription, as a first open does.
        if !state.history.expanded {
            table.resume.insert(thread_id.clone(), ResumeEntry { state, closed_at: Instant::now() });
        }
        table.sweep(Instant::now());
    }
}

/// A mounted thread. Read [`ThreadHandle::state`] for the current value and
/// [`ThreadHandle::receiver`] to learn of changes.
pub struct ThreadHandle {
    store: Weak<Inner>,
    thread_id: ThreadId,
    state: watch::Receiver<Arc<ThreadState>>,
    commands: mpsc::UnboundedSender<ThreadCommand>,
}

impl ThreadHandle {
    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// The current state, without copying the projection.
    pub fn state(&self) -> Arc<ThreadState> {
        self.state.borrow().clone()
    }

    /// A receiver that wakes on each state change.
    pub fn receiver(&self) -> watch::Receiver<Arc<ThreadState>> {
        self.state.clone()
    }

    async fn request<R>(&self, command: impl FnOnce(oneshot::Sender<R>) -> ThreadCommand) -> Option<R> {
        let (reply, result) = oneshot::channel();
        self.commands.send(command(reply)).ok()?;
        result.await.ok()
    }

    /// Loads older history until a page holds a turn start or history ends.
    pub async fn load_earlier(&self) -> LoadEarlierResult {
        self.request(|reply| ThreadCommand::LoadEarlier { through_entry_id: None, work_anchor: None, reply })
            .await
            .unwrap_or(LoadEarlierResult::Noop)
    }

    /// Loads older history through `entry_id` (a search hit or a link target).
    pub async fn load_through(&self, entry_id: String) -> LoadEarlierResult {
        self.request(|reply| ThreadCommand::LoadEarlier { through_entry_id: Some(entry_id), work_anchor: None, reply })
            .await
            .unwrap_or(LoadEarlierResult::Noop)
    }

    /// Loads the hidden rows of the collapsed work group after `anchor`.
    pub async fn expand_work_group(&self, anchor: TurnItemId) -> LoadEarlierResult {
        self.request(|reply| ThreadCommand::LoadEarlier { through_entry_id: None, work_anchor: Some(anchor), reply })
            .await
            .unwrap_or(LoadEarlierResult::Noop)
    }

    /// After a stop: settles background items the server says ended.
    pub async fn reconcile_pending_work(&self) {
        self.request(|reply| ThreadCommand::ReconcilePendingWork { reply }).await;
    }
}

impl Drop for ThreadHandle {
    fn drop(&mut self) {
        if let Some(store) = self.store.upgrade() {
            store.unmount(&self.thread_id, false);
        }
    }
}

/// Publishes each session's config and its later events.
async fn follow_config(store: Weak<Inner>) {
    let Some(mut sessions) = store.upgrade().map(|inner| inner.context.supervisor.session.clone()) else { return };
    loop {
        let session = sessions.borrow_and_update().clone();
        if let Some(session) = session {
            let Some(inner) = store.upgrade() else { return };
            let mut configs = session.config();
            let config = configs.borrow_and_update().clone();
            inner.config.send_replace(Some(Arc::clone(&config)));
            let cache = inner.context.cache.clone();
            let environment_id = inner.context.environment_id.clone();
            tokio::task::spawn_blocking(move || cache.save_config(&environment_id, &config));
            let mut events = session.config_events();
            drop(inner);
            loop {
                tokio::select! {
                    // The session folds snapshots, settings, provider statuses and usage sources in.
                    changed = configs.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let Some(inner) = store.upgrade() else { return };
                        inner.config.send_replace(Some(configs.borrow_and_update().clone()));
                    }
                    event = events.recv() => match event {
                        Ok(event) => {
                            let Some(inner) = store.upgrade() else { return };
                            _ = inner.config_events.send(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    () = wait_for_new_session(&mut sessions, &session) => break,
                }
            }
            continue;
        }
        if sessions.changed().await.is_err() {
            return;
        }
    }
}

fn is_running(status: ShellStatus) -> bool {
    matches!(status, ShellStatus::Preparing | ShellStatus::Queued | ShellStatus::Starting | ShellStatus::Running)
}

/// A kept thread may be released once its own stream is live and shows no running run.
fn is_detail_done(state: &ThreadState) -> bool {
    use t3code_contracts::OrchestrationV2RunStatus as Run;
    if state.status == ThreadStatus::Deleted || state.error.is_some() {
        return true;
    }
    state.status == ThreadStatus::Live
        && !state.data.as_ref().is_some_and(|data| {
            data.runs.iter().any(|run| matches!(run.status, Run::Preparing | Run::Queued | Run::Starting | Run::Running))
        })
}

/// Keeps the detail stream of running threads open (desktop keep-alive).
async fn keep_running_threads(store: Weak<Inner>) {
    let Some(mut shell) = store.upgrade().map(|inner| inner.shell.subscribe()) else { return };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            changed = shell.changed() => if changed.is_err() { return },
            _ = tick.tick() => {}
        }
        let Some(inner) = store.upgrade() else { return };
        let running: Vec<ThreadId> = {
            let view = shell.borrow_and_update().clone();
            let mut rows: Vec<_> = view
                .snapshot
                .iter()
                .flat_map(|snapshot| snapshot.threads.iter())
                .filter(|row| is_running(row.status))
                .collect();
            rows.sort_by(|a, b| b.latest_run_started_at.cmp(&a.latest_run_started_at));
            rows.into_iter().take(KEEP_ALIVE_LIMIT).map(|row| row.id.clone()).collect()
        };
        let running_set: HashSet<&ThreadId> = running.iter().collect();
        let store_handle = EnvironmentStore { inner: Arc::clone(&inner) };
        let mut release = Vec::new();
        {
            let mut table = inner.threads.lock().expect("threads lock");
            for thread_id in &running {
                match table.live.get_mut(thread_id) {
                    Some(live) => live.keep_alive = true,
                    None => {
                        let seed = table
                            .resume
                            .remove(thread_id)
                            .map(|entry| entry.state.as_cached())
                            .unwrap_or_else(|| store_handle.load_cached_thread(thread_id));
                        let (live, _) = store_handle.spawn_thread(thread_id, seed, false, 0, true);
                        table.live.insert(thread_id.clone(), live);
                    }
                }
            }
            for (thread_id, live) in &table.live {
                if live.keep_alive && !running_set.contains(thread_id) && is_detail_done(&live.state.borrow()) {
                    release.push(thread_id.clone());
                }
            }
        }
        for thread_id in release {
            inner.unmount(&thread_id, true);
        }
    }
}
