//! The T3 Code UI, drawn with GPUI. Mirrors `apps/web/src`: a module here has
//! the same path as the TypeScript file it ports, in snake_case.

// `t!` expands to `::t3code_web::i18n::…`, which must also resolve inside this crate.
extern crate self as t3code_web;

pub mod app_root;
pub mod appearance_fonts;
pub mod assets;
pub mod command_palette_store;
pub mod components;
pub mod fixtures;
pub mod connection;
pub mod editor_preferences;
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
pub mod project_scripts;
pub mod remote_open;
pub mod model_search;
pub mod model_selection;
pub mod markdown_pipeline;
pub mod right_panel_store;
pub mod router;
pub mod keybindings;
pub mod navigation_history;
pub mod routes;
pub mod scenes;
pub mod shortcut_modifier_state;
pub mod shortcuts;
pub mod source_control_presentation;
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
pub mod thread_notifications;
pub mod timestamp_format;
pub mod worktree_cleanup;
pub mod ui_state_store;

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
        fonts: Default::default(),
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
    components::sidebar::init(cx);
    components::chat::composer::init(cx);
    components::branch_picker::bind_keys(cx);
    terminal::surface::init(cx);
    components::thread_terminal_drawer::init(cx);
    cx.set_global(terminal_ui_state_store::TerminalUiStateStore::default());
    app_root::init(cx);
    components::diffs::init(cx);
    // Last: app keybindings must follow every widget's bindings.
    shortcuts::init(cx);
    components::files::init(cx);
}
