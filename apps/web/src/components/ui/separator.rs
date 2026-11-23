//! A 1px rule in the border colour. Ports `separator.tsx`.

use gpui_kit::{App, IntoElement, RenderOnce, Window, div, prelude::*, px};

use crate::ActiveAppTheme as _;

/// Horizontal fills its width; vertical stretches to its parent's height.
#[derive(IntoElement, Default)]
pub struct Separator {
    vertical: bool,
}

impl Separator {
    pub fn horizontal() -> Self {
        Self { vertical: false }
    }

    pub fn vertical() -> Self {
        Self { vertical: true }
    }
}

impl RenderOnce for Separator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let rule = div().flex_none().bg(cx.app_theme().colors.border);
        if self.vertical { rule.w(px(1.)).h_full() } else { rule.h(px(1.)).w_full() }
    }
}
