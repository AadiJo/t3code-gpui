//! Native math typesetting for chat markdown, in place of the fork's KaTeX
//! (`MarkdownMath.tsx`, `MathTypeset.tsx`). katex-native runs katex-rs (a Rust
//! port of KaTeX 0.18) and applies KaTeX's stylesheet with its font metrics, so
//! a formula comes out as KaTeX lays it out: glyph runs in KaTeX's fonts, rules
//! and filled paths, in ems of the formula's font size. [`paint`] draws them.
//!
//! As in the fork, TeX KaTeX cannot parse (also mid-stream) shows as its
//! source; [`typeset`] returns `None` for it. Layouts are cached per formula;
//! they do not depend on the font size, which only scales them when painted.

pub mod atom;
pub mod display;
pub mod dump;

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use gpui_kit::{
    App, Bounds, ContentMask, Font, FontFeatures, FontStyle, FontWeight, Hsla, PathBuilder, Pixels, Point, Rgba,
    SharedString, TextRun, Window, point, px, size,
};
use katex_native::{Item, PathCommand, katex};

use crate::Assets;

/// `.katex { font-size: 1.21em }`: a formula's em against the surrounding text.
pub const KATEX_SCALE: f32 = 1.21;
/// `.katex { line-height: 1.2 }`.
pub const KATEX_LINE_HEIGHT: f32 = 1.2;

/// A laid-out formula, in ems of its font size, with the origin at the left
/// end of the baseline and y pointing down.
#[derive(Debug)]
pub struct Formula {
    pub width: f32,
    /// Extent above the baseline.
    pub height: f32,
    /// Extent below the baseline.
    pub depth: f32,
    items: Box<[MathItem]>,
    /// An equation tag (`\tag`), drawn against the right edge in display mode.
    pub tag: Option<Arc<Formula>>,
}

#[derive(Debug)]
enum MathItem {
    Text { x: f32, y: f32, text: SharedString, font: Font, size: f32, color: Option<Hsla> },
    Rect { x: f32, y: f32, width: f32, height: f32, color: Option<Hsla> },
    Path { commands: Box<[PathCommand]>, clip: (f32, f32, f32, f32), color: Option<Hsla> },
    Line { from: (f32, f32), to: (f32, f32), thickness: f32, color: Option<Hsla> },
}

thread_local! {
    static CONTEXT: katex::KatexContext = katex::KatexContext::default();
    static CACHE: RefCell<HashMap<(bool, SharedString), Option<Arc<Formula>>>> = RefCell::new(HashMap::new());
}

/// Typeset formulas kept before the cache starts over (the fork's LRU holds 1000).
const CACHE_CAPACITY: usize = 1000;

/// The formula for `tex`, or `None` when KaTeX cannot parse it. Cached, so a
/// virtualized timeline that rebuilds a message lays its formulas out once.
pub fn typeset(tex: &SharedString, display: bool) -> Option<Arc<Formula>> {
    let key = (display, tex.clone());
    if let Some(hit) = CACHE.with_borrow(|cache| cache.get(&key).cloned()) {
        return hit;
    }
    let formula = CONTEXT.with(|ctx| {
        let settings = settings(display);
        katex_native::layout_with(ctx, tex, &settings)
            .or_else(|error| match escape_literal_percents(tex) {
                Some(repaired) => katex_native::layout_with(ctx, &repaired, &settings),
                None => Err(error),
            })
            .ok()
            .map(|layout| Arc::new(Formula::from(layout)))
    });
    CACHE.with_borrow_mut(|cache| {
        if cache.len() >= CACHE_CAPACITY {
            cache.clear();
        }
        cache.insert(key, formula.clone());
    });
    formula
}

/// The fork's KaTeX options (`MathTypeset.tsx`): errors throw so the source
/// shows instead, no trusted commands, lenient strictness, `maxSize` 20 and
/// `maxExpand` 1000.
fn settings(display: bool) -> katex::Settings {
    katex::Settings::builder()
        .display_mode(display)
        .throw_on_error(true)
        .trust(katex::TrustSetting::Bool(false))
        .strict(katex::StrictSetting::Mode(katex::StrictMode::Ignore))
        .max_size(20.)
        .max_expand(1000)
        .build()
}

