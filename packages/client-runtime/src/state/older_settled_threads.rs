//! Older settled threads and thread families of one environment (`olderSettledThreads.ts`,
//! `threadFamilies.ts`).
//!
//! Shells that omit older settled threads name the cut (`olderSettled.before`) and count, per
//! project, what they left out. [`OlderSettledSync`] keeps those counts current, reads older
//! threads in pages of [`OLDER_SETTLED_PAGE_SIZE`] below the cut for the Settled shelf, and reads
//! the family of each mounted thread (an open thread the shell leaves out). Reads are keyed by the
//! shell's generation: a new authoritative snapshot (a reconnect) reads every requested page and
//! family again, and nothing is read for generation 0 (a cached shell). Read rows merge into the
//! shell through [`ShellStore::mount_rows`]; none are persisted.
//!
//! The pure helpers ([`settled_shelf_frontier`], [`is_listed_settled`], [`wanted_pages`]) combine
//! environments for a shelf that spans several.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::FuturesUnordered;
use futures::{Future, StreamExt};
use t3code_contracts::rpc::methods::{OrchestrationGetOlderSettledThreads, OrchestrationGetThreadFamily};
use t3code_contracts::{
    EnvironmentId, OrchestrationV2GetOlderSettledThreadsInput, OrchestrationV2GetThreadFamilyInput,
    OrchestrationV2OlderSettledCount, OrchestrationV2OlderSettledCursor, OrchestrationV2ThreadShell,
    OrchestrationV2ThreadShellSettledOverride as Settled, ProjectId, ThreadId,
};
use tokio::sync::watch;

use super::environment::EnvironmentContext;
use super::models::{EpochMs, parse_ms};
use super::shell::{ShellState, ShellStore};
use super::thread_families::FamilyRows;
use crate::rpc::RpcSession;

/// Rows per page of older settled threads.
pub const OLDER_SETTLED_PAGE_SIZE: u64 = 25;
/// A burst of changes (a PR sync touching many old threads) costs one count read.
const COUNT_REREAD_DELAY: Duration = Duration::from_millis(500);
const PAGES_KEY: &str = "older-settled";

/// Where an environment's shell cut settled threads, and how many are older per project.
#[derive(Debug, Clone, PartialEq)]
pub struct OlderSettled {
    pub before: String,
    pub before_ms: EpochMs,
    pub counts: Arc<HashMap<ProjectId, u64>>,
}

/// A position in the server's order of older settled threads (settle time desc, id asc).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SettledCursor {
    pub settled_at: EpochMs,
    /// The server's own string: it compares cursors as strings.
    pub settled_at_iso: String,
    pub thread_id: Option<ThreadId>,
}

impl SettledCursor {
    fn wire(&self) -> OrchestrationV2OlderSettledCursor {
        OrchestrationV2OlderSettledCursor { settled_at: self.settled_at_iso.clone(), thread_id: self.thread_id.clone() }
    }
}

/// Whether the pages a shelf asked for have loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PagesStatus {
    #[default]
    Ready,
    Pending,
    Failed,
}

/// The pages read below an environment's cut (`OlderSettledPages`). Everything at or above
/// `frontier` is local. `None` means no more can load: every older thread is loaded, or this
/// session has no snapshot yet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OlderSettledPages {
    pub loaded: usize,
    pub rows: usize,
    pub frontier: Option<SettledCursor>,
    pub status: PagesStatus,
}

/// Which older settled pages to read: the shelf's projects in this environment (`None` for
/// all) and how many pages.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShelfRequest {
    pub project_ids: Option<Vec<ProjectId>>,
    pub pages: usize,
}

/// Whether a family read can decide anything yet (`ThreadFamilyRead`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FamilyStatus {
    Pending,
    Failed,
    Settled,
}

/// What one environment's older settled sync shows (`OlderSettledShelfEnvironment`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OlderSettledView {
    /// The cut and current counts, or `None` when the shell keeps every settled thread.
    pub older: Option<OlderSettled>,
    pub pages: OlderSettledPages,
    /// The request `pages` answers.
    pub request: ShelfRequest,
    /// Each mounted thread's family read.
    pub families: BTreeMap<ThreadId, FamilyStatus>,
}

