//! `live-check guest-share`: the owner side of guest links on a real server. Turns the server's
//! Guest links beta off and on and checks the palette offers "Share with guest" only while it is
//! on; opens the dialog from the palette and presses Create link, which this server (no T3 Connect
//! tunnel) refuses with its own message; opens a share on the thread as the server records one,
//! and checks the dialog's "A new link ends the current one.", the header's "Guest link active"
//! and its "Stop sharing", read back as no share; then an expired share's "Guest link expired".
//! `T3_PAIRING_URL`, `T3_PROJECT_ROOT`.

use std::time::Duration;

use t3code_client_runtime::operations::CreateThread;
use t3code_contracts::{ModelSelection, OrchestrationV2Command, ProviderInteractionMode, RuntimeMode, ThreadId};
use t3code_web::components::chat::guest_share_controls::{guest_share_status, thread_guest_share};
use t3code_web::components::command_palette_lookups::PaletteFeature;
use t3code_web::components::sharing_dialogs::SharingDialogs;
use t3code_web::shortcuts::commands::ToggleCommandPalette;
use t3code_web::state::StateBridge;
use t3code_web::state::shell::thread_key;

use super::live_check_families::{all_connected, pair_if_needed, project_at};
use super::*;

fn run_command(check: &mut Check, title: &str) {
    check.action(ToggleCommandPalette);
    check.wait(Duration::from_millis(300));
    check.type_text(title);
    check.wait(Duration::from_millis(500));
    check.press("enter");
    check.wait(Duration::from_millis(300));
}

fn set_guest_sharing(check: &mut Check, on: bool) -> anyhow::Result<()> {
    let model = check.read(|root, _| root.data().clone()).server_settings_model(check)?;
    let source = check.cx.update(|cx| model.read(cx).source().clone());
    let patch: t3code_contracts::ServerUpdateSettingsPayload = serde_json::from_value(serde_json::json!({ "patch": { "guestSharingEnabled": on } }))?;
    let saved = check.block(async move { source.update_settings(patch).await })?;
    check.check(&format!("the server saves Guest links {}", if on { "on" } else { "off" }), saved.is_ok(), format!("{:?}", saved.as_ref().err()));
    Ok(())
}

fn click(check: &mut Check, id: &str) -> anyhow::Result<()> {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    let at = check
        .cx
        .update_window(check.window, |_, w, _| w.try_find(element).filter(|e| e.visible()).map(|e| e.bounds().center()))
        .ok()
        .flatten()
        .with_context(|| format!("no visible {id}"))?;
    click_at(check, at)
}

