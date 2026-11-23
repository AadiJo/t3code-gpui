//! The on-disk cache of server state, in the app's data directory: per environment, the shell
//! rows and the idle threads' projections (`orchestrationCache.ts`, `shellCacheEncoding.ts`,
//! `threadCacheEncoding.ts`, the web's IndexedDB stores).
//!
//! Format: JSON, because the generated contract types decode only from JSON (their
//! forward-compatible unions dispatch on raw JSON text); a binary format would need a second
//! hand-written codec for hundreds of types. The shell is JSON lines, a header then one thread row
//! per line, so loading splits the rows across cores and 5,000 rows decode in a few milliseconds.
//! A record whose schema version differs, or that does not decode, is deleted: there is no
//! migration, a format change means a cold load.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use t3code_contracts::{
    EnvironmentId, OrchestrationProjectShell, OrchestrationV2OlderSettledThreads, OrchestrationV2ThreadShell, ServerConfig,
    ThreadId,
};
use t3code_shared::orchestration_v2_bounded_snapshot::{omit_local_visible_turn_items, restore_local_visible_turn_items};
use t3code_shared::thread_projection::ThreadProjection;

use super::models::EpochMs;
use super::shell_reducer::ShellSnapshot;

const SHELL_SCHEMA_VERSION: u32 = 1;
const THREAD_SCHEMA_VERSION: u32 = 1;
const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Projections whose JSON is larger than this are not cached, and an older copy is deleted.
pub const THREAD_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// What a cached shell descends from. Held rows may resume from their cursor only while the
/// server's build, migration and omission flags match and the snapshot is under 12 hours old.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOrigin {
    pub server_version: String,
    pub database_migration: Option<i64>,
    pub omit_idle_subagents: bool,
    pub omit_older_settled: bool,
    pub snapshot_at: EpochMs,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShellHeader {
    schema_version: u32,
    environment_id: EnvironmentId,
    origin: Option<ShellOrigin>,
    snapshot_schema_version: u64,
    snapshot_sequence: u64,
    projects: Vec<Arc<OrchestrationProjectShell>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    older_settled: Option<OrchestrationV2OlderSettledThreads>,
}

/// A shell read back from disk.
pub struct CachedShell {
    pub snapshot: ShellSnapshot,
    pub origin: Option<ShellOrigin>,
}

/// An idle thread's projection as saved (`OrchestrationV2ThreadDetailSnapshot`).
#[derive(Debug, Clone)]
pub struct CachedThread {
    pub snapshot_sequence: u64,
    pub projection: Arc<ThreadProjection>,
    pub history_cursor: Option<String>,
    pub has_more_history: bool,
    pub latest_local_turn_ordinal: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadRecord {
    schema_version: u32,
    environment_id: EnvironmentId,
    thread_id: ThreadId,
    snapshot_sequence: u64,
    /// Local timeline items are left out of `turnItems` and restored from the timeline.
    turn_items_omit_local_visible: bool,
    history_cursor: Option<String>,
    has_more_history: bool,
    latest_local_turn_ordinal: Option<u64>,
    projection: ThreadProjection,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigRecord {
    schema_version: u32,
    config: ServerConfig,
}

/// Write-order tickets: a save or removal enqueued later always wins, so a late save cannot
/// overwrite a newer save or a removal.
#[derive(Default)]
struct Tickets {
    next: u64,
    written: HashMap<PathBuf, u64>,
}

/// The cache directory of every environment. Cheap to clone.
#[derive(Clone)]
pub struct CacheStore {
    root: PathBuf,
    tickets: Arc<Mutex<Tickets>>,
}

/// A place in the write order, taken when a save or removal is decided.
#[derive(Debug, Clone, Copy)]
pub struct Ticket(u64);

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest[..10].iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_data()?;
    drop(file);
    std::fs::rename(temp, path)
}

fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), %error, "could not remove a cache file");
    }
}

impl CacheStore {
    /// The cache under `data_dir`.
    pub fn new(data_dir: &Path) -> Self {
        Self { root: data_dir.join("state-cache").join("v1"), tickets: Default::default() }
    }

    fn environment_dir(&self, environment_id: &EnvironmentId) -> PathBuf {
        self.root.join(short_hash(environment_id.as_str()))
    }

