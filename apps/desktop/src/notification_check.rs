//! Fixture driver for notifications: feeds a scripted sequence of thread
//! changes, focus, route and settings changes through the real engine, inbox
//! store and model, and writes one JSON line per step with what was decided,
//! the inbox, the badge and the Done threads.
//!
//! Usage: notification-check <fixture.json> <out.jsonl> <darwin|linux> [--notify]
//! `--notify` also sends one real notification through the Linux backend.

#[allow(dead_code)]
mod notifications;

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};
use t3code_contracts::NotificationMode;
use t3code_web::components::sidebar::model::{
    EnvironmentId, ErrorClass, LatestRun, ProjectId, RunStatus, RuntimeStatus, SettledOverride, SidebarThread,
    SubagentRequest, ThreadId, ThreadKey, ThreadRuntime, TimestampMs,
};
use t3code_web::components::thread_notification_coordinator::{
    EnvironmentShell, GateReason, NotificationEffect, NotificationEngine, NotificationInput, NotificationThread,
    Outcome, QuietReason, ThreadLineage,
};
use t3code_web::components::thread_notification_coordinator_logic::{SnoozeState, ThreadNotificationKind};
use t3code_web::desktop_notifications::{DesktopNotificationCommand, format_iso, parse_iso};
use t3code_web::thread_notifications::NotificationSettings;

use notifications::store::InboxStore;