/// `escapeLiteralPercents` (`MathTypeset.logic.ts`): escapes each `%` written
/// right after other text, as in `\text{3.18% yield}`, so KaTeX does not read
/// the rest of the line as a comment. `None` when nothing changes.
fn escape_literal_percents(tex: &str) -> Option<String> {
    let chars: Vec<char> = tex.chars().collect();
    let mut repaired = String::with_capacity(tex.len() + 4);
    let mut changed = false;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c == '\\' {
            if chars[index..].starts_with(&['\\', 'v', 'e', 'r', 'b']) {
                let starred = chars.get(index + 5) == Some(&'*');
                let delimiter = chars.get(index + if starred { 6 } else { 5 }).copied();
                if let Some(delimiter) = delimiter.filter(|d| starred || !(d.is_ascii_alphabetic() || *d == '*')) {
                    let body = index + if starred { 7 } else { 6 };
                    let mut close = body;
                    while close < chars.len() && ![delimiter, '\n', '\r'].contains(&chars[close]) {
                        close += 1;
                    }
                    // KaTeX rejects a \verb that does not close on its own line.
                    if chars.get(close) != Some(&delimiter) {
                        return None;
                    }
                    repaired.extend(&chars[index..=close]);
                    index = close + 1;
                    continue;
                }
            }
            // A control symbol such as `\%` or `\\` is two characters.
            repaired.extend(chars.get(index..index + 2).unwrap_or(&chars[index..]));
            index += 2;
        } else if c == '%' && index > 0 && !chars[index - 1].is_whitespace() {
            repaired.push_str("\\%");
            changed = true;
            index += 1;
        } else if c == '%' {
            while index < chars.len() && chars[index] != '\n' {
                repaired.push(chars[index]);
                index += 1;
            }
        } else {
            repaired.push(c);
            index += 1;
        }
    }
    changed.then_some(repaired)
}

impl From<katex_native::Layout> for Formula {
    fn from(layout: katex_native::Layout) -> Self {
        let color = |c: Option<katex_native::Color>| {
            c.map(|c| Hsla::from(Rgba { r: c.r as f32 / 255., g: c.g as f32 / 255., b: c.b as f32 / 255., a: c.a as f32 / 255. }))
        };
        let items = layout
            .items
            .into_iter()
            .filter_map(|item| match item {
                Item::Text { x, y, text, font, size, color: c } => Some(MathItem::Text {
                    x: x as f32,
                    y: y as f32,
                    text: text.into(),
                    font: face(font),
                    size: size as f32,
                    color: color(c),
                }),
                Item::Rect { x, y, width, height, color: c } => Some(MathItem::Rect {
                    x: x as f32,
                    y: y as f32,
                    width: width as f32,
                    height: height as f32,
                    color: color(c),
                }),
                Item::Path { commands, clip, color: c } => Some(MathItem::Path {
                    commands: commands.into(),
                    clip: (clip.x as f32, clip.y as f32, clip.width as f32, clip.height as f32),
                    color: color(c),
                }),
                Item::Line { from, to, thickness, color: c } => Some(MathItem::Line {
                    from: (from.0 as f32, from.1 as f32),
                    to: (to.0 as f32, to.1 as f32),
                    thickness: thickness as f32,
                    color: color(c),
                }),
                _ => None,
            })
            .collect();
        Self {
            width: layout.width as f32,
            height: layout.height as f32,
            depth: layout.depth as f32,
            items,
            tag: layout.tag.map(|tag| Arc::new(Self::from(*tag))),
        }
    }
}

/// The KaTeX faces, each under its own family (see `scripts/math/build_math_fonts.py`).
const FACES: [&str; 20] = [
    "AMS-Regular",
    "Caligraphic-Bold",
    "Caligraphic-Regular",
    "Fraktur-Bold",
    "Fraktur-Regular",
    "Main-Bold",
    "Main-BoldItalic",
    "Main-Italic",
    "Main-Regular",
    "Math-BoldItalic",
    "Math-Italic",
    "SansSerif-Bold",
    "SansSerif-Italic",
    "SansSerif-Regular",
    "Script-Regular",
    "Size1-Regular",
    "Size2-Regular",
    "Size3-Regular",
    "Size4-Regular",
    "Typewriter-Regular",
];

