//! `live-check linked-items`: the Linked items tab on server data. Makes a thread with five linked
//! pull requests (three chained by branch, one merged, one the host has not been read for) and a
//! linked issue, their host snapshots set as the server's own link sync sets them, and an empty
//! thread (`linked-state.json`). Opens the tab on each, checks the list order and depths against
//! the server's links, watches and unlinks through the panel's commands and reads them back.
//! `T3_PAIRING_URL`, `T3_PROJECT_ROOT`.

use std::time::Duration;

use t3code_client_runtime::operations::CreateThread;
use t3code_client_runtime::state::thread_pull_requests::{pull_request_list_lines, resolve_thread_pull_request_chains};
use t3code_contracts::{ModelSelection, OrchestrationV2Command, ProviderInteractionMode, RuntimeMode, ThreadId, ThreadPullRequestLink};
use t3code_web::components::pull_request::thread_pull_requests_panel::ThreadPullRequestsPanel;
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;

use super::live_check_families::{all_connected, pair_if_needed, project_at};
use super::{Check, commands_of};

const STATE_FILE: &str = "linked-state.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct LinkedState {
    linked_thread: String,
    empty_thread: String,
}

fn links(check: &mut Check, key: &ThreadKey) -> Vec<ThreadPullRequestLink> {
    check.cx.update(|cx| StateBridge::shell(cx).read(cx).row(key).and_then(|row| row.source.pull_requests.clone()).unwrap_or_default())
}

fn command(value: serde_json::Value) -> anyhow::Result<OrchestrationV2Command> {
    Ok(serde_json::from_value(value)?)
}

fn iso(offset_minutes: i64) -> String {
    t3code_client_runtime::state::models::format_ms(t3code_client_runtime::state::models::now_ms() + offset_minutes * 60_000)
}

/// The commands that link the fixture's pull requests and issue and set their host state.
fn link_commands(thread: &str) -> anyhow::Result<Vec<OrchestrationV2Command>> {
    let id = || uuid::Uuid::new_v4().to_string();
    let mut out = Vec::new();
    let prs = [
        (101, "feature/clamp", "main", "open", false, "Add clamp to the math module", Some("passing"), Some("approved"), (48, 3), -40),
        (102, "feature/saturate", "feature/clamp", "open", false, "Saturate on top of clamp", Some("pending"), None, (22, 1), -30),
        (103, "feature/docs", "feature/saturate", "open", true, "Document the helpers", None, None, (9, 0), -20),
        (104, "fix/overflow", "main", "merged", false, "Fix overflow in add", Some("passing"), Some("approved"), (5, 5), -300),
    ];
    for (number, head, base, state, draft, title, checks, review, (additions, deletions), updated) in prs {
        out.push(command(serde_json::json!({
            "type": "thread.pull-request.link", "commandId": id(), "threadId": thread, "host": "github.com", "repository": "scenario/orbit",
            "number": number, "url": format!("https://github.com/scenario/orbit/pull/{number}"), "source": if number == 101 { "created" } else { "agent" },
        }))?);
        let mut snapshot = serde_json::json!({
            "state": state, "title": title, "headBranch": head, "baseBranch": base, "isDraft": draft, "updatedAt": iso(updated), "syncedAt": iso(-2),
            "author": { "login": "ada-scenario", "name": "Ada Scenario" }, "additions": additions, "deletions": deletions, "changedFiles": 2,
        });
        if let Some(checks) = checks {
            snapshot["checksState"] = checks.into();
        }
        if let Some(review) = review {
            snapshot["reviewDecision"] = review.into();
        }
        out.push(command(serde_json::json!({
            "type": "thread.pull-request-link.sync", "commandId": id(), "threadId": thread, "host": "github.com", "repository": "scenario/orbit",
            "number": number, "snapshot": snapshot, "stack": null,
        }))?);
    }
    out.push(command(serde_json::json!({
        "type": "thread.pull-request.link", "commandId": id(), "threadId": thread, "host": "github.com", "repository": "scenario/orbit-web",
        "number": 7, "url": "https://github.com/scenario/orbit-web/pull/7", "source": "manual",
    }))?);
    Ok(out)
}