/// A settled top-level thread settled before `before_ms` (`isOlderSettledThreadShell`).
pub fn is_older_settled(thread: &OrchestrationV2ThreadShell, before_ms: EpochMs) -> bool {
    thread.lineage.is_listed()
        && thread.settled_override == Some(Settled::Settled)
        && thread.settled_at.as_deref().and_then(parse_ms).is_some_and(|at| at < before_ms)
}

/// Changes whenever the older count can (`olderSettledCountKey`): any removal, an older row that
/// arrives, changes or moves project, or an old row arriving outside the set. Hashed per row and
/// summed, so the order of rows does not matter.
fn count_key(threads: &[Arc<OrchestrationV2ThreadShell>], before_ms: EpochMs, removed_at: &HashMap<ThreadId, u64>) -> (u64, u64) {
    let last_removal = removed_at.values().copied().max().unwrap_or(0);
    let mut sum = 0u64;
    for thread in threads {
        if !thread.lineage.is_listed() {
            continue;
        }
        let older = is_older_settled(thread, before_ms);
        if older || parse_ms(&thread.created_at).is_some_and(|at| at < before_ms) {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (&thread.id, &thread.project_id, older).hash(&mut hasher);
            sum = sum.wrapping_add(hasher.finish());
        }
    }
    (last_removal, sum)
}

/// Whether a row sits at or above a cursor in the server's order.
fn at_or_above(settled_at: Option<EpochMs>, thread_id: &str, cursor: &SettledCursor) -> bool {
    let Some(settled_at) = settled_at else { return true };
    !(settled_at < cursor.settled_at
        || (cursor.thread_id.as_ref().is_some_and(|id| settled_at == cursor.settled_at && thread_id > id.as_str())))
}

/// Where a shelf over these environments stops listing (`settledShelfFrontier`): the highest
/// frontier of any environment that can still load older threads. A failed one holds no one back.
pub fn settled_shelf_frontier<'a>(environments: impl IntoIterator<Item = &'a OlderSettledView>) -> Option<SettledCursor> {
    let mut highest: Option<&SettledCursor> = None;
    for view in environments {
        if view.older.is_none() || view.pages.status == PagesStatus::Failed {
            continue;
        }
        let Some(frontier) = &view.pages.frontier else { continue };
        let higher = match highest {
            None => true,
            Some(highest) => {
                frontier.settled_at > highest.settled_at
                    || (frontier.settled_at == highest.settled_at
                        && frontier.thread_id.is_some()
                        && highest.thread_id.as_ref().is_none_or(|id| frontier.thread_id.as_ref().is_some_and(|f| f < id)))
            }
        };
        if higher {
            highest = Some(frontier);
        }
    }
    highest.cloned()
}

/// Whether the Settled shelf may list a row: at or above the shelf's frontier
/// (`isListedSettledThread`).
pub fn is_listed_settled(settled_at: Option<EpochMs>, thread_id: &str, frontier: Option<&SettledCursor>) -> bool {
    frontier.is_none_or(|frontier| at_or_above(settled_at, thread_id, frontier))
}

/// The pages each environment should read (`wantedOlderSettledPages`): while the shelf lists
/// fewer than `visible_count` rows, one more for each environment whose frontier is the shelf's,
/// once its pages have loaded. Returns `None` when nothing changes.
pub fn wanted_pages(
    requested: &HashMap<EnvironmentId, usize>,
    environments: &[(EnvironmentId, &OlderSettledView)],
    listed_count: usize,
    visible_count: usize,
) -> Option<HashMap<EnvironmentId, usize>> {
    if listed_count >= visible_count {
        return None;
    }
    let frontier = settled_shelf_frontier(environments.iter().map(|(_, view)| *view))?;
    let mut next: Option<HashMap<EnvironmentId, usize>> = None;
    for (environment_id, view) in environments {
        if view.older.is_none() || view.pages.status != PagesStatus::Ready || view.pages.frontier.as_ref() != Some(&frontier) {
            continue;
        }
        let pages = requested.get(environment_id).copied().unwrap_or(0);
        next.get_or_insert_with(|| requested.clone()).insert(environment_id.clone(), pages + 1);
    }
    next
}

