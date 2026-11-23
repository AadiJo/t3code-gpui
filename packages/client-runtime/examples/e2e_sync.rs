//! End-to-end check of the state and sync layer against a real T3 Code server.
//!
//! Usage: `e2e_sync <phase> <out_dir>`; phases run in order as separate processes:
//! - `live` (needs an unused pairing URL in `T3_PAIRING_URL`): pairs, syncs the shell, creates a
//!   project and threads, sends turns and follows them, approves, answers a question, loads a
//!   withheld command output, checks keep-alive of an unwatched running thread, interrupts,
//!   drops the socket mid-stream and checks the resume, then renames, pins, snoozes, settles,
//!   archives and deletes. Measures the memory-cached reopen.
//! - `reopen`: a new process on the same data directory opens the shell and a thread from the
//!   disk cache before the network answers, then catches up.
//! - `cold`: clears the state cache, opens the long thread cold (bounded snapshot), and pages
//!   older history to the start, checking it against the server's full projection.
//! - `bench`: offline measurements (shell cache of 5,000 rows, memory, 10,000 events).
//!
//! Writes `<out_dir>/e2e-<phase>.txt` (notes), `<out_dir>/e2e-commands.jsonl` (one record per
//! command: optimistic row, reply, confirming row) and `<out_dir>/e2e-frames-<phase>.jsonl`
//! (one complete wire frame per line).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use t3code_client_runtime::authorization::RemoteAuthorization;
use t3code_client_runtime::connection::credential_store::CredentialStore;
use t3code_client_runtime::connection::onboarding::{pair, parse_pairing_url};
use t3code_client_runtime::connection::{ConnectionContext, ConnectionPhase, ConnectionRegistry};
use t3code_client_runtime::operations::{EnvironmentCommands, SendMessage};
use t3code_client_runtime::platform::{ClientPresentation, FileSecretStore, SecretStore};
use t3code_client_runtime::relay::DpopKey;
use t3code_client_runtime::rpc::protocol::FrameJoiner;
use t3code_client_runtime::rpc::{FrameDirection, FrameTap, RpcSessionOptions};
use t3code_client_runtime::state::cache_store::{CacheStore, CachedThread};
use t3code_client_runtime::state::environment_http::{endpoint_path, get_bytes};
use t3code_client_runtime::state::models::{format_ms, now_ms};
use t3code_client_runtime::state::orchestration::Bodies;
use t3code_client_runtime::state::orchestration_v2_projection::apply_projection_event;
use t3code_client_runtime::state::shell_reducer::ShellSnapshot;
use t3code_client_runtime::state::sidebar::{PartitionOptions, partition, row_status};
use t3code_client_runtime::state::thread_detail::pending_requests;
use t3code_client_runtime::state::thread_shell::{ShellRows, navigation_rows};
use t3code_client_runtime::state::thread_state::{ThreadStreamItem, decode_stream_item};
use t3code_client_runtime::state::threads::LoadEarlierResult;
use t3code_client_runtime::state::{EnvironmentStore, ShellState, StateRuntime, SyncStatus, ThreadHandle, ThreadState, ThreadStatus};
use t3code_contracts::rpc::methods::OrchestrationGetThreadProjection;
use t3code_contracts::{
    CommandId, ModelSelection, OrchestrationV2DomainEvent, OrchestrationV2GetThreadProjectionInput, OrchestrationV2RunStatus,
    OrchestrationV2ThreadShell, OrchestrationV2TurnItem, ProjectId, ProjectMutation, ProjectMutationProjectCreate,
    ProviderApprovalDecision, ProviderInteractionMode, RuntimeMode, ThreadId,
};
use t3code_shared::thread_projection::ThreadProjection;

const STATE_FILE: &str = "e2e-state.json";

/// Counts live heap bytes, so memory figures do not depend on allocator reuse.
struct CountingAlloc;

static LIVE_HEAP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe impl std::alloc::GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        LIVE_HEAP.fetch_add(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        LIVE_HEAP.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        LIVE_HEAP.fetch_add(new_size, std::sync::atomic::Ordering::Relaxed);
        LIVE_HEAP.fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn heap_mib() -> f64 {
    LIVE_HEAP.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1_048_576.0
}

/// Notes and per-command records.
struct Recorder {
    started: Instant,
    notes: Mutex<Vec<String>>,
    commands: Mutex<std::fs::File>,
}

impl Recorder {
    fn note(&self, text: impl AsRef<str>) {
        let line = format!("{:>9.3}s  {}", self.started.elapsed().as_secs_f64(), text.as_ref());
        println!("{line}");
        self.notes.lock().unwrap().push(line);
    }

    fn command(&self, record: Value) {
        let mut file = self.commands.lock().unwrap();
        serde_json::to_writer(&mut *file, &record).unwrap();
        file.write_all(b"\n").unwrap();
    }
}

/// Writes every whole frame as one JSON line, and remembers thread stream event sequences per
/// subscription so the resume check can look for gaps and duplicates.
struct Frames {
    file: Mutex<std::io::BufWriter<std::fs::File>>,
    joiner: Mutex<FrameJoiner>,
    started: Instant,
    thread_subscriptions: Mutex<HashMap<String, String>>,
    thread_events: Mutex<Vec<(String, u64)>>,
}

impl Frames {
    fn tap(self: &Arc<Self>) -> FrameTap {
        let frames = Arc::clone(self);
        Arc::new(move |direction, text| frames.frame(direction, text))
    }

    fn frame(&self, direction: FrameDirection, text: &str) {
        let whole = match direction {
            FrameDirection::Sent => Some(text.to_owned()),
            FrameDirection::Received => self.joiner.lock().unwrap().push(text.to_owned()),
        };
        let Some(whole) = whole else { return };
        let parsed: Value = serde_json::from_str(&whole).unwrap_or(Value::Null);
        let dir = match direction {
            FrameDirection::Sent => "sent",
            FrameDirection::Received => "received",
        };
        if parsed["_tag"] == "Request" && parsed["tag"] == "orchestration.subscribeThread" {
            let id = parsed["id"].to_string();
            let thread = parsed["payload"]["threadId"].as_str().unwrap_or_default().to_owned();
            self.thread_subscriptions.lock().unwrap().insert(id, thread);
        }
        if parsed["_tag"] == "Chunk" {
            let id = parsed["requestId"].to_string();
            if self.thread_subscriptions.lock().unwrap().contains_key(&id) {
                for value in parsed["values"].as_array().into_iter().flatten() {
                    if value["kind"] == "event"
                        && let Some(sequence) = value["sequence"].as_u64()
                    {
                        self.thread_events.lock().unwrap().push((id.clone(), sequence));
                    }
                }
            }
        }
        let line = json!({ "t": self.started.elapsed().as_secs_f64(), "dir": dir, "frame": parsed });
        let mut file = self.file.lock().unwrap();
        serde_json::to_writer(&mut *file, &line).unwrap();
        file.write_all(b"\n").unwrap();
    }

    fn flush(&self) {
        self.file.lock().unwrap().flush().unwrap();
    }
}

/// Facts the later phases need.
#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct E2eState {
    environment_id: String,
    long_thread_id: String,
    long_thread_turns: usize,
    project_id: String,
}

