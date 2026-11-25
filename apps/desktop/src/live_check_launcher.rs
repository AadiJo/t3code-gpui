//! `live-check launcher`: the right panel's surface launcher on server data. Makes a thread with
//! a turn and the linked pull requests of `linked-items`, and a thread with a turn and nothing
//! linked (`launcher-state.json`). On the first, "Pull request" opens the tab of the pull request
//! the server's links resolve to and "Side chat" starts a side chat the server records under the
//! thread; on the second, "Pull request" is unavailable and opens nothing. `T3_PAIRING_URL`,
//! `T3_PROJECT_ROOT`.

use std::time::Duration;

use t3code_client_runtime::operations::{CreateThread, SendMessage};
use t3code_client_runtime::state::thread_pull_requests::thread_pull_request_panel_target;
use t3code_contracts::{ModelSelection, ProviderInteractionMode, RuntimeMode, ThreadId};
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::right_panel_store::RightPanelSurface;
use t3code_web::state::StateBridge;
use t3code_web::state::app_data::SideChatLaunch;
use t3code_web::state::shell::thread_key;

use super::live_check_families::{all_connected, pair_if_needed, project_at};
use super::live_check_update::{click, element_visible};
use super::{Check, commands_of, shell_thread, side_chat_tab, tid};

const STATE_FILE: &str = "launcher-state.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct LauncherState {
    linked_thread: String,
    empty_thread: String,
}

/// Shows `key`'s right panel with no tabs: the surface launcher.
fn show_launcher(check: &mut Check, key: &ThreadKey) {
    let panel_key = t3code_web::fixtures::scoped_key(key);
    let (store, panel) = check.read(|root, _| (root.right_panels().clone(), root.right_panel().clone()));
    let ids: Vec<String> = check.cx.update(|cx| store.read(cx).state(&panel_key).surfaces.iter().map(|s| s.id().to_owned()).collect());
    for id in ids {
        check.cx.update(|cx| panel.update(cx, |panel, cx| panel.close_surface(&id, cx)));
    }
    // Closing the last tab may close the panel; the titlebar toggle opens it on the launcher.
    for _ in 0..5 {
        check.wait(Duration::from_millis(300));
        if element_visible(check, "launch-browser") {
            break;
        }
        // As the titlebar toggle does: show the panel, then the window follows the store.
        let (store, panel, panel_key) = (store.clone(), panel.clone(), panel_key.clone());
        check.cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.show(&panel_key);
                cx.notify();
            });
            panel.update(cx, |_, cx| cx.emit(t3code_web::components::right_panel_tabs::RightPanelEvent::Changed));
        });
    }
}

/// The active tab of `key`'s right panel.
fn active_surface(check: &mut Check, key: &ThreadKey) -> Option<RightPanelSurface> {
    let panel_key = t3code_web::fixtures::scoped_key(key);
    check.read(|root, cx| root.right_panels().read(cx).state(&panel_key).active().cloned())
}

fn settled(check: &mut Check, key: &ThreadKey) -> bool {
    check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(key).is_some_and(|row| row.latest_run.as_ref().is_some_and(|r| r.completed_at.is_some())))
}