/// What the client asks of one environment's sync.
#[derive(Default)]
struct Requests {
    shelf: ShelfRequest,
    /// Bumped to read failed pages again.
    retry: u64,
    /// Mounted threads and how many mounts each has.
    families: BTreeMap<ThreadId, usize>,
    /// Bumped when a family is mounted again, so a failed read retries.
    family_retry: u64,
}

/// The client side of one environment's [`OlderSettledSync`]. Cheap to clone.
#[derive(Clone)]
pub struct OlderSettledThreads {
    requests: watch::Sender<Requests>,
    view: watch::Receiver<Arc<OlderSettledView>>,
}

/// Keeps a thread's family merged into the shell until dropped.
pub struct FamilyMount {
    requests: watch::Sender<Requests>,
    thread_id: ThreadId,
}

impl Drop for FamilyMount {
    fn drop(&mut self) {
        self.requests.send_modify(|requests| {
            if let Some(count) = requests.families.get_mut(&self.thread_id) {
                *count -= 1;
                if *count == 0 {
                    requests.families.remove(&self.thread_id);
                }
            }
        });
    }
}

impl OlderSettledThreads {
    /// The cut, counts, pages and family reads, and changes to them.
    pub fn subscribe(&self) -> watch::Receiver<Arc<OlderSettledView>> {
        self.view.clone()
    }

    /// Asks for `request`'s pages (0 pages keeps none, as a collapsed shelf does).
    pub fn set_shelf(&self, request: ShelfRequest) {
        self.requests.send_if_modified(|requests| {
            let changed = requests.shelf != request;
            requests.shelf = request;
            changed
        });
    }

    /// Reads a failed page again; the pages after it follow (`retryPages`).
    pub fn retry(&self) {
        self.requests.send_modify(|requests| requests.retry += 1);
    }

    /// Merges `thread_id`'s family into the shell until the mount drops (`mount`). Mounting
    /// again after a failure retries.
    pub fn mount_family(&self, thread_id: ThreadId) -> FamilyMount {
        self.requests.send_modify(|requests| {
            *requests.families.entry(thread_id.clone()).or_default() += 1;
            requests.family_retry += 1;
        });
        FamilyMount { requests: self.requests.clone(), thread_id }
    }
}

type PageKey = (u64, Option<Vec<ProjectId>>, SettledCursor);
type Read<T> = Option<Result<T, ()>>;
type ReadFuture = Pin<Box<dyn Future<Output = Done> + Send>>;

enum Done {
    Count { key: (u64, u64), generation: u64, result: Result<Arc<HashMap<ProjectId, u64>>, ()> },
    Page { key: PageKey, result: Result<Arc<FamilyRows>, ()>, last: Option<SettledCursor>, full: bool },
    Family { key: (u64, ThreadId), result: Result<Arc<FamilyRows>, ()> },
}

/// A read page: its rows (for the shell) and where the next one starts.
#[derive(Clone)]
struct Page {
    rows: Arc<FamilyRows>,
    next: Option<SettledCursor>,
}

/// The sync task of one environment. See the module docs.
pub(crate) struct OlderSettledSync {
    context: Arc<EnvironmentContext>,
    shell: ShellStore,
    requests: watch::Receiver<Requests>,
    view: watch::Sender<Arc<OlderSettledView>>,
    generation: u64,
    from_snapshot: Option<OlderSettled>,
    key: (u64, u64),
    key_inputs: Option<(usize, usize)>,
    snapshot_key: Option<(u64, u64)>,
    last_read: Option<Arc<HashMap<ProjectId, u64>>>,
    count_wanted: Option<((u64, u64), u64)>,
    count_in_flight: Option<(u64, u64)>,
    pages: HashMap<PageKey, Read<Page>>,
    families: HashMap<(u64, ThreadId), Read<Arc<FamilyRows>>>,
    mounted_families: HashSet<ThreadId>,
    retry_seen: u64,
    family_retry_seen: u64,
}

