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
/// which lays `<text textLength>` out the way the browser does.
fn letters_svg(text: &str, family: &str) -> String {
    let length = if text.graphemes(true).count() == 1 { 6 } else { 12 };
    let mut out = String::with_capacity(320);
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><text x="8" y="10.8" text-anchor="middle" font-family="'{}', monospace" font-size="8.25" font-weight="700" textLength="{length}" lengthAdjust="spacingAndGlyphs" text-rendering="geometricPrecision">"#,
        xml_escape(family),
    );
    out.push_str(&xml_escape(text));
    out.push_str("</text></svg>");
    out
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('\'', "&apos;")
}

impl RenderOnce for ProjectMonogram {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let ink = project_icon_ink(self.color, cx.app_theme().appearance);
        let text: String = self.text.graphemes(true).take(2).collect();
        let letters = letters_svg(&text, &mono_font(FontWeight::BOLD).family);
        div()
            .flex_none()
            .size(self.size)
            .rounded(self.size * 0.25)
            .bg(with_alpha(ink, 0.14))
            .child(svg().data(letters.as_bytes()).size_full().text_color(ink))
    }
}
