//! live-check `update-pill`: the sidebar's update-everything control against two real isolated
//! servers, each behind an update stand-in (`update-standin.mjs`; `T3_STANDIN_URL`,
//! `T3_STANDIN_CONTROL` for the first, `T3_STANDIN_URL_B`, `T3_STANDIN_CONTROL_B` for the
//! second; paired from `T3_PAIRING_URL` and `T3_PAIRING_URL_B`). The stand-ins report a server
//! version and a Claude release; they answer the update RPCs and log each one, so nothing is
//! installed anywhere. Each state is shot at the fork capture's size (`capture-pill.mjs`).
//!
//!   1. both servers current, no provider behind: the idle control and no pill
//!   2. the first server one release behind and Claude behind on both: the pill says
//!      "Update 1 server and 2 providers"; its dialog lists both machines
//!   3. Update: the capsule shows, providers update first on both machines, then the server;
//!      both stand-ins logged their updates, the run closes, a toast says what updated
//!   4. the server's update fails: the capsule names it with Retry; Retry after the host is fixed
//!      updates it and the run closes

use std::path::PathBuf;
use std::time::Duration;

use t3code_web::components::provider_updates_action::ProviderUpdates;
use t3code_web::components::sidebar::sidebar_update_pill::{SidebarUpdatePill, UpdateEverythingPlan};
use t3code_web::components::sidebar::update_everything_run::UpdateRunStore;
use t3code_web::version_skew;

use super::live_check_families::{all_connected, pair_if_needed};
use super::live_check_update::{click, element_visible, server_version};
use super::*;

/// One stand-in: its control directory and URL.
struct Standin {
    control: PathBuf,
    url: String,
}

impl Standin {
    fn from_env(suffix: &str) -> anyhow::Result<Self> {
        let control = std::env::var(format!("T3_STANDIN_CONTROL{suffix}")).with_context(|| format!("T3_STANDIN_CONTROL{suffix}"))?;
        let url = std::env::var(format!("T3_STANDIN_URL{suffix}")).with_context(|| format!("T3_STANDIN_URL{suffix}"))?;
        Ok(Self { control: control.into(), url })
    }

    fn set(&self, name: &str, value: Option<&str>) -> anyhow::Result<()> {
        let path = self.control.join(name);
        match value {
            Some(value) => std::fs::write(path, value)?,
            None => _ = std::fs::remove_file(path),
        }
        Ok(())
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.control.join("updates.log")).unwrap_or_default()
    }

    fn kick(&self) -> anyhow::Result<()> {
        let status = std::process::Command::new("curl").args(["-sf", &format!("{}/__standin/kick", self.url)]).output()?;
        anyhow::ensure!(status.status.success(), "kick failed");
        Ok(())
    }
}

const CLAUDE_BEHIND: &str = r#"{"claudeAgent": {"latest": "2.1.91", "ms": 2500}}"#;

fn plan(check: &mut Check) -> UpdateEverythingPlan {
    check.cx.update(UpdateEverythingPlan::current)
}

fn has_run(check: &mut Check) -> bool {
    check.cx.update(|cx| UpdateRunStore::run(cx).is_some())
}

/// Sets each stand-in's version, provider release and failure, then reconnects both.
fn reset(check: &mut Check, a: &Standin, b: &Standin, versions: (&str, &str), providers: bool, fail: Option<&str>) -> anyhow::Result<()> {
    for (standin, version) in [(a, versions.0), (b, versions.1)] {
        standin.set("version", Some(version))?;
        standin.set("providers", providers.then_some(CLAUDE_BEHIND))?;
        for name in ["fail", "self-update", "protocol", "no-progress", "restart-ms"] {
            standin.set(name, None)?;
        }
    }
    a.set("fail", fail)?;
    a.set("stage-ms", Some(if fail.is_some() { "300" } else { "1500" }))?;
    a.kick()?;
    b.kick()?;
    let (va, vb) = (versions.0.to_owned(), versions.1.to_owned());
    check.pump(
        move |c| {
            let ids = c.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments.iter().map(|e| e.id().clone()).collect::<Vec<_>>());
            all_connected(c, 2)
                && ids.iter().map(|id| server_version(c, id)).collect::<Vec<_>>() == vec![Some(va.clone()), Some(vb.clone())]
        },
        Duration::from_secs(60),
        "both servers report their versions",
    )?;
    let want = if providers { 2 } else { 0 };
    check.pump(
        move |c| c.cx.update(|cx| ProviderUpdates::machines(cx).len()) == want,
        Duration::from_secs(30),
        "the provider releases arrive",
    )?;
    check.wait(Duration::from_millis(800));
    Ok(())
}

fn away(check: &mut Check) -> anyhow::Result<()> {
    hover_at(check, point(px(600.), px(400.)))
}

fn hover_element(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible element {id}"))?;
    hover_at(check, at)?;
    check.wait(Duration::from_millis(900));
    Ok(())
}

