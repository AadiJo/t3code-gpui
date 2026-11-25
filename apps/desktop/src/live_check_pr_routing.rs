//! live-check `pr-routing`: pull request sharing on Settings → Environments and the routing it
//! drives, against the two isolated servers behind their stand-ins (as in `update-pill`). Neither
//! server has a source control account, so each stand-in answers the routing calls from its
//! control file `pr`: the first server knows the account but cannot read the pull request, the
//! second knows the same account and can.
//!
//!   1. the folded section says Off; open, it lists both machines with their selects
//!   2. with no grant, a summary read on the first machine fails there and the second is never asked
//!   3. with both machines on "Read PRs", the summary says so, the grants are stored under the web's
//!      keys, and the same read is answered by the second machine for the verified host and account,
//!      with the origin's project; the origin is told the state it saw
//!   4. when the second machine reports another account, it is not used

use std::time::Duration;

use t3code_client_runtime::connection::pull_request_routing::PullRequestRoutingPermission as Permission;
use t3code_contracts::rpc::methods::PullRequestsSummary;
use t3code_contracts::{PullRequestRef, PullRequestSummary};
use t3code_web::components::settings::pull_request_routing_settings::set_environment_permission;
use t3code_web::router::SettingsPanel;

use super::live_check_families::{all_connected, pair_if_needed};
use super::live_check_update::element_visible;
use super::*;

const IDENTITY: &str = r#"{"accountId": "acct-1", "host": "github.com", "provider": "github", "viewer": "octo"}"#;

fn write_pr(control: &str, value: &str) -> anyhow::Result<()> {
    std::fs::write(std::path::Path::new(&std::env::var(control).context("control")?).join("pr"), value)?;
    Ok(())
}

fn log_of(control: &str) -> String {
    std::env::var(control).ok().and_then(|c| std::fs::read_to_string(std::path::Path::new(&c).join("updates.log")).ok()).unwrap_or_default()
}

fn summary(check: &mut Check, id: &t3code_contracts::EnvironmentId) -> anyhow::Result<Result<PullRequestSummary, String>> {
    let input = PullRequestRef {
        project_id: t3code_contracts::ProjectId::new("project-origin"),
        host: None,
        expected_account_id: None,
        allow_stale: None,
        repository: "acme/orbit".into(),
        number: serde_json::from_value(serde_json::json!(42))?,
    };
    let id = id.clone();
    let task = check.cx.update(|cx| {
        let runtime = ConnectionRuntime::global(cx);
        let link = t3code_web::live::rpc::RpcLink { link: runtime.environment_link(&id).expect("link"), tokio: runtime.tokio().clone() };
        let call = link.call_routed::<PullRequestsSummary>(input);
        cx.spawn(async move |_| call.await.map_err(|e| e.to_string()))
    });
    check.wait_task(task)
}