pub(super) fn run_linked_items(check: &mut Check) -> anyhow::Result<()> {
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
    let state = match std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<LinkedState>(&bytes).ok()) {
        Some(state) => state,
        None => {
            let project = project_at(check, &environment_id, &std::env::var("T3_PROJECT_ROOT")?)?;
            let commands = commands_of(check, &environment_id)?;
            let mut ids = Vec::new();
            for title in ["Linked items", "Nothing linked"] {
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
                ids.push(thread_id.to_string());
            }
            for command in link_commands(&ids[0])? {
                let c = commands.clone();
                check.block(async move { c.dispatch(command).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
            }
            let issue = command(serde_json::json!({
                "type": "thread.issue.link", "commandId": uuid::Uuid::new_v4().to_string(), "threadId": ids[0], "tracker": "github", "host": "github.com",
                "scope": "scenario/orbit", "key": "42", "projectId": project.as_str(), "url": "https://github.com/scenario/orbit/issues/42",
                "displayKey": "#42", "title": "Clamp should saturate at the bounds", "source": "manual",
            }))?;
            let c = commands.clone();
            check.block(async move { c.dispatch(issue).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
            let state = LinkedState { linked_thread: ids[0].clone(), empty_thread: ids[1].clone() };
            std::fs::write(&path, serde_json::to_vec_pretty(&state)?)?;
            state
        }
    };
    let linked = thread_key(&environment_id, &ThreadId::new(state.linked_thread.as_str()));
    let empty = thread_key(&environment_id, &ThreadId::new(state.empty_thread.as_str()));
    let key = linked.clone();
    check.pump(move |c| links(c, &key).iter().filter(|l| l.snapshot.is_some()).count() >= 4, Duration::from_secs(30), "the server holds the links and their host state")?;

    let server_links = links(check, &linked);
    let chains = resolve_thread_pull_request_chains(&server_links);
    let lines = pull_request_list_lines(&chains);
    let order: Vec<(u64, usize, Option<usize>)> = lines.iter().map(|l| (l.link.number, l.depth, l.stack.map(|s| s.1))).collect();
    check.check(
        "the list is newest first: #7 (linked now, no host state), the 101 > 102 > 103 branch chain, the merged 104",
        order == [(7, 0, None), (101, 0, Some(3)), (102, 1, None), (103, 2, None), (104, 0, None)],
        format!("{order:?}"),
    );

    check.go(&linked);
    check.frame();
    let root = check.root.clone();
    check.cx.update(|cx| root.read(cx).right_panel().clone().update(cx, |panel, cx| panel.open_linked_items(cx)));
    check.wait(Duration::from_millis(800));
    check.shot("linked-01-items.png")?;

    // The panel's own commands, as its row menu runs them.
    let panels = check.read(|root, _| root.right_panels().clone());
    let panel = check.cx.update(|cx| gpui_kit::AppContext::new(cx, |cx| ThreadPullRequestsPanel::new(linked.clone(), panels, cx)));
    let pr = |number: u64, links: &[ThreadPullRequestLink]| links.iter().find(|l| l.number == number).cloned();
    let first = pr(101, &server_links).ok_or_else(|| anyhow::anyhow!("no #101"))?;
    let supports_watch = check.cx.update(|cx| StateBridge::environment(&environment_id, cx).and_then(|e| e.read(cx).config.clone())).is_some_and(|c| c.environment.capabilities.thread_pull_request_watch == Some(true));
    if supports_watch {
        let (p, link) = (panel.clone(), first.clone());
        check.cx.update(|cx| p.update(cx, |p, cx| p.set_watching(&link, true, cx)));
        let key = linked.clone();
        let watched = check.pump(move |c| pr(101, &links(c, &key)).is_some_and(|l| l.watch.is_some()), Duration::from_secs(15), "watching #101");
        check.check("Watch for changes reads back as watching", watched.is_ok(), format!("{watched:?}"));
        check.shot("linked-02-watching.png")?;
    } else {
        check.note("the server does not offer pull request watches; the menu leaves Watch out");
    }
    let unlinked = pr(7, &server_links).ok_or_else(|| anyhow::anyhow!("no #7"))?;
    let p = panel.clone();
    check.cx.update(|cx| p.update(cx, |p, cx| p.unlink(&unlinked, cx)));
    let key = linked.clone();
    check.pump(move |c| pr(7, &links(c, &key)).is_none(), Duration::from_secs(15), "Unlink from thread removes #7")?;
    let left = links(check, &linked).len();
    check.check("Unlink from thread reads back without #7", true, format!("{left} links left"));
    check.shot("linked-03-unlinked.png")?;

    check.go(&empty);
    check.frame();
    let root = check.root.clone();
    check.cx.update(|cx| root.read(cx).right_panel().clone().update(cx, |panel, cx| panel.open_linked_items(cx)));
    check.wait(Duration::from_millis(800));
    check.shot("linked-04-empty.png")?;
    Ok(())
}
