//! The composer's server update banner. Ports the server update item of
//! `useSystemComposerBanners.tsx`: while the thread's server updates, "Updating server" with the
//! stage; after it failed, "Could not update server" with the host's reason, Retry and Dismiss.
//! An available update is offered by the sidebar's update pill, not here.
//!
//! The auto-balance banner of `useAutoBalanceUpdateBanner.tsx` is not built: it belongs to
//! drafts that pick their machine automatically, which this client does not start.

use gpui_kit::{App, SharedString};
use t3code_client_runtime::connection::server_update::ServerUpdateState;
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::ServerSelfUpdateCapability;

use crate::components::chat::composer::{BannerTone, ComposerBanner, ComposerEvent};
use crate::components::server_update_action::{ServerUpdateStore, ServerUpdateTarget, banner_icon, copy_manual_command, failure_text, stage_label};
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::state::StateBridge;
use crate::components::sidebar::model::ThreadKey;
use crate::version_skew::{config_version_mismatch, dismiss_server_update_failure, is_server_update_failure_dismissed, manual_command_installs};

/// The banner id's prefix; the environment id follows.
const SERVER_UPDATE_BANNER: &str = "server-version:";

/// The thread whose composer shows the banner, so a change of update state redraws it.
#[derive(Default)]
struct Shown {
    key: Option<ThreadKey>,
    observing: bool,
}

impl gpui_kit::Global for Shown {}

/// The server update banner for the thread `key`, if its server is updating or its update
/// failed and the notice was not dismissed.
pub fn server_update_banner(key: &ThreadKey, cx: &mut App) -> Option<ComposerBanner> {
    follow_updates(key, cx);
    let environment_id = EnvironmentId::new(key.environment_id.0.as_ref());
    let config = ServerSettingsStore::config(cx, &environment_id);
    let state = ServerUpdateStore::shown_state(&environment_id, config.as_ref().map(|c| c.environment.server_version.as_str()), cx);
    if state == ServerUpdateState::Idle || is_server_update_failure_dismissed(&environment_id, &state, cx) {
        return None;
    }
    let environments = ConnectionRuntime::state(cx).read(cx).environments.clone();
    // Only a client with several machines names the server.
    let name: Option<SharedString> = (environments.len() > 1).then(|| {
        environments
            .iter()
            .find(|e| e.id() == &environment_id)
            .map(|e| SharedString::from(e.entry.label.clone()))
            .or_else(|| config.as_ref().map(|c| SharedString::from(c.environment.label.clone())))
            .unwrap_or_else(|| SharedString::from(environment_id.as_str().to_owned()))
    });
    let failed = matches!(state, ServerUpdateState::Failed { .. });
    let title = match (&name, failed) {
        (Some(name), true) => t!("Could not update {environment} server", environment = name.as_ref()),
        (Some(name), false) => t!("Updating {environment} server", environment = name.as_ref()),
        (None, true) => t!("Could not update server"),
        (None, false) => t!("Updating server"),
    };
    let description = match &state {
        ServerUpdateState::Failed { failure, .. } => failure_text(failure),
        ServerUpdateState::Running { stage, .. } => stage_label(*stage),
        ServerUpdateState::Idle => return None,
    };
    let action = failed.then(|| retry_target(&environment_id, name.as_ref(), cx)).flatten().map(|target| match target.self_update {
        None if manual_command_installs(target.installation.as_ref()) => t!("Copy update command"),
        None => t!("Copy relaunch command"),
        Some(_) => t!("Retry", context = "verb, retries the server update"),
    });
    Some(ComposerBanner {
        id: format!("{SERVER_UPDATE_BANNER}{}", environment_id.as_str()).into(),
        tone: if failed { BannerTone::Error } else { BannerTone::Neutral },
        icon: banner_icon(&state),
        title,
        description: Some(description),
        action,
        secondary_action: None,
        dismiss: failed.then(|| t!("Dismiss update notice")),
        body: None,
    })
}

/// The icon colour of a banner: a failed server update's icon is `text-error`
/// (`ComposerServerUpdateIcon`); every other banner icon keeps the muted ink.
pub fn banner_icon_color(banner: &ComposerBanner, error: gpui_kit::Hsla) -> Option<gpui_kit::Hsla> {
    (banner.id.starts_with(SERVER_UPDATE_BANNER) && matches!(banner.tone, BannerTone::Error)).then_some(error)
}

/// What Retry updates: the server, to this client's version, unless the host is current or a
/// desktop app runs it.
fn retry_target(environment_id: &EnvironmentId, name: Option<&SharedString>, cx: &mut App) -> Option<ServerUpdateTarget> {
    let config = ServerSettingsStore::config(cx, environment_id);
    let mismatch = config_version_mismatch(config.as_deref(), cx)?;
    let label = match name {
        Some(name) => t!("{environment} server", environment = name.as_ref()),
        None => t!("server", context = "noun, the T3 Code server"),
    };
    let target = ServerUpdateTarget::for_config(environment_id.clone(), label, config.as_deref(), mismatch.client_version);
    // A desktop app relaunches only after a confirmation, which the composer has no room for.
    (target.self_update != Some(ServerSelfUpdateCapability::DesktopManaged)).then_some(target)
}

/// Handles the banner's Retry and Dismiss. Returns whether `event` was this banner's.
pub fn handle_banner_event(key: &ThreadKey, event: &ComposerEvent, cx: &mut App) -> bool {
    let (id, dismissed) = match event {
        ComposerEvent::BannerAction(id) => (id, false),
        ComposerEvent::BannerDismissed(id) => (id, true),
        _ => return false,
    };
    let Some(environment) = id.strip_prefix(SERVER_UPDATE_BANNER) else { return false };
    let environment_id = EnvironmentId::new(environment);
    if dismissed {
        let state = ServerUpdateStore::state(&environment_id, cx);
        dismiss_server_update_failure(&environment_id, &state, cx);
    } else {
        let several = ConnectionRuntime::state(cx).read(cx).environments.len() > 1;
        let name = several
            .then(|| ConnectionRuntime::state(cx).read(cx).environment(&environment_id).map(|e| SharedString::from(e.entry.label.clone())))
            .flatten();
        if let Some(target) = retry_target(&environment_id, name.as_ref(), cx) {
            if target.self_update.is_none() {
                let command = crate::version_skew::manual_server_update_command(&target.target_version, target.installation.as_ref());
                copy_manual_command(&command, manual_command_installs(target.installation.as_ref()), &target.server_label, cx);
            } else {
                ServerUpdateStore::update(target, false, cx).detach();
            }
        }
    }
    refresh(key, cx);
    true
}

/// Redraws the shown thread's composer whenever an update state changes.
fn follow_updates(key: &ThreadKey, cx: &mut App) {
    let shown = cx.default_global::<Shown>();
    shown.key = Some(key.clone());
    if shown.observing {
        return;
    }
    shown.observing = true;
    let states = ServerUpdateStore::entity(cx);
    cx.observe(&states, |_, cx| {
        if let Some(key) = cx.try_global::<Shown>().and_then(|s| s.key.clone()) {
            refresh(&key, cx);
        }
    })
    .detach();
}

fn refresh(key: &ThreadKey, cx: &mut App) {
    if let Some(thread) = StateBridge::threads(cx).read(cx).find(key, cx).cloned() {
        thread.update(cx, |_, cx| cx.emit(crate::thread_sync::LiveThreadEvent::Status));
    }
}
