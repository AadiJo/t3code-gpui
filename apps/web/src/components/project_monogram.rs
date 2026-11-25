//! `ProjectMonogram.tsx`: a project's one- or two-letter badge, a rounded
//! tile tinted 14% with the project colour and the letters in bold mono.

use std::fmt::Write as _;

use gpui_kit::{
    App, FontWeight, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _,
    Window, div, px, svg,
};
use t3code_contracts::ProjectIconColor;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::mono_font;
use crate::project_icon_colors::project_icon_ink;
use crate::theme_color::with_alpha;

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

/// The fork's SVG letters: a 16-unit view box, the baseline at 10.8, 8.25-unit
/// bold mono glyphs stretched (`spacingAndGlyphs`) to 12 units, or 6 for one glyph.
/// GPUI cannot stretch a text run, so the letters go through its SVG renderer,
/// which lays `<text textLength>` out the way the browser does. `bold_stroke` is the
/// synthetic bold Chromium draws when the family has no bold face, in view box units.
fn letters_svg(text: &str, family: &str, bold_stroke: Option<f32>) -> String {
    let length = if text.graphemes(true).count() == 1 { 6 } else { 12 };
    let weight = if bold_stroke.is_some() { 400 } else { 700 };
    let mut out = String::with_capacity(360);
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><text x="8" y="10.8" text-anchor="middle" font-family="'{}', monospace" font-size="8.25" font-weight="{weight}" textLength="{length}" lengthAdjust="spacingAndGlyphs" text-rendering="geometricPrecision""#,
        xml_escape(family),
    );
    if let Some(width) = bold_stroke {
        let _ = write!(out, r#" stroke="black" stroke-width="{width:.4}""#);
    }
    out.push('>');
    out.push_str(&xml_escape(text));
    out.push_str("</text></svg>");
    out
}

/// The width of the stroke Skia adds around a glyph drawn at `device_size` device pixels to
/// fake bold: 1/24 of the size up to 9px, 1/32 from 36px, interpolated between
/// (`kStdFakeBoldInterpKeys` and `kStdFakeBoldInterpValues`).
fn synthetic_bold_stroke(device_size: f32) -> f32 {
    let t = ((device_size - 9.) / (36. - 9.)).clamp(0., 1.);
    device_size * (1. / 24. + t * (1. / 32. - 1. / 24.))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\'', "&apos;")
}

impl RenderOnce for ProjectMonogram {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ink = project_icon_ink(self.color, cx.app_theme().appearance);
        let text: String = self.text.graphemes(true).take(2).collect();
        let font = mono_font(FontWeight::BOLD);
        // Device pixels per view box unit.
        let unit = f32::from(self.size) / 16. * window.scale_factor();
        let bold_stroke = cx
            .text_system()
            .is_synthetic_bold(cx.text_system().resolve_font(&font))
            .then(|| synthetic_bold_stroke(8.25 * unit) / unit);
        let letters = letters_svg(&text, &font.family, bold_stroke);
        div()
            .flex_none()
            .size(self.size)
            .rounded(self.size * 0.25)
            .bg(with_alpha(ink, 0.14))
            .child(svg().data(letters.as_bytes()).size_full().text_color(ink))
    }
}
