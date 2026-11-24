// live-check `server-update`: version skew and the server Update control on Settings →
// Environments, against a real isolated server behind the update stand-in
// (`update-standin.mjs`, `T3_STANDIN_URL`, its control directory `T3_STANDIN_CONTROL`). The
// stand-in reports the server one release behind this client and answers the update RPCs with
// progress, a restart and the new version; nothing is installed anywhere.
//
//   1. the row shows the server's version and the Update icon; the header shows "Update all"
//   2. Update runs: Downloading…, Restarting… (the row says Restarting), then the host is back on
//      this client's version, the icon and "Update all" are gone and the stand-in saw one update
//   3. the host falls behind again and its update fails: the row shows the host's reason and the
//      icon says "Retry update"; a retry that succeeds clears the failure
//   4. the host reports the older orchestration protocol: the session is refused, the row offers
//      the bare-socket Update, which updates the host and reconnects

use std::path::PathBuf;
use std::time::Duration;

use t3code_client_runtime::connection::server_update::{ServerUpdateStage, ServerUpdateState};
use t3code_web::components::server_update_action::ServerUpdateStore;
use t3code_web::hooks::use_settings::ServerSettingsStore;
use t3code_web::router::SettingsPanel;
use t3code_web::version_skew;

use super::live_check_families::pair_if_needed;
use super::*;

fn control() -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(std::env::var("T3_STANDIN_CONTROL").context("T3_STANDIN_CONTROL")?))
}

fn set_control(name: &str, value: Option<&str>) -> anyhow::Result<()> {
    let path = control()?.join(name);
    match value {
        Some(value) => std::fs::write(path, value)?,
        None => _ = std::fs::remove_file(path),
    }
    Ok(())
}

/// Closes the stand-in's sockets so the client reconnects and reads the descriptors again.
fn kick() -> anyhow::Result<()> {
    let url = std::env::var("T3_STANDIN_URL").context("T3_STANDIN_URL")?;
    let status = std::process::Command::new("curl").args(["-sf", &format!("{url}/__standin/kick")]).output()?;
    anyhow::ensure!(status.status.success(), "kick failed");
    Ok(())
}

fn element_visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}

fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible element {id}"))?;
    super::click_at(check, at)
}

fn server_version(check: &mut Check, id: &t3code_contracts::EnvironmentId) -> Option<String> {
    check.cx.update(|cx| ServerSettingsStore::config(cx, id).map(|c| c.environment.server_version.clone()))
}

fn update_state(check: &mut Check, id: &t3code_contracts::EnvironmentId) -> ServerUpdateState {
    check.cx.update(|cx| ServerUpdateStore::state(id, cx))
}

fn updates_log() -> String {
    control().ok().and_then(|c| std::fs::read_to_string(c.join("updates.log")).ok()).unwrap_or_default()
}

