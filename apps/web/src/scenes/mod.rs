//! Named views that render with fixture data and no server, so the headless
//! `shot` binary can screenshot one part of the UI at a time.

mod app;
mod chat;
mod command_palette;
pub mod connections;
mod i18n;
pub mod composer;
pub mod diffs;
pub mod settings_server;
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
        ("settings-server", settings_server::build),
        ("settings-server-light", settings_server::build_light),
        ("settings-server-claude", settings_server::build_claude),
        ("settings-server-claude-light", settings_server::build_claude_light),
        ("settings-server-add", settings_server::build_add),
        ("settings-server-add-light", settings_server::build_add_light),
        ("settings-server-threads", settings_server::build_threads),
        ("settings-server-threads-light", settings_server::build_threads_light),
        ("settings-server-source-control", settings_server::build_source_control),
        ("settings-server-source-control-light", settings_server::build_source_control_light),
        ("settings-server-task-editor", settings_server::build_task_editor),
        ("settings-server-task-editor-light", settings_server::build_task_editor_light),
        ("settings-server-tasks", settings_server::build_tasks),
        ("settings-server-worktrees", settings_server::build_worktrees),
        ("settings-server-maintenance", settings_server::build_maintenance),
        ("settings-server-devices", settings_server::build_devices),
        ("settings-server-projects", settings_server::build_projects),
        ("settings-server-access", settings_server::build_access),
    ]
}

pub fn scene(name: &str) -> Option<SceneBuilder> {
    scenes().iter().chain(ui::SCENES).find(|(n, _)| *n == name).map(|(_, build)| *build)
}