    fn shell_path(&self, environment_id: &EnvironmentId) -> PathBuf {
        self.environment_dir(environment_id).join("shell.jsonl")
    }

    fn thread_path(&self, environment_id: &EnvironmentId, thread_id: &ThreadId) -> PathBuf {
        self.environment_dir(environment_id).join("threads").join(format!("{}.json", short_hash(thread_id.as_str())))
    }

    fn config_path(&self, environment_id: &EnvironmentId) -> PathBuf {
        self.environment_dir(environment_id).join("server-config.json")
    }

    /// Reserves the next place in the write order.
    pub fn ticket(&self) -> Ticket {
        let mut tickets = self.tickets.lock().expect("cache tickets");
        tickets.next += 1;
        Ticket(tickets.next)
    }

    /// Runs `write` unless a later ticket already wrote `path`. The lock is held across the write
    /// so two writers of one file cannot interleave.
    fn ordered(&self, path: &Path, ticket: Ticket, write: impl FnOnce()) {
        let mut tickets = self.tickets.lock().expect("cache tickets");
        if tickets.written.get(path).is_some_and(|&written| written > ticket.0) {
            return;
        }
        tickets.written.insert(path.to_owned(), ticket.0);
        write();
    }

    /// Loads the cached shell, decoding rows on every core.
    pub fn load_shell(&self, environment_id: &EnvironmentId) -> Option<CachedShell> {
        let path = self.shell_path(environment_id);
        let text = std::fs::read_to_string(&path).ok()?;
        match decode_shell(&text, environment_id) {
            Some(shell) => Some(shell),
            None => {
                tracing::warn!(%environment_id, "discarding a shell cache that did not decode");
                remove_quietly(&path);
                None
            }
        }
    }

    /// Saves the shell rows and their origin.
    pub fn save_shell(&self, environment_id: &EnvironmentId, snapshot: &ShellSnapshot, origin: Option<&ShellOrigin>, ticket: Ticket) {
        let path = self.shell_path(environment_id);
        let header = ShellHeader {
            schema_version: SHELL_SCHEMA_VERSION,
            environment_id: environment_id.clone(),
            origin: origin.cloned(),
            snapshot_schema_version: snapshot.schema_version,
            snapshot_sequence: snapshot.snapshot_sequence,
            projects: snapshot.projects.to_vec(),
            older_settled: snapshot.older_settled.clone(),
        };
        let mut bytes = serde_json::to_vec(&header).expect("shell header encodes");
        for thread in snapshot.threads.iter() {
            bytes.push(b'\n');
            serde_json::to_writer(&mut bytes, &**thread).expect("shell row encodes");
        }
        self.ordered(&path, ticket, || {
            if let Err(error) = write_atomic(&path, &bytes) {
                tracing::warn!(%environment_id, %error, "could not save the shell cache");
            }
        });
    }

    /// Loads an idle thread's cached projection.
    pub fn load_thread(&self, environment_id: &EnvironmentId, thread_id: &ThreadId) -> Option<CachedThread> {
        let path = self.thread_path(environment_id, thread_id);
        let bytes = std::fs::read(&path).ok()?;
        let record = serde_json::from_slice::<ThreadRecord>(&bytes)
            .ok()
            .filter(|record| record.schema_version == THREAD_SCHEMA_VERSION && &record.thread_id == thread_id);
        let Some(mut record) = record else {
            tracing::warn!(%environment_id, %thread_id, "discarding a thread cache that did not decode");
            remove_quietly(&path);
            return None;
        };
        if record.turn_items_omit_local_visible {
            restore_local_visible_turn_items(&mut record.projection);
        } else {
            record.projection.share_local_items();
        }
        Some(CachedThread {
            snapshot_sequence: record.snapshot_sequence,
            projection: Arc::new(record.projection),
            history_cursor: record.history_cursor,
            has_more_history: record.has_more_history,
            latest_local_turn_ordinal: record.latest_local_turn_ordinal,
        })
    }