pub(super) fn run_server_update(check: &mut Check) -> anyhow::Result<()> {
    let client = check.cx.update(|cx| version_skew::client_version(cx).to_string());
    let behind = "0.1.10";
    set_control("version", Some(behind))?;
    for name in ["fail", "protocol", "no-progress", "updates.log"] {
        set_control(name, None)?;
    }
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    kick()?;
    check.pump(|c| c.connected(), Duration::from_secs(60), "connected through the stand-in")?;
    let raw = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    let id = t3code_contracts::EnvironmentId::new(raw.as_str());
    let key = raw.clone();
    let env = id.clone();
    check.pump(move |c| server_version(c, &env).as_deref() == Some(behind), Duration::from_secs(30), "the config reports the older version")?;
    let router = check.read(|root, _| root.router().clone());
    let route = Route::Settings { panel: SettingsPanel::Connections, query: SharedString::default() };
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
    check.frame();
    check.wait(Duration::from_millis(800));

    // 1. The mismatch.
    let mismatch = version_skew::resolve_version_mismatch(Some(behind), &client);
    check.check("a server behind the client is a mismatch", mismatch.is_some(), format!("{behind} vs {client}"));
    let icon = format!("env-update-{key}");
    let env_ready = key.clone();
    check.pump(move |c| element_visible(c, &format!("env-update-{env_ready}")), Duration::from_secs(15), "the row's Update icon")?;
    let (row, header) = (element_visible(check, &icon), element_visible(check, "environments-update-all"));
    check.check("the row offers Update", row, &icon);
    check.check("the header offers Update all", header, "environments-update-all");
    check.shot("u0-mismatch.png")?;

    // 2. A successful update with progress.
    click(check, &icon)?;
    let env = id.clone();
    check.pump(
        move |c| matches!(update_state(c, &env), ServerUpdateState::Running { stage: ServerUpdateStage::Downloading, .. }),
        Duration::from_secs(10),
        "Downloading…",
    )?;
    let (state, shown) = (update_state(check, &id), element_visible(check, &icon));
    check.check("the update reports downloading", true, format!("{state:?}"));
    check.check("the row hides Update while it runs", !shown, "");
    check.shot("u1-downloading.png")?;
    let env = id.clone();
    check.pump(
        move |c| matches!(update_state(c, &env), ServerUpdateState::Running { stage: ServerUpdateStage::Resuming, .. }),
        Duration::from_secs(20),
        "Restarting…",
    )?;
    check.shot("u2-restarting.png")?;
    let env = id.clone();
    check.pump(move |c| update_state(c, &env) == ServerUpdateState::Idle, Duration::from_secs(90), "the update settles")?;
    let env = id.clone();
    let expect = client.clone();
    check.pump(move |c| server_version(c, &env).as_deref() == Some(expect.as_str()) && c.connected(), Duration::from_secs(60), "reconnected on the client's version")?;
    check.wait(Duration::from_millis(500));
    let (version, row, header) = (server_version(check, &id), element_visible(check, &icon), element_visible(check, "environments-update-all"));
    check.check("the host now runs the client's version", version.as_deref() == Some(client.as_str()), format!("{version:?}"));
    check.check("Update is gone", !row, "");
    check.check("Update all is gone", !header, "");
    let log = updates_log();
    check.check("the stand-in answered one progress update", log.matches("server.updateServerWithProgress").count() == 1, log.trim().replace('\n', " | "));
    let toasts = check.cx.update(|cx| t3code_web::components::ui::toast::Toaster::titles(cx));
    check.check("a success toast names the server", toasts.iter().any(|t| t.ends_with("server updated")), format!("{toasts:?}"));
    check.shot("u3-updated.png")?;

    // 3. A failing update, then a retry.
    set_control("version", Some(behind))?;
    set_control("fail", Some("The npm registry could not be reached."))?;
    kick()?;
    let env = id.clone();
    check.pump(move |c| server_version(c, &env).as_deref() == Some(behind) && c.connected(), Duration::from_secs(60), "behind again")?;
    let env_ready = key.clone();
    check.pump(move |c| element_visible(c, &format!("env-update-{env_ready}")), Duration::from_secs(15), "Update is back")?;
    click(check, &icon)?;
    let env = id.clone();
    check.pump(move |c| matches!(update_state(c, &env), ServerUpdateState::Failed { .. }), Duration::from_secs(20), "the update fails")?;
    check.wait(Duration::from_millis(300));
    let failed = update_state(check, &id);
    check.check("the failure keeps the host's reason", format!("{failed:?}").contains("npm registry"), format!("{failed:?}"));
    let (line, retry) = (element_visible(check, &format!("env-update-progress-{key}")), element_visible(check, &icon));
    check.check("the failure line shows under the row", line, "");
    check.check("the row offers the retry", retry, "");
    check.shot("u4-failed.png")?;
    set_control("fail", None)?;
    click(check, &icon)?;
    let env = id.clone();
    check.pump(move |c| matches!(update_state(c, &env), ServerUpdateState::Running { .. }), Duration::from_secs(10), "the retry runs")?;
    let env = id.clone();
    let expect = client.clone();
    check.pump(
        move |c| update_state(c, &env) == ServerUpdateState::Idle && server_version(c, &env).as_deref() == Some(expect.as_str()),
        Duration::from_secs(90),
        "the retry updates the host",
    )?;
    check.check("the retry clears the failure", true, "");

    // 4. A host on the older protocol.
    set_control("version", Some(behind))?;
    set_control("protocol", Some("1"))?;
    kick()?;
    let env = id.clone();
    check.pump(
        move |c| {
            c.cx.update(|cx| {
                ConnectionRuntime::state(cx).read(cx).environment(&env).and_then(|e| e.status.clone()).is_some_and(|s| {
                    s.phase == ConnectionPhase::Blocked && s.last_failure.as_ref().is_some_and(|f| f.server_update_required)
                })
            })
        },
        Duration::from_secs(60),
        "the client refuses the older protocol",
    )?;
    let outdated = format!("env-outdated-update-{key}");
    let wanted = outdated.clone();
    check.pump(move |c| element_visible(c, &wanted), Duration::from_secs(15), "the outdated host's Update")?;
    check.shot("u5-outdated.png")?;
    click(check, &outdated)?;
    let env = id.clone();
    check.pump(move |c| update_state(c, &env).is_running(), Duration::from_secs(10), "the bare-socket update runs")?;
    check.shot("u6-outdated-running.png")?;
    let env = id.clone();
    let expect = client.clone();
    check.pump(
        move |c| update_state(c, &env) == ServerUpdateState::Idle && c.connected() && server_version(c, &env).as_deref() == Some(expect.as_str()),
        Duration::from_secs(120),
        "the outdated host comes back compatible and reconnects",
    )?;
    let log = updates_log();
    check.check("the bare socket asked for the client's version", log.contains(&format!("to {client}")), log.trim().replace('\n', " | "));
    check.shot("u7-outdated-updated.png")?;
    Ok(())
}
