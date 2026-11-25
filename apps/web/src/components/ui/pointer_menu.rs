//! A context menu its owner keeps in its own state and opens at the pointer, for triggers
//! whose layout the wrapping [`super::menu::ContextMenu`] element would change.

use gpui_kit::{AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Styled as _, Window, anchored, deferred, div, px};

use super::menu::MenuPopup;
use crate::ActiveAppTheme as _;

/// Draws `popup` at `at` over everything. `close` runs after a row is picked and on a press
/// outside it; the owner clears its state there.
pub fn pointer_menu(id: &'static str, at: Point<Pixels>, popup: MenuPopup, close: impl Fn(&mut Window, &mut App) + Clone + 'static, cx: &App) -> AnyElement {
    let dismiss = close.clone();
    // The fork's fallback context menu: `min-w-32`, kept 4px inside the window.
    let popup = popup.context().min_width(cx.app_theme().rem(8.)).on_dismiss(move |window, cx| dismiss(window, cx));
    deferred(
        anchored()
            .position(at)
            .snap_to_window_with_margin(px(4.))
            .child(div().id(id).occlude().on_mouse_down_out(move |_, window, cx| close(window, cx)).child(popup)),
    )
    .with_priority(5)
    .into_any_element()
}