fn rss_kib() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm.split_whitespace().nth(1).and_then(|value| value.parse().ok()).unwrap_or(0);
    pages * 4
}

async fn wait_shell(store: &EnvironmentStore, what: &str, secs: u64, done: impl Fn(&ShellState) -> bool) -> anyhow::Result<Arc<ShellState>> {
    let mut shell = store.shell().subscribe();
    let wait = async {
        loop {
            let current = shell.borrow_and_update().clone();
            if done(&current) {
                return current;
            }
            shell.changed().await.expect("shell store alive");
        }
    };
    tokio::time::timeout(Duration::from_secs(secs), wait).await.map_err(|_| anyhow::anyhow!("timed out waiting for {what}"))
}

async fn wait_thread(handle: &ThreadHandle, what: &str, secs: u64, done: impl Fn(&ThreadState) -> bool) -> anyhow::Result<Arc<ThreadState>> {
    let mut state = handle.receiver();
    let wait = async {
        loop {
            let current = state.borrow_and_update().clone();
            if done(&current) {
                return current;
            }
            state.changed().await.expect("thread alive");
        }
    };
    tokio::time::timeout(Duration::from_secs(secs), wait)
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for {what}: {:?}", handle.state().status))
}

fn latest_run(state: &ThreadState) -> Option<OrchestrationV2RunStatus> {
    state.data.as_ref()?.runs.last().map(|run| run.status)
}

fn run_count(state: &ThreadState) -> usize {
    state.data.as_ref().map_or(0, |data| data.runs.len())
}

fn turn_settled(state: &ThreadState, runs: usize) -> bool {
    state.status == ThreadStatus::Live
        && run_count(state) >= runs
        && latest_run(state).is_some_and(|status| {
            !matches!(
                status,
                OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Queued | OrchestrationV2RunStatus::Starting | OrchestrationV2RunStatus::Running | OrchestrationV2RunStatus::Waiting
            )
        })
}

fn row_json(row: Option<&Arc<OrchestrationV2ThreadShell>>) -> Value {
    row.map_or(Value::Null, |row| {
        json!({
            "title": row.title,
            "pinnedAt": row.pinned_at,
            "snoozedUntil": row.snoozed_until,
            "settledOverride": row.settled_override.map(|s| s.as_str()),
            "archivedAt": row.archived_at,
            "status": row.status.as_str(),
        })
    })
}

fn projection_summary(projection: &ThreadProjection) -> Value {
    json!({
        "runs": projection.runs.iter().map(|run| json!({"id": run.id, "status": run.status.as_str()})).collect::<Vec<_>>(),
        "visibleTurnItems": projection.visible_turn_items.len(),
        "turnItems": projection.turn_items.len(),
        "messages": projection.messages.len(),
        "pendingRequests": pending_requests(projection).len(),
        "updatedAt": projection.updated_at,
    })
}

/// Runs a shell command, recording the row before, right after sending (the optimistic layer),
/// the reply, and the row once the shell reached the reply's sequence.
async fn shell_command<F>(
    recorder: &Recorder,
    store: &EnvironmentStore,
    name: &str,
    thread_id: &ThreadId,
    command: F,
) -> anyhow::Result<u64>
where
    F: std::future::Future<Output = Result<t3code_client_runtime::operations::Accepted, t3code_client_runtime::operations::CommandError>> + Send + 'static,
{
    let before = row_json(store.shell().current().thread(thread_id));
    let sent = Instant::now();
    let task = tokio::spawn(command);
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(2)).await;
    let optimistic_view = store.shell().current();
    let optimistic = row_json(optimistic_view.thread(thread_id));
    let accepted = task.await??;
    let replied_ms = sent.elapsed().as_secs_f64() * 1000.0;
    let confirmed = wait_shell(store, name, 20, |shell| {
        shell.snapshot.as_ref().is_some_and(|snapshot| snapshot.snapshot_sequence >= accepted.sequence)
    })
    .await?;
    let source = store.shell().source();
    recorder.command(json!({
        "command": name,
        "threadId": thread_id,
        "before": before,
        "optimistic": optimistic,
        "reply": { "sequence": accepted.sequence, "ms": replied_ms },
        "confirmed": row_json(confirmed.thread(thread_id)),
        "confirmedServerRow": row_json(source.thread(thread_id)),
        "shellSequence": confirmed.snapshot.as_ref().map(|s| s.snapshot_sequence),
    }));
    recorder.note(format!("{name}: reply sequence {} after {replied_ms:.1} ms", accepted.sequence));
    Ok(accepted.sequence)
}

struct Client {
    runtime: StateRuntime,
    registry: ConnectionRegistry,
    store: EnvironmentStore,
}

