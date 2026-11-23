//! One environment's shell: sidebar rows and projects, from the disk cache, an HTTP snapshot and
//! the `orchestration.subscribeShell` stream (`shell.ts`).
//!
//! One task owns every write, so the race guards of the TypeScript hold by construction: items of
//! a closed subscription are dropped with it before the next one starts, an HTTP load and stream
//! items never interleave, and only a socket `synchronized` marker makes the shell live.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use t3code_contracts::rpc::methods::OrchestrationSubscribeShell;
use t3code_contracts::{
    OrchestrationV2ShellStreamItem as Item, OrchestrationV2SubscribeShellInput, OrchestrationV2ThreadShell, ServerConfig,
    ThreadId,
};
use tokio::sync::watch;

use super::cache_persistence::Persister;
use super::cache_store::{CacheStore, ShellOrigin, Ticket};
use super::environment::{EnvironmentContext, SyncStatus, connection_phase, ConnectionProjection, wait_for_new_session};
use super::environment_http::{endpoint_path, get_bytes};
use super::models::now_ms;
use super::shell_reducer::{ShellSnapshot, apply_shell_deltas, merge_snapshot_projects, reuse_unchanged_threads};
use super::thread_lifecycle::PendingUpdates;
use crate::rpc::{RpcError, RpcSession};

/// Rows from a snapshot older than this reload rather than resume: replay does not refresh the
/// older-settled cut or projections the server rebuilt.
const CACHE_RESUME_MAX_AGE_MS: i64 = 12 * 60 * 60 * 1000;
/// How long held rows that can resume wait for the HTTP answer before the socket resumes them.
const EARLY_LOAD_GRACE: Duration = Duration::from_secs(3);
const HTTP_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(20);
const RETRY_BASE: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// Shared by every shell, so a rebuilt one never reuses a generation another used.
static LAST_GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    LAST_GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

/// `250 ms * 2^n`, capped at 30 s (`subscriptionRetryDelay`).
pub fn retry_delay(failures: u32) -> Duration {
    RETRY_BASE.saturating_mul(1u32 << failures.min(10)).min(RETRY_MAX)
}

/// One environment's shell as views read it (`EnvironmentShellState`).
#[derive(Debug, Clone)]
pub struct ShellState {
    pub snapshot: Option<Arc<ShellSnapshot>>,
    pub status: SyncStatus,
    pub error: Option<Arc<str>>,
    /// Changes with each authoritative snapshot; 0 until the first. Family reads made for an
    /// older generation may miss changes and are read again.
    pub generation: u64,
    /// Sequences of thread removals applied since the latest authoritative snapshot.
    pub removed_at: Arc<HashMap<ThreadId, u64>>,
}

impl ShellState {
    fn empty() -> Self {
        Self { snapshot: None, status: SyncStatus::Empty, error: None, generation: 0, removed_at: Arc::default() }
    }

    fn offline_status(&self) -> SyncStatus {
        if self.snapshot.is_some() { SyncStatus::Cached } else { SyncStatus::Empty }
    }

    /// The row of `thread_id`, if the shell has it. A missing row does not mean a deleted thread.
    pub fn thread(&self, thread_id: &ThreadId) -> Option<&Arc<OrchestrationV2ThreadShell>> {
        self.snapshot.as_ref()?.threads.iter().find(|thread| &thread.id == thread_id)
    }
}

/// What a shell snapshot descends from, minus its time.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellIdentity {
    server_version: String,
    database_migration: Option<i64>,
    omit_idle_subagents: bool,
    omit_older_settled: bool,
}

impl ShellIdentity {
    fn of(config: &ServerConfig) -> Self {
        let omit_idle_subagents = config.shell_thread_families == Some(true);
        Self {
            server_version: config.environment.server_version.clone(),
            database_migration: config.environment.database_migration,
            omit_idle_subagents,
            omit_older_settled: config.shell_older_settled == Some(true) && omit_idle_subagents,
        }
    }

    fn origin(&self, snapshot_at: i64) -> ShellOrigin {
        ShellOrigin {
            server_version: self.server_version.clone(),
            database_migration: self.database_migration,
            omit_idle_subagents: self.omit_idle_subagents,
            omit_older_settled: self.omit_older_settled,
            snapshot_at,
        }
    }

    fn matches(&self, origin: &ShellOrigin) -> bool {
        self.server_version == origin.server_version
            && self.database_migration == origin.database_migration
            && self.omit_idle_subagents == origin.omit_idle_subagents
            && self.omit_older_settled == origin.omit_older_settled
    }
}

