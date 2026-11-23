use gpui_kit::{component::TitleBar, *};
use t3code_web::AppRoot;

/// Options for the main window: client-drawn titlebar, the web app's default size.
pub fn main_window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(None, size(px(1100.), px(780.)), cx)),
        window_min_size: Some(size(px(400.), px(360.))),
        ..TitleBar::window_options()
    }
}

/// Opens the main window with the app root as its view.
pub fn open_main_window(
    options: WindowOptions,
    cx: &mut App,
) -> anyhow::Result<(AnyWindowHandle, Entity<AppRoot>)> {
    gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| AppRoot::new(window, cx)))
}
