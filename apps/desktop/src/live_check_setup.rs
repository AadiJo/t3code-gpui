// live-check `setup`: the worktree setup row against the server's own setup stream. The project
// at `T3_PROJECT_ROOT` gets a setup script that prints and sleeps; a thread launches into a new
// worktree, and while the script runs the row's phase, stages (state, detail) and output tail
// equal a separate `subscribeWorktreeSetup` read. Cancel (the card's button action) ends it: the
// row turns "Worktree setup cancelled" with the server's cancelled phase. A second launch without
// the script finishes ("Worktree ready" or folds away as the web does).

use super::*;
use t3code_contracts::rpc::methods::SubscribeWorktreeSetup;
use t3code_contracts::{
    OrchestrationV2ThreadLaunchInput, OrchestrationV2ThreadLaunchInputInitialMessage, OrchestrationV2ThreadLaunchWorkspaceStrategy,
    OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree, ProjectMutationProjectUpdate, WorktreeSetupSnapshot, WorktreeSetupSubscribeInput,
};
use t3code_web::components::chat::messages_timeline::TimelineAction;
use t3code_web::components::chat::messages_timeline_logic::RowKind;
use t3code_web::components::chat::timeline_entries::{SetupPhase, SetupStageStatus, WorktreeSetup};

/// The server's current setup snapshot for a thread, from its own subscription.
fn server_setup(check: &mut Check, environment_id: &t3code_contracts::EnvironmentId, thread_id: &ThreadId) -> anyhow::Result<Option<WorktreeSetupSnapshot>> {
    use futures::StreamExt as _;
    let session = check
        .cx
        .update(|cx| StateBridge::environment(environment_id, cx).and_then(|env| env.read(cx).store.context().supervisor.session.borrow().clone()))
        .context("session")?;
    let thread_id = thread_id.clone();
    check.block(async move {
        let mut stream = session.client.subscribe::<SubscribeWorktreeSetup>(WorktreeSetupSubscribeInput { thread_id });
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(Ok(items))) => items.into_iter().last().flatten(),
            _ => None,
        }
    })
}

/// The row's data as a comparable line, and the server snapshot's.
fn describe_row(setup: &WorktreeSetup) -> String {
    let stages: Vec<String> = setup.stages.iter().map(|s| format!("{:?}:{:?}:{}", s.id, s.status, s.detail.as_deref().unwrap_or(""))).collect();
    format!("{:?} [{}] tail {}", setup.phase, stages.join(", "), setup.stages.iter().map(|s| s.tail.len()).sum::<usize>())
}

fn describe_server(snapshot: &WorktreeSetupSnapshot) -> String {
    use t3code_contracts::{WorktreeSetupPhase as P, WorktreeSetupStageId as I, WorktreeSetupStageStatus as S};
    let phase = match snapshot.phase {
        P::Running => "Running",
        P::Done => "Done",
        P::Failed => "Failed",
        _ => "Cancelled",
    };
    let stages: Vec<String> = snapshot
        .stages
        .iter()
        .map(|s| {
            let id = match s.id {
                I::Fetch => "Fetch",
                I::Checkout => "Checkout",
                I::Submodules => "Submodules",
                I::SetupScript => "SetupScript",
                _ => "Agent",
            };
            let status = match s.status {
                S::Pending => "Pending",
                S::Running => "Running",
                S::Done => "Done",
                S::Skipped => "Skipped",
                S::Warning => "Warning",
                _ => "Failed",
            };
            format!("{id}:{status}:{}", s.detail.as_deref().unwrap_or(""))
        })
        .collect();
    format!("{phase} [{}] tail {}", stages.join(", "), snapshot.stages.iter().map(|s| s.tail.len()).sum::<usize>())
}

fn shown_setup_row(check: &mut Check) -> Option<WorktreeSetup> {
    check.read(|root, cx| {
        let timeline = root.thread_view().read(cx).chat().read(cx).timeline().read(cx);
        timeline.rows().iter().find_map(|row| match &row.kind {
            RowKind::WorktreeSetup(setup) => Some(setup.clone()),
            _ => None,
        })
    })
}

