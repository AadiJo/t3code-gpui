//! `ProjectMonogram.tsx`: a project's one- or two-letter badge, a rounded
//! tile tinted 14% with the project colour and the letters in bold mono.

use gpui_kit::{
    App, FontWeight, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _,
    Window, div, px,
};
use t3code_contracts::ProjectIconColor;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::mono_font;
use crate::project_icon_colors::project_icon_ink;
use crate::theme_color::with_alpha;

/// The SVG's own coordinate space: a 16-unit square, text baseline at 10.8,
/// 8.25-unit bold glyphs stretched to 12 units (6 for one glyph).
const VIEWBOX: f32 = 16.;
const BASELINE: f32 = 10.8;
const FONT_SIZE: f32 = 8.25;
const CELL: f32 = 6.;

/// A monogram badge, 16px by default (`size-4`); the breadcrumb uses 14px.
#[derive(IntoElement)]
pub struct ProjectMonogram {
    text: SharedString,
    color: ProjectIconColor,
    size: Pixels,
}

impl ProjectMonogram {
    pub fn new(text: impl Into<SharedString>, color: ProjectIconColor) -> Self {
        Self { text: text.into(), color, size: px(16.) }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for ProjectMonogram {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let ink = project_icon_ink(self.color, cx.app_theme().appearance);
        let scale = f32::from(self.size) / VIEWBOX;
        let glyphs: Vec<&str> = self.text.graphemes(true).take(2).collect();
        // `textLength` spreads the glyphs evenly over 6 or 12 units centred on x=8:
        // GPUI cannot stretch a glyph, so each one is centred in its share instead.
        // Glyph centres sit 6.5 units apart: unstretched glyphs need the extra
        // half unit to cover the same 11-unit ink width.
        let pitch = CELL + 0.5;
        let first_centre = VIEWBOX / 2. - pitch / 2. * (glyphs.len() as f32 - 1.);
        let font_size = FONT_SIZE * scale;
        // A line box as tall as the font puts the baseline about 0.68em down.
        let line_top = (BASELINE - FONT_SIZE * 0.68) * scale;
        div()
            .relative()
            .flex_none()
            .size(self.size)
            .rounded(self.size * 0.25)
            .bg(with_alpha(ink, 0.14))
            .font(mono_font(FontWeight::BOLD))
            .text_size(px(font_size))
            .line_height(px(font_size))
            .text_color(ink)
            .children(glyphs.into_iter().enumerate().map(|(ix, glyph)| {
                let centre = first_centre + pitch * ix as f32;
                div()
                    .absolute()
                    .top(px(line_top))
                    .left(px((centre - CELL / 2.) * scale))
                    .w(px(CELL * scale))
                    .flex()
                    .justify_center()
                    .child(SharedString::from(glyph.to_owned()))
            }))
    }
}
