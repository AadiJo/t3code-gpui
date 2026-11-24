//! The boxes `MarkdownMath.tsx` puts around a formula, measured as Blink lays
//! them out. Inline: `inline-block align-middle`, holding a viewport block with
//! `px-0.5 py-0.75` around an `inline-block w-max` that holds `.katex`.
//! Display: a block with `my-[0.65rem]`, the same viewport, and `.katex-display`
//! centred in an `inline-block min-w-full`.
//!
//! A line box's height is the union of its inline boxes around the baseline.
//! Blink rounds each font's ascent and descent to whole pixels and floors the
//! leading above them, so a strut of font `f` and line height `lh` spans
//! `round(a) + floor((lh - round(a) - round(d)) / 2)` above the baseline
//! ([`line_baseline_offset`], which places every text line the same way).

use std::sync::Arc;

use gpui_kit::{App, Bounds, ContentMask, Font, Hsla, Pixels, Point, SharedString, TextRun, Window, WindowTextSystem, line_baseline_offset, point, size};

use super::{Formula, KATEX_LINE_HEIGHT, KATEX_SCALE};
use crate::ActiveAppTheme as _;

/// Above and below a baseline.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extent {
    pub above: Pixels,
    pub below: Pixels,
}

impl Extent {
    fn union(self, other: Self) -> Self {
        Self { above: self.above.max(other.above), below: self.below.max(other.below) }
    }

    pub fn height(self) -> Pixels {
        self.above + self.below
    }
}

/// The strut a font at `font_size` with `line_height` puts in a line box, as Blink computes it.
pub fn strut(font: &Font, font_size: Pixels, line_height: Pixels, text_system: &WindowTextSystem) -> Extent {
    let id = text_system.resolve_font(font);
    let above = line_baseline_offset(line_height, text_system.ascent(id, font_size), text_system.descent(id, font_size).abs());
    Extent { above, below: line_height - above }
}

/// The line box a formula sits in inside `.katex`: KaTeX_Main's strut at the formula's
/// size and `line-height: 1.2`, and the formula's own struts (`.strut`: its height and depth).
fn katex_line(formula: Option<&Formula>, em: Pixels, text_system: &WindowTextSystem) -> Extent {
    let main = super::face(katex_native::Font::default());
    let line = strut(&main, em, em * KATEX_LINE_HEIGHT, text_system);
    formula.map_or(line, |f| line.union(Extent { above: em * f.height, below: em * f.depth }))
}

/// The `\(tex\)` source an unparsable inline formula shows instead, as the fork's fallback does.
pub fn inline_source(tex: &str) -> SharedString {
    format!("\\({}\\)", tex.trim()).into()
}

/// The `\[ … \]` source of an unparsable display formula.
pub fn display_source(tex: &str) -> SharedString {
    format!("\\[\n{}\n\\]", tex.trim()).into()
}

/// One inline formula in a paragraph: what a text line reserves for it and draws there.
#[derive(Debug)]
pub struct InlineMath {
    pub tex: SharedString,
    /// `None` shows the source (`\(tex\)`) in the paragraph's font.
    pub formula: Option<Arc<Formula>>,
    pub color: Hsla,
}

/// An inline formula's box once measured against its paragraph.
#[derive(Clone, Copy, Debug)]
pub struct InlineBox {
    pub width: Pixels,
    /// The box around the paragraph's baseline after `vertical-align: middle`.
    pub extent: Extent,
    /// The formula's baseline below the box top.
    pub baseline: Pixels,
    pub em: Pixels,
    /// The formula is wider than the line and clipped (the fork scrolls it).
    pub clipped: bool,
}

impl InlineMath {
    /// What `Copy` puts on the clipboard for this formula, as `data-markdown-copy` does.
    pub fn copy_text(&self) -> SharedString {
        inline_source(&self.tex)
    }

    /// Measures the box in a paragraph set in `font` at `font_size` and `line_height`,
    /// at most `max_width` wide (`max-w-full`).
    pub fn measure(
        &self,
        font: &Font,
        font_size: Pixels,
        line_height: Pixels,
        max_width: Option<Pixels>,
        text_system: &WindowTextSystem,
        cx: &App,
    ) -> InlineBox {
        let theme = cx.app_theme();
        let (pad_x, pad_y) = (theme.rem(0.125), theme.rem(0.1875));
        let em = font_size * KATEX_SCALE;
        let paragraph = strut(font, font_size, line_height, text_system);
        let (content_width, content) = match &self.formula {
            Some(formula) => (em * formula.width, paragraph.union(katex_line(Some(formula), em, text_system))),
            None => (self.fallback_width(font, font_size, text_system), paragraph),
        };
        let natural = content_width + pad_x * 2.;
        let width = max_width.map_or(natural, |max| natural.min(max));
        let height = content.height() + pad_y * 2.;
        // `vertical-align: middle`: the box's middle sits half an x-height above the baseline.
        let x_height = text_system.x_height(text_system.resolve_font(font), font_size);
        let above = x_height / 2. + height / 2.;
        InlineBox {
            width,
            extent: Extent { above, below: height - above },
            baseline: pad_y + content.above,
            em,
            clipped: width < natural,
        }
    }

    fn fallback_width(&self, font: &Font, font_size: Pixels, text_system: &WindowTextSystem) -> Pixels {
        let source = self.copy_text();
        let run = TextRun { len: source.len(), font: font.clone(), color: self.color, background_color: None, underline: None, strikethrough: None };
        text_system.layout_line(source.as_ref(), font_size, &[run], None).width
    }

    /// Draws the formula (or its source in `font`) in a box whose top left is `origin`.
    pub fn paint(&self, measured: &InlineBox, origin: Point<Pixels>, font: &Font, font_size: Pixels, window: &mut Window, cx: &mut App) {
        let pad_x = cx.app_theme().rem(0.125);
        let baseline = point(origin.x + pad_x, origin.y + measured.baseline);
        let clip = ContentMask { bounds: Bounds::new(origin, size(measured.width, measured.extent.height())) };
        window.with_content_mask(Some(clip), |window| match &self.formula {
            Some(formula) => super::paint(formula, baseline, measured.em, self.color, window, cx),
            None => {
                let source = self.copy_text();
                let run = TextRun { len: source.len(), font: font.clone(), color: self.color, background_color: None, underline: None, strikethrough: None };
                let line = window.text_system().layout_line(source.as_ref(), font_size, &[run], None);
                for run in &line.runs {
                    for glyph in &run.glyphs {
                        let _ = window.paint_glyph(point(baseline.x + glyph.position.x, baseline.y), run.font_id, glyph.id, font_size, self.color);
                    }
                }
            }
        });
    }
}

/// A display formula's line box inside its viewport: the viewport's strut (the paragraph
/// font) and the content inline-block, whose baseline is `.katex`'s line.
pub fn display_line(formula: &Formula, font: &Font, font_size: Pixels, line_height: Pixels, text_system: &WindowTextSystem) -> (Extent, Pixels) {
    let em = font_size * KATEX_SCALE;
    let viewport = strut(font, font_size, line_height, text_system);
    (viewport.union(katex_line(Some(formula), em, text_system)), em)
}