fn launch_worktree(check: &mut Check, commands: &EnvironmentCommands, project: &ProjectId, title: &str) -> anyhow::Result<ThreadId> {
    let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let input = OrchestrationV2ThreadLaunchInput {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        creation_source: None,
        thread_id: Some(thread_id.clone()),
        reuse_existing_thread: None,
        project_id: project.clone(),
        title: title.into(),
        generate_title: Some(false),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        workspace_strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(OrchestrationV2ThreadLaunchWorkspaceStrategyWorktree {
            base_ref: "main".into(),
            branch: Some(format!("setup-{}", now_ms() % 1_000_000)),
            start_from_origin: Some(false),
        }),
        initial_message: Some(OrchestrationV2ThreadLaunchInputInitialMessage { message_id: None, text: "Start.".into(), context: None, attachments: Vec::new() }),
    };
    let c = commands.clone();
    check.block(async move { c.launch_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(thread_id)
}

fn set_scripts(check: &mut Check, commands: &EnvironmentCommands, project: &ProjectId, scripts: serde_json::Value) -> anyhow::Result<()> {
    let mutation = ProjectMutation::ProjectUpdate(ProjectMutationProjectUpdate {
        command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
        project_id: project.clone(),
        scripts: Some(serde_json::from_value(scripts)?),
        ..serde_json::from_value(serde_json::json!({ "type": "project.update", "commandId": "x", "projectId": "x" }))?
    });
    let c = commands.clone();
    check.block(async move { c.mutate_project(mutation).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

pub(super) fn run_setup(check: &mut Check) -> anyhow::Result<()> {
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let environment_id = check
        .cx
        .update(|cx| ConnectionRuntime::state(cx).read(cx).environments.first().map(|e| t3code_contracts::EnvironmentId::new(e.entry.environment_id.as_str())))
        .context("an environment")?;
    let root = std::env::var("T3_PROJECT_ROOT").context("T3_PROJECT_ROOT")?;
    let commands = commands_of(check, &environment_id)?;
    let project = check
        .cx
        .update(|cx| {
            let shell = StateBridge::environment(&environment_id, cx)?.read(cx).shell.clone();
            shell.snapshot.as_ref()?.projects.iter().find(|p| p.workspace_root == root).map(|p| p.id.clone())
        })
        .context("project at T3_PROJECT_ROOT")?;
    set_scripts(
        check,
        &commands,
        &project,
        serde_json::json!([{ "id": "install", "name": "Install deps", "command": "for i in 1 2 3 4 5 6; do echo step $i; sleep 1; done; sleep 60", "icon": "configure", "runOnWorktreeCreate": true, "async": false }]),
    )?;
    check.note("project setup script set");

    // A running setup, then Cancel.
    let thread_id = launch_worktree(check, &commands, &project, "Setup to cancel")?;
    let key = thread_key(&environment_id, &thread_id);
    let k = key.clone();
    check.pump(move |c| sidebar_row(c, &k).is_some(), Duration::from_secs(20), "launched thread listed")?;
    check.go(&key);
    check.pump(
        |c| shown_setup_row(c).is_some_and(|s| s.phase == SetupPhase::Running && s.stages.iter().any(|st| st.status == SetupStageStatus::Running && !st.tail.is_empty())),
        Duration::from_secs(60),
        "the setup script runs with output",
    )?;
    check.wait(Duration::from_millis(1500));
    let row = shown_setup_row(check).context("setup row")?;
    let server = server_setup(check, &environment_id, &thread_id)?.context("server snapshot")?;
    // The tail grows each second; compare the rest, then the tail's last line.
    let strip = |line: String| line.split(" tail ").next().unwrap_or_default().to_owned();
    check.check("running: the row's phase and stages are the server's", strip(describe_row(&row)) == strip(describe_server(&server)), format!("row {} | server {}", describe_row(&row), describe_server(&server)));
    check.shot("setup-00-running.png")?;

    let timeline = check.read(|root, cx| root.thread_view().read(cx).chat().read(cx).timeline().clone());
    check.cx.update(|cx| timeline.update(cx, |_, cx| cx.emit(TimelineAction::CancelWorktreeSetup)));
    check.pump(|c| shown_setup_row(c).is_some_and(|s| s.phase == SetupPhase::Cancelled), Duration::from_secs(30), "the row turns cancelled")?;
    let row = shown_setup_row(check).context("setup row")?;
    let server = server_setup(check, &environment_id, &thread_id)?.context("server snapshot")?;
    check.check("cancelled: the row is the server's cancelled setup", describe_row(&row) == describe_server(&server), format!("row {} | server {}", describe_row(&row), describe_server(&server)));
    check.check("cancelled is its own phase, not a failed stage state", row.phase == SetupPhase::Cancelled, format!("{:?}", row.phase));
    check.shot("setup-01-cancelled.png")?;

    // A setup without a script finishes.
    set_scripts(check, &commands, &project, serde_json::json!([]))?;
    let done_id = launch_worktree(check, &commands, &project, "Setup that finishes")?;
    let done_key = thread_key(&environment_id, &done_id);
    let k = done_key.clone();
    check.pump(move |c| sidebar_row(c, &k).is_some(), Duration::from_secs(20), "second thread listed")?;
    check.go(&done_key);
    check.pump(
        |c| shown_setup_row(c).is_none_or(|s| s.phase != SetupPhase::Running),
        Duration::from_secs(60),
        "the second setup ends",
    )?;
    check.wait(Duration::from_secs(2));
    let row = shown_setup_row(check);
    let server = server_setup(check, &environment_id, &done_id)?;
    check.check(
        "finished: the row shows or folds as the web does for the server's snapshot",
        match (&row, &server) {
            (Some(row), Some(server)) => describe_row(row) == describe_server(server),
            (None, Some(_)) => true,
            _ => false,
        },
        format!("row {:?} | server {:?}", row.as_ref().map(describe_row), server.as_ref().map(describe_server)),
    );
    check.shot("setup-02-finished.png")
}

/// `draft-banner`: a draft whose launch the server refuses (a provider instance it does not have)
/// stays a draft, as the web keeps it: the route returns to the draft, the banner shows the
/// server's message there and the prompt is back in its composer.
pub(super) fn run_draft_banner(check: &mut Check) -> anyhow::Result<()> {
    check.pump(|c| c.connected(), Duration::from_secs(30), "connected")?;
    let router = check.read(|root, _| root.router().clone());
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(Route::Index, cx)));
    check.pump(|c| matches!(c.route(), Route::Draft(_)), Duration::from_secs(10), "a draft opens")?;
    let Route::Draft(draft) = check.route() else { bail!("no draft") };
    check.pump(|c| c.read(|root, cx| root.thread_view().read(cx).branch_strip().is_some()), Duration::from_secs(10), "draft shown")?;
    let composer = composer_of(check);
    let prompt = "This launch is refused by the server.";
    check.cx.update(|cx| {
        composer.update(cx, |_, cx| {
            cx.emit(t3code_web::components::chat::composer::ComposerEvent::Submit {
                prompt: prompt.into(),
                attachments: Vec::new(),
                model: t3code_web::model_selection::ModelSelection {
                    instance_id: t3code_web::model_selection::ProviderInstanceId("no-such-instance".into()),
                    model: "nothing".into(),
                    effort: None,
                    speed: None,
                    ultracode: false,
                },
                intent: t3code_web::composer_logic::ComposerSubmissionIntent::Foreground,
                dispatch: None,
            })
        })
    });
    let d = draft.clone();
    check.pump(move |c| c.route() == Route::Draft(d.clone()) && banner_input(c).is_some(), Duration::from_secs(20), "the refused launch lands back on the draft")?;
    let shown = banner_input(check);
    let composer = composer_of(check);
    let restored = check.cx.update(|cx| composer.read(cx).editor().read(cx).text().to_owned());
    let route = check.route();
    check.check("the draft route is back", route == Route::Draft(draft.clone()), format!("{route:?}"));
    check.check("the draft's banner shows the server's message", shown.as_ref().is_some_and(|b| !b.error.is_empty()), format!("{:?}", shown.map(|b| b.error)));
    check.check("the refused prompt is back in the draft's composer", restored.trim() == prompt, format!("{restored:?}"));
    check.shot("draft-banner-00.png")
}

