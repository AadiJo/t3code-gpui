//! Runs the ported git actions, branch toolbar, scripts, cleanup and remote
//! open logic over a table of cases and writes the decisions as JSON, in the
//! shape the TypeScript runner writes, so the two can be compared row by row.
//! Usage: git_actions_logic <cases.json> <out.json>

use serde::Deserialize;
use serde_json::{Value, json};
use t3code_contracts::{EditorId, GitStackedAction, ProjectScript, VcsStatusResult};
use t3code_web::components::branch_toolbar_logic as b;
use t3code_web::components::git_actions_control_logic as g;
use t3code_web::{project_scripts as p, remote_open, source_control_presentation as scp, worktree_cleanup as w};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitCase {
    status: Option<VcsStatusResult>,
    is_busy: bool,
    is_default_ref: bool,
    has_primary_remote: bool,
}

fn action(name: &str) -> GitStackedAction {
    GitStackedAction::from_wire(name).expect("known action")
}

fn env_mode(name: &str) -> b::EnvMode {
    if name == "worktree" { b::EnvMode::Worktree } else { b::EnvMode::Local }
}

fn s(v: &Value) -> Option<&str> {
    v.as_str()
}

fn quick(q: &g::GitQuickAction) -> Value {
    json!({
        "label": q.label.as_ref(),
        "disabled": q.disabled,
        "kind": q.kind_str(),
        "action": q.action().map(|a| a.as_str()),
        "hint": q.hint().map(|h| h.as_ref()),
    })
}

fn git_row(case: &GitCase) -> Value {
    let status = case.status.as_ref();
    let items = g::build_menu_items(status, case.is_busy, case.has_primary_remote);
    let q = g::resolve_quick_action(status, case.is_busy, case.is_default_ref, case.has_primary_remote);
    let detached = status.is_some_and(|s| s.ref_name.is_none());
    let clean_behind = status.is_some_and(|s| {
        s.ref_name.is_some() && !s.has_working_tree_changes && s.behind_count > 0 && s.ahead_count == 0
    });
    let dialog = |d: g::GitDialogAction| match d {
        g::GitDialogAction::Commit => "commit",
        g::GitDialogAction::Push => "push",
        g::GitDialogAction::CreatePr => "create_pr",
    };
    let confirm: Vec<bool> = ["commit", "push", "create_pr", "commit_push", "commit_push_pr"]
        .map(|a| g::requires_default_branch_confirmation(action(a), case.is_default_ref))
        .to_vec();
    json!({
        "quick": quick(&q),
        "items": items.iter().map(|i| json!({
            "id": i.id.as_str(),
            "label": i.label.as_ref(),
            "disabled": i.disabled,
            "dialog": dialog(i.dialog_action),
            "reason": g::get_menu_action_disabled_reason(i, status, case.is_busy, case.has_primary_remote).map(|r| r.to_string()),
        })).collect::<Vec<_>>(),
        "usable": g::has_usable_git_menu_content(&items, true, status.is_some() && !case.has_primary_remote, detached || clean_behind),
        "confirm": confirm,
    })
}

fn progress_row(v: &Value) -> Value {
    let operation = match v["operation"].as_str() {
        Some("run_change_request") => Some(g::VcsActionOperation::RunChangeRequest),
        Some("pull") => Some(g::VcsActionOperation::Pull),
        Some("switch_ref") => Some(g::VcsActionOperation::SwitchRef),
        _ => None,
    };
    let text = |key: &str| v[key].as_str().map(|t| t.to_owned().into());
    let state = g::VcsActionState {
        is_running: v["isRunning"].as_bool().unwrap(),
        operation,
        current_label: text("currentLabel"),
        hook_name: text("hookName"),
        last_output_line: text("lastOutputLine"),
        phase_started_at_ms: v["phaseStartedAtMs"].as_i64(),
        hook_started_at_ms: v["hookStartedAtMs"].as_i64(),
    };
    match g::resolve_git_action_progress_presentation(&state) {
        None => Value::Null,
        Some(p) => json!({ "status": p.status.as_ref(), "output": p.output.as_deref(), "startedAtMs": p.started_at_ms }),
    }
}

