//! Shared UI primitives. Ports `components/ui`. Each primitive computes its
//! colours for an interaction state in one function, used both for the live
//! `hover`/`active` styles and for a state forced by the gallery.

pub mod badge;
pub mod button;
pub mod checkbox;
pub mod dialog;
pub mod empty;
pub mod input;
pub mod kbd;
pub mod menu;
pub mod motion;
pub mod popover;
pub mod scroll_area;
pub mod select;
pub mod separator;
pub mod sheet;
pub mod skeleton;
pub mod spinner;
pub mod switch;
pub mod toast;
pub mod toggle;
pub mod tooltip;

use gpui_kit::{
    AbsoluteLength, BoxShadow, FontWeight, Hsla, Pixels, Styled, div, prelude::*, px,
};

use crate::AppTheme;
use crate::appearance_fonts::{TextSize, mono_font, ui_font};

/// An interaction state. Primitives take one to draw that state statically,
/// which is how the gallery shows hover, pressed and focus without a pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ControlState {
    #[default]
    Idle,
    Hover,
    Active,
    FocusVisible,
    Disabled,
}

impl ControlState {
    pub const ALL: [ControlState; 5] =
        [Self::Idle, Self::Hover, Self::Active, Self::FocusVisible, Self::Disabled];

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "default",
            Self::Hover => "hover",
            Self::Active => "active",
            Self::FocusVisible => "focus-visible",
            Self::Disabled => "disabled",
        }
    }
}

/// `opacity-64` on a disabled control.
pub const DISABLED_OPACITY: f32 = 0.64;

/// Text styling in the app's type scale.
pub trait TextStyleExt: Styled + Sized {
    /// UI font at a scale step and weight, with the step's line height.
    fn ui_text(self, theme: &AppTheme, size: TextSize, weight: FontWeight) -> Self {
        let rem = theme.fonts.interface;
        self.font(ui_font(weight))
            .text_size(px(size.size(rem)))
            .line_height(px(size.line_height(rem)))
    }

    /// Mono font at a pixel size.
    fn mono_text(self, size: Pixels, weight: FontWeight) -> Self {
        self.font(mono_font(weight)).text_size(AbsoluteLength::Pixels(size))
    }
}

impl<T: Styled> TextStyleExt for T {}

/// The focus-visible ring of buttons, badges and switches:
/// `ring-2 ring-inset ring-ring ring-offset-1 ring-offset-background`.
/// Inside the border: 1px of background, then 2px of ring.
pub fn focus_ring_inset(theme: &AppTheme) -> Vec<BoxShadow> {
    let ring = BoxShadow::new(px(0.), px(0.), theme.colors.ring()).spread_radius(px(3.)).inset();
    let offset = BoxShadow::new(px(0.), px(0.), theme.colors.background())
        .spread_radius(px(1.))
        .inset();
    vec![ring, offset]
}

/// The 3px inset ring an input draws while focused (`ring-[3px] ring-inset ring-ring/24`).
pub fn input_focus_ring(color: Hsla) -> BoxShadow {
    BoxShadow::new(px(0.), px(0.), color).spread_radius(px(3.)).inset()
}

/// The 1px `before:` highlight a control draws over its top border in dark
/// mode (bottom in light): an overlay the size of the padding box whose
/// 1px offset shadow lands on the border, as the CSS pseudo-element does.
pub fn control_highlight(theme: &AppTheme, radius: Pixels) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .rounded(radius - px(1.))
        .shadow(vec![theme.shadows.control_highlight.clone()])
}
