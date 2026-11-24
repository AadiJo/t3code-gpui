//! Dragging a thread row out of the palette. Ports `CommandPalette.drag.ts`:
//! the palette closes as the drag starts, and a drop target such as the
//! composer accepts [`PaletteThreadDrag`] with `.on_drop(...)`.

use crate::components::ui::GlassExt as _;
use gpui_kit::{Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div};

use super::sidebar::model::ThreadKey;
use super::ui::TextStyleExt as _;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};

/// A thread dragged from the palette, for drop targets to attach as context.
#[derive(Clone, Debug)]
pub struct PaletteThreadDrag {
    pub thread: ThreadKey,
    pub title: SharedString,
}

impl Render for PaletteThreadDrag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        div()
            .px(theme.sp(2.))
            .py(theme.sp(1.))
            .rounded(theme.radii.md)
            .border_1()
            .border_color(theme.derived.dropdown_glass_border)
            .glass(theme.derived.glass_dropdown)
            .shadow(theme.shadows.md5.clone())
            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
            .text_color(theme.colors.foreground())
            .child(self.title.clone())
    }
}
