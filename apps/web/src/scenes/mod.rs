//! Named views that render with fixture data and no server, so the headless
//! `shot` binary can screenshot one part of the UI at a time.

mod app;
mod chat;
mod command_palette;
pub mod connections;
pub mod git_actions;
mod i18n;
mod notifications;
pub mod pull_requests;
pub mod composer;
pub mod diffs;
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
        ("notifications", notifications::build),
        ("notifications-empty", notifications::build_empty),
        ("git-actions", git_actions::build),
        ("git-actions-light", git_actions::build_light),
        ("git-actions-panel", git_actions::build_panel),
        ("git-actions-panel-light", git_actions::build_panel_light),
        ("pull-requests", pull_requests::build),
        ("pull-requests-light", pull_requests::build),
        ("pull-requests-summary", pull_requests::build_summary),
        ("pull-requests-summary-light", pull_requests::build_summary),
        ("pull-requests-guide", pull_requests::build_guide),
        ("pull-requests-guide-light", pull_requests::build_guide),
        ("pull-requests-timeline", pull_requests::build_timeline),
        ("pull-requests-timeline-light", pull_requests::build_timeline),
        ("pull-requests-code", pull_requests::build_code),
        ("pull-requests-code-light", pull_requests::build_code),
        ("issues", pull_requests::build_issues),
        ("issues-light", pull_requests::build_issues),
    ]
}

pub fn scene(name: &str) -> Option<SceneBuilder> {
    scenes().iter().chain(ui::SCENES).find(|(n, _)| *n == name).map(|(_, build)| *build)
}