/// The font for a KaTeX face. A face KaTeX does not ship (an italic AMS, say)
/// falls back to the family's nearest file, as a browser without synthesis would.
pub(crate) fn face(font: katex_native::Font) -> Font {
    let family = font.family.name().trim_start_matches("KaTeX_");
    let styles: &[&str] = match (font.bold, font.italic) {
        (true, true) => &["BoldItalic", "Bold", "Italic", "Regular"],
        (true, false) => &["Bold", "Regular"],
        (false, true) => &["Italic", "Regular", "BoldItalic"],
        (false, false) => &["Regular", "Italic", "Bold"],
    };
    let name = styles
        .iter()
        .map(|style| format!("{family}-{style}"))
        .find(|name| FACES.contains(&name.as_str()))
        .unwrap_or_else(|| "Main-Regular".into());
    Font {
        family: format!("T3Math_{name}").into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// Follows the Render math setting: math parses only while it is on, so a change
/// re-renders every window. Call after the settings store exists.
pub fn init(cx: &mut App) {
    use crate::hooks::use_settings::{ActiveClientSettings as _, ClientSettingsStore};
    crate::markdown_pipeline::set_math_enabled(cx.client_settings().math_rendering_enabled);
    cx.observe_global::<ClientSettingsStore>(|cx| {
        if crate::markdown_pipeline::set_math_enabled(cx.client_settings().math_rendering_enabled) {
            cx.refresh_windows();
        }
    })
    .detach();
}

/// Registers KaTeX's fonts and the spacer font with the text system, once,
/// the first time math is drawn: most threads have no math.
pub fn ensure_fonts(cx: &App) {
    thread_local!(static LOADED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if LOADED.replace(true) {
        return;
    }
    let fonts = FACES
        .iter()
        .map(|face| format!("fonts/math/T3Math_{face}.ttf"))
        .chain(["fonts/math/spacer.ttf".to_owned()])
        .filter_map(|path| Assets::get(&path))
        .collect();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        eprintln!("failed to load math fonts: {error:#}");
    }
}

/// Chromium draws a border thinner than 1px as 1px, and others in whole device pixels.
/// KaTeX draws its rules (fraction bars, over- and underlines, `\hline`) as borders.
fn border_width(width: Pixels, scale: f32) -> Pixels {
    if width < px(1.) { px(1.) } else { px((f32::from(width) * scale).floor() / scale) }
}

/// Thinnest dimension, in ems, that KaTeX writes as a border rather than a box.
const BORDER_EMS: f32 = 0.1;

/// Paints `formula` with its baseline starting at `origin`, at `em` per em, in
/// `color` where the TeX sets none.
pub fn paint(formula: &Formula, origin: Point<Pixels>, em: Pixels, color: Hsla, window: &mut Window, _cx: &mut App) {
    let at = |x: f32, y: f32| point(origin.x + em * x, origin.y + em * y);
    let scale = window.scale_factor();
    let mut dumped = dump::enabled().then(Vec::new);
    for item in formula.items.iter() {
        match item {
            MathItem::Text { x, y, text, font, size, color: c } => {
                let font_size = em * *size;
                let run = TextRun { len: text.len(), font: font.clone(), color: c.unwrap_or(color), background_color: None, underline: None, strikethrough: None };
                let line = window.text_system().layout_line(text.as_ref(), font_size, &[run], None);
                let baseline = at(*x, *y);
                for run in &line.runs {
                    for glyph in &run.glyphs {
                        let origin = point(baseline.x + glyph.position.x, baseline.y);
                        let ink = c.unwrap_or(color);
                        if let Some(dumped) = dumped.as_mut() {
                            dumped.push(dump::glyph(text, glyph.index, font, origin - at(0., 0.), font_size));
                        }
                        let _ = if glyph.is_emoji {
                            window.paint_emoji(origin, run.font_id, glyph.id, font_size)
                        } else {
                            window.paint_glyph(origin, run.font_id, glyph.id, font_size, ink)
                        };
                    }
                }
            }
            MathItem::Rect { x, y, width, height, color: c } => {
                let (mut top, mut left) = (at(*x, *y).y, at(*x, *y).x);
                let (mut w, mut h) = (em * *width, em * *height);
                if *height > 0. && *height < BORDER_EMS {
                    let bottom = top + h;
                    h = border_width(h, scale);
                    top = bottom - h;
                }
                if *width > 0. && *width < BORDER_EMS {
                    let right = left + w;
                    w = border_width(w, scale);
                    left = right - w;
                }
                if let Some(dumped) = dumped.as_mut() {
                    dumped.push(dump::rect(point(left, top) - at(0., 0.), size(w, h)));
                }
                window.paint_quad(gpui_kit::fill(Bounds::new(point(left, top), size(w, h)), c.unwrap_or(color)));
            }
            MathItem::Path { commands, clip, color: c } => {
                let mut builder = PathBuilder::fill();
                for command in commands.iter() {
                    match *command {
                        PathCommand::MoveTo(x, y) => builder.move_to(at(x as f32, y as f32)),
                        PathCommand::LineTo(x, y) => builder.line_to(at(x as f32, y as f32)),
                        PathCommand::CubicTo(x1, y1, x2, y2, x, y) => builder.cubic_bezier_to(
                            at(x as f32, y as f32),
                            at(x1 as f32, y1 as f32),
                            at(x2 as f32, y2 as f32),
                        ),
                        PathCommand::Close => builder.close(),
                    }
                }
                let Ok(path) = builder.build() else { continue };
                let (cx_, cy, cw, ch) = *clip;
                let mask = ContentMask { bounds: Bounds::new(at(cx_, cy), size(em * cw, em * ch)) };
                if let Some(dumped) = dumped.as_mut() {
                    dumped.push(dump::rect(mask.bounds.origin - at(0., 0.), mask.bounds.size));
                }
                window.with_content_mask(Some(mask), |window| window.paint_path(path, c.unwrap_or(color)));
            }
            MathItem::Line { from, to, thickness, color: c } => {
                if let Some(dumped) = dumped.as_mut() {
                    let (a, b) = (at(from.0, from.1) - at(0., 0.), at(to.0, to.1) - at(0., 0.));
                    dumped.push(dump::rect(point(a.x.min(b.x), a.y.min(b.y)), size((a.x - b.x).abs(), (a.y - b.y).abs())));
                }
                let mut builder = PathBuilder::stroke(em * *thickness);
                builder.move_to(at(from.0, from.1));
                builder.line_to(at(to.0, to.1));
                if let Ok(path) = builder.build() {
                    window.paint_path(path, c.unwrap_or(color));
                }
            }
        }
    }
    if let Some(items) = dumped {
        dump::write(serde_json::json!({ "kind": "formula", "origin": [f32::from(origin.x), f32::from(origin.y)], "em": f32::from(em), "items": items }));
    }
}

/// One run of inline text in the spacer font that is exactly `width` wide at
/// `font_size`, for an inline formula's slot in a text line: [`SPACER_LEN`]
/// characters, each U+E000 + k (2^k / 512 em, k < 15) or the blank U+E010.
pub fn spacer_text(width: Pixels, font_size: Pixels) -> String {
    let units = ((f32::from(width) / f32::from(font_size)) * SPACER_UNITS_PER_EM).round().clamp(0., 32767.) as u32;
    (0..SPACER_BITS)
        .map(|bit| if units & (1 << bit) != 0 { char::from_u32(0xE000 + bit).unwrap() } else { '\u{E010}' })
        .collect()
}

/// `spacer.ttf`'s units per em and how many width bits a slot spells out.
const SPACER_UNITS_PER_EM: f32 = 512.;
const SPACER_BITS: u32 = 16;
/// The byte length of [`spacer_text`]: 16 three-byte characters.
pub const SPACER_LEN: usize = 48;

/// The spacer font, for the run that holds a [`spacer_text`].
pub fn spacer_font() -> Font {
    Font {
        family: "T3MathSpacer".into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}
