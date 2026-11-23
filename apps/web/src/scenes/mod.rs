//! Named views that render with fixture data and no server, so the headless
//! `shot` binary can screenshot one part of the UI at a time.

mod app;
mod chat;
mod command_palette;
pub mod connections;
mod i18n;
pub mod composer;
pub mod diffs;
pub mod settings;
mod sidebar;
mod terminal;
pub mod ui;

use gpui_kit::{AnyView, App, Window};

pub type SceneBuilder = fn(&mut Window, &mut App) -> AnyView;

/// Every scene by name. `shot <name> <out.png>` renders one.
pub fn scenes() -> &'static [(&'static str, SceneBuilder)] {
    &[
        ("app", app::build),
        ("app-mac", app::build_mac),
        ("ui", ui::build),
        ("sidebar", sidebar::build),
        ("chat", chat::build),
        ("chat-light", chat::build_light),
        ("composer", composer::build),
        ("terminal", terminal::build),
        ("terminal-light", terminal::build_light),
        ("terminal-find", terminal::build_find),
        ("i18n", i18n::build),
        ("composer-light", composer::build_light),
        ("composer-popovers", composer::build_popovers),
        ("composer-popovers-light", composer::build_popovers_light),
        ("diffs", diffs::build),
        ("command-palette", command_palette::build),
        ("command-palette-root", command_palette::build_root),
        ("command-palette-nested", command_palette::build_nested),
        ("command-palette-empty", command_palette::build_empty),
        ("connections", connections::build),
        ("connections-add", connections::build_add),
        ("connections-signed-out", connections::build_signed_out),
        ("connections-sign-in", connections::build_sign_in),
        ("connections-empty", connections::build_empty),
        ("settings", settings::build),
        ("settings-appearance", settings::build_appearance),
        ("settings-chat", settings::build_chat),
        ("settings-diffs", settings::build_diffs),
        ("settings-keybindings", settings::build_keybindings),
        ("settings-keybindings-recording", settings::build_keybindings_recording),
        ("settings-voice", settings::build_voice),
        ("settings-snap-shot", settings::build_snap_shot),
        ("settings-labs", settings::build_labs),
        ("settings-archived", settings::build_archived),
        ("settings-about", settings::build_about),
    ]
}

pub fn scene(name: &str) -> Option<SceneBuilder> {
    scenes().iter().chain(ui::SCENES).find(|(n, _)| *n == name).map(|(_, build)| *build)
}