async fn connect(data_dir: &Path, frames: &Arc<Frames>, recorder: &Recorder, pairing_url: Option<&str>) -> anyhow::Result<Client> {
    let secrets: Arc<dyn SecretStore> = Arc::new(FileSecretStore::new(data_dir.join("secrets.json")));
    let key = DpopKey::load_or_create(secrets.as_ref())?;
    let presentation = ClientPresentation { label: "t3code-gpui state e2e".into(), ..ClientPresentation::default() };
    let auth = Arc::new(RemoteAuthorization::new(presentation, key, CredentialStore::new(secrets), None));
    let context = Arc::new(ConnectionContext { auth: auth.clone(), session_options: RpcSessionOptions::default(), frame_tap: Some(frames.tap()) });
    let registry = ConnectionRegistry::open(context, data_dir).await?;
    if let Some(url) = pairing_url {
        let paired = pair(&auth, &parse_pairing_url(url)?, None).await?;
        recorder.note(format!("paired environment {} ({})", paired.environment_id, paired.label));
        registry.save_paired(&paired).await?;
    }
    let runtime = StateRuntime::start(registry.clone(), auth, data_dir);
    let mut stores = runtime.stores();
    let store = loop {
        if let Some(store) = stores.borrow_and_update().first().cloned() {
            break store;
        }
        stores.changed().await?;
    };
    Ok(Client { runtime, registry, store })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let phase = args.next().expect("usage: e2e_sync <live|reopen|cold|bench> <out_dir>");
    let out_dir = PathBuf::from(args.next().expect("usage: e2e_sync <phase> <out_dir>"));
    std::fs::create_dir_all(&out_dir)?;
    let data_dir = out_dir.join("client-data");
    if phase == "live" {
        _ = std::fs::remove_file(out_dir.join("e2e-commands.jsonl"));
    }
    let recorder = Recorder {
        started: Instant::now(),
        notes: Mutex::default(),
        commands: Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(out_dir.join("e2e-commands.jsonl"))?),
    };
    let frames = Arc::new(Frames {
        file: Mutex::new(std::io::BufWriter::new(std::fs::File::create(out_dir.join(format!("e2e-frames-{phase}.jsonl")))?)),
        joiner: Mutex::default(),
        started: Instant::now(),
        thread_subscriptions: Mutex::default(),
        thread_events: Mutex::default(),
    });
    let result = match phase.as_str() {
        "live" => live(&out_dir, &data_dir, &recorder, &frames).await,
        "reopen" => reopen(&out_dir, &data_dir, &recorder, &frames).await,
        "cold" => cold(&out_dir, &data_dir, &recorder, &frames).await,
        "bench" => bench(&out_dir, &recorder),
        other => Err(anyhow::anyhow!("unknown phase {other}")),
    };
    if let Err(error) = &result {
        recorder.note(format!("FAILED: {error:#}"));
    }
    frames.flush();
    std::fs::write(out_dir.join(format!("e2e-{phase}.txt")), recorder.notes.lock().unwrap().join("\n") + "\n")?;
    result
}

