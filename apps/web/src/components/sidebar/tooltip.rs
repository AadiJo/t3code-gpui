//! A plain tooltip (`tooltip.tsx` default variant) until the shared Tooltip
//! primitive lands: popover fill, 1px border, radius 6px, 12px text.

use gpui_kit::*;

use super::style::ShellColors;
use crate::ActiveAppTheme as _;

/// Tooltip body view. Build one with [`tooltip`].
pub struct ShellTooltip {
    text: SharedString,
}

impl Render for ShellTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ShellColors::from_theme(cx.app_theme());
        div()
            .px_2()
            .py_1()
            .max_w(px(320.))
            .rounded(px(6.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.popover)
            .text_xs()
            .line_height(px(16.5))
            .text_color(colors.foreground)
            .child(self.text.clone())
    }
}

/// A `tooltip(...)` builder for `StatefulInteractiveElement::tooltip`.
pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        cx.new(|_| ShellTooltip {
            text: text.clone(),
        })
        .into()
    }
}