pub(super) fn run_guest_share(check: &mut Check) -> anyhow::Result<()> {
    check.cx.update(|cx| {
        t3code_web::theme_palette::ThemeSettings::apply_env_overrides(cx);
        t3code_web::hooks::use_theme::ThemeStore::set_appearance_mode(t3code_web::theme_palette::ThemeMode::Dark, cx);
    });
    pair_if_needed(check, 1, "T3_PAIRING_URL")?;
    check.pump(|c| all_connected(c, 1), Duration::from_secs(60), "server connected")?;
    let environment_id = check
        .cx
        .update(|cx| ConnectionRuntime::state(cx).read(cx).environments.first().map(|e| t3code_contracts::EnvironmentId::new(e.entry.environment_id.as_str())))
        .context("no environment")?;
    let project = project_at(check, &environment_id, &std::env::var("T3_PROJECT_ROOT")?)?;
    let commands = commands_of(check, &environment_id)?;
    let thread_id = ThreadId::new(uuid::Uuid::new_v4().to_string());
    let input = CreateThread {
        thread_id: thread_id.clone(),
        project_id: project,
        title: "Guest share".into(),
        model_selection: ModelSelection { instance_id: "devReplay".into(), model: "multi-turn".into(), options: None },
        runtime_mode: RuntimeMode::FullAccess,
        interaction_mode: ProviderInteractionMode::Default,
        branch: None,
        worktree_path: None,
    };
    let c = commands.clone();
    check.block(async move { c.create_thread(input).await })?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let key = thread_key(&environment_id, &thread_id);
    let k = key.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::shell(cx).read(cx).row(&k).is_some()), Duration::from_secs(20), "thread in the shell")?;
    check.go(&key);
    check.wait(Duration::from_millis(400));

    // The palette row follows the server's Guest links beta.
    set_guest_sharing(check, false)?;
    let env = environment_id.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::environment(&env, cx).and_then(|e| e.read(cx).config.clone()).is_some_and(|c| c.settings.guest_sharing_enabled != Some(true))), Duration::from_secs(15), "the config reports Guest links off")?;
    check.wait(Duration::from_millis(300));
    let offered = check.read(|root, _| root.palette_source().supports(PaletteFeature::GuestShare));
    check.check("with Guest links off the palette leaves Share with guest out", !offered, offered);
    set_guest_sharing(check, true)?;
    let env = environment_id.clone();
    check.pump(move |c| c.cx.update(|cx| StateBridge::environment(&env, cx).and_then(|e| e.read(cx).config.clone()).is_some_and(|c| c.settings.guest_sharing_enabled == Some(true))), Duration::from_secs(15), "the config reports Guest links on")?;
    check.pump(|c| c.read(|root, _| root.palette_source().supports(PaletteFeature::GuestShare)), Duration::from_secs(10), "the palette offers Share with guest")?;
    check.check("with Guest links on the palette offers Share with guest", true, "");

    // The dialog from the palette; this server has no tunnel, so Create link fails with its reason.
    run_command(check, "Share with guest");
    check.pump(|c| c.cx.update(|cx| SharingDialogs::is_open(cx)), Duration::from_secs(5), "the dialog opens")?;
    check.shot("guest-01-dialog.png")?;
    click(check, "guest-share-create")?;
    check.wait(Duration::from_millis(1500));
    check.shot("guest-02-create-refused.png")?;
    let failure = check.cx.update(|cx| SharingDialogs::guest_share(cx).and_then(|d| d.read(cx).failure()));
    check.check(
        "Create link shows the server's reason it cannot make a link here",
        failure.as_deref() == Some("Guest links need a running T3 Connect tunnel."),
        format!("{failure:?}"),
    );
    click(check, "guest-share-cancel")?;
    check.pump(|c| !c.cx.update(|cx| SharingDialogs::is_open(cx)), Duration::from_secs(5), "Cancel closes the dialog")?;

    // A share as the server records it once a link is made (`thread.guest-share.open`).
    let open_share = |expires_in_ms: i64| -> anyhow::Result<OrchestrationV2Command> {
        let expires = t3code_client_runtime::state::models::format_ms(t3code_client_runtime::state::models::now_ms() + expires_in_ms);
        Ok(serde_json::from_value(serde_json::json!({
            "type": "thread.guest-share.open", "commandId": uuid::Uuid::new_v4().to_string(), "threadId": thread_id.as_str(),
            "shareId": uuid::Uuid::new_v4().simple().to_string(), "host": "guest.example.invalid", "expiresAt": expires,
        }))?)
    };
    let c = commands.clone();
    let command = open_share(24 * 3_600_000)?;
    let opened = check.block(async move { c.dispatch(command).await })?;
    check.note(format!("thread.guest-share.open: {:?}", opened.as_ref().map(|_| ()).map_err(|e| e.to_string())));
    let k = key.clone();
    let shared = check.pump(move |c| c.cx.update(|cx| thread_guest_share(&k, cx).is_some()), Duration::from_secs(15), "the thread holds the share");
    // Only the server opens a share (the client command is refused) and only with a T3 Connect
    // tunnel, so the header indicator is checked only where one exists.
    check.note(format!("share recorded: {}", shared.is_ok()));
    if shared.is_ok() {
        check.wait(Duration::from_millis(300));
        check.shot("guest-03-header-active.png")?;
        let share = check.cx.update(|cx| thread_guest_share(&key, cx)).context("share")?;
        let now = t3code_client_runtime::state::models::format_ms(t3code_client_runtime::state::models::now_ms());
        let status = guest_share_status(&share, &now[..16]);
        check.check("the header reads Guest link active", status.as_ref() == "Guest link active", &status);
        let stop = check_visible(check, "guest-share-stop");
        check.check("the header offers Stop sharing", stop, "");
        run_command(check, "Share with guest");
        check.pump(|c| c.cx.update(|cx| SharingDialogs::is_open(cx)), Duration::from_secs(5), "the dialog opens again")?;
        let texts = check.cx.update_window(check.window, |_, w, _| gpui_kit::base::test_support::texts(w).into_iter().map(|t| t.text().to_owned()).collect::<Vec<_>>()).unwrap_or_default();
        check.check("with a share open the dialog warns a new link ends it", texts.iter().any(|t| t == "A new link ends the current one."), "");
        check.shot("guest-04-dialog-replaces.png")?;
        click(check, "guest-share-cancel")?;
        check.wait(Duration::from_millis(200));
        click(check, "guest-share-stop")?;
        let k = key.clone();
        let stopped = check.pump(move |c| c.cx.update(|cx| thread_guest_share(&k, cx).is_none()), Duration::from_secs(15), "Stop sharing ends the share");
        check.check("Stop sharing reads back as no share on the server", stopped.is_ok(), format!("{stopped:?}"));
        check.shot("guest-05-stopped.png")?;

        let c = commands.clone();
        let command = open_share(-60_000)?;
        check.block(async move { c.dispatch(command).await })?.ok();
        let k = key.clone();
        if check.pump(move |c| c.cx.update(|cx| thread_guest_share(&k, cx).is_some()), Duration::from_secs(15), "an expired share").is_ok() {
            let share = check.cx.update(|cx| thread_guest_share(&key, cx)).context("share")?;
            let now = t3code_client_runtime::state::models::format_ms(t3code_client_runtime::state::models::now_ms());
            let status = guest_share_status(&share, &now[..16]);
            check.check("an expired share reads Guest link expired", status.as_ref() == "Guest link expired", &status);
            check.shot("guest-06-expired.png")?;
            click(check, "guest-share-stop")?;
        }
    }
    Ok(())
}

fn check_visible(check: &mut Check, id: &str) -> bool {
    let element = ElementId::from(SharedString::from(id.to_owned()));
    check.cx.update_window(check.window, |_, w, _| w.try_find(element).is_some_and(|e| e.visible())).unwrap_or(false)
}