async fn live(out_dir: &Path, data_dir: &Path, recorder: &Recorder, frames: &Arc<Frames>) -> anyhow::Result<()> {
    _ = std::fs::remove_dir_all(data_dir);
    std::fs::create_dir_all(data_dir)?;
    let pairing_url = std::env::var("T3_PAIRING_URL")?;
    let client = connect(data_dir, frames, recorder, Some(&pairing_url)).await?;
    let store = client.store.clone();
    let commands = EnvironmentCommands::new(store.clone());

    let shell = wait_shell(&store, "live shell", 60, |shell| shell.status == SyncStatus::Live).await?;
    let snapshot = shell.snapshot.clone().expect("live shell has rows");
    recorder.note(format!(
        "shell live: generation {}, sequence {}, {} threads, {} projects",
        shell.generation,
        snapshot.snapshot_sequence,
        snapshot.threads.len(),
        snapshot.projects.len()
    ));

    // A project in a workspace the run owns.
    let workspace = out_dir.join("workspaces").join(&uuid::Uuid::new_v4().to_string()[..8]);
    std::fs::create_dir_all(&workspace)?;
    let project_id = ProjectId::new(uuid::Uuid::new_v4().to_string());
    let project = commands
        .mutate_project(ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
            command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
            project_id: project_id.clone(),
            title: "State e2e".into(),
            workspace_root: workspace.display().to_string(),
            create_workspace_root_if_missing: Some(true),
            default_model_selection: None,
            scripts: None,
        }))
        .await?;
    recorder.note(format!("created project {} at {}", project.id, workspace.display()));
    wait_shell(&store, "project row", 20, |shell| shell.snapshot.as_ref().is_some_and(|s| s.projects.iter().any(|p| p.id == project_id))).await?;
    recorder.command(json!({"command": "projects.mutate project.create", "projectId": project_id, "confirmed": "project.updated delta reached the shell"}));

    let new_thread = |title: &str, model: &str, runtime_mode: RuntimeMode| t3code_client_runtime::operations::CreateThread {
        thread_id: ThreadId::new(uuid::Uuid::new_v4().to_string()),
        project_id: project_id.clone(),
        title: title.into(),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: model.into(), options: None },
        runtime_mode,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
    };

    // A thread with a turn, followed to completion.
    let create = new_thread("Long thread", "multi-turn", RuntimeMode::FullAccess);
    let long_id = create.thread_id.clone();
    let created = commands.create_thread(create).await?;
    let shell = wait_shell(&store, "thread row", 20, |shell| shell.thread(&long_id).is_some()).await?;
    recorder.command(json!({"command": "thread.create", "threadId": long_id, "reply": {"sequence": created.sequence}, "confirmed": row_json(shell.thread(&long_id))}));
    let handle = store.open_thread(&long_id);
    wait_thread(&handle, "thread live", 20, |state| state.status == ThreadStatus::Live).await?;
    let turns = 12;
    for turn in 0..turns {
        let message = SendMessage::text(long_id.clone(), format!("Turn {} of the long thread.", turn + 1));
        let message_id = message.message_id.clone();
        let sent = Instant::now();
        let accepted = commands.send_message(message).await?;
        let state = wait_thread(&handle, "turn to settle", 90, |state| turn_settled(state, turn + 1)).await?;
        let data = state.data.as_ref().expect("live thread has data");
        let echoed = data.messages.iter().any(|message| message.id == message_id);
        if turn == 0 || turn == turns - 1 {
            recorder.command(json!({
                "command": "message.dispatch",
                "threadId": long_id,
                "messageId": message_id,
                "optimistic": "local message until the projection holds its messageId",
                "reply": {"sequence": accepted.sequence},
                "confirmed": {"echoed": echoed, "threadSequence": state.sequence, "caughtUp": state.has_caught_up_to(accepted.sequence)},
                "final": projection_summary(data),
            }));
        }
        recorder.note(format!("turn {} settled in {:.0} ms ({:?}), {} timeline rows", turn + 1, sent.elapsed().as_secs_f64() * 1000.0, latest_run(&state), data.visible_turn_items.len()));
    }
    std::fs::write(
        out_dir.join(STATE_FILE),
        serde_json::to_vec_pretty(&E2eState {
            environment_id: store.environment_id().to_string(),
            long_thread_id: long_id.to_string(),
            long_thread_turns: turns,
            project_id: project_id.to_string(),
        })?,
    )?;

    // Memory-cached reopen: close the view, reopen, read the state synchronously.
    drop(handle);
    let started = Instant::now();
    let handle = store.open_thread(&long_id);
    let reopened = handle.state();
    let micros = started.elapsed().as_secs_f64() * 1e6;
    recorder.note(format!(
        "memory-cached reopen: {micros:.0} us to a projection with {} timeline rows (status {:?})",
        reopened.data.as_ref().map_or(0, |data| data.visible_turn_items.len()),
        reopened.status
    ));
    anyhow::ensure!(reopened.data.is_some(), "the resume cache gave no projection");

    // Approval.
    let create = new_thread("Approval", "approval", RuntimeMode::ApprovalRequired);
    let approval_id = create.thread_id.clone();
    commands.create_thread(create).await?;
    let approval = store.open_thread(&approval_id);
    wait_thread(&approval, "approval thread live", 20, |state| state.status == ThreadStatus::Live).await?;
    commands.send_message(SendMessage::text(approval_id.clone(), "Please write the file.")).await?;
    let state = wait_thread(&approval, "approval request", 60, |state| state.data.as_deref().is_some_and(|data| !pending_requests(data).is_empty()) || turn_settled(state, 1)).await?;
    if let Some(request) = state.data.as_deref().and_then(|data| pending_requests(data).into_iter().next()) {
        let shell_row = row_json(store.shell().current().thread(&approval_id));
        recorder.note(format!("approval requested: {} ({:?}), sidebar status {:?}", request.request.id, request.request.kind, store.shell().current().thread(&approval_id).map(|row| row.status)));
        let accepted = commands.respond_to_approval(&approval_id, request.request.id.clone(), ProviderApprovalDecision::Accept, None).await?;
        let state = wait_thread(&approval, "approval resolved", 60, |state| turn_settled(state, 1)).await?;
        recorder.command(json!({
            "command": "runtime-request.respond (approve)",
            "threadId": approval_id,
            "requestId": request.request.id,
            "shellBefore": shell_row,
            "reply": {"sequence": accepted.sequence},
            "confirmed": {"pendingAfter": state.data.as_deref().map(|data| pending_requests(data).len())},
            "final": state.data.as_deref().map(projection_summary),
        }));
    } else {
        recorder.note("the approval scenario finished without asking; nothing to approve");
    }

    // Question.
    let create = new_thread("Question", "question", RuntimeMode::FullAccess);
    let question_id = create.thread_id.clone();
    commands.create_thread(create).await?;
    let question = store.open_thread(&question_id);
    wait_thread(&question, "question thread live", 20, |state| state.status == ThreadStatus::Live).await?;
    commands.send_message(SendMessage::text(question_id.clone(), "Ask me something.")).await?;
    // The request can land a moment before its card; answer once the card is in.
    let state = wait_thread(&question, "question", 60, |state| {
        state.data.as_deref().is_some_and(|data| pending_requests(data).iter().any(|request| request.card.is_some())) || turn_settled(state, 1)
    })
    .await?;
    if let Some(request) = state.data.as_deref().and_then(|data| pending_requests(data).into_iter().next()) {
        let mut answers = serde_json::Map::new();
        if let Some(OrchestrationV2TurnItem::UserInputRequest(card)) = request.card.as_deref() {
            for q in &card.questions {
                let answer = q.options.first().map_or_else(|| "yes".to_owned(), |option| option.label.clone());
                answers.insert(q.id.clone(), Value::String(answer));
            }
        }
        recorder.note(format!("question asked: {} with answers {}", request.request.id, Value::Object(answers.clone())));
        let accepted = commands.answer_question(&question_id, request.request.id.clone(), answers.clone()).await?;
        let state = wait_thread(&question, "question answered", 60, |state| turn_settled(state, 1)).await?;
        recorder.command(json!({
            "command": "runtime-request.respond (answer)",
            "threadId": question_id,
            "requestId": request.request.id,
            "answers": answers,
            "reply": {"sequence": accepted.sequence},
            "final": state.data.as_deref().map(projection_summary),
        }));
    } else {
        recorder.note("the question scenario finished without asking");
    }

    // Withheld bodies: a command's output, fetched by item and followed as a stream.
    let create = new_thread("Command output", "command-output", RuntimeMode::FullAccess);
    let output_id = create.thread_id.clone();
    commands.create_thread(create).await?;
    let output_thread = store.open_thread(&output_id);
    wait_thread(&output_thread, "output thread live", 20, |state| state.status == ThreadStatus::Live).await?;
    commands.send_message(SendMessage::text(output_id.clone(), "Run the command.")).await?;
    let state = wait_thread(&output_thread, "command turn", 60, |state| turn_settled(state, 1)).await?;
    let command_item = state.data.as_ref().and_then(|data| {
        data.turn_items.iter().find(|item| matches!(&***item, OrchestrationV2TurnItem::CommandExecution(_))).cloned()
    });
    if let Some(item) = command_item {
        let OrchestrationV2TurnItem::CommandExecution(streamed) = &*item else { unreachable!() };
        let bodies = Bodies::new(store.clone());
        let full = bodies.turn_item(&output_id, &streamed.id).await;
        let full_output = full.as_deref().and_then(|item| match item {
            OrchestrationV2TurnItem::CommandExecution(command) => command.output.clone(),
            _ => None,
        });
        let (mut output, _guard) = bodies.command_output(&output_id, &streamed.id);
        let followed = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let current = output.borrow_and_update().clone();
                if !current.running {
                    return current;
                }
                if output.changed().await.is_err() {
                    return current;
                }
            }
        })
        .await?;
        recorder.command(json!({
            "command": "orchestration.getTurnItem + subscribeCommandOutput",
            "threadId": output_id,
            "itemId": streamed.id,
            "streamOutputOmitted": streamed.output_omitted,
            "streamOutputBytes": streamed.output.as_ref().map_or(0, |o| o.len()),
            "fetchedOutputBytes": full_output.as_ref().map_or(0, |o| o.len()),
            "followedOutputBytes": followed.text.len(),
            "followedRunning": followed.running,
        }));
        recorder.note(format!(
            "command output: stream withheld {:?} ({} bytes), getTurnItem {} bytes, subscribeCommandOutput {} bytes",
            streamed.output_omitted,
            streamed.output.as_ref().map_or(0, |o| o.len()),
            full_output.as_ref().map_or(0, |o| o.len()),
            followed.text.len()
        ));
    } else {
        recorder.note("the command-output scenario produced no command item");
    }
    drop(output_thread);

    // Keep-alive: a running thread with no open view keeps its detail stream, and is released
    // once its own stream shows the run ended.
    let create = new_thread("Keep-alive", "multi-turn", RuntimeMode::FullAccess);
    let kept_id = create.thread_id.clone();
    commands.create_thread(create).await?;
    let kept = store.open_thread(&kept_id);
    wait_thread(&kept, "kept thread live", 20, |state| state.status == ThreadStatus::Live).await?;
    drop(kept);
    commands.send_message(SendMessage::text(kept_id.clone(), "Run while nobody watches.")).await?;
    let started = Instant::now();
    let mut mounted_while_running = false;
    let mut released = false;
    while started.elapsed() < Duration::from_secs(30) {
        let (live_ids, resume_ids) = store.held_threads();
        let running = store.shell().current().thread(&kept_id).is_some_and(|row| {
            matches!(row.status, t3code_contracts::OrchestrationV2ShellThreadStatus::Running | t3code_contracts::OrchestrationV2ShellThreadStatus::Starting | t3code_contracts::OrchestrationV2ShellThreadStatus::Preparing)
        });
        if running && live_ids.contains(&kept_id) {
            mounted_while_running = true;
        }
        if mounted_while_running && !running && !live_ids.contains(&kept_id) && resume_ids.contains(&kept_id) {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let reopened = Instant::now();
    let kept = store.open_thread(&kept_id);
    let kept_state = kept.state();
    recorder.command(json!({
        "command": "keep-alive (no command; observed)",
        "threadId": kept_id,
        "mountedWhileRunning": mounted_while_running,
        "releasedAfterRunEnded": released,
        "reopenMicros": reopened.elapsed().as_secs_f64() * 1e6,
        "reopened": kept_state.data.as_deref().map(projection_summary),
    }));
    recorder.note(format!(
        "keep-alive: mounted while running {mounted_while_running}, released to the resume cache after the run {released}, reopen gave {} runs",
        run_count(&kept_state)
    ));
    anyhow::ensure!(mounted_while_running && released && run_count(&kept_state) == 1, "keep-alive did not hold the running thread");
    drop(kept);

    // Interrupt a running turn.
    let create = new_thread("Interrupt", "showcase", RuntimeMode::FullAccess);
    let interrupt_id = create.thread_id.clone();
    commands.create_thread(create).await?;
    let interrupt = store.open_thread(&interrupt_id);
    wait_thread(&interrupt, "interrupt thread live", 20, |state| state.status == ThreadStatus::Live).await?;
    commands.send_message(SendMessage::text(interrupt_id.clone(), "Start something long.")).await?;
    wait_thread(&interrupt, "run to start", 30, |state| matches!(latest_run(state), Some(OrchestrationV2RunStatus::Running | OrchestrationV2RunStatus::Starting))).await?;
    let accepted = commands.interrupt(&interrupt_id, None, None).await?;
    let state = wait_thread(&interrupt, "run to stop", 30, |state| turn_settled(state, 1)).await?;
    recorder.command(json!({
        "command": "interrupt (thread.stop or run.interrupt)",
        "threadId": interrupt_id,
        "reply": {"sequence": accepted.sequence},
        "confirmed": {"latestRun": latest_run(&state).map(|status| status.as_str())},
    }));
    recorder.note(format!("interrupted: latest run {:?}", latest_run(&state)));

    // Socket drop mid-stream: a turn streams, the socket closes, the stream resumes.
    let before_events = frames.thread_events.lock().unwrap().len();
    commands.send_message(SendMessage::text(long_id.clone(), "A turn that streams through a reconnect.")).await?;
    wait_thread(&handle, "streaming", 30, |state| matches!(latest_run(state), Some(OrchestrationV2RunStatus::Running))).await?;
    let applied_before = handle.state().sequence;
    let generation = store.context().supervisor.status.borrow().generation;
    store.session().expect("connected").client.close();
    recorder.note(format!("closed the socket mid-stream at thread sequence {applied_before}"));
    let mut status = store.context().supervisor.status.clone();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            {
                let current = status.borrow_and_update();
                if current.phase == ConnectionPhase::Connected && current.generation > generation {
                    return;
                }
            }
            status.changed().await.unwrap();
        }
    })
    .await?;
    let state = wait_thread(&handle, "resumed turn to settle", 90, |state| turn_settled(state, turns + 1)).await?;
    recorder.note(format!("reconnected (generation {}) and the turn settled at sequence {}", store.context().supervisor.status.borrow().generation, state.sequence));
    let session = store.session().expect("connected");
    let server = session.client.call::<OrchestrationGetThreadProjection>(OrchestrationV2GetThreadProjectionInput { thread_id: long_id.clone() }).await?;
    let local = state.data.as_ref().expect("data");
    let local_ids: Vec<String> = local.visible_turn_items.iter().map(|row| row.source_item_id.to_string()).collect();
    let server_ids: Vec<String> = server.visible_turn_items.iter().map(|row| row.source_item_id.to_string()).collect();
    let local_unique: HashSet<&String> = local_ids.iter().collect();
    let tail = |ids: &[String], n: usize| ids[ids.len().saturating_sub(n)..].to_vec();
    let window = local_ids.len().min(server_ids.len());
    let same_items = tail(&local_ids, window) == tail(&server_ids, window);
    let local_runs: Vec<(String, &str)> = local.runs.iter().map(|run| (run.id.to_string(), run.status.as_str())).collect();
    let server_runs: Vec<(String, &str)> = server.runs.iter().map(|run| (run.id.to_string(), run.status.as_str())).collect();
    let events = frames.thread_events.lock().unwrap()[before_events..].to_vec();
    let mut by_subscription: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for (subscription, sequence) in &events {
        by_subscription.entry(subscription.clone()).or_default().push(*sequence);
    }
    let all: Vec<u64> = events.iter().map(|(_, sequence)| *sequence).collect();
    let duplicates = all.len() - all.iter().collect::<HashSet<_>>().len();
    let ascending = all.windows(2).all(|pair| pair[0] < pair[1]);
    recorder.command(json!({
        "command": "socket drop mid-stream",
        "threadId": long_id,
        "appliedBeforeDrop": applied_before,
        "eventsBySubscription": by_subscription.iter().map(|(id, seqs)| json!({"request": id, "count": seqs.len(), "first": seqs.first(), "last": seqs.last()})).collect::<Vec<_>>(),
        "duplicateSequences": duplicates,
        "strictlyAscendingAcrossResubscribe": ascending,
        "timelineMatchesServer": same_items && local_ids.len() == local_unique.len(),
        "runsMatchServer": local_runs == server_runs,
        "finalSequence": state.sequence,
    }));
    recorder.note(format!(
        "resume check: {} events over {} subscriptions, {duplicates} duplicates, ascending {ascending}, timeline matches server {}, runs match {}",
        all.len(),
        by_subscription.len(),
        same_items && local_ids.len() == local_unique.len(),
        local_runs == server_runs
    ));
    anyhow::ensure!(duplicates == 0 && ascending && same_items && local_runs == server_runs, "resume diverged from the server");

    // Row actions, each with its optimistic state and confirming delta.
    let c = commands.clone();
    let id = approval_id.clone();
    shell_command(recorder, &store, "thread.metadata.update (rename)", &approval_id, async move { c.rename(&id, "Renamed by the e2e").await }).await?;
    let c = commands.clone();
    let id = approval_id.clone();
    shell_command(recorder, &store, "thread.pin", &approval_id, async move { c.pin(&id, None).await }).await?;
    let c = commands.clone();
    let id = question_id.clone();
    let until = format_ms(now_ms() + 3_600_000);
    shell_command(recorder, &store, "thread.snooze", &question_id, async move { c.snooze(&id, until).await }).await?;
    let c = commands.clone();
    let id = interrupt_id.clone();
    shell_command(recorder, &store, "thread.settle", &interrupt_id, async move { c.settle(&id).await }).await?;
    let c = commands.clone();
    let id = question_id.clone();
    shell_command(recorder, &store, "thread.archive", &question_id, async move { c.archive(&id).await }).await?;
    drop(interrupt);
    let c = commands.clone();
    let id = interrupt_id.clone();
    shell_command(recorder, &store, "thread.delete", &interrupt_id, async move { c.delete(&id, false).await }).await?;

    // The sidebar as the UI would read it.
    let shell = store.shell().current();
    let snapshot = shell.snapshot.clone().expect("rows");
    let mut presenter = ShellRows::default();
    let rows = presenter.present(&[(store.environment_id().clone(), snapshot.threads.as_slice())]);
    let navigation = navigation_rows(&rows, &HashSet::new());
    let refs: Vec<_> = navigation.iter().map(|row| &**row).collect();
    let sections = partition(&refs, now_ms(), |_| true, None, PartitionOptions::default());
    let names = |rows: &[&t3code_client_runtime::state::models::ThreadRow]| rows.iter().map(|row| format!("{} [{:?}]", row.title(), row_status(row))).collect::<Vec<_>>();
    recorder.note(format!(
        "sidebar: pinned {:?}, active {:?}, snoozed {:?}, settled {:?}",
        names(&sections.pinned),
        names(&sections.active),
        names(&sections.snoozed),
        names(&sections.settled)
    ));
    let (live_threads, resumable) = store.held_threads();
    recorder.note(format!("held in memory: {} mounted or kept alive, {} in the resume cache", live_threads.len(), resumable.len()));

    drop((handle, approval, question));
    drop(commands);
    drop(store);
    drop(client.runtime);
    drop(client.registry);
    // Let the persisters' blocking saves finish.
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