impl OlderSettledSync {
    /// The task and its client handle.
    pub(crate) fn new(context: Arc<EnvironmentContext>, shell: ShellStore) -> (Self, OlderSettledThreads) {
        let requests = watch::Sender::new(Requests::default());
        let view = watch::Sender::new(Arc::new(OlderSettledView::default()));
        let handle = OlderSettledThreads { requests: requests.clone(), view: view.subscribe() };
        let sync = Self {
            context,
            shell,
            requests: requests.subscribe(),
            view,
            generation: u64::MAX,
            from_snapshot: None,
            key: (0, 0),
            key_inputs: None,
            snapshot_key: None,
            last_read: None,
            count_wanted: None,
            count_in_flight: None,
            pages: HashMap::new(),
            families: HashMap::new(),
            mounted_families: HashSet::new(),
            retry_seen: 0,
            family_retry_seen: 0,
        };
        // The handle keeps the sender alive; the task holds a receiver.
        drop(requests);
        (sync, handle)
    }

    pub(crate) async fn run(mut self) {
        let mut shell = self.shell.subscribe_source();
        let mut in_flight: FuturesUnordered<ReadFuture> = FuturesUnordered::new();
        let mut count_timer: Option<Pin<Box<tokio::time::Sleep>>> = None;
        loop {
            let state = shell.borrow_and_update().clone();
            let reads = self.step(&state, &mut count_timer);
            in_flight.extend(reads);
            tokio::select! {
                changed = shell.changed() => if changed.is_err() { return },
                changed = self.requests.changed() => if changed.is_err() { return },
                Some(done) = in_flight.next() => self.finish(done),
                () = async { count_timer.as_mut().expect("timer").await }, if count_timer.is_some() => {
                    count_timer = None;
                    if let Some((key, generation)) = self.count_wanted.take() {
                        self.count_in_flight = Some(key);
                        in_flight.push(Box::pin(read_counts(self.context.clone(), self.from_snapshot.as_ref().map(|o| o.before.clone()).unwrap_or_default(), key, generation)));
                    }
                }
            }
        }
    }

    fn finish(&mut self, done: Done) {
        match done {
            Done::Count { key, generation, result } => {
                if self.count_in_flight == Some(key) {
                    self.count_in_flight = None;
                }
                if generation == self.generation && key == self.key && let Ok(counts) = result {
                    self.last_read = Some(counts);
                }
            }
            Done::Page { key, result, last, full } => {
                if let Some(slot) = self.pages.get_mut(&key) {
                    *slot = Some(result.map(|rows| Page { rows, next: if full { last } else { None } }));
                }
            }
            Done::Family { key, result } => {
                if let Some(slot) = self.families.get_mut(&key) {
                    *slot = Some(result);
                }
            }
        }
    }

