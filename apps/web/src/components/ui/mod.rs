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
pub mod number_field;
pub mod motion;
pub mod pointer_menu;
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
    AbsoluteLength, BoxShadow, FontWeight, Hsla, Pixels, Styled, px,
};

use crate::AppTheme;
use crate::appearance_fonts::{TextSize, mono_font, ui_font};
use crate::theme_tokens::{Glass, backdrop_blur};

/// Paints a glass surface's background: its fill over the blurred backdrop,
/// as CSS `backdrop-filter` does, or its opaque colour where the renderer
/// cannot blur. Every glass surface paints its background with this.
pub trait GlassExt: Styled + Sized {
    fn glass(self, glass: Glass) -> Self {
        match glass.blur.filter(|_| backdrop_blur()) {
            Some(blur) => self.bg(glass.fill).backdrop_blur(blur.radius, blur.saturation),
            None => self.bg(glass.opaque),
        }
    }
}

impl<T: Styled> GlassExt for T {}

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

/// GPUI draws inset shadows from the border box and paints the 1px border
/// over them; CSS draws them inside the border. Inset shadows on bordered
/// controls are pushed in by this much to land where the browser draws them.
pub const BORDER: Pixels = px(1.);

/// The focus-visible ring of buttons, badges and switches:
/// `ring-2 ring-inset ring-ring ring-offset-1 ring-offset-background`.
/// Inside the border: 1px of background, then 2px of ring. Pass the
/// element's border width ([`BORDER`] or zero).
pub fn focus_ring_inset(theme: &AppTheme, border: Pixels) -> Vec<BoxShadow> {
    let ring = BoxShadow::new(px(0.), px(0.), theme.colors.ring()).spread_radius(border + px(3.)).inset();
    let offset = BoxShadow::new(px(0.), px(0.), theme.colors.background())
        .spread_radius(border + px(1.))
        .inset();
    vec![ring, offset]
}

/// The 3px inset ring an input draws while focused (`ring-[3px] ring-inset ring-ring/24`).
pub fn input_focus_ring(color: Hsla) -> BoxShadow {
    BoxShadow::new(px(0.), px(0.), color).spread_radius(BORDER + px(3.)).inset()
}

/// The 1px `before:` highlight a control draws at its top edge in dark mode
/// (bottom in light). CSS paints it over the border; GPUI paints borders
/// over everything inside, so here it is a 1px inset line just inside the border.
pub fn control_highlight(theme: &AppTheme) -> BoxShadow {
    let line = theme.shadows.control_highlight;
    let offset = if line.top { BORDER + px(1.) } else { -(BORDER + px(1.)) };
    BoxShadow::new(px(0.), offset, line.color).inset()
}

/// Adds [`control_highlight`] to an element's shadows. Call after `.shadow(..)`.
pub trait ControlHighlightExt: Styled + Sized {
    fn control_highlight(mut self, theme: &AppTheme) -> Self {
        let highlight = control_highlight(theme);
        self.style().box_shadow.get_or_insert_with(Vec::new).push(highlight);
        self
    }
}

impl<T: Styled> ControlHighlightExt for T {}