pub(super) fn run_update_pill(check: &mut Check) -> anyhow::Result<()> {
    let (a, b) = (Standin::from_env("")?, Standin::from_env("_B")?);
    a.set("updates.log", None)?;
    b.set("updates.log", None)?;
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    pair_if_needed(check, 2, "T3_PAIRING_URL_B")?;
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    let client = check.cx.update(|cx| version_skew::client_version(cx).to_string());
    let behind = "0.1.10";

    // 1. Nothing to update.
    reset(check, &a, &b, (&client, &client), false, None)?;
    away(check)?;
    let (idle, pill) = (element_visible(check, "sidebar-check-updates"), element_visible(check, "sidebar-update-everything"));
    check.check("nothing to update shows the idle control", idle && !pill, format!("idle {idle} pill {pill}"));
    check.shot("p0-idle.png")?;

    // 2. One server and two providers behind.
    reset(check, &a, &b, (behind, &client), true, None)?;
    check.pump(|c| element_visible(c, "sidebar-update-everything"), Duration::from_secs(20), "the pill")?;
    away(check)?;
    let current = plan(check);
    check.check(
        "the pill says what it updates",
        current.view.summary.as_ref() == "Update 1 server and 2 providers",
        &current.view.summary,
    );
    check.shot("p1-pill.png")?;
    hover_element(check, "sidebar-update-everything")?;
    check.shot("p2-pill-tooltip.png")?;
    click(check, "sidebar-update-everything")?;
    check.wait(Duration::from_millis(200));
    let confirming = check.cx.update(|cx| SidebarUpdatePill::view(cx).read(cx).is_confirming());
    check.check("the pill opens its dialog", confirming, "");
    check.shot("p3-dialog-checking.png")?;
    check.pump(|c| !c.cx.update(|cx| ProviderUpdates::is_checking(cx)), Duration::from_secs(30), "the dialog's provider check")?;
    check.wait(Duration::from_millis(400));
    let rows: Vec<String> = plan(check)
        .view
        .rows
        .iter()
        .map(|r| format!("{}:{}:{}", r.label, r.app.map(|a| a.label().to_string()).unwrap_or_default(), r.providers.join("+")))
        .collect();
    check.check("the dialog lists both machines", rows == ["srva:Server:Claude", "srvb::Claude"], rows.join(" | "));
    check.shot("p4-dialog.png")?;

    // 3. The run.
    a.set("stage-ms", Some("3000"))?;
    a.set("restart-ms", Some("4000"))?;
    click(check, "update-everything-confirm")?;
    check.pump(has_run, Duration::from_secs(5), "the capsule")?;
    away(check)?;
    check.wait(Duration::from_millis(400));
    let shown = element_visible(check, "sidebar-update-capsule");
    check.check("the capsule replaces the pill", shown, "");
    check.shot("p5-capsule-providers.png")?;
    check.cx.update(|cx| SidebarUpdatePill::view(cx).update(cx, |p, cx| p.set_popover_open(true, cx)));
    check.wait(Duration::from_millis(400));
    check.shot("p6-capsule-popover-providers.png")?;
    check.cx.update(|cx| SidebarUpdatePill::view(cx).update(cx, |p, cx| p.set_popover_open(false, cx)));
    let expect = client.clone();
    check.pump(
        move |c| {
            let id = c.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].id().clone());
            !has_run(c) && server_version(c, &id).as_deref() == Some(expect.as_str())
        },
        Duration::from_secs(120),
        "the run finishes",
    )?;
    let (log_a, log_b) = (a.log(), b.log());
    let provider_first = log_a.find("server.updateProvider").zip(log_a.find("server.updateServer")).is_some_and(|(p, s)| p < s);
    check.check("providers update before the server", provider_first, log_a.trim().replace('\n', " | "));
    check.check("the second machine updated its provider only", log_b.contains("server.updateProvider") && !log_b.contains("updateServer"), log_b.trim().replace('\n', " | "));
    let toasts = check.cx.update(|cx| t3code_web::components::ui::toast::Toaster::titles(cx));
    check.check("a toast says what updated", toasts.iter().any(|t| t == "Updated 1 server and 2 providers"), format!("{toasts:?}"));
    check.pump(|c| c.cx.update(|cx| ProviderUpdates::machines(cx).is_empty()), Duration::from_secs(20), "the providers are current")?;
    check.pump(|c| !element_visible(c, "sidebar-update-everything"), Duration::from_secs(20), "the pill goes away")?;
    check.shot("p7-run-done.png")?;

    // 4. A failing server update, then Retry.
    a.set("updates.log", None)?;
    reset(check, &a, &b, (behind, &client), false, Some("Download failed: the release server is unreachable."))?;
    check.pump(|c| element_visible(c, "sidebar-update-everything"), Duration::from_secs(20), "the pill")?;
    click(check, "sidebar-update-everything")?;
    check.pump(|c| !c.cx.update(|cx| ProviderUpdates::is_checking(cx)), Duration::from_secs(30), "the dialog's provider check")?;
    click(check, "update-everything-confirm")?;
    check.pump(
        |c| c.cx.update(|cx| UpdateRunStore::run(cx)).is_some_and(|run| run.machines.iter().any(|m| m.failure.is_some())),
        Duration::from_secs(30),
        "the server update fails",
    )?;
    away(check)?;
    check.wait(Duration::from_millis(400));
    let failure = check.cx.update(|cx| UpdateRunStore::run(cx)).and_then(|run| run.machines[0].failure.clone()).unwrap_or_default();
    check.check("the failed machine keeps the host's reason", failure.contains("release server is unreachable"), &failure);
    check.shot("p8-capsule-failed.png")?;
    check.cx.update(|cx| SidebarUpdatePill::view(cx).update(cx, |p, cx| p.set_popover_open(true, cx)));
    check.wait(Duration::from_millis(400));
    let retry = element_visible(check, "sidebar-update-popover-action");
    check.check("the popover offers Retry", retry, "");
    check.shot("p9-capsule-popover-failed.png")?;
    a.set("fail", None)?;
    click(check, "sidebar-update-popover-action")?;
    let expect = client.clone();
    check.pump(
        move |c| {
            let id = c.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].id().clone());
            !has_run(c) && server_version(c, &id).as_deref() == Some(expect.as_str())
        },
        Duration::from_secs(120),
        "Retry updates the server and the run closes",
    )?;
    let log = a.log();
    check.check("the stand-in saw the failed update and the retry", log.matches("server.updateServer").count() == 2, log.trim().replace('\n', " | "));
    check.shot("p10-retried.png")?;
    Ok(())
}