/// Whether held rows may resume from their cursor (`canResumeFrom`).
fn can_resume(origin: Option<&ShellOrigin>, identity: &ShellIdentity, now: i64) -> bool {
    origin.is_some_and(|origin| {
        let age = now - origin.snapshot_at;
        identity.matches(origin) && (0..CACHE_RESUME_MAX_AGE_MS).contains(&age)
    })
}

struct Inner {
    /// The rows as the server sent them, before optimistic updates.
    source: Arc<ShellState>,
    origin: Option<ShellOrigin>,
    pending: PendingUpdates,
}

/// The shell store of one environment. Views read [`ShellStore::subscribe`]; optimistic updates
/// from commands are applied on top of the server rows.
#[derive(Clone)]
pub struct ShellStore {
    inner: Arc<Mutex<Inner>>,
    view: watch::Sender<Arc<ShellState>>,
}

impl ShellStore {
    /// A store seeded from the disk cache, so rows show before the network answers.
    pub fn load(context: &EnvironmentContext) -> Self {
        let started = std::time::Instant::now();
        let cached = context.cache.load_shell(&context.environment_id);
        let mut state = ShellState::empty();
        let mut origin = None;
        if let Some(cached) = cached {
            tracing::debug!(
                environment_id = %context.environment_id,
                rows = cached.snapshot.threads.len(),
                elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                "loaded the cached shell"
            );
            state.snapshot = Some(Arc::new(cached.snapshot));
            state.status = SyncStatus::Cached;
            origin = cached.origin;
        }
        let state = Arc::new(state);
        Self {
            inner: Arc::new(Mutex::new(Inner { source: Arc::clone(&state), origin, pending: PendingUpdates::default() })),
            view: watch::Sender::new(state),
        }
    }

    /// The current shell with optimistic updates applied, and changes to it.
    pub fn subscribe(&self) -> watch::Receiver<Arc<ShellState>> {
        self.view.subscribe()
    }

    /// The current shell without waiting.
    pub fn current(&self) -> Arc<ShellState> {
        self.view.borrow().clone()
    }

    /// The server rows without optimistic updates.
    pub fn source(&self) -> Arc<ShellState> {
        Arc::clone(&self.inner.lock().expect("shell lock").source)
    }

    fn publish(&self, inner: &mut Inner) {
        inner.pending.retire(inner.source.snapshot.as_ref().map(|snapshot| snapshot.snapshot_sequence));
        let view = inner.pending.apply(&inner.source);
        self.view.send_if_modified(|current| {
            if Arc::ptr_eq(current, &view) {
                return false;
            }
            *current = view;
            true
        });
    }

    fn update(&self, change: impl FnOnce(&mut Inner) -> Option<ShellState>) {
        let mut inner = self.inner.lock().expect("shell lock");
        if let Some(next) = change(&mut inner) {
            inner.source = Arc::new(next);
        }
        self.publish(&mut inner);
    }

    /// Runs `edit` on the optimistic layer and republishes.
    pub(crate) fn edit_pending<R>(&self, edit: impl FnOnce(&mut PendingUpdates) -> R) -> R {
        let mut inner = self.inner.lock().expect("shell lock");
        let result = edit(&mut inner.pending);
        self.publish(&mut inner);
        result
    }

    fn set_status(&self, status: impl FnOnce(&ShellState) -> SyncStatus, clear_error: bool) {
        self.update(|inner| {
            let next = status(&inner.source);
            (next != inner.source.status || (clear_error && inner.source.error.is_some())).then(|| ShellState {
                status: next,
                error: if clear_error { None } else { inner.source.error.clone() },
                ..(*inner.source).clone()
            })
        });
    }

    fn set_error(&self, message: &str) {
        self.update(|inner| {
            Some(ShellState { status: inner.source.offline_status(), error: Some(message.into()), ..(*inner.source).clone() })
        });
    }
}

/// How one subscription ended.
enum Ended {
    SnapshotRequired,
    Failed(String),
}

/// Answers of the HTTP shell route.
enum HttpShell {
    Snapshot { snapshot: ShellSnapshot, project_identities: Option<String> },
    Resumable,
}

/// `OrchestrationV2ShellResumable`: a few bytes saying held rows may resume.
#[derive(serde::Deserialize)]
struct HttpShellResumable {
    resumable: bool,
}

/// `OrchestrationV2ShellHttpSnapshot` with shared rows.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpShellSnapshot {
    schema_version: u64,
    snapshot_sequence: u64,
    threads: t3code_shared::thread_projection::Rows<OrchestrationV2ThreadShell>,
    #[serde(default)]
    archived_threads: t3code_shared::thread_projection::Rows<OrchestrationV2ThreadShell>,
    projects: t3code_shared::thread_projection::Rows<t3code_contracts::OrchestrationProjectShell>,
    #[serde(default)]
    older_settled: Option<t3code_contracts::OrchestrationV2OlderSettledThreads>,
    #[serde(default)]
    project_identities: Option<String>,
}