async fn reopen(out_dir: &Path, data_dir: &Path, recorder: &Recorder, frames: &Arc<Frames>) -> anyhow::Result<()> {
    let state: E2eState = serde_json::from_slice(&std::fs::read(out_dir.join(STATE_FILE))?)?;
    let thread_id = ThreadId::new(state.long_thread_id);
    let started = Instant::now();
    let client = connect(data_dir, frames, recorder, None).await?;
    let store = client.store.clone();
    let shell = store.shell().current();
    recorder.note(format!(
        "restart: shell from disk with status {:?} and {} rows, {:.1} ms after start, before any network answer",
        shell.status,
        shell.snapshot.as_ref().map_or(0, |s| s.threads.len()),
        started.elapsed().as_secs_f64() * 1000.0
    ));
    let opened = Instant::now();
    let handle = store.open_thread(&thread_id);
    let first = handle.state();
    let micros = opened.elapsed().as_secs_f64() * 1e6;
    let connected_yet = store.session().is_some();
    recorder.note(format!(
        "disk-cached open: {micros:.0} us to a projection with {} timeline rows, status {:?}, socket connected yet: {connected_yet}",
        first.data.as_ref().map_or(0, |data| data.visible_turn_items.len()),
        first.status
    ));
    anyhow::ensure!(first.data.is_some(), "the disk cache gave no projection");
    recorder.command(json!({
        "command": "reopen from disk cache",
        "threadId": thread_id,
        "openMicros": micros,
        "beforeNetwork": !connected_yet,
        "cached": first.data.as_deref().map(projection_summary),
        "cachedSequence": first.sequence,
    }));
    let live = wait_thread(&handle, "resume to live", 60, |state| state.status == ThreadStatus::Live).await?;
    let resumed_with_snapshot = frames
        .thread_subscriptions
        .lock()
        .unwrap()
        .len();
    recorder.note(format!("caught up: live at sequence {} (cached {}), {resumed_with_snapshot} thread subscription(s)", live.sequence, first.sequence));
    let shell = wait_shell(&store, "shell live", 60, |shell| shell.status == SyncStatus::Live).await?;
    recorder.note(format!("shell live again at sequence {}", shell.snapshot.as_ref().map_or(0, |s| s.snapshot_sequence)));
    drop(handle);
    drop(store);
    drop(client.runtime);
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}

