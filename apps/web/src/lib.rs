//! The T3 Code UI, drawn with GPUI. Mirrors `apps/web/src`: a module here has
//! the same path as the TypeScript file it ports, in snake_case.

// `t!` expands to `::t3code_web::i18n::…`, which must also resolve inside this crate.
extern crate self as t3code_web;

pub mod app_root;
pub mod quit_hold;
pub mod thread_undo;
pub mod appearance_contrast;
pub mod appearance_fonts;
pub mod assets;
pub mod client_persistence_storage;
pub mod browser;
pub mod browser_history_store;
pub mod command_palette_store;
pub mod components;
pub mod fixtures;
pub mod connection;
pub mod editor_preferences;
pub mod hooks;
pub mod i18n;
pub mod composer_code_block;
pub mod composer_code_languages;
pub mod composer_code_theme;
pub mod composer_draft_store;
pub mod composer_editor_mentions;
pub mod composer_list_continuation;
pub mod composer_logic;
pub mod composer_rich_text;
pub mod composer_rich_text_doc;
pub mod composer_text_direction;
pub mod composer_undo_grouping;
pub mod desktop_notifications;
pub mod model_ordering;
pub mod port_discovery_state;
pub mod project_icon_colors;
pub mod project_scripts;
pub mod remote_open;
pub mod model_search;
pub mod model_selection;
pub mod markdown_pipeline;
pub mod right_panel_store;
pub mod panel_animations;
pub mod router;
pub mod keybindings;
pub mod live;
pub mod navigation_history;
pub mod project_identity;
pub mod provider_auth_delivery;
pub mod provider_instances;
pub mod provider_models;
pub mod provider_skill_search;
pub mod routes;
pub mod scenes;
pub mod session_logic;
pub mod work_entry_labels;
pub mod provider_models_live;
pub mod state;
pub mod thread_sync;
pub mod shortcut_modifier_state;
pub mod shortcuts;
pub mod source_control_presentation;
pub mod thread_pull_requests;
pub mod terminal;
pub mod terminal_links;
pub mod terminal_ui_state_store;
pub mod text_direction;
pub mod theme_color;
pub mod theme_component;
pub mod diff_count;
pub mod diff_file_kind;
pub mod diff_file_order;
pub mod diff_panel_store;
pub mod diff_patch;
pub mod file_context_menu;
pub mod file_path_display;
pub mod file_preview;
pub mod file_source;
pub mod pierre_icons;
pub mod syntax;
pub mod theme_palette;
pub mod theme_tokens;
pub mod thread_routes;
pub mod thread_find_store;
pub mod thread_notifications;
pub mod timestamp_format;
pub mod worktree_cleanup;
pub mod ui_state_store;
pub mod version_skew;
pub mod open_vsx_themes;
pub mod vscode_theme_import;

pub use app_root::AppRoot;
pub use assets::Assets;
pub use theme_palette::{ActiveAppTheme, AppTheme};

use std::sync::Arc;

use gpui_kit::App;
use theme_palette::{Appearance, ThemeDocument, ThemeMode, ThemeSettings};

/// Registers components, fonts, and the theme. Call once before opening a window.
/// The theme starts as Graphite following the OS appearance; call
/// [`ThemeSettings::follow_window`] for each window so it tracks OS changes
/// and uses the Interface size as its rem. Render `Toaster::view(cx)` once
/// at each window root for toasts.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    if let Err(error) = appearance_fonts::load_fonts(cx) {
        eprintln!("failed to load bundled fonts: {error:#}");
    }
    let settings = ThemeSettings {
        document: Arc::new(ThemeDocument::graphite()),
        mode: ThemeMode::System,
        system: Appearance::from(cx.window_appearance()),
        ..Default::default()
    };
    theme_palette::install(settings, cx);
    components::ui::toast::Toaster::init(cx);
    i18n::runtime::init(cx);
    desktop_notifications::NotificationInbox::init(cx);
    components::background_thread_announcer::BackgroundThreadAnnouncer::init(cx);
    components::thread_notification_coordinator::ThreadNotificationCoordinator::init(
        cfg!(any(target_os = "macos", target_os = "windows")),
        Default::default(),
        cx,
    );
    hooks::init(cx);
    components::sidebar::init(cx);
    components::chat::composer::init(cx);
    components::chat::thread_find_bar::init(cx);
    components::branch_picker::bind_keys(cx);
    terminal::surface::init(cx);
    cx.set_global(terminal_ui_state_store::TerminalUiStateStore::default());
    app_root::init(cx);
    components::diffs::init(cx);
    components::files::init(cx);
    components::pull_request::init(cx);
    // Last: app keybindings must follow every widget's bindings.
    shortcuts::init(cx);
}