    /// Saves an idle thread. A projection over [`THREAD_CACHE_MAX_BYTES`] deletes the cached
    /// copy instead.
    pub fn save_thread(&self, environment_id: &EnvironmentId, thread: &CachedThread, ticket: Ticket) {
        let path = self.thread_path(environment_id, &thread.projection.thread.id);
        let compact = omit_local_visible_turn_items(&thread.projection);
        let record = ThreadRecord {
            schema_version: THREAD_SCHEMA_VERSION,
            environment_id: environment_id.clone(),
            thread_id: thread.projection.thread.id.clone(),
            snapshot_sequence: thread.snapshot_sequence,
            turn_items_omit_local_visible: compact.is_some(),
            history_cursor: thread.history_cursor.clone(),
            has_more_history: thread.has_more_history,
            latest_local_turn_ordinal: thread.latest_local_turn_ordinal,
            projection: compact.unwrap_or_else(|| (*thread.projection).clone()),
        };
        let bytes = serde_json::to_vec(&record).expect("thread cache encodes");
        self.ordered(&path, ticket, || {
            if bytes.len() > THREAD_CACHE_MAX_BYTES {
                remove_quietly(&path);
            } else if let Err(error) = write_atomic(&path, &bytes) {
                tracing::warn!(%environment_id, %error, "could not save a thread cache");
            }
        });
    }

    /// Deletes a thread's cached projection.
    pub fn remove_thread(&self, environment_id: &EnvironmentId, thread_id: &ThreadId, ticket: Ticket) {
        let path = self.thread_path(environment_id, thread_id);
        self.ordered(&path, ticket, || remove_quietly(&path));
    }

    /// The last server config, so a restart can show settings before the socket answers.
    pub fn load_config(&self, environment_id: &EnvironmentId) -> Option<ServerConfig> {
        let bytes = std::fs::read(self.config_path(environment_id)).ok()?;
        serde_json::from_slice::<ConfigRecord>(&bytes)
            .ok()
            .filter(|record| record.schema_version == CONFIG_SCHEMA_VERSION)
            .map(|record| record.config)
    }

    pub fn save_config(&self, environment_id: &EnvironmentId, config: &ServerConfig) {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Borrowed<'a> {
            schema_version: u32,
            config: &'a ServerConfig,
        }
        let bytes = serde_json::to_vec(&Borrowed { schema_version: CONFIG_SCHEMA_VERSION, config }).expect("config encodes");
        if let Err(error) = write_atomic(&self.config_path(environment_id), &bytes) {
            tracing::warn!(%environment_id, %error, "could not save the server config cache");
        }
    }

    /// Deletes everything cached for an environment (on removal, not on disconnect).
    pub fn remove_environment(&self, environment_id: &EnvironmentId) {
        let dir = self.environment_dir(environment_id);
        if let Err(error) = std::fs::remove_dir_all(&dir)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%environment_id, %error, "could not clear the environment cache");
        }
    }
}

/// Decodes a shell file: the header on the first line, then rows split across threads.
pub fn decode_shell(text: &str, environment_id: &EnvironmentId) -> Option<CachedShell> {
    let (header, body) = text.split_once('\n').unwrap_or((text, ""));
    let header: ShellHeader = serde_json::from_str(header).ok()?;
    if header.schema_version != SHELL_SCHEMA_VERSION || &header.environment_id != environment_id {
        return None;
    }
    let lines: Vec<&str> = body.lines().filter(|line| !line.is_empty()).collect();
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let chunk = lines.len().div_ceil(workers).max(256);
    let decoded: Option<Vec<Vec<Arc<OrchestrationV2ThreadShell>>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = lines
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|line| serde_json::from_str::<OrchestrationV2ThreadShell>(line).ok().map(Arc::new))
                        .collect::<Option<Vec<_>>>()
                })
            })
            .collect();
        handles.into_iter().map(|handle| handle.join().ok().flatten()).collect()
    });
    let threads: Vec<_> = decoded?.into_iter().flatten().collect();
    Some(CachedShell {
        snapshot: ShellSnapshot {
            schema_version: header.snapshot_schema_version,
            snapshot_sequence: header.snapshot_sequence,
            threads: Arc::new(threads),
            archived_threads: Arc::default(),
            projects: Arc::new(header.projects),
            older_settled: header.older_settled,
        },
        origin: header.origin,
    })
}
