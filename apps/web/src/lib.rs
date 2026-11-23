//! The T3 Code UI, drawn with GPUI. Mirrors `apps/web/src`: a module here has
//! the same path as the TypeScript file it ports, in snake_case.

pub mod app_root;
pub mod assets;

pub use app_root::AppRoot;
pub use assets::Assets;

use gpui_kit::App;

/// Registers components, fonts, and the theme. Call once before opening a window.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
}
