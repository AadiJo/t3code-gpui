//! Ports of `apps/web/src/hooks`: the settings and theme stores.

pub mod settings_bridge;
pub mod use_settings;
pub mod use_theme;

use std::path::Path;
use std::sync::Arc;

use gpui_kit::App;

use crate::client_persistence_storage::{DataDir, LocalStorage};
use crate::i18n::runtime::{I18n, apply_language_setting};
use use_settings::{ClientSettingsStore, DesktopSettingsStore, ServerSettingsStore};
use use_theme::ThemeStore;

/// Installs every store on defaults in memory, so any view can read them.
/// Called by [`crate::init`]; the app then calls [`load`].
pub fn init(cx: &mut App) {
    install(cx);
    cx.observe_global::<ClientSettingsStore>(|cx| {
        apply_language(cx);
        settings_bridge::apply(cx);
    })
    .detach();
}

/// Points the stores at the app's data directory and reloads them from
/// its files, and follows server settings when the connection runtime runs.
/// Call once after [`crate::connection::ConnectionRuntime::init`].
pub fn load(data_dir: &Path, cx: &mut App) {
    cx.set_global(DataDir(Some(Arc::from(data_dir))));
    install(cx);
}

fn install(cx: &mut App) {
    LocalStorage::init(cx);
    ClientSettingsStore::init(cx);
    DesktopSettingsStore::init(cx);
    ServerSettingsStore::init(cx);
    ThemeStore::init(cx);
    apply_language(cx);
    settings_bridge::apply(cx);
}

/// Follows the `language` setting.
fn apply_language(cx: &mut App) {
    let language = ClientSettingsStore::get(cx).language.clone();
    if cx.try_global::<I18n>().is_none_or(|i18n| i18n.language != language) {
        apply_language_setting(&language, cx);
    }
}
