//! The world the owner's Mac reference captures were taken in (`artifacts/macparity/ref`): six
//! projects (three checkouts of `example/t3code`), ten threads, and every thread's real
//! projection as the reference server held it. `reference_world.json` is extracted from that
//! server's database by `scripts/parity/mac_world.py`; each timeline goes through the live
//! mapper (`session_logic::timeline_input`), so the fixture shows what a live server would.
//! What only the browser held (read state, git remotes) is set here.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::SharedString;
use serde::Deserialize;
use t3code_client_runtime::state::models::parse_ms;
use t3code_contracts::{OrchestrationV2RunStatus as WireRun, OrchestrationV2RuntimeRequestStatus as WireRequest, TurnItemId};
use t3code_shared::thread_projection::{ThreadProjection, VisibleTurnItem};
use t3code_shared::turn_item::TurnItemExt;

use crate::components::chat::timeline_entries::{PendingQuestion, TimelineInput};
use crate::components::sidebar::model::*;
use crate::components::thread_hover_card::ThreadHoverDetails;
use crate::session_logic::{TimelineMapper, pending_questions, timeline_input};

const WORLD: &str = include_str!("reference_world.json");

/// The environment label the captures' hover cards and details card show. The real one is a
/// machine name, which stays out of the repo; the Mac comparison excludes the label.
pub const ENVIRONMENT_LABEL: &str = "Scenario";

/// Threads the capture browser had opened since their last run (titles drawn at weight 400).
const READ: [&str; 5] =
    ["Sync engine audit", "Port sidebar virtualization to GPUI", "Rotate relay TLS certificates", "Migrate CI runners to arm64", "Profile the event sink"];

/// The thread every reference view opens on.
pub const ACTIVE_TITLE: &str = "Sync engine audit";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct World {
    projects: Vec<Project>,
    threads: Vec<Thread>,
    workspace: Checkout,
}

/// The "t3code" checkout as the captures saw it: its files and its uncommitted changes.
#[derive(Deserialize)]
pub struct Checkout {
    files: std::collections::BTreeMap<String, CheckoutFile>,
    /// `git diff` of the working tree, untracked files included.
    pub patch: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum CheckoutFile {
    Text(String),
    Base64(String),
}

impl Checkout {
    /// Writes the checkout's files under `root` (once) and returns `root`.
    pub fn write(&self, root: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
        use base64::Engine as _;
        for (path, file) in &self.files {
            let full = root.join(path);
            if full.exists() {
                continue;
            }
            std::fs::create_dir_all(full.parent().unwrap_or(root))?;
            match file {
                CheckoutFile::Text(text) => std::fs::write(&full, text)?,
                CheckoutFile::Base64(data) => {
                    std::fs::write(&full, base64::engine::general_purpose::STANDARD.decode(data).unwrap_or_default())?
                }
            }
        }
        Ok(root.to_path_buf())
    }

