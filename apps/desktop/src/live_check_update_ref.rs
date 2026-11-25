//! live-check `update-ref`: the server Update states on Settings → Environments, shot at the fork
//! capture's size and settings (`T3_LIVE_WINDOW`, `T3CODE_*`, dark) behind the update stand-in
//! (`T3_STANDIN_URL`, `T3_STANDIN_CONTROL`), one shot per state of the fork capture
//! (`capture-update.mjs`): the row with its version, the Update tooltip, "Update all",
//! Downloading…, Installing (still "Downloading…"), Restarting…, updated, a failed update with
//! "Retry update", the copy-command tooltip and toast, and the desktop-app confirm dialog. Each
//! state is read back from the stand-in's log or the update store.

use std::time::Duration;

use t3code_client_runtime::connection::server_update::{ServerUpdateStage, ServerUpdateState};
use t3code_web::router::SettingsPanel;

use super::live_check_families::pair_if_needed;
use super::live_check_update::{click, control, element_visible, kick, server_version, set_control, update_state};
use super::*;

/// Puts the stand-in back one release behind with `extra` control files, and waits for the row.
fn reset(check: &mut Check, id: &t3code_contracts::EnvironmentId, icon: &str, extra: &[(&str, &str)]) -> anyhow::Result<()> {
    set_control("version", Some("0.1.10"))?;
    for name in ["fail", "self-update", "stage-ms", "restart-ms", "protocol", "no-progress"] {
        set_control(name, None)?;
    }
    for (name, value) in extra {
        set_control(name, Some(value))?;
    }
    kick()?;
    let env = id.clone();
    check.pump(move |c| server_version(c, &env).as_deref() == Some("0.1.10") && c.connected(), Duration::from_secs(60), "one release behind")?;
    let wanted = icon.to_owned();
    check.pump(move |c| element_visible(c, &wanted), Duration::from_secs(20), "the row's Update icon")?;
    check.wait(Duration::from_millis(800));
    Ok(())
}

fn away(check: &mut Check) -> anyhow::Result<()> {
    hover_at(check, point(px(5.), px(770.)))
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

pub(super) fn run_update_ref(check: &mut Check) -> anyhow::Result<()> {
    set_control("updates.log", None)?;
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| c.connected(), Duration::from_secs(60), "connected through the stand-in")?;
    check.wait(Duration::from_secs(1));
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    let raw = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.to_string());
    let id = t3code_contracts::EnvironmentId::new(raw.as_str());
    let icon = format!("env-update-{raw}");
    let router = check.read(|root, _| root.router().clone());
    let route = Route::Settings { panel: SettingsPanel::Connections, query: SharedString::default() };
    check.cx.update(|cx| router.update(cx, |router, cx| router.navigate(route, cx)));
    reset(check, &id, &icon, &[])?;

    away(check)?;
    check.shot("env-row.png")?;
    hover_element(check, &icon)?;
    check.shot("env-update-tooltip.png")?;
    hover_element(check, "environments-update-all")?;
    check.shot("env-update-all-hover.png")?;

    reset(check, &id, &icon, &[("stage-ms", "6000"), ("restart-ms", "8000")])?;
    click(check, &icon)?;
    away(check)?;
    let env = id.clone();
    check.pump(move |c| matches!(update_state(c, &env), ServerUpdateState::Running { stage: ServerUpdateStage::Downloading, .. }), Duration::from_secs(10), "downloading")?;
    check.wait(Duration::from_millis(1000));
    check.shot("env-downloading.png")?;
    let env = id.clone();
    check.pump(move |c| matches!(update_state(c, &env), ServerUpdateState::Running { stage: ServerUpdateStage::Installing, .. }), Duration::from_secs(15), "installing")?;
    check.wait(Duration::from_millis(1000));
    check.shot("env-installing.png")?;
    let env = id.clone();
    check.pump(move |c| matches!(update_state(c, &env), ServerUpdateState::Running { stage: ServerUpdateStage::Resuming, .. }), Duration::from_secs(15), "restarting")?;
    check.wait(Duration::from_millis(1500));
    check.shot("env-restarting.png")?;
    let env = id.clone();
    check.pump(move |c| update_state(c, &env) == ServerUpdateState::Idle && c.connected(), Duration::from_secs(60), "updated")?;
    check.wait(Duration::from_millis(1000));
    check.shot("env-updated.png")?;

    reset(check, &id, &icon, &[("stage-ms", "300"), ("fail", "Download failed: the release server is unreachable.")])?;
    click(check, &icon)?;
    away(check)?;
    let env = id.clone();
    check.pump(move |c| matches!(update_state(c, &env), ServerUpdateState::Failed { .. }), Duration::from_secs(20), "failed")?;
    check.wait(Duration::from_millis(800));
    check.shot("env-failed.png")?;
    // The failure toast sits over the row's controls until it times out.
    check.wait(Duration::from_secs(6));
    hover_element(check, &icon)?;
    check.shot("env-retry-tooltip.png")?;

    reset(check, &id, &icon, &[("self-update", "none")])?;
    hover_element(check, &icon)?;
    check.shot("env-copy-tooltip.png")?;
    click(check, &icon)?;
    away(check)?;
    check.wait(Duration::from_millis(800));
    let copied = check.cx.update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    check.check("the copy button puts the relaunch command on the clipboard", copied.as_deref().is_some_and(|c| c.contains("0.1.11")), format!("{copied:?}"));
    check.shot("env-copy-toast.png")?;

    reset(check, &id, &icon, &[("self-update", "desktop-managed")])?;
    click(check, &icon)?;
    check.wait(Duration::from_millis(800));
    let confirm = element_visible(check, "confirm-update");
    check.check("a desktop-managed host asks before updating", confirm, "confirm-update");
    check.shot("env-desktop-confirm.png")?;
    click(check, "confirm-cancel")?;

    let log = std::fs::read_to_string(control()?.join("updates.log")).unwrap_or_default();
    check.check(
        "the stand-in saw the progress update and the failed one",
        log.matches("server.updateServerWithProgress").count() == 2 && log.contains("failed:"),
        log.trim().replace('\n', " | "),
    );
    reset(check, &id, &icon, &[])?;
    Ok(())
}
