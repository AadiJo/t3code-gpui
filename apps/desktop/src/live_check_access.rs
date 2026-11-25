//! `live-check access-controls`: Settings → Sharing & access on a real server
//! (`T3_PAIRING_URL`, its direct address `T3_SERVER_ORIGIN`). Creates a pairing link from
//! Authorized clients' Create link (label and the Standard preset), reads it back from the access
//! stream, signs a second client in with that one-time code (`/api/auth/browser-session`), revokes
//! that client from its row, signs two more in and uses Revoke others, each read back from the
//! server's own access stream; then turns device approval on and checks the device coordinator
//! follows that environment and that Use in another app opens the wizard for it.

use std::time::Duration;

use t3code_contracts::{AuthAccessSnapshot, ClientServerSettingsPatch, EnvironmentId};
use t3code_web::components::sharing_dialogs::SharingDialogs;
use t3code_web::hooks::use_settings::ServerSettingsStore;
use t3code_web::router::Route;

use super::live_check_families::{all_connected, pair_if_needed};
use super::*;

pub(super) fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible {id}"))?;
    click_at(check, at)
}

/// The server's access state, read on a fresh `subscribeAuthAccess` (its first snapshot).
pub(super) fn server_access(check: &mut Check, id: &EnvironmentId) -> anyhow::Result<AuthAccessSnapshot> {
    let link = check.cx.update(|cx| ConnectionRuntime::global(cx).environment_link(id)).context("no link")?;
    let mut events = check.cx.update(|cx| t3code_web::components::settings::connections_settings_access_clients::follow_access(link, cx));
    let first = check.block(async move {
        use futures::StreamExt as _;
        let mut snapshot = None;
        while let Some(event) = events.next().await {
            t3code_web::components::settings::connections_settings_access_clients::apply_access_event(&mut snapshot, event);
            if snapshot.is_some() {
                break;
            }
        }
        snapshot
    })?;
    first.context("no snapshot")
}