fn script(v: &Value) -> ProjectScript {
    serde_json::from_value(v.clone()).expect("script")
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (input, output) = (args.next().expect("cases.json"), args.next().expect("out.json"));
    let c: Value = serde_json::from_str(&std::fs::read_to_string(input)?)?;
    let rows = |key: &str| c[key].as_array().expect(key).clone();
    let mut out = serde_json::Map::new();

    let git: Vec<GitCase> = serde_json::from_value(c["git"].clone())?;
    out.insert("git".into(), git.iter().map(git_row).collect());
    out.insert("progress".into(), rows("progress").iter().map(progress_row).collect());
    out.insert(
        "elapsed".into(),
        rows("elapsed")
            .iter()
            .map(|r| json!(g::format_git_action_elapsed(r["startedAtMs"].as_i64(), r["nowMs"].as_i64().unwrap()).map(|s| s.to_string())))
            .collect(),
    );
    out.insert(
        "dialog".into(),
        rows("dialog")
            .iter()
            .map(|r| {
                let terminology = scp::source_control_definition(s(&r["provider"])).terminology;
                let copy = g::resolve_default_branch_action_dialog_copy(
                    action(r["action"].as_str().unwrap()),
                    r["branchName"].as_str().unwrap(),
                    r["includesCommit"].as_bool().unwrap(),
                    terminology,
                );
                json!({ "title": copy.title.as_ref(), "description": copy.description.as_ref(), "continueLabel": copy.continue_label.as_ref() })
            })
            .collect(),
    );
    out.insert(
        "feature".into(),
        rows("feature")
            .iter()
            .map(|r| {
                json!(g::resolve_feature_branch_unavailable_reason(
                    action(r["action"].as_str().unwrap()),
                    r["branchName"].as_str().unwrap(),
                    r["hasWorkingTreeChanges"].as_bool().unwrap(),
                )
                .map(|s| s.to_string()))
            })
            .collect(),
    );
    out.insert(
        "live".into(),
        rows("live")
            .iter()
            .map(|r| {
                let status: Option<VcsStatusResult> = serde_json::from_value(r["gitStatus"].clone()).unwrap();
                match g::resolve_live_thread_branch_update(s(&r["threadBranch"]), status.as_ref()) {
                    None => Value::Null,
                    Some(branch) => json!({ "branch": branch }),
                }
            })
            .collect(),
    );
    out.insert(
        "changes".into(),
        rows("changes")
            .iter()
            .map(|r| {
                let status: Option<VcsStatusResult> = serde_json::from_value(r["status"].clone()).unwrap();
                match g::resolve_changes_totals(status.as_ref()) {
                    None => Value::Null,
                    Some(t) => json!({ "insertions": t.insertions, "deletions": t.deletions, "baseRef": t.base_ref }),
                }
            })
            .collect(),
    );
    out.insert(
        "trigger".into(),
        rows("trigger")
            .iter()
            .map(|r| {
                json!(b::resolve_branch_trigger_label(
                    s(&r["activeWorktreePath"]),
                    env_mode(r["effectiveEnvMode"].as_str().unwrap()),
                    s(&r["resolvedActiveBranch"]),
                    r["resolvedActiveBranchIsRemote"].as_bool(),
                    r["startFromOrigin"].as_bool().unwrap(),
                )
                .as_ref())
            })
            .collect(),
    );
    out.insert(
        "envmode".into(),
        rows("envmode")
            .iter()
            .map(|r| {
                json!(b::resolve_effective_env_mode(
                    s(&r["activeWorktreePath"]),
                    r["hasServerThread"].as_bool().unwrap(),
                    s(&r["draftThreadEnvMode"]).map(env_mode),
                    r["preparingWorktree"].as_bool().unwrap(),
                )
                .as_str())
            })
            .collect(),
    );
    out.insert(
        "toolbarvalue".into(),
        rows("toolbarvalue")
            .iter()
            .map(|r| {
                json!(b::resolve_branch_toolbar_value(
                    env_mode(r["envMode"].as_str().unwrap()),
                    s(&r["activeWorktreePath"]),
                    s(&r["activeThreadBranch"]),
                    s(&r["currentGitBranch"]),
                ))
            })
            .collect(),
    );
    out.insert(
        "selection".into(),
        rows("selection")
            .iter()
            .map(|r| {
                let t = b::resolve_branch_selection_target(
                    r["activeProjectCwd"].as_str().unwrap(),
                    s(&r["activeProjectCheckoutRoot"]),
                    s(&r["activeWorktreePath"]),
                    r["refName"]["isDefault"].as_bool().unwrap(),
                    s(&r["refName"]["worktreePath"]),
                );
                json!({
                    "checkoutCwd": t.checkout_cwd,
                    "nextWorktreePath": t.next_worktree_path,
                    "reuseExistingWorktree": t.reuse_existing_worktree,
                    "staysInActiveCheckout": t.stays_in_active_checkout,
                })
            })
            .collect(),
    );
    out.insert(
        "include".into(),
        rows("include")
            .iter()
            .map(|r| {
                json!(b::should_include_branch_picker_item(
                    r["itemValue"].as_str().unwrap(),
                    r["normalizedQuery"].as_str().unwrap(),
                    s(&r["createBranchItemValue"]),
                    s(&r["checkoutPullRequestItemValue"]),
                ))
            })
            .collect(),
    );
    out.insert(
        "refnames".into(),
        rows("refnames")
            .iter()
            .map(|r| {
                let v = r.as_str().unwrap();
                json!({
                    "sanitized": b::sanitize_new_ref_name(v),
                    "local": b::derive_local_branch_name_from_remote_ref(v),
                    "temporary": g::is_temporary_worktree_branch(v),
                })
            })
            .collect(),
    );
    out.insert(
        "temporary".into(),
        rows("live").iter().map(|r| json!(s(&r["threadBranch"]).map(g::is_temporary_worktree_branch))).collect(),
    );
    out.insert(
        "locked".into(),
        rows("locked")
            .iter()
            .map(|r| {
                json!(b::resolve_locked_workspace_label(s(&r["activeWorktreePath"]), env_mode(r["effectiveEnvMode"].as_str().unwrap()))
                    .as_ref())
            })
            .collect(),
    );
    out.insert(
        "prev".into(),
        rows("prev")
            .iter()
            .map(|r| {
                let threads: Vec<b::WorktreeThread> = r["threads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| b::WorktreeThread {
                        branch: s(&t["branch"]).map(str::to_owned),
                        worktree_path: s(&t["worktreePath"]).map(str::to_owned),
                        updated_at_ms: s(&t["updatedAt"])
                            .and_then(|v| v.parse::<jiff::Timestamp>().ok())
                            .map(|ts| ts.as_millisecond()),
                        archived: !t["archivedAt"].is_null(),
                    })
                    .collect();
                match b::resolve_previous_worktree_seed(&threads, s(&r["currentWorktreePath"])) {
                    None => Value::Null,
                    Some(seed) => json!({
                        "branch": seed.branch,
                        "worktreePath": seed.worktree_path,
                        "label": b::resolve_previous_worktree_label(&seed).as_ref(),
                    }),
                }
            })
            .collect(),
    );
    out.insert(
        "envlabel".into(),
        rows("envlabel")
            .iter()
            .map(|r| {
                json!(b::resolve_environment_option_label(
                    r["isPrimary"].as_bool().unwrap(),
                    &t3code_contracts::EnvironmentId::new(r["environmentId"].as_str().unwrap()),
                    s(&r["runtimeLabel"]),
                    s(&r["savedLabel"]),
                )
                .as_ref())
            })
            .collect(),
    );
    out.insert(
        "primary".into(),
        rows("primary")
            .iter()
            .map(|r| {
                let scripts: Vec<ProjectScript> = r["scripts"].as_array().unwrap().iter().map(script).collect();
                json!(p::primary_project_script(&scripts).map(|s| s.id.clone()))
            })
            .collect(),
    );
    out.insert(
        "scriptids".into(),
        rows("scriptids")
            .iter()
            .map(|r| {
                let existing: Vec<&str> = r["existing"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
                json!(p::next_project_script_id(r["name"].as_str().unwrap(), existing))
            })
            .collect(),
    );
    out.insert(
        "commands".into(),
        rows("commands")
            .iter()
            .map(|r| {
                let command = p::command_for_project_script(r.as_str().unwrap());
                let back = command.as_deref().and_then(p::project_script_id_from_command).map(str::to_owned);
                json!({ "command": command, "back": back })
            })
            .collect(),
    );
    out.insert(
        "scriptlabels".into(),
        rows("scriptlabels").iter().map(|r| json!(p::project_script_menu_label(&script(r)).as_ref())).collect(),
    );
    out.insert(
        "cleanup".into(),
        rows("cleanup")
            .iter()
            .map(|r| {
                let threads: Vec<w::WorktreeThreadRef> = r["threads"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| w::WorktreeThreadRef { id: t["id"].as_str().unwrap(), worktree_path: s(&t["worktreePath"]) })
                    .collect();
                json!(w::get_orphaned_worktree_path_for_thread(&threads, r["threadId"].as_str().unwrap()))
            })
            .collect(),
    );
    out.insert(
        "display".into(),
        rows("display").iter().map(|r| json!(w::format_worktree_path_for_display(r.as_str().unwrap()))).collect(),
    );
    out.insert(
        "remote".into(),
        rows("remote")
            .iter()
            .map(|r| {
                let editor = EditorId::from_wire(r["editor"].as_str().unwrap()).unwrap();
                json!(remote_open::build_remote_open_url(
                    editor,
                    r["host"].as_str().unwrap(),
                    s(&r["username"]),
                    r["absolutePath"].as_str().unwrap(),
                    r["isFile"].as_bool().unwrap(),
                ))
            })
            .collect(),
    );
    std::fs::write(output, serde_json::to_string(&Value::Object(out))?)?;
    Ok(())
}