/// A thread as the fixture writes it. Times are milliseconds after `start`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureThread {
    id: String,
    title: String,
    project: String,
    #[serde(default)]
    lineage: Lineage,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    settled: bool,
    #[serde(default)]
    snoozed_until: Option<i64>,
    #[serde(default)]
    snoozed_at: Option<i64>,
    #[serde(default)]
    hand_raised: bool,
    #[serde(default)]
    approvals: bool,
    #[serde(default)]
    user_input: bool,
    #[serde(default)]
    subagent_request: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    guest_messages: u32,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    error_class: Option<String>,
    #[serde(default)]
    blocked_on_wait: bool,
    #[serde(default)]
    run: Option<FixtureRun>,
    #[serde(default)]
    active_run_id: Option<String>,
    #[serde(default)]
    stop_requested: bool,
    #[serde(default)]
    background_wake: bool,
    #[serde(default)]
    monitor_holds_wake: bool,
    #[serde(default)]
    last_visited_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Lineage {
    #[default]
    Root,
    Subagent,
    SideChat,
    Delegated,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureRun {
    id: String,
    status: String,
    #[serde(default)]
    completed_at: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureSettings {
    mode: NotificationMode,
    in_app: bool,
    app_icon_badge: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ThreadRef {
    env: String,
    thread: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Event {
    Settings(FixtureSettings),
    Environment { id: String, label: String, threads: Vec<FixtureThread> },
    Patch { env: String, thread: String, set: serde_json::Map<String, serde_json::Value> },
    Offline(String),
    Online(String),
    Remove(String),
    Focus(bool),
    Route(Option<ThreadRef>),
    Read(Vec<String>),
    ReadAll(bool),
}

#[derive(Debug, Deserialize)]
struct Step {
    label: String,
    #[serde(default)]
    advance: i64,
    events: Vec<Event>,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    start: String,
    steps: Vec<Step>,
}

fn run_status(status: &str) -> RunStatus {
    match status {
        "preparing" => RunStatus::Preparing,
        "queued" => RunStatus::Queued,
        "starting" => RunStatus::Starting,
        "running" => RunStatus::Running,
        "waiting" => RunStatus::Waiting,
        "completed" => RunStatus::Completed,
        "interrupted" => RunStatus::Interrupted,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        "rolled_back" => RunStatus::RolledBack,
        other => panic!("unknown run status {other}"),
    }
}

/// Builds the sync layer's input from a fixture thread.
fn thread_input(env: &str, thread: &FixtureThread, start: i64) -> Arc<NotificationThread> {
    let at = |offset: Option<i64>| offset.map(|offset| TimestampMs(start + offset));
    let key = ThreadKey { environment_id: EnvironmentId(env.to_string().into()), thread_id: ThreadId(thread.id.clone().into()) };
    let runtime = thread.runtime.as_deref().map(|status| ThreadRuntime {
        status: if status == "idle" { RuntimeStatus::Idle } else { RuntimeStatus::Run(run_status(status)) },
        last_error_class: thread.error_class.as_deref().map(|class| {
            if class == "usage_limit" { ErrorClass::UsageLimit } else { ErrorClass::Other }
        }),
    });
    let row = SidebarThread {
        key,
        project_id: ProjectId(thread.project.clone().into()),
        title: thread.title.clone().into(),
        branch: None,
        in_worktree: false,
        created_at: TimestampMs(start),
        updated_at: TimestampMs(start),
        latest_user_message_at: None,
        latest_user_authored_message_at: None,
        runtime,
        latest_run: thread.run.as_ref().map(|run| LatestRun {
            status: run_status(&run.status),
            requested_at: None,
            started_at: None,
            completed_at: at(run.completed_at),
        }),
        blocked_on_thread_wait: thread.blocked_on_wait,
        has_pending_approvals: thread.approvals,
        has_pending_user_input: thread.user_input,
        pending_subagent_request: thread.subagent_request.as_deref().map(|request| {
            if request == "approval" { SubagentRequest::Approval } else { SubagentRequest::UserInput }
        }),
        pending_guest_message_count: thread.guest_messages,
        goal_active: false,
        last_visited_at: at(thread.last_visited_at),
        has_unread_completion_notification: false,
        settled_override: thread.settled.then_some(SettledOverride::Settled),
        settled_at: None,
        unsettled_at: None,
        snoozed_until: at(thread.snoozed_until),
        woke_at: None,
        pinned_at: None,
        pin_order_key: None,
        active_order_key: None,
        archived: thread.archived,
        has_unsent_draft: false,
        diff: None,
        pull_request: None,
        issue_key: None,
        running_terminal_count: 0,
        remote_machine: false,
    };
    Arc::new(NotificationThread {
        row: Arc::new(row),
        project_title: thread.project.clone().into(),
        lineage: match thread.lineage {
            Lineage::Root => ThreadLineage::Root,
            Lineage::Subagent => ThreadLineage::Subagent,
            Lineage::SideChat => ThreadLineage::SideChat,
            Lineage::Delegated => ThreadLineage::DelegatedUnderParent,
        },
        latest_run_id: thread.run.as_ref().map(|run| run.id.clone().into()),
        active_run_id: thread.active_run_id.clone().map(Into::into),
        pending_request_id: thread.request_id.clone().map(Into::into),
        snooze: SnoozeState {
            snoozed_until: at(thread.snoozed_until),
            snoozed_at: at(thread.snoozed_at),
            hand_raised: thread.hand_raised,
        },
        stop_requested: thread.stop_requested,
        background_wake: thread.background_wake,
        monitor_holds_wake: thread.monitor_holds_wake,
    })
}

/// One environment as the fixture holds it.
struct FixtureEnvironment {
    id: String,
    label: String,
    threads: Vec<(FixtureThread, Arc<NotificationThread>)>,
    live: bool,
}

impl FixtureEnvironment {
    fn shell(&self) -> EnvironmentShell {
        EnvironmentShell {
            id: EnvironmentId(self.id.clone().into()),
            label: self.label.clone().into(),
            threads: self.live.then(|| self.threads.iter().map(|(_, input)| input.clone()).collect()),
        }
    }
}

/// What one step wrote.
#[derive(Default, Serialize)]
struct StepLog {
    step: usize,
    label: String,
    now: String,
    decisions: Vec<serde_json::Value>,
    system: Vec<String>,
    banners: Vec<String>,
    visits: Vec<String>,
    announce: Vec<String>,
    inbox: Vec<String>,
    badge: usize,
    done: Vec<String>,
}

fn scoped(key: &ThreadKey) -> String {
    format!("{}:{}", key.environment_id.0, key.thread_id.0)
}

fn reason_name(reason: GateReason) -> &'static str {
    match reason {
        GateReason::FirstSighting => "first-sighting",
        GateReason::ArchivedOrSettled => "archived-or-settled",
        GateReason::Snoozed => "snoozed",
        GateReason::StoppedOrWaiting => "stopped-or-waiting",
        GateReason::BackgroundWakeGrace => "background-wake-grace",
        GateReason::AlreadyNotified => "already-notified",
    }
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Toast => "toast",
        Outcome::System => "system",
        Outcome::Quiet(QuietReason::InboxPlatform) => "quiet:inbox-platform",
        Outcome::Quiet(QuietReason::Mode) => "quiet:mode",
        Outcome::Quiet(QuietReason::Focused) => "quiet:focused",
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, fixture, out, platform, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: notification-check <fixture.json> <out.jsonl> <darwin|linux> [--notify]");
    };
    if rest.iter().any(|arg| arg == "--notify") {
        return send_real_notification();
    }
    let inbox_platform = platform == "darwin";
    let fixture: Fixture = serde_json::from_slice(&std::fs::read(fixture)?)?;
    let start = parse_iso(&fixture.start).ok_or_else(|| anyhow::anyhow!("bad start"))?.0;
    let inbox_path = std::path::Path::new(out).with_extension(format!("{platform}.notifications.json"));
    let _ = std::fs::remove_file(&inbox_path);
    let mut store = InboxStore::load(inbox_path.clone())?;
    let mut engine = NotificationEngine::new(inbox_platform, NotificationSettings::default());
    let mut environments: Vec<FixtureEnvironment> = Vec::new();
    let mut now = start;
    let mut focused = false;
    let mut mode = NotificationMode::Off;
    let mut lines = Vec::new();
    let mut effects: VecDeque<NotificationEffect> = VecDeque::new();
    if inbox_platform {
        // The host hands the saved items over at launch (`getNotifications`).
        effects.extend(engine.apply(NotificationInput::InboxItems(store.items().into()), TimestampMs(now)));
    }
    for (index, step) in fixture.steps.iter().enumerate() {
        now += step.advance;
        let at = TimestampMs(now);
        let mut log = StepLog { step: index, label: step.label.clone(), now: format_iso(at).to_string(), ..Default::default() };
        effects.extend(engine.apply(NotificationInput::Tick, at));
        // The first pass carries out what the clock released, before any event.
        for event in std::iter::once(None).chain(step.events.iter().map(Some)) {
            let input = match event { None => None, Some(event) => match event {
                Event::Settings(settings) => {
                    mode = settings.mode;
                    Some(NotificationInput::Settings(NotificationSettings {
                        mode: settings.mode,
                        in_app: settings.in_app,
                        inbox_enabled: true,
                        app_icon_badge: settings.app_icon_badge,
                    }))
                }
                Event::Environment { id, label, threads } => {
                    let threads = threads.iter().map(|thread| (thread.clone(), thread_input(id, thread, start))).collect();
                    let env = FixtureEnvironment { id: id.clone(), label: label.clone(), threads, live: true };
                    let shell = env.shell();
                    match environments.iter_mut().find(|existing| existing.id == *id) {
                        Some(existing) => *existing = env,
                        None => environments.push(env),
                    }
                    Some(NotificationInput::Environment(shell))
                }
                Event::Patch { env, thread, set } => {
                    let environment = environments.iter_mut().find(|e| e.id == *env).expect("environment");
                    let index = environment.threads.iter().position(|(t, _)| t.id == *thread);
                    let mut value = match index {
                        Some(index) => serde_json::to_value(&environment.threads[index].0)?,
                        None => serde_json::json!({ "id": thread, "title": thread, "project": "" }),
                    };
                    for (field, patch) in set {
                        value[field] = patch.clone();
                    }
                    let patched: FixtureThread = serde_json::from_value(value)?;
                    let input = thread_input(env, &patched, start);
                    match index {
                        Some(index) => environment.threads[index] = (patched, input),
                        None => environment.threads.push((patched, input)),
                    }
                    Some(NotificationInput::Environment(environment.shell()))
                }
                Event::Offline(id) | Event::Online(id) => {
                    let environment = environments.iter_mut().find(|e| e.id == *id).expect("environment");
                    environment.live = matches!(event, Event::Online(_));
                    if environment.live {
                        // A fresh snapshot after reconnecting: new thread values.
                        for (fixture_thread, input) in &mut environment.threads {
                            *input = thread_input(id, fixture_thread, start);
                        }
                    }
                    Some(NotificationInput::Environment(environment.shell()))
                }
                Event::Remove(id) => {
                    environments.retain(|e| e.id != *id);
                    Some(NotificationInput::RemoveEnvironment(EnvironmentId(id.clone().into())))
                }
                Event::Focus(value) => {
                    focused = *value;
                    Some(NotificationInput::Focus(*value))
                }
                Event::Route(route) => Some(NotificationInput::Route(route.as_ref().map(|route| ThreadKey {
                    environment_id: EnvironmentId(route.env.clone().into()),
                    thread_id: ThreadId(route.thread.clone().into()),
                }))),
                Event::Read(ids) => {
                    effects.push_back(NotificationEffect::InboxRead(DesktopNotificationCommand::Read(
                        ids.iter().map(|id| SharedString::from(id.clone())).collect(),
                    )));
                    None
                }
                Event::ReadAll(_) => {
                    let ids = store.items().iter().filter(|item| !item.read).map(|item| item.id.clone()).collect();
                    effects.push_back(NotificationEffect::InboxRead(DesktopNotificationCommand::Read(ids)));
                    None
                }
            }};
            if let Some(input) = input {
                effects.extend(engine.apply(input, at));
            }
            // Carry out effects until the inbox settles, as the host and the UI would.
            while let Some(effect) = effects.pop_front() {
                let commit = match effect {
                    NotificationEffect::Suppressed { thread, reason } => {
                        log.decisions.push(serde_json::json!({
                            "thread": scoped(&thread), "result": "suppressed", "reason": reason_name(reason),
                        }));
                        None
                    }
                    NotificationEffect::Notified { thread, kind, title, sound, outcome } => {
                        log.decisions.push(serde_json::json!({
                            "thread": scoped(&thread), "result": "notified",
                            "kind": if kind == ThreadNotificationKind::Completion { "completion" } else { "input" },
                            "title": title.as_ref(), "sound": sound, "outcome": outcome_name(outcome),
                        }));
                        None
                    }
                    NotificationEffect::ShowSystem(notification) => {
                        log.system.push(format!("show {} {} | {}", notification.tag, notification.title, notification.body));
                        None
                    }
                    NotificationEffect::CloseSystem(tag) => {
                        log.system.push(format!("close {tag}"));
                        None
                    }
                    NotificationEffect::Visit { thread, at } => {
                        log.visits.push(format!("{}@{}", scoped(&thread), format_iso(at)));
                        None
                    }
                    NotificationEffect::Announce(text) => {
                        log.announce.push(text);
                        None
                    }
                    NotificationEffect::InboxSync(snapshot) => {
                        let commit = store.sync(&snapshot, &format_iso(at))?;
                        let notifications = matches!(
                            mode,
                            NotificationMode::Notifications | NotificationMode::NotificationsAndSound
                        );
                        if notifications && !focused {
                            log.banners.extend(commit.added.iter().map(|item| item.id.to_string()));
                        }
                        Some(commit)
                    }
                    NotificationEffect::InboxRead(command) => Some(store.update(&command)?),
                    NotificationEffect::Sound(_)
                    | NotificationEffect::Toast { .. }
                    | NotificationEffect::CloseToasts(_)
                    | NotificationEffect::Badge(_) => None,
                };
                if let Some(commit) = commit.filter(|commit| commit.changed) {
                    let _ = commit;
                    effects.extend(engine.apply(NotificationInput::InboxItems(store.items().into()), at));
                }
            }
        }
        log.inbox = store
            .items()
            .iter()
            .map(|item| {
                format!(
                    "{} {:?} {}:{} {}{}",
                    item.id,
                    item.kind,
                    item.environment_id,
                    item.thread_id,
                    if item.read { "read" } else { "unread" },
                    if item.resolved { " resolved" } else { "" }
                )
                .to_lowercase()
            })
            .collect();
        log.badge = engine.badge();
        let mut done: Vec<String> = engine
            .items()
            .iter()
            .filter(|item| item.kind == t3code_web::desktop_notifications::DesktopNotificationKind::Completed && !item.read)
            .map(|item| format!("{}:{}", item.environment_id, item.thread_id))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        done.dedup();
        log.done = done;
        lines.push(serde_json::to_string(&log)?);
    }
    std::fs::write(out, lines.join("\n") + "\n")?;
    // The saved file must load back to the same state.
    let reloaded = InboxStore::load(store.path().to_path_buf())?;
    anyhow::ensure!(reloaded.state() == store.state(), "notifications.json did not round-trip");
    let observed: BTreeMap<_, _> = store.state().observed.iter().collect();
    eprintln!(
        "{} steps, {} inbox items, {} observed threads, saved to {}",
        fixture.steps.len(),
        store.items().len(),
        observed.len(),
        inbox_path.display()
    );
    Ok(())
}

/// Sends one notification and one badge update through the Linux backend,
/// then waits two seconds for a click or a failure.
fn send_real_notification() -> anyhow::Result<()> {
    let (activations, mut clicks) = futures::channel::mpsc::unbounded();
    let backend = notifications::backend(activations).ok_or_else(|| anyhow::anyhow!("no notification backend"))?;
    backend.show(notifications::NativeNotification {
        id: "check".into(),
        title: "Thread completed".into(),
        lines: vec!["Fix login bug".into()],
        activation: notifications::Activation::Thread(ThreadKey {
            environment_id: EnvironmentId("env-a".into()),
            thread_id: ThreadId("t1".into()),
        }),
    });
    backend.set_badge(1);
    // A click (the `default` action) comes back as an activation.
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(Some(activation)) = clicks.try_next() {
            println!("clicked: {activation:?}");
            return Ok(());
        }
    }
    eprintln!("sent through the backend; no click within two seconds; failures, if any, are printed above");
    Ok(())
}
