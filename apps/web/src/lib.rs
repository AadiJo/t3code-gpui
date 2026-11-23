//! The T3 Code UI, drawn with GPUI. Mirrors `apps/web/src`: a module here has
//! the same path as the TypeScript file it ports, in snake_case.

pub mod app_root;
pub mod assets;
pub mod components;
pub mod diff_count;
pub mod diff_file_kind;
pub mod diff_file_order;
pub mod diff_panel_store;
pub mod diff_patch;
pub mod scenes;
pub mod syntax;
pub mod theme_palette;

pub use app_root::AppRoot;
pub use assets::Assets;
pub use theme_palette::{ActiveAppTheme, AppTheme};

use gpui_kit::App;
use theme_palette::{Appearance, ThemeDocument, set_app_theme};

/// Registers components, fonts, and the theme. Call once before opening a window.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    set_app_theme(AppTheme::new(&ThemeDocument::graphite(), Appearance::Dark), cx);
    components::diffs::init(cx);
}