    /// Brings the view and mounts up to date; returns reads to start.
    fn step(&mut self, state: &Arc<ShellState>, count_timer: &mut Option<Pin<Box<tokio::time::Sleep>>>) -> Vec<ReadFuture> {
        let mut reads: Vec<ReadFuture> = Vec::new();
        let requests = self.requests.borrow_and_update();
        let shelf = requests.shelf.clone();
        let retry = requests.retry;
        let family_retry = requests.family_retry;
        let wanted_families: Vec<ThreadId> = requests.families.keys().cloned().collect();
        drop(requests);

        let snapshot = state.snapshot.as_ref();
        let cut = snapshot.and_then(|snapshot| snapshot.older_settled.as_ref());
        let generation = state.generation;

        // Counts (`olderSettledAtom`).
        let older = match (snapshot, cut) {
            (Some(snapshot), Some(cut)) => {
                let before_ms = parse_ms(&cut.before).unwrap_or(0);
                if self.from_snapshot.as_ref().is_none_or(|o| o.before != cut.before) || generation != self.generation {
                    self.from_snapshot = Some(OlderSettled { before: cut.before.clone(), before_ms, counts: Arc::new(counts_of(&cut.counts)) });
                }
                let inputs = (Arc::as_ptr(&snapshot.threads) as usize, Arc::as_ptr(&state.removed_at) as usize);
                if self.key_inputs != Some(inputs) {
                    self.key_inputs = Some(inputs);
                    self.key = count_key(&snapshot.threads, before_ms, &state.removed_at);
                }
                if generation != self.generation {
                    self.snapshot_key = (snapshot.snapshot_sequence == cut.as_of_sequence).then_some(self.key);
                    self.last_read = None;
                }
                let from_snapshot = self.from_snapshot.clone().expect("set above");
                if Some(self.key) == self.snapshot_key || generation == 0 {
                    self.count_wanted = None;
                    *count_timer = None;
                    Some(from_snapshot)
                } else {
                    // A newer key restarts the wait; one already read keeps its counts.
                    if self.count_wanted.map(|(key, _)| key) != Some(self.key) && self.count_in_flight != Some(self.key) {
                        self.count_wanted = Some((self.key, generation));
                        *count_timer = Some(Box::pin(tokio::time::sleep(COUNT_REREAD_DELAY)));
                    }
                    Some(match &self.last_read {
                        Some(counts) => OlderSettled { counts: Arc::clone(counts), ..from_snapshot },
                        None => from_snapshot,
                    })
                }
            }
            _ => {
                self.from_snapshot = None;
                self.count_wanted = None;
                *count_timer = None;
                None
            }
        };
        if generation != self.generation {
            self.generation = generation;
            self.pages.retain(|key, _| key.0 == generation);
            self.families.retain(|key, _| key.0 == generation);
        }

        // Pages (`pagesAtom`).
        if retry != self.retry_seen {
            self.retry_seen = retry;
            self.pages.retain(|_, read| !matches!(read, Some(Err(()))));
        }
        let scope = shelf.project_ids.clone().map(|mut ids| {
            ids.sort();
            ids
        });
        if shelf.pages == 0 {
            // A collapsed shelf keeps no read; expanding reads its pages again.
            self.pages.clear();
        } else {
            self.pages.retain(|key, _| key.1 == scope);
        }
        let mut page_rows: Vec<Arc<FamilyRows>> = Vec::new();
        let pages = match (cut, older.as_ref()) {
            (Some(_), Some(older)) if generation != 0 => {
                let mut frontier = Some(SettledCursor { settled_at: older.before_ms, settled_at_iso: older.before.clone(), thread_id: None });
                let mut status = PagesStatus::Ready;
                for _ in 0..shelf.pages {
                    let Some(cursor) = frontier.clone() else { break };
                    let key: PageKey = (generation, scope.clone(), cursor.clone());
                    match self.pages.get(&key) {
                        Some(Some(Ok(page))) => {
                            page_rows.push(Arc::clone(&page.rows));
                            frontier = page.next.clone();
                        }
                        Some(Some(Err(()))) => {
                            status = PagesStatus::Failed;
                            break;
                        }
                        Some(None) => {
                            status = PagesStatus::Pending;
                            break;
                        }
                        None => {
                            self.pages.insert(key.clone(), None);
                            reads.push(Box::pin(read_page(self.context.clone(), key, cursor.wire())));
                            status = PagesStatus::Pending;
                            break;
                        }
                    }
                }
                OlderSettledPages { loaded: page_rows.len(), rows: page_rows.iter().map(|page| page.threads.len()).sum(), frontier, status }
            }
            // A cached shell lists what it holds and holds no other environment back.
            _ => OlderSettledPages::default(),
        };
        if page_rows.is_empty() {
            self.shell.unmount_rows(PAGES_KEY);
        } else {
            self.shell.mount_rows(PAGES_KEY, page_rows);
        }

        // Families (`familyAtom`, `mount`).
        if family_retry != self.family_retry_seen {
            self.family_retry_seen = family_retry;
            self.families.retain(|_, read| !matches!(read, Some(Err(()))));
        }
        let mut families = BTreeMap::new();
        for thread_id in wanted_families {
            let status = if generation == 0 {
                FamilyStatus::Pending
            } else {
                let key = (generation, thread_id.clone());
                match self.families.get(&key) {
                    Some(Some(Ok(rows))) => {
                        self.shell.mount_rows(&family_key(&thread_id), vec![Arc::clone(rows)]);
                        self.mounted_families.insert(thread_id.clone());
                        FamilyStatus::Settled
                    }
                    Some(Some(Err(()))) => FamilyStatus::Failed,
                    Some(None) => FamilyStatus::Pending,
                    None => {
                        self.families.insert(key.clone(), None);
                        reads.push(Box::pin(read_family(self.context.clone(), key)));
                        FamilyStatus::Pending
                    }
                }
            };
            families.insert(thread_id, status);
        }
        // A family whose new generation's read is pending keeps its old rows; the shell drops
        // them by sequence once the new snapshot lands.
        let released: Vec<ThreadId> = self.mounted_families.iter().filter(|id| !families.contains_key(*id)).cloned().collect();
        for thread_id in released {
            self.shell.unmount_rows(&family_key(&thread_id));
            self.mounted_families.remove(&thread_id);
        }
        self.families.retain(|key, _| families.contains_key(&key.1));

        let view = OlderSettledView { older, pages, request: ShelfRequest { project_ids: scope, pages: shelf.pages }, families };
        self.view.send_if_modified(|current| {
            if **current == view {
                return false;
            }
            *current = Arc::new(view);
            true
        });
        reads
    }
}

