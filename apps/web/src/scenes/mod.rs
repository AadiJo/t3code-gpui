//! Named views that render with fixture data and no server, so the headless
//! `shot` binary can screenshot one part of the UI at a time.

mod app;
mod chat;
mod i18n;
mod notifications;
pub mod composer;
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
        ("notifications", notifications::build),
        ("notifications-empty", notifications::build_empty),
    ]
}

pub fn scene(name: &str) -> Option<SceneBuilder> {
    scenes().iter().chain(ui::SCENES).find(|(n, _)| *n == name).map(|(_, build)| *build)
}
