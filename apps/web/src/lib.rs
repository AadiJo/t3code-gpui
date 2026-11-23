//! The T3 Code UI, drawn with GPUI. Mirrors `apps/web/src`: a module here has
//! the same path as the TypeScript file it ports, in snake_case.

pub mod app_root;
pub mod appearance_fonts;
pub mod assets;
pub mod components;
pub mod scenes;
pub mod theme_color;
pub mod theme_component;
pub mod theme_palette;
pub mod theme_tokens;

pub use app_root::AppRoot;
pub use assets::Assets;
pub use theme_palette::{ActiveAppTheme, AppTheme};

use std::sync::Arc;

use gpui_kit::App;
use theme_palette::{Appearance, ThemeDocument, ThemeMode, ThemeSettings};

/// Registers components, fonts, and the theme. Call once before opening a window.
/// The theme starts as Graphite following the OS appearance; call
/// [`ThemeSettings::follow_window`] for each window so it tracks changes.
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
}
