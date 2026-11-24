//! The device preference pages of Settings with a fixture environment whose
//! server settings and keybindings come from a made-up config, so pages that
//! read or write server settings render without a server.

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::ServerConfig;
use t3code_shared::keybindings::default_resolved_keybindings;

use crate::AppRoot;
use crate::components::settings::settings_search::SettingsPath;
use crate::components::ui::dialog::Modal;
use crate::connection::ConnectionsState;
use crate::hooks::use_settings::ServerSettingsStore;
use crate::routes::settings::SettingsRoute;

/// The fixture environment's id.
pub const FIXTURE_ENVIRONMENT: &str = "env-fixture";

/// A server config for a made-up host with the default keymap and the
/// built-in server settings, like the reference capture's server (which
/// reports no fast worktree checkout).
pub fn fixture_config() -> ServerConfig {
    let keybindings = serde_json::to_value(default_resolved_keybindings()).expect("keybindings encode");
    let config = serde_json::json!({
        "environment": {
            "environmentId": FIXTURE_ENVIRONMENT,
            "label": "Studio",
            "platform": { "os": "darwin", "arch": "arm64" },
            "serverVersion": "0.1.11",
            "capabilities": {}
        },
        "auth": {
            "policy": "loopback-browser",
            "bootstrapMethods": [],
            "sessionMethods": [],
            "sessionCookieName": "t3_session"
        },
        "cwd": "/srv/projects",
        "keybindingsConfigPath": "~/.t3/userdata/keybindings.json",
        "keybindings": keybindings,
        "issues": [],
        "providers": [],
        "availableEditors": [],
        "observability": {
            "logsDirectoryPath": "/srv/logs",
            "localTracingEnabled": false,
            "otlpTracesEnabled": false,
            "otlpMetricsEnabled": false
        },
        "settings": {}
    });
    serde_json::from_value(config).expect("fixture server config decodes")
}

/// Seeds the fixture environment into the server settings store.
pub fn seed_fixture(cx: &mut App) {
    ServerSettingsStore::insert_fixture(EnvironmentId::from(FIXTURE_ENVIRONMENT), fixture_config(), cx);
}

struct SettingsScene {
    app: Entity<AppRoot>,
    settings: Entity<SettingsRoute>,
}

impl Render for SettingsScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.app.clone()).child(Modal::dialog("settings-route", self.settings.clone(), |_, _| {}))
    }
}

/// Loads a client settings document (`T3CODE_CLIENT_SETTINGS=<path to a
/// T3 Code client settings JSON>`) over the defaults, so a scene can render
/// with someone's real settings without them living in the repo.
pub fn apply_client_settings_override(cx: &mut App) {
    let Some(path) = std::env::var_os("T3CODE_CLIENT_SETTINGS") else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    let Ok(serde_json::Value::Object(document)) = serde_json::from_slice(&bytes) else { return };
    let (settings, _) = crate::hooks::use_settings::ClientSettings::from_document(document);
    crate::hooks::use_settings::ClientSettingsStore::update(cx, |current| *current = settings);
}

/// Opens Settings on `path` over the app shell, with the fixture environment.
pub fn open(path: SettingsPath, window: &mut Window, cx: &mut App) -> (AnyView, Entity<SettingsRoute>) {
    seed_fixture(cx);
    apply_client_settings_override(cx);
    let state = cx.new(|_| ConnectionsState {
        environments: Vec::new(),
        cloud: None,
        verification_code: "ABCD-1234".into(),
        electron_import_available: false,
    });
    let app = cx.new(|cx| AppRoot::new(window, cx));
    let settings = cx.new(|cx| SettingsRoute::with_state(path, state, window, cx));
    let view: AnyView = cx.new(|_| SettingsScene { app, settings: settings.clone() }).into();
    (view, settings)
}

fn page(path: SettingsPath) -> fn(&mut Window, &mut App) -> AnyView {
    match path {
        SettingsPath::General => |w, cx| open(SettingsPath::General, w, cx).0,
        SettingsPath::Appearance => |w, cx| open(SettingsPath::Appearance, w, cx).0,
        SettingsPath::Chat => |w, cx| open(SettingsPath::Chat, w, cx).0,
        SettingsPath::Diffs => |w, cx| open(SettingsPath::Diffs, w, cx).0,
        SettingsPath::Keybindings => |w, cx| open(SettingsPath::Keybindings, w, cx).0,
        SettingsPath::Voice => |w, cx| open(SettingsPath::Voice, w, cx).0,
        SettingsPath::SnapShot => |w, cx| open(SettingsPath::SnapShot, w, cx).0,
        SettingsPath::Labs => |w, cx| open(SettingsPath::Labs, w, cx).0,
        SettingsPath::Archived => |w, cx| open(SettingsPath::Archived, w, cx).0,
        _ => |w, cx| open(SettingsPath::About, w, cx).0,
    }
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::General)(window, cx)
}

pub fn build_appearance(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Appearance)(window, cx)
}

pub fn build_chat(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Chat)(window, cx)
}

pub fn build_diffs(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Diffs)(window, cx)
}

pub fn build_keybindings(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Keybindings)(window, cx)
}

/// Keybindings with a shortcut being recorded on "Command Palette: Toggle".
pub fn build_keybindings_recording(window: &mut Window, cx: &mut App) -> AnyView {
    let (view, settings) = open(SettingsPath::Keybindings, window, cx);
    let keybindings = settings.read(cx).pages().read(cx).keybindings().clone();
    keybindings.update(cx, |page, cx| page.start_recording_for_scene("commandPalette.toggle", window, cx));
    view
}

pub fn build_voice(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Voice)(window, cx)
}

pub fn build_snap_shot(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::SnapShot)(window, cx)
}

pub fn build_labs(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Labs)(window, cx)
}

pub fn build_archived(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::Archived)(window, cx)
}

pub fn build_about(window: &mut Window, cx: &mut App) -> AnyView {
    page(SettingsPath::About)(window, cx)
}