    /// Per-file `(path, insertions, deletions)` of the patch, in patch order.
    pub fn file_stats(&self) -> Vec<(String, u64, u64)> {
        let mut stats: Vec<(String, u64, u64)> = Vec::new();
        for line in self.patch.lines() {
            if let Some(rest) = line.strip_prefix("diff --git a/") {
                stats.push((rest.split(" b/").next().unwrap_or(rest).to_owned(), 0, 0));
            } else if let Some(last) = stats.last_mut() {
                if line.starts_with('+') && !line.starts_with("+++") {
                    last.1 += 1;
                } else if line.starts_with('-') && !line.starts_with("---") {
                    last.2 += 1;
                }
            }
        }
        stats
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    title: String,
    workspace_root: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    project_id: String,
    projection: StoredProjection,
}

/// A projection whose `visibleTurnItems` are stored as `(position, turn item id)` pairs.
#[derive(Deserialize)]
struct StoredProjection {
    positions: Vec<(u64, TurnItemId)>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// One reference thread: its sidebar row and what its chat view shows.
pub struct ReferenceThread {
    pub row: SidebarThread,
    pub project_title: SharedString,
    pub cwd: SharedString,
    pub timeline: TimelineInput,
    pub questions: Arc<[PendingQuestion]>,
    /// A run is in progress (the composer offers Stop).
    pub working: bool,
    /// The model of the latest run (a Replay scenario slug).
    pub model: SharedString,
    /// `approval-required` threads run supervised.
    pub supervised: bool,
}

/// The reference world: the sidebar snapshot and every thread's content.
pub struct ReferenceWorld {
    pub snapshot: SidebarSnapshot,
    pub threads: Vec<ReferenceThread>,
    pub checkout: Arc<Checkout>,
}

fn ms(iso: &str) -> TimestampMs {
    TimestampMs(parse_ms(iso).unwrap_or_default())
}

/// `github.com/example/<repo>` for the projects the seed gave an origin remote, with the
/// project's path inside the repository and the label repository grouping shows.
fn repository(title: &str) -> (Option<&'static str>, Option<&'static str>, Option<&'static str>) {
    match title {
        "t3code" | "t3code-pr" => (Some("github.com/example/t3code"), None, Some("example/t3code")),
        "server" => (Some("github.com/example/t3code"), Some("apps/server"), Some("example/t3code")),
        "relay" => (Some("github.com/example/relay"), None, None),
        "marketing-site" => (Some("github.com/example/marketing-site"), None, None),
        _ => (None, None, None),
    }
}

fn run_status(status: WireRun) -> RunStatus {
    match status {
        WireRun::Preparing => RunStatus::Preparing,
        WireRun::Queued => RunStatus::Queued,
        WireRun::Starting => RunStatus::Starting,
        WireRun::Running => RunStatus::Running,
        WireRun::Waiting => RunStatus::Waiting,
        WireRun::Completed => RunStatus::Completed,
        WireRun::Interrupted => RunStatus::Interrupted,
        WireRun::Failed => RunStatus::Failed,
        WireRun::Cancelled => RunStatus::Cancelled,
        WireRun::RolledBack => RunStatus::RolledBack,
        _ => RunStatus::Completed,
    }
}

fn projection(stored: StoredProjection) -> ThreadProjection {
    let mut value = serde_json::Value::Object(stored.rest);
    value["visibleTurnItems"] = serde_json::Value::Array(Vec::new());
    let mut projection: ThreadProjection = serde_json::from_value(value).expect("reference_world.json decodes as a thread projection");
    let by_id: HashMap<TurnItemId, Arc<_>> = projection.turn_items.iter().map(|item| (item.item_id(), item.clone())).collect();
    let thread_id = projection.thread.id.clone();
    projection.visible_turn_items = Arc::new(
        stored
            .positions
            .into_iter()
            .filter_map(|(position, id)| {
                Some(VisibleTurnItem {
                    position,
                    visibility: t3code_contracts::OrchestrationV2ThreadProjectionVisibleTurnItemsItemVisibility::Local,
                    source_thread_id: thread_id.clone(),
                    item: by_id.get(&id)?.clone(),
                    source_item_id: id,
                })
            })
            .collect(),
    );
    projection
}

/// Decodes and projects the reference world in the environment `env`.
pub fn build(env: &EnvironmentId) -> ReferenceWorld {
    let world: World = serde_json::from_str(WORLD).expect("reference_world.json parses");
    let projects: Vec<SidebarProject> = world
        .projects
        .iter()
        .map(|p| {
            let (key, relative, label) = repository(&p.title);
            SidebarProject {
                project_ref: ProjectRef { environment_id: env.clone(), project_id: ProjectId(p.id.clone().into()) },
                title: p.title.clone().into(),
                workspace_root: p.workspace_root.clone().into(),
                repository_key: key.map(SharedString::from),
                repository_relative_path: relative.map(SharedString::from),
                // The group's name comes from `group_label`, as the live layer gives it.
                repository_display_name: None,
                repository_name: None,
                has_custom_title: false,
                created_at: ms(&p.created_at),
                updated_at: ms(&p.updated_at),
                pinned: false,
                archived: false,
                group_label: label.map(SharedString::from),
                icon: None,
                favicon: None,
            }
        })
        .collect();
    let replay = SidebarProvider { mark: SidebarProviderMark::AcpRegistry, initials: "RR".into(), show_badge: true };
    let mut mapper = TimelineMapper::default();
    let mut hover_details = HashMap::new();
    let threads: Vec<ReferenceThread> = world
        .threads
        .into_iter()
        .map(|thread| {
            let projection = projection(thread.projection);
            let app = &projection.thread;
            let key = ThreadKey { environment_id: env.clone(), thread_id: ThreadId(app.id.to_string().into()) };
            let latest = projection.runs.iter().max_by_key(|run| run.ordinal);
            let status = latest.map(|run| run_status(run.status)).unwrap_or(RunStatus::Completed);
            let requested = latest.map(|run| ms(&run.requested_at)).unwrap_or(ms(&app.created_at));
            let completed = latest.and_then(|run| run.completed_at.as_deref()).map(ms);
            let last_user = projection
                .messages
                .iter()
                .filter(|m| m.role == t3code_contracts::OrchestrationV2ConversationMessageRole::User)
                .map(|m| ms(&m.created_at))
                .max();
            let updated = completed.unwrap_or(requested);
            let pending = projection.runtime_requests.iter().filter(|r| r.status == WireRequest::Pending);
            let (mut approvals, mut input) = (false, false);
            for request in pending {
                match request.kind {
                    t3code_contracts::OrchestrationV2RuntimeRequestKind::UserInput => input = true,
                    _ => approvals = true,
                }
            }
            let title: SharedString = app.title.to_string().into();
            let read = READ.contains(&title.as_ref());
            let working = completed.is_none();
            let row = SidebarThread {
                key: key.clone(),
                project_id: ProjectId(thread.project_id.clone().into()),
                title: title.clone(),
                branch: app.branch.as_ref().map(|b| b.to_string().into()),
                in_worktree: app.worktree_path.is_some(),
                created_at: ms(&app.created_at),
                updated_at: updated,
                latest_user_message_at: last_user,
                latest_user_authored_message_at: last_user,
                runtime: Some(ThreadRuntime { status: RuntimeStatus::Run(status), last_error_class: None }),
                latest_run: Some(LatestRun { status, requested_at: Some(requested), started_at: Some(requested), completed_at: completed }),
                has_pending_approvals: approvals,
                has_pending_user_input: input,
                last_visited_at: Some(if read { updated.minus(-1) } else { requested.minus(TimestampMs::MINUTE) }),
                provider: Some(replay.clone()),
                ..super::blank_thread(&key)
            };
            let model = projection.runs.iter().max_by_key(|run| run.ordinal).map(|run| run.model_selection.model.to_string());
            let slug: SharedString = model.clone().unwrap_or_default().into();
            let supervised = app.runtime_mode == t3code_contracts::RuntimeMode::ApprovalRequired;
            hover_details.insert(
                key.clone(),
                ThreadHoverDetails {
                    environment_label: Some(ENVIRONMENT_LABEL.into()),
                    environment_machine: Some(t3code_contracts::EnvironmentMachineKind::Linux),
                    model_label: model.map(|m| format!("{} · Replay (rich)", scenario_name(&m)).into()),
                    ..Default::default()
                },
            );
            let project = projects.iter().find(|p| p.project_ref.project_id.0.as_ref() == thread.project_id);
            let questions: Arc<[PendingQuestion]> = pending_questions(&projection).into_iter().map(|(q, _)| q).collect();
            ReferenceThread {
                row,
                project_title: project.map(|p| p.title.clone()).unwrap_or_default(),
                cwd: project.map(|p| p.workspace_root.clone()).unwrap_or_default(),
                timeline: timeline_input(&mut mapper, &projection, &[]),
                questions,
                working,
                model: slug,
                supervised,
            }
        })
        .collect();
    let active = threads.iter().find(|t| t.row.title.as_ref() == ACTIVE_TITLE).map(|t| t.row.key.clone());
    let snapshot = SidebarSnapshot {
        projects,
        threads: threads.iter().map(|t| Arc::new(t.row.clone())).collect(),
        notifications: Vec::new(),
        settings: SidebarSettings::default(),
        active_thread: active,
        environment_pill: None,
        now: threads.iter().map(|t| t.row.updated_at).max().unwrap_or_default(),
        auto_settle_disabled: Default::default(),
        hover_details,
        search_content_matches: None,
        snooze_presets: Default::default(),
        drafts: Vec::new(),
        active_draft: None,
    };
    ReferenceWorld { snapshot, threads, checkout: Arc::new(world.workspace) }
}

/// The Replay agent's display name for a scenario model id (`rich-replay-agent.ts`).
pub fn scenario_name(model: &str) -> &str {
    match model {
        "showcase" => "Showcase",
        "todo-list" => "Plan and reads",
        "approval" => "Write approval",
        "question" => "Question",
        "multi-turn" => "Multi turn",
        "command-output" => "Command output",
        "working" => "Keeps working",
        "failing" => "Fails",
        other => other,
    }
}

/// The reference server's model catalog: the two Replay instances the composer's picker lists,
/// "Replay (rich)" carrying the scenario models.
pub fn model_catalog() -> crate::model_selection::ModelCatalog {
    use crate::model_selection::{ModelCatalog, ModelOption, ProviderDriverKind, ProviderInstanceEntry, ProviderInstanceId};
    let rich = ProviderInstanceId("replayRich".into());
    let dev = ProviderInstanceId("replay".into());
    let entry = |id: &ProviderInstanceId, name: &str| ProviderInstanceEntry {
        instance_id: id.clone(),
        driver_kind: ProviderDriverKind::Other("acpRegistry".into()),
        display_name: name.into(),
        ready: true,
        unavailable_reason: None,
    };
    let models = ["showcase", "todo-list", "approval", "question", "multi-turn", "command-output", "working", "failing"]
        .into_iter()
        .map(|slug| ModelOption {
            slug: slug.into(),
            name: scenario_name(slug).to_owned().into(),
            short_name: None,
            sub_provider: None,
            is_legacy: false,
            is_unavailable: false,
            effort: None,
            speed: None,
            ultracode: false,
        })
        .collect();
    ModelCatalog {
        instances: vec![entry(&dev, "Replay (dev)"), entry(&rich, "Replay (rich)")],
        models: HashMap::from([(rich, models), (dev, Vec::new())]),
        favorites: Default::default(),
        hidden: Default::default(),
    }
}