async fn cold(out_dir: &Path, data_dir: &Path, recorder: &Recorder, frames: &Arc<Frames>) -> anyhow::Result<()> {
    let state: E2eState = serde_json::from_slice(&std::fs::read(out_dir.join(STATE_FILE))?)?;
    let thread_id = ThreadId::new(state.long_thread_id);
    _ = std::fs::remove_dir_all(data_dir.join("state-cache"));
    let client = connect(data_dir, frames, recorder, None).await?;
    let store = client.store.clone();
    wait_shell(&store, "shell live", 60, |shell| shell.status == SyncStatus::Live).await?;
    let started = Instant::now();
    let handle = store.open_thread(&thread_id);
    let first = wait_thread(&handle, "cold open", 30, |state| state.data.is_some()).await?;
    let cold_ms = started.elapsed().as_secs_f64() * 1000.0;
    recorder.note(format!(
        "cold open: {cold_ms:.1} ms to a projection with {} timeline rows; history cursor {:?}, more history {}",
        first.data.as_ref().map_or(0, |data| data.visible_turn_items.len()),
        first.history.history_cursor.is_some(),
        first.history.has_more_history
    ));
    let mut pages = 0;
    loop {
        let before = handle.state().data.as_ref().map_or(0, |data| data.visible_turn_items.len());
        let result = handle.load_earlier().await;
        let after = handle.state();
        recorder.note(format!("load earlier: {result:?}, rows {before} -> {}", after.data.as_ref().map_or(0, |data| data.visible_turn_items.len())));
        match result {
            LoadEarlierResult::Loaded => pages += 1,
            LoadEarlierResult::Noop => break,
            other => anyhow::bail!("paging failed: {other:?}"),
        }
        if !after.history.has_more_history {
            break;
        }
    }
    // The full timeline from the server's transcript route (the projection RPC is bounded too).
    let session = store.session().expect("connected");
    let path = endpoint_path::<t3code_contracts::http::endpoints::OrchestrationThreadTranscript>(&[("threadId", thread_id.as_str())]);
    let transcript: Value = serde_json::from_slice(&get_bytes(store.context(), &session, &path, &[], Duration::from_secs(10)).await?)?;
    let local = handle.state();
    let local_ids: Vec<String> = local.data.as_ref().expect("data").visible_turn_items.iter().map(|row| row.source_item_id.to_string()).collect();
    let server_ids: Vec<String> = transcript["items"].as_array().into_iter().flatten().filter_map(|row| row["sourceItemId"].as_str().map(str::to_owned)).collect();
    let unique = local_ids.iter().collect::<HashSet<_>>().len() == local_ids.len();
    recorder.command(json!({
        "command": "load earlier history",
        "threadId": thread_id,
        "coldOpenMs": cold_ms,
        "pages": pages,
        "rows": local_ids.len(),
        "serverRows": server_ids.len(),
        "matchesServer": local_ids == server_ids,
        "unique": unique,
        "expanded": local.history.expanded,
    }));
    recorder.note(format!("after {pages} page(s): {} rows, server has {}, identical order {}, no duplicates {unique}", local_ids.len(), server_ids.len(), local_ids == server_ids));
    anyhow::ensure!(local_ids == server_ids && unique, "paged history differs from the server");
    anyhow::ensure!(pages > 0, "the thread was not long enough to page");
    drop(handle);
    drop(store);
    drop(client.runtime);
    Ok(())
}

