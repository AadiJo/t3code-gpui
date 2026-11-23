//! Scroll area. Ports `scroll-area.tsx`: native scrolling with the app's
//! overlay scrollbar (6px rounded thumb, transparent track), styled through
//! the gpui-base scrollbar theme set in `theme_component`.

use gpui_kit::{
    Div, ElementId, Stateful,
    component::scroll::{ScrollableElement as _, Scrollable},
    div, prelude::*,
};

/// A vertically scrolling container with the app's scrollbar. Add children and size it as usual.
pub fn scroll_area(id: impl Into<ElementId>) -> Scrollable<Stateful<Div>> {
    div().id(id).overflow_y_scrollbar()
}