pub(super) fn run_launcher(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check
        .cx
        .update(|cx| t3code_web::connection::ConnectionRuntime::state(cx).read(cx).environments.first().map(|e| t3code_contracts::EnvironmentId::new(e.entry.environment_id.as_str())))
        .ok_or_else(|| anyhow::anyhow!("no environment"))?;
    let path = check.out.join(STATE_FILE);
    let state = match std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<LauncherState>(&bytes).ok()) {
        Some(state) => state,
        None => {
            let project = project_at(check, &environment_id, &std::env::var("T3_PROJECT_ROOT")?)?;
            let commands = commands_of(check, &environment_id)?;
            let mut ids = Vec::new();
            for title in ["Launcher", "No pull request"] {
                let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
                let input = CreateThread {
                    thread_id: thread_id.clone(),
                    project_id: project.clone(),
                    title: title.into(),
                    model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
                    runtime_mode: RuntimeMode::FullAccess,
                    interaction_mode: ProviderInteractionMode::Default,
                    branch: None,
                    worktree_path: None,
                };
                let c = commands.clone();
                check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
                let message = SendMessage::text(thread_id.clone(), "Start the review.");
                let c = commands.clone();
                check.block(async move { c.send_message(message).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
                ids.push(thread_id.to_string());
            }
            for command in super::live_check_linked::link_commands(&ids[0])? {
                let c = commands.clone();
                check.block(async move { c.dispatch(command).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            let state = LauncherState { linked_thread: ids[0].clone(), empty_thread: ids[1].clone() };
            std::fs::write(&path, serde_json::to_vec_pretty(&state)?)?;
            state
        }
    };
    let linked = thread_key(&environment_id, &ThreadId::new(state.linked_thread.as_str()));
    let empty = thread_key(&environment_id, &ThreadId::new(state.empty_thread.as_str()));
    for key in [&linked, &empty] {
        let k = key.clone();
        check.pump(move |c| settled(c, &k), Duration::from_secs(60), "the thread's first turn settles")?;
    }
    let k = linked.clone();
    check.pump(
        move |c| shell_thread(c, &k).and_then(|t| t.pull_requests.clone()).is_some_and(|links| links.iter().filter(|l| l.snapshot.is_some()).count() >= 4),
        Duration::from_secs(30),
        "the server holds the links and their host state",
    )?;

    // `threadPullRequestPanelTarget` on the server's row: of four open links, #7 is linked last,
    // so its one-link chain orders first.
    let shell = shell_thread(check, &linked).ok_or_else(|| anyhow::anyhow!("no shell row"))?;
    let target = thread_pull_request_panel_target(&shell);
    check.check(
        "the server's links resolve to scenario/orbit-web#7 as the thread's pull request",
        target.as_ref().is_some_and(|t| t.repository == "scenario/orbit-web" && t.number == 7),
        format!("{target:?}"),
    );

    check.go(&linked);
    check.frame();
    show_launcher(check, &linked);
    let shown = check.pump(|c| element_visible(c, "launch-browser"), Duration::from_secs(5), "the launcher shows");
    check.note(format!("launcher shown: {shown:?}"));
    let (pull_request, side_chat) = (element_visible(check, "launch-pull-request"), element_visible(check, "launch-side-chat"));
    check.check("the launcher lists Pull request", pull_request, "");
    check.check("the launcher lists Side chat", side_chat, "");
    check.shot("launcher-00-rows.png")?;

    click(check, "launch-pull-request")?;
    let k = linked.clone();
    check.pump(move |c| matches!(active_surface(c, &k), Some(RightPanelSurface::PullRequest { .. })), Duration::from_secs(10), "Pull request opens a tab")?;
    let opened = active_surface(check, &linked);
    check.check(
        "Pull request opens the tab of the resolved pull request",
        matches!(&opened, Some(RightPanelSurface::PullRequest { repository, number, .. }) if repository == "scenario/orbit-web" && *number == 7),
        format!("{opened:?}"),
    );
    check.wait(Duration::from_millis(800));
    check.shot("launcher-01-pull-request.png")?;

    show_launcher(check, &linked);
    click(check, "launch-side-chat")?;
    let k = linked.clone();
    check.pump(move |c| side_chat_tab(c, &k).is_some(), Duration::from_secs(30), "Side chat opens a side chat tab")?;
    let side_id = side_chat_tab(check, &linked).ok_or_else(|| anyhow::anyhow!("no side chat tab"))?;
    let side = thread_key(&environment_id, &ThreadId::new(side_id.as_str()));
    let s = side.clone();
    check.pump(move |c| shell_thread(c, &s).is_some(), Duration::from_secs(20), "the side chat reaches the shell")?;
    let side_shell = shell_thread(check, &side).ok_or_else(|| anyhow::anyhow!("no side chat row"))?;
    check.check(
        "the server records a side chat of the thread, with no message sent",
        side_shell.lineage.side_chat == Some(true) && side_shell.lineage.parent_thread_id.as_ref() == Some(&tid(&linked)) && side_shell.latest_user_message_at.is_none(),
        format!("side_chat={:?} parent={:?} user_message={:?}", side_shell.lineage.side_chat, side_shell.lineage.parent_thread_id, side_shell.latest_user_message_at),
    );
    let launch = check.read(|root, cx| root.data().side_chat_launch(&side, cx));
    check.check("a side chat offers no Side chat of its own", launch == SideChatLaunch::Hidden, format!("{launch:?}"));
    check.wait(Duration::from_millis(600));
    check.shot("launcher-02-side-chat.png")?;

    check.go(&empty);
    check.frame();
    show_launcher(check, &empty);
    let shell = shell_thread(check, &empty).ok_or_else(|| anyhow::anyhow!("no shell row"))?;
    check.check("the server has no pull request for the other thread", thread_pull_request_panel_target(&shell).is_none(), "");
    check.shot("launcher-03-no-pull-request.png")?;
    click(check, "launch-pull-request")?;
    check.wait(Duration::from_millis(500));
    let opened = active_surface(check, &empty);
    check.check("an unavailable Pull request opens nothing", opened.is_none(), format!("{opened:?}"));
    Ok(())
}