/// Signs a client in with a one-time code, as a browser on another device would.
fn sign_in_with(code: &str, agent: &str) -> anyhow::Result<()> {
    let origin = std::env::var("T3_SERVER_ORIGIN").context("T3_SERVER_ORIGIN")?;
    let status = std::process::Command::new("curl")
        .args(["-s", "-o", "/dev/null", "-w", "%{http_code}", "-A", agent, "-H", "content-type: application/json", "-X", "POST"])
        .arg(format!("{origin}/api/auth/browser-session"))
        .arg("--data")
        .arg(format!(r#"{{"credential":"{code}"}}"#))
        .output()?;
    let code = String::from_utf8_lossy(&status.stdout).to_string();
    anyhow::ensure!(code == "200", "browser-session answered {code}");
    Ok(())
}

pub(super) fn panel_access(check: &mut Check) -> Option<AuthAccessSnapshot> {
    check.cx.update(|cx| t3code_web::components::settings::connections_settings_access::shown_access(cx))
}

pub(super) fn run_access_controls(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.clone());
    // Start from this client alone.
    let link = check.cx.update(|cx| ConnectionRuntime::global(cx).environment_link(&id)).context("no link")?;
    let _ = check.block(async move { t3code_client_runtime::state::connect_device_approval::revoke_other_clients(&link).await });
    let root = check.root.clone();
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |r, cx| r.navigate(Route::parse("/settings/access"), cx));
    });
    check.pump(|c| panel_access(c).is_some(), Duration::from_secs(20), "the access page follows the access stream")?;
    check.cx.update(t3code_web::components::settings::connections_settings_access::open_clients);
    check.wait(Duration::from_millis(300));
    check.shot("access-01-clients.png")?;

    // Create link.
    click(check, "access-create-link")?;
    check.wait(Duration::from_millis(300));
    click(check, "pairing-label")?;
    check.type_text("Phone");
    click(check, "pairing-preset-standard")?;
    check.wait(Duration::from_millis(200));
    check.shot("access-02-create-link.png")?;
    click(check, "pairing-create")?;
    let made = check.pump(|c| panel_access(c).is_some_and(|a| a.pairing_links.iter().any(|l| l.label.as_deref() == Some("Phone"))), Duration::from_secs(15), "the link shows");
    check.check("Create link makes a labelled pairing link and the stream shows it", made.is_ok(), format!("{made:?}"));
    let on_server = server_access(check, &id)?;
    let created = on_server.pairing_links.iter().find(|l| l.label.as_deref() == Some("Phone")).cloned();
    check.check(
        "the server holds the Phone link with the Standard scopes",
        created.as_ref().and_then(|l| l.permissions.clone()).is_some_and(|p| p.len() == 16 && !p.iter().any(|s| s.as_str() == "relay:configure")),
        format!("{:?}", created.as_ref().map(|l| l.permissions.as_ref().map(|p| p.iter().map(|s| s.as_str()).collect::<Vec<_>>()))),
    );
    let code = check.cx.update(|cx| t3code_web::components::settings::connections_settings_access::created_credential("Phone", cx)).context("the new code")?;
    check.wait(Duration::from_millis(300));
    check.shot("access-03-link-created.png")?;

    // A second client signs in with it, then is revoked from its row.
    sign_in_with(&code, "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Safari/605.1.15")?;
    let two = check.pump(|c| panel_access(c).is_some_and(|a| a.client_sessions.len() == 2), Duration::from_secs(15), "a second client");
    check.check("a client signed in with the code shows under Authorized clients", two.is_ok(), format!("{two:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("access-04-two-clients.png")?;
    let other = panel_access(check).and_then(|a| a.client_sessions.into_iter().find(|s| !s.current)).context("the other client")?;
    click(check, &format!("access-revoke-client-{}", other.session_id.as_str()))?;
    let one = check.pump(|c| panel_access(c).is_some_and(|a| a.client_sessions.len() == 1), Duration::from_secs(15), "one client");
    check.check("Revoke on its row removes that client", one.is_ok(), format!("{one:?}"));
    let on_server = server_access(check, &id)?;
    check.check("the server no longer holds it", !on_server.client_sessions.iter().any(|s| s.session_id == other.session_id), on_server.client_sessions.len());

    // Two more, then Revoke others.
    for agent in ["Mozilla/5.0 (X11; Linux x86_64) Firefox/133.0", "Mozilla/5.0 (Windows NT 10.0) Chrome/131.0"] {
        let link = check.cx.update(|cx| ConnectionRuntime::global(cx).environment_link(&id)).context("no link")?;
        let made = check.block(async move { t3code_client_runtime::state::connect_device_approval::create_pairing_link(&link, None, vec![t3code_contracts::AuthGrantScope::OrchestrationRead]).await })?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        sign_in_with(&made.credential, agent)?;
    }
    check.pump(|c| panel_access(c).is_some_and(|a| a.client_sessions.len() == 3), Duration::from_secs(15), "three clients")?;
    check.wait(Duration::from_millis(300));
    check.shot("access-05-three-clients.png")?;
    click(check, "access-revoke-others")?;
    let alone = check.pump(|c| panel_access(c).is_some_and(|a| a.client_sessions.len() == 1 && a.client_sessions[0].current), Duration::from_secs(15), "this client alone");
    check.check("Revoke others leaves this device alone", alone.is_ok(), format!("{alone:?}"));
    let on_server = server_access(check, &id)?;
    check.check("the server holds only this client", on_server.client_sessions.len() == 1 && on_server.client_sessions[0].current, on_server.client_sessions.len());
    check.wait(Duration::from_millis(300));
    check.shot("access-06-revoked-others.png")?;

    // Device approval on: the coordinator follows this environment.
    let target = id.clone();
    let task = check.cx.update(move |cx| ServerSettingsStore::update(target, ClientServerSettingsPatch { require_connect_device_approval: Some(true), ..Default::default() }, cx));
    let saved = check.wait_task(task)?;
    check.check("the server saves Require approval for new devices", saved.is_ok(), format!("{:?}", saved.err().map(|e| e.0)));
    let pending = check.cx.update(|cx| t3code_web::components::connect_device_approval_coordinator::ConnectDeviceApprovalCoordinator::pending(cx));
    check.note(format!("waiting devices: {}", pending.len()));

    // Use in another app opens the wizard for this environment.
    click(check, "access-use-in-another-app")?;
    let opened = check.pump(|c| c.cx.update(|cx| SharingDialogs::use_in_another_app(cx).is_some()), Duration::from_secs(5), "the wizard");
    check.check("Use in another app (MCP) opens the wizard", opened.is_ok(), format!("{opened:?}"));
    check.wait(Duration::from_millis(300));
    check.shot("access-07-use-in-another-app.png")?;
    check.cx.update(SharingDialogs::close);
    Ok(())
}

/// `live-check device-approval`: a T3 Connect device asks to connect to a server that requires
/// approval (run `relay-check approval` against the same stand-in at the same time). The device
/// coordinator raises its toast; Sharing & access lists it under T3 Connect devices with its
/// code; Approve trusts it (read back from the server); Revoke removes it. With approval off, the
/// device signs straight back in and Revoke first offers to turn approval on, which is saved and
/// read back.
pub(super) fn run_device_approval(check: &mut Check) -> anyhow::Result<()> {
    use t3code_web::components::connect_device_approval_coordinator::ConnectDeviceApprovalCoordinator;
    check.cx.update(|cx| {
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let id = check.cx.update(|cx| ConnectionRuntime::state(cx).read(cx).environments[0].entry.environment_id.clone());
    let target = id.clone();
    let task = check.cx.update(move |cx| ServerSettingsStore::update(target, ClientServerSettingsPatch { require_connect_device_approval: Some(true), ..Default::default() }, cx));
    check.wait_task(task)?.map_err(|e| anyhow::anyhow!("{}", e.0))?;
    let toast = check.pump(|c| !c.cx.update(|cx| ConnectDeviceApprovalCoordinator::pending(cx)).is_empty(), Duration::from_secs(90), "a waiting device");
    check.check("a device waiting for approval raises the coordinator's toast", toast.is_ok(), format!("{toast:?}"));
    check.wait(Duration::from_millis(400));
    check.shot("device-01-toast.png")?;
    let root = check.root.clone();
    check.cx.update(|cx| {
        let router = root.read(cx).router().clone();
        router.update(cx, |r, cx| r.navigate(Route::parse("/settings/access"), cx));
    });
    check.pump(|c| panel_access(c).is_some_and(|a| a.pairing_links.iter().any(|l| l.connect_device_request.is_some())), Duration::from_secs(20), "the request on the page")?;
    let request = panel_access(check).and_then(|a| a.pairing_links.into_iter().find(|l| l.connect_device_request.is_some())).context("request")?;
    let code = request.connect_device_request.as_ref().map(|r| r.verification_code.clone()).unwrap_or_default();
    check.wait(Duration::from_millis(300));
    check.shot("device-02-waiting.png")?;
    let listed = visible(check, &format!("device-approve-{}", request.id));
    check.check("T3 Connect devices lists the waiting device with Approve", listed, &code);
    click(check, &format!("device-approve-{}", request.id))?;
    let trusted = check.pump(move |c| panel_access(c).is_some_and(|a| a.connect_devices.as_ref().is_some_and(|d| !d.is_empty())), Duration::from_secs(20), "a trusted device");
    check.check("Approve trusts the device", trusted.is_ok(), format!("{trusted:?}"));
    let on_server = server_access(check, &id)?;
    let device = on_server.connect_devices.clone().unwrap_or_default().into_iter().find(|d| d.verification_code == code);
    check.check("the server trusts the device with the code shown", device.is_some(), &code);
    check.wait(Duration::from_millis(300));
    check.shot("device-03-trusted.png")?;
    let device = device.context("device")?;
    click(check, &format!("device-revoke-{}", device.proof_key_thumbprint))?;
    let thumb = device.proof_key_thumbprint.clone();
    let gone = check.pump(move |c| panel_access(c).is_some_and(|a| !a.connect_devices.as_ref().is_some_and(|d| d.iter().any(|x| x.proof_key_thumbprint == thumb))), Duration::from_secs(20), "revoked");
    check.check("with approval on, Revoke removes the device at once", gone.is_ok(), format!("{gone:?}"));
    let on_server = server_access(check, &id)?;
    check.check("the server no longer trusts it", !on_server.connect_devices.unwrap_or_default().iter().any(|d| d.proof_key_thumbprint == device.proof_key_thumbprint), "");
    Ok(())
}

pub(super) fn visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}
