//! Keyboard shortcut hint. Ports `kbd.tsx`.

use gpui_kit::{App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::TextStyleExt as _;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};

/// `default` is a tinted chip; `outline` draws a physical key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KbdVariant {
    #[default]
    Default,
    Outline,
}

/// One key or chord, such as "⌘K".
#[derive(IntoElement)]
pub struct Kbd {
    keys: SharedString,
    variant: KbdVariant,
}

impl Kbd {
    pub fn new(keys: impl Into<SharedString>) -> Self {
        Self { keys: keys.into(), variant: KbdVariant::Default }
    }

    pub fn variant(mut self, variant: KbdVariant) -> Self {
        self.variant = variant;
        self
    }
}

impl RenderOnce for Kbd {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let kbd = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(theme.sp(1.))
            .h(theme.sp(5.))
            .min_w(theme.sp(5.))
            .rounded(px(4.))
            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
            .text_color(theme.colors.muted_foreground)
            .child(self.keys);
        match self.variant {
            KbdVariant::Default => kbd.px(theme.sp(1.)).bg(theme.derived.kbd),
            KbdVariant::Outline => kbd
                .px(theme.sp(1.5))
                .border_1()
                .border_b_2()
                .border_color(theme.derived.kbd_outline_border),
        }
    }
}