fn decode_http_shell(bytes: &[u8]) -> Result<HttpShell, serde_json::Error> {
    if bytes.len() < 256
        && let Ok(answer) = serde_json::from_slice::<HttpShellResumable>(bytes)
        && answer.resumable
    {
        return Ok(HttpShell::Resumable);
    }
    let body: HttpShellSnapshot = serde_json::from_slice(bytes)?;
    Ok(HttpShell::Snapshot {
        snapshot: ShellSnapshot {
            schema_version: body.schema_version,
            snapshot_sequence: body.snapshot_sequence,
            threads: body.threads,
            archived_threads: body.archived_threads,
            projects: body.projects,
            older_settled: body.older_settled,
        },
        project_identities: body.project_identities,
    })
}

/// The shell sync loop of one environment. Runs until aborted.
pub(crate) struct ShellSync {
    pub context: Arc<EnvironmentContext>,
    pub store: ShellStore,
    pub persister: Persister<(Arc<ShellSnapshot>, Option<ShellOrigin>, Ticket)>,
}

/// A persister that skips writes when only the cursor moved.
pub(crate) fn shell_persister(cache: CacheStore, environment_id: t3code_contracts::EnvironmentId) -> Persister<(Arc<ShellSnapshot>, Option<ShellOrigin>, Ticket)> {
    let last: Mutex<Option<(Arc<ShellSnapshot>, Option<ShellOrigin>)>> = Mutex::new(None);
    Persister::spawn(move |(snapshot, origin, ticket): (Arc<ShellSnapshot>, Option<ShellOrigin>, Ticket)| {
        let mut last = last.lock().expect("shell persist lock");
        if let Some((previous, previous_origin)) = last.as_ref()
            && previous_origin == &origin
            && snapshot.snapshot_sequence >= previous.snapshot_sequence
            && Arc::ptr_eq(&snapshot.threads, &previous.threads)
            && Arc::ptr_eq(&snapshot.projects, &previous.projects)
        {
            return;
        }
        cache.save_shell(&environment_id, &snapshot, origin.as_ref(), ticket);
        *last = Some((snapshot, origin));
    })
}

impl ShellSync {
    pub async fn run(self) {
        let mut sessions = self.context.supervisor.session.clone();
        let mut failures = 0u32;
        loop {
            let Some(session) = sessions.borrow_and_update().clone() else {
                if sessions.changed().await.is_err() {
                    return;
                }
                continue;
            };
            let ended = tokio::select! {
                ended = self.follow(&session, &mut failures) => Some(ended),
                () = wait_for_new_session(&mut sessions, &session) => None,
            };
            match ended {
                None => failures = 0,
                Some(Ended::SnapshotRequired) => {
                    // Rows stay on screen; the next pass reloads instead of resuming.
                    self.store.inner.lock().expect("shell lock").origin = None;
                }
                Some(Ended::Failed(message)) => {
                    tracing::warn!(environment_id = %self.context.environment_id, %message, "shell subscription failed");
                    self.store.set_error("Could not synchronize environment data.");
                    let delay = retry_delay(failures);
                    failures += 1;
                    tokio::select! {
                        () = tokio::time::sleep(delay) => {}
                        () = wait_for_new_session(&mut sessions, &session) => failures = 0,
                    }
                }
            }
        }
    }

    /// Tracks the connection: rows stay readable while disconnected.
    pub async fn track_connection(store: ShellStore, mut status: watch::Receiver<crate::connection::ConnectionStatus>) {
        loop {
            let phase = connection_phase(&status.borrow_and_update());
            match phase {
                ConnectionProjection::Synchronizing => store.set_status(|_| SyncStatus::Synchronizing, true),
                ConnectionProjection::Ready => store.set_status(
                    |state| if state.status == SyncStatus::Live { SyncStatus::Live } else { SyncStatus::Synchronizing },
                    true,
                ),
                ConnectionProjection::Disconnected => store.set_status(ShellState::offline_status, false),
            }
            if status.changed().await.is_err() {
                return;
            }
        }
    }

    fn persist(&self, snapshot: &Arc<ShellSnapshot>, origin: Option<ShellOrigin>) {
        self.persister.offer((Arc::clone(snapshot), origin, self.context.cache.ticket()));
    }