/// Offline measurements, from the long thread the server produced.
fn bench(out_dir: &Path, recorder: &Recorder) -> anyhow::Result<()> {
    let data_dir = out_dir.join("client-data");
    let state: E2eState = serde_json::from_slice(&std::fs::read(out_dir.join(STATE_FILE))?)?;
    let environment_id = t3code_contracts::EnvironmentId::new(state.environment_id.clone());
    let cache = CacheStore::new(&data_dir);
    let shell = cache.load_shell(&environment_id).ok_or_else(|| anyhow::anyhow!("no cached shell; run live first"))?;
    let template = shell.snapshot.threads.first().cloned().ok_or_else(|| anyhow::anyhow!("no row"))?;
    let thread = cache.load_thread(&environment_id, &ThreadId::new(state.long_thread_id.clone())).ok_or_else(|| anyhow::anyhow!("no cached thread"))?;

    // 1. A 5,000-row shell cache.
    let bench_dir = out_dir.join("bench-data");
    _ = std::fs::remove_dir_all(&bench_dir);
    let bench_cache = CacheStore::new(&bench_dir);
    let rows: Vec<_> = (0..5_000)
        .map(|i| {
            let mut row = (*template).clone();
            row.id = ThreadId::new(format!("bench-thread-{i:05}-{}", uuid::Uuid::new_v4()));
            row.title = format!("Bench thread {i} about something the user asked");
            row.latest_visible_message = template.latest_visible_message.clone();
            Arc::new(row)
        })
        .collect();
    let snapshot = ShellSnapshot { threads: Arc::new(rows), ..shell.snapshot.clone() };
    bench_cache.save_shell(&environment_id, &snapshot, shell.origin.as_ref(), bench_cache.ticket());
    let file = bench_dir.join("state-cache/v1");
    let bytes: u64 = walk_size(&file);
    drop(snapshot);
    drop(shell);
    let mut timings = Vec::new();
    for _ in 0..7 {
        let started = Instant::now();
        let loaded = bench_cache.load_shell(&environment_id).expect("bench shell loads");
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
        drop(loaded);
    }
    timings.sort_by(f64::total_cmp);
    let heap_before = heap_mib();
    let rss_before = rss_kib();
    let held = bench_cache.load_shell(&environment_id).expect("bench shell loads");
    let heap_with_shell = heap_mib();
    let rss_with_shell = rss_kib();
    recorder.note(format!(
        "shell cache: {} rows, {:.1} MiB on disk, load median {:.1} ms (min {:.1}, max {:.1}) over 7 runs",
        held.snapshot.threads.len(),
        bytes as f64 / 1_048_576.0,
        timings[3],
        timings[0],
        timings[6]
    ));
    recorder.note(format!(
        "5,000-row shell in memory: +{:.1} MiB live heap ({:.1} KiB per row); process resident {:.1} MiB (+{:.1} MiB)",
        heap_with_shell - heap_before,
        (heap_with_shell - heap_before) * 1024.0 / 5000.0,
        rss_with_shell as f64 / 1024.0,
        (rss_with_shell as f64 - rss_before as f64) / 1024.0
    ));

    // 2. 20 threads in the resume cache: distinct deep copies of a real projection.
    let json = serde_json::to_string(&*thread.projection)?;
    let heap_before_threads = heap_mib();
    let projections: Vec<Arc<ThreadProjection>> = (0..20)
        .map(|_| {
            let mut projection: ThreadProjection = serde_json::from_str(&json).expect("projection decodes");
            projection.share_local_items();
            Arc::new(projection)
        })
        .collect();
    let heap_with_threads = heap_mib();
    recorder.note(format!(
        "20 threads in the resume cache ({} timeline rows, {:.0} KiB of JSON each): +{:.1} MiB live heap ({:.0} KiB per thread); total with the shell {:.1} MiB heap, {:.1} MiB resident",
        thread.projection.visible_turn_items.len(),
        json.len() as f64 / 1024.0,
        heap_with_threads - heap_before_threads,
        (heap_with_threads - heap_before_threads) * 1024.0 / 20.0,
        heap_with_threads,
        rss_kib() as f64 / 1024.0
    ));

    // 3. Applying 10,000 stream events: streamed assistant text over new and updated items.
    let base = (*projections[0]).clone();
    let template_item = base
        .turn_items
        .iter()
        .rev()
        .find(|item| matches!(&***item, OrchestrationV2TurnItem::AssistantMessage(_)))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no assistant item to stream"))?;
    let thread_id = base.thread.id.clone();
    let mut raw_events = Vec::with_capacity(10_000);
    let mut text = String::new();
    let mut item_index = 0;
    for sequence in 0..10_000u64 {
        if sequence % 200 == 0 {
            item_index += 1;
            text.clear();
        }
        text.push_str("word ");
        let OrchestrationV2TurnItem::AssistantMessage(mut item) = (*template_item).clone() else { unreachable!() };
        item.id = t3code_contracts::TurnItemId::new(format!("bench-item-{item_index}"));
        item.message_id = t3code_contracts::MessageId::new(format!("bench-message-{item_index}"));
        item.ordinal = 1_000_000 + item_index;
        item.text = text.clone();
        item.streaming = true;
        item.thread_id = thread_id.clone();
        let event = json!({
            "kind": "event",
            "sequence": 1_000_000 + sequence,
            "event": {
                "type": "turn-item.updated",
                "id": format!("bench-event-{sequence}"),
                "threadId": thread_id,
                "occurredAt": format_ms(now_ms()),
                "payload": OrchestrationV2TurnItem::AssistantMessage(item),
            }
        });
        raw_events.push(serde_json::value::to_raw_value(&event)?);
    }
    let started = Instant::now();
    let decoded: Vec<OrchestrationV2DomainEvent> = raw_events
        .iter()
        .filter_map(|raw| match decode_stream_item(raw) {
            ThreadStreamItem::Event { event: Some(event), .. } => Some(event),
            _ => None,
        })
        .collect();
    let decode_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut projection = Arc::new(base);
    let started = Instant::now();
    // Batches of 8, as the server chunks a busy stream; each batch publishes a new projection.
    for batch in decoded.chunks(8) {
        let mut next = (*projection).clone();
        for event in batch {
            apply_projection_event(&mut next, event, None);
        }
        projection = Arc::new(next);
    }
    let apply_ms = started.elapsed().as_secs_f64() * 1000.0;
    recorder.note(format!(
        "10,000 stream events: decode {decode_ms:.1} ms, apply {apply_ms:.1} ms ({:.1} us per event) into {} timeline rows",
        apply_ms * 1000.0 / 10_000.0,
        projection.visible_turn_items.len()
    ));
    let _ = (held, projections, CachedThread { ..thread });
    Ok(())
}

fn walk_size(path: &Path) -> u64 {
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    let path = entry.path();
                    if path.is_dir() { walk_size(&path) } else { entry.metadata().map_or(0, |m| m.len()) }
                })
                .sum()
        })
        .unwrap_or(0)
}