fn family_key(thread_id: &ThreadId) -> String {
    format!("family:{}", thread_id.as_str())
}

fn counts_of(counts: &[OrchestrationV2OlderSettledCount]) -> HashMap<ProjectId, u64> {
    counts.iter().map(|count| (count.project_id.clone(), count.count)).collect()
}

/// The live session; waits for one between sessions.
async fn session(context: &EnvironmentContext) -> Option<Arc<RpcSession>> {
    let mut sessions = context.supervisor.session.clone();
    loop {
        if let Some(session) = sessions.borrow_and_update().clone() {
            return Some(session);
        }
        sessions.changed().await.ok()?;
    }
}

async fn read_counts(context: Arc<EnvironmentContext>, before: String, key: (u64, u64), generation: u64) -> Done {
    let result = async {
        let session = session(&context).await.ok_or(())?;
        let input = OrchestrationV2GetOlderSettledThreadsInput {
            before: OrchestrationV2OlderSettledCursor { settled_at: before, thread_id: None },
            project_ids: None,
            limit: 0,
        };
        let page = session.client.call::<OrchestrationGetOlderSettledThreads>(input).await.map_err(|error| {
            tracing::warn!(environment_id = %context.environment_id, %error, "older settled count read failed");
        })?;
        Ok(Arc::new(counts_of(&page.counts)))
    }
    .await;
    Done::Count { key, generation, result }
}

async fn read_page(context: Arc<EnvironmentContext>, key: PageKey, before: OrchestrationV2OlderSettledCursor) -> Done {
    let input = OrchestrationV2GetOlderSettledThreadsInput { before, project_ids: key.1.clone(), limit: OLDER_SETTLED_PAGE_SIZE };
    let read = async {
        let session = session(&context).await.ok_or(())?;
        session.client.call::<OrchestrationGetOlderSettledThreads>(input).await.map_err(|error| {
            tracing::warn!(environment_id = %context.environment_id, %error, "older settled page read failed");
        })
    }
    .await;
    match read {
        Ok(page) => {
            let full = page.threads.len() as u64 >= OLDER_SETTLED_PAGE_SIZE;
            let last = page.threads.last().and_then(|thread| {
                let iso = thread.settled_at.clone()?;
                Some(SettledCursor { settled_at: parse_ms(&iso)?, settled_at_iso: iso, thread_id: Some(thread.id.clone()) })
            });
            Done::Page { key, result: Ok(FamilyRows::new(page.snapshot_sequence, page.threads)), full: full && last.is_some(), last }
        }
        Err(()) => Done::Page { key, result: Err(()), last: None, full: false },
    }
}

async fn read_family(context: Arc<EnvironmentContext>, key: (u64, ThreadId)) -> Done {
    let result = async {
        let session = session(&context).await.ok_or(())?;
        // Servers without families send every child in the shell, so absent means gone.
        if session.initial_config.shell_thread_families != Some(true) {
            return Ok(FamilyRows::new(0, Vec::new()));
        }
        let input = OrchestrationV2GetThreadFamilyInput { thread_id: key.1.clone() };
        let family = session.client.call::<OrchestrationGetThreadFamily>(input).await.map_err(|error| {
            tracing::warn!(environment_id = %context.environment_id, %error, "thread family read failed");
        })?;
        Ok(FamilyRows::new(family.snapshot_sequence, family.threads))
    }
    .await;
    Done::Family { key, result }
}