    async fn load_over_http(&self, session: &RpcSession, identity: &ShellIdentity, resume_from: Option<u64>) -> Option<HttpShell> {
        let mut query = Vec::new();
        if identity.omit_idle_subagents {
            query.push(("omitIdleSubagents", "1".to_owned()));
        }
        if identity.omit_older_settled {
            query.push(("omitOlderSettled", "1".to_owned()));
        }
        if let Some(sequence) = resume_from {
            query.push(("resumeFrom", sequence.to_string()));
        }
        let path = endpoint_path::<t3code_contracts::http::endpoints::OrchestrationShellSnapshot>(&[]);
        let bytes = match get_bytes(&self.context, session, &path, &query, HTTP_SNAPSHOT_TIMEOUT).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(environment_id = %self.context.environment_id, %error, "HTTP shell snapshot failed; using the socket snapshot");
                return None;
            }
        };
        let decoded = tokio::task::spawn_blocking(move || decode_http_shell(&bytes)).await.ok()?;
        match decoded {
            Ok(answer) => Some(answer),
            Err(error) => {
                tracing::warn!(environment_id = %self.context.environment_id, %error, "HTTP shell snapshot did not decode");
                None
            }
        }
    }

    /// Installs an authoritative snapshot. One from HTTP never makes the shell live.
    fn install_snapshot(&self, mut snapshot: ShellSnapshot, identity: &ShellIdentity, from_http: bool) {
        let mut inner = self.store.inner.lock().expect("shell lock");
        let previous = inner.source.snapshot.clone();
        reuse_unchanged_threads(previous.as_deref(), &mut snapshot);
        let snapshot = Arc::new(merge_snapshot_projects(previous.as_deref(), snapshot, None));
        let origin = Some(identity.origin(now_ms()));
        inner.origin = origin.clone();
        let disconnected = matches!(inner.source.status, SyncStatus::Empty | SyncStatus::Cached);
        let status = match (from_http, disconnected) {
            (true, true) => SyncStatus::Cached,
            (true, false) => SyncStatus::Synchronizing,
            (false, _) => inner.source.status,
        };
        inner.source = Arc::new(ShellState {
            snapshot: Some(Arc::clone(&snapshot)),
            status,
            error: None,
            generation: next_generation(),
            removed_at: Arc::default(),
        });
        self.store.publish(&mut inner);
        drop(inner);
        self.persist(&snapshot, origin);
    }

    async fn follow(&self, session: &Arc<RpcSession>, failures: &mut u32) -> Ended {
        let identity = ShellIdentity::of(&session.initial_config);
        let (held_sequence, resumable_rows) = {
            let inner = self.store.inner.lock().expect("shell lock");
            let sequence = inner.source.snapshot.as_ref().map(|snapshot| snapshot.snapshot_sequence);
            (sequence, sequence.is_some() && can_resume(inner.origin.as_ref(), &identity, now_ms()))
        };
        self.store.set_status(|_| SyncStatus::Synchronizing, true);

        // Held rows that can resume ask HTTP whether they still can, for a few bytes, and give
        // it a short grace; anything else loads the HTTP snapshot so a stale cursor cannot stand.
        let answer = if resumable_rows {
            tokio::time::timeout(EARLY_LOAD_GRACE, self.load_over_http(session, &identity, held_sequence)).await.ok().flatten()
        } else {
            self.load_over_http(session, &identity, None).await
        };
        let mut known_project_identities = None;
        let loaded_snapshot = match answer {
            Some(HttpShell::Snapshot { snapshot, project_identities }) => {
                self.install_snapshot(snapshot, &identity, true);
                known_project_identities = project_identities;
                true
            }
            _ => false,
        };
        let resuming_held = !loaded_snapshot && resumable_rows;
        let cursor = self.store.source().snapshot.as_ref().map(|snapshot| snapshot.snapshot_sequence);
        let omit = |flag: bool| flag.then_some(true);
        let mut input = OrchestrationV2SubscribeShellInput {
            request_completion_marker: Some(true),
            omit_idle_subagents: omit(identity.omit_idle_subagents),
            omit_older_settled: omit(identity.omit_older_settled),
            ..Default::default()
        };
        if let Some(cursor) = cursor.filter(|_| resuming_held || loaded_snapshot) {
            input.after_sequence = Some(cursor);
            // Only rows the client already held may be refused; a refusal of rows just loaded
            // would loop.
            input.http_snapshot_fallback = resuming_held.then_some(t3code_contracts::wire::True);
            input.known_project_identities = known_project_identities;
        }

        let mut stream = session.client.subscribe::<OrchestrationSubscribeShell>(input);
        let mut awaiting_completion = true;
        while let Some(chunk) = stream.next().await {
            let items = match chunk {
                Ok(items) => items,
                Err(RpcError::Closed(reason)) => return Ended::Failed(reason.to_string()),
                Err(error) => return Ended::Failed(error.to_string()),
            };
            *failures = 0;
            if self.apply(items, &identity, &mut awaiting_completion) {
                return Ended::SnapshotRequired;
            }
        }
        Ended::Failed("the shell stream ended".to_owned())
    }

    /// Applies one delivered chunk in a single store update. Returns whether the server asked
    /// for a fresh snapshot.
    fn apply(&self, items: Vec<Item>, identity: &ShellIdentity, awaiting_completion: &mut bool) -> bool {
        let mut snapshot_required = false;
        let mut authoritative = false;
        let mut inner = self.store.inner.lock().expect("shell lock");
        let initial = Arc::clone(&inner.source);
        let mut next = (*initial).clone();
        let mut deltas: Vec<Item> = Vec::new();
        let flush = |next: &mut ShellState, deltas: &mut Vec<Item>, waiting: bool| {
            let Some(snapshot) = &next.snapshot else {
                deltas.clear();
                return;
            };
            let mut removed: Option<HashMap<ThreadId, u64>> = None;
            let mut sequence = snapshot.snapshot_sequence;
            for delta in deltas.iter() {
                let delta_sequence = match delta {
                    Item::ThreadRemoved(removal) if removal.sequence > sequence => {
                        removed.get_or_insert_with(|| (*next.removed_at).clone()).insert(removal.thread_id.clone(), removal.sequence);
                        removal.sequence
                    }
                    Item::ThreadUpdated(delta) => delta.sequence,
                    Item::ProjectUpdated(delta) => delta.sequence,
                    Item::ProjectRemoved(delta) => delta.sequence,
                    _ => continue,
                };
                sequence = sequence.max(delta_sequence);
            }
            if let Some(applied) = apply_shell_deltas(snapshot, deltas) {
                next.snapshot = Some(Arc::new(applied));
            }
            if let Some(removed) = removed {
                next.removed_at = Arc::new(removed);
            }
            next.status = if waiting { SyncStatus::Synchronizing } else { SyncStatus::Live };
            next.error = None;
            deltas.clear();
        };
        for item in items {
            match item {
                Item::SnapshotRequired(_) => snapshot_required = true,
                Item::Synchronized(_) => {
                    flush(&mut next, &mut deltas, *awaiting_completion);
                    *awaiting_completion = false;
                    if next.snapshot.is_some() {
                        next.status = SyncStatus::Live;
                        next.error = None;
                        if next.generation == 0 {
                            // A cache resume that caught up is as current as a snapshot.
                            next.generation = next_generation();
                        }
                    }
                }
                Item::Snapshot(item) => {
                    flush(&mut next, &mut deltas, *awaiting_completion);
                    let is_authoritative = item.resolved_repository_identity_roots.is_none();
                    let mut snapshot = ShellSnapshot::from(item.snapshot);
                    if is_authoritative {
                        reuse_unchanged_threads(next.snapshot.as_deref(), &mut snapshot);
                    }
                    let merged = merge_snapshot_projects(
                        next.snapshot.as_deref(),
                        snapshot,
                        item.resolved_repository_identity_roots.as_deref(),
                    );
                    authoritative |= is_authoritative;
                    next = ShellState {
                        snapshot: Some(Arc::new(merged)),
                        status: if *awaiting_completion { SyncStatus::Synchronizing } else { SyncStatus::Live },
                        error: None,
                        generation: if is_authoritative { next_generation() } else { next.generation },
                        removed_at: if is_authoritative { Arc::default() } else { next.removed_at },
                    };
                }
                Item::Unknown(_) => {}
                delta => deltas.push(delta),
            }
        }
        flush(&mut next, &mut deltas, *awaiting_completion);
        if snapshot_required {
            inner.origin = None;
        } else if authoritative {
            inner.origin = Some(identity.origin(now_ms()));
        }
        let changed_rows = match (&next.snapshot, &initial.snapshot) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (a, b) => a.is_some() != b.is_some(),
        };
        let changed = changed_rows || next.status != initial.status || next.generation != initial.generation || next.error != initial.error;
        if changed {
            inner.source = Arc::new(next);
            self.store.publish(&mut inner);
        }
        let to_persist = changed_rows.then(|| (inner.source.snapshot.clone(), inner.origin.clone()));
        drop(inner);
        if let Some((Some(snapshot), origin)) = to_persist {
            self.persist(&snapshot, origin);
        }
        snapshot_required
    }
}