pub(super) fn run_pr_routing(check: &mut Check) -> anyhow::Result<()> {
    for control in ["T3_STANDIN_CONTROL", "T3_STANDIN_CONTROL_B"] {
        let dir = std::path::PathBuf::from(std::env::var(control).context("control")?);
        _ = std::fs::remove_file(dir.join("updates.log"));
    }
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    check.pump(|c| all_connected(c, 2), Duration::from_secs(60), "both servers connected")?;
    let ids = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| e.id().clone()).collect::<Vec<_>>());
    let (a, b) = (ids[0].clone(), ids[1].clone());
    for id in [&a, &b] {
        check.cx.update(|cx| set_environment_permission(id, Permission::Off, cx));
    }
    write_pr("T3_STANDIN_CONTROL", &format!(r#"{{"routing": {{"accountId": "acct-1", "host": "github.com", "provider": "github", "viewer": "octo", "projectTitle": "orbit", "workspaceRoot": "/srv/orbit"}}, "identity": {IDENTITY}, "summary": "fail"}}"#))?;
    write_pr("T3_STANDIN_CONTROL_B", &format!(r#"{{"identity": {IDENTITY}, "summary": {{"title": "Answered by srvb"}}}}"#))?;

    // 1. The section.
    let router = check.read(|root, _| root.router().clone());
    let route = Route::Settings { panel: SettingsPanel::Connections, query: SharedString::default() };
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
    check.frame();
    check.pump(|c| element_visible(c, "pull-request-routing-toggle"), Duration::from_secs(10), "the Pull request sharing section")?;
    hover_at(check, point(px(5.), px(770.)))?;
    check.wait(Duration::from_millis(500));
    check.shot("r0-section-off.png")?;
    super::live_check_update::click(check, "pull-request-routing-toggle")?;
    check.wait(Duration::from_millis(400));
    let (row_a, row_b) = (
        element_visible(check, &format!("pull-request-routing-{}", a.as_str())),
        element_visible(check, &format!("pull-request-routing-{}", b.as_str())),
    );
    check.check("open, the section lists both machines with a select", row_a && row_b, "");
    check.shot("r1-section-open.png")?;

    // 2. No grant.
    let off = summary(check, &a)?;
    let b_log = log_of("T3_STANDIN_CONTROL_B");
    check.check("without a grant the origin's failure stands", off.is_err(), format!("{off:?}"));
    check.check("without a grant the second machine is never asked", !b_log.contains("pullRequests.summary"), b_log.trim().replace('\n', " | "));

    // 3. Both machines read PRs.
    for id in [&a, &b] {
        check.cx.update(|cx| set_environment_permission(id, Permission::Read, cx));
    }
    check.wait(Duration::from_millis(300));
    let stored = check.cx.update(|cx| {
        t3code_web::client_persistence_storage::LocalStorage::get::<serde_json::Value>(cx, &format!("t3code:github-routing:{}", a.as_str()))
    });
    check.check(
        "the grant is stored under the web's key with its endpoints",
        stored.as_ref().is_some_and(|v| v["permission"] == "read" && v["connectionKey"].as_str().is_some_and(|k| k.contains("BearerConnectionTarget"))),
        format!("{stored:?}"),
    );
    check.shot("r2-section-read.png")?;
    let routed = summary(check, &a)?;
    let b_log = log_of("T3_STANDIN_CONTROL_B");
    check.check(
        "the second machine answers the read, with the origin's project",
        routed.as_ref().is_ok_and(|s| s.title == "Answered by srvb" && s.project_id.as_str() == "project-origin"),
        format!("{:?}", routed.as_ref().map(|s| (&s.title, s.project_id.as_str()))),
    );
    check.check(
        "the routed read carries the verified host and account",
        b_log.contains("pullRequests.summary host=github.com account=acct-1 allowStale=false"),
        b_log.trim().replace('\n', " | "),
    );
    check.pump(|_| log_of("T3_STANDIN_CONTROL").contains("pullRequests.reportState"), Duration::from_secs(10), "the origin hears the state")?;
    check.check("the origin is told the state the routed read saw", true, "");

    // 4. Another account.
    write_pr("T3_STANDIN_CONTROL_B", r#"{"identity": {"accountId": "acct-2", "host": "github.com", "provider": "github", "viewer": "other"}, "summary": {"title": "Answered by srvb"}}"#)?;
    let before = log_of("T3_STANDIN_CONTROL_B").matches("pullRequests.summary").count();
    let refused = summary(check, &a)?;
    let after = log_of("T3_STANDIN_CONTROL_B").matches("pullRequests.summary").count();
    check.check("a machine on another account is not used", refused.is_err() && after == before, format!("{refused:?} {before}->{after}"));

    for id in [&a, &b] {
        check.cx.update(|cx| set_environment_permission(id, Permission::Off, cx));
    }
    for control in ["T3_STANDIN_CONTROL", "T3_STANDIN_CONTROL_B"] {
        _ = std::fs::remove_file(std::path::Path::new(&std::env::var(control)?).join("pr"));
    }
    Ok(())
}
