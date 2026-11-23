//! Paints a terminal grid. Ports the role of `terminal/ghostty/renderer.ts`
//! with the approach of Zed's `terminal_element.rs`: per row, merged
//! background rects and batched text runs per style, shaped once with a forced
//! monospace advance. Rows keep their shaped lines until the grid reports them
//! damaged, so an idle or blinking terminal paints without reshaping.

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use gpui_kit::{
    App, Bounds, ContentMask, Corners, Element, ElementId, ElementInputHandler, Entity, Font, FontFeatures,
    FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId, LayoutId, Pixels,
    ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign, TextRun, UnderlineStyle, Window, fill, point,
    px, relative, size,
};

use super::backend::GridSize;
use super::core::{Damage, TerminalCore};
use super::palette::{TerminalPalette, blend, to_hsla};
use super::surface::TerminalSurface;

/// Space between the grid and the element's edge, as in the web surface.
pub const CONTENT_PADDING: Pixels = px(4.);
const SCROLLBAR_WIDTH: Pixels = px(6.);
const MIN_SCROLLBAR_THUMB: Pixels = px(18.);

/// Cell size and font for one terminal.
#[derive(Clone, Debug, PartialEq)]
pub struct CellMetrics {
    pub font: Font,
    pub font_size: Pixels,
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl CellMetrics {
    /// Measures a monospace font: advance of `m`, line height 1.35x rounded (the web metrics).
    pub fn measure(font: Font, font_size: Pixels, window: &Window) -> Self {
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|advance| advance.width)
            .unwrap_or(font_size * 0.6);
        let line_height = px((f32::from(font_size) * 1.35).round().max(1.));
        Self { font, font_size, cell_width, line_height }
    }

    /// The grid that fits a content box.
    pub fn grid_size(&self, bounds: Bounds<Pixels>) -> GridSize {
        let width = (bounds.size.width - CONTENT_PADDING * 2.).max(px(0.));
        let height = (bounds.size.height - CONTENT_PADDING * 2.).max(px(0.));
        // Two columns minimum keeps a wide glyph from breaking the grid.
        GridSize::clamped(
            ((width / self.cell_width).floor() as usize).max(2),
            ((height / self.line_height).floor() as usize).max(1),
        )
    }
}

/// Monospace families tried in order, matching the web mono stack's resolved face per platform.
const FONT_CANDIDATES: &[&str] = if cfg!(target_os = "macos") {
    &["SF Mono", "Menlo", "Monaco"]
} else if cfg!(target_os = "windows") {
    &["Consolas", "Cascadia Mono", "Courier New"]
} else {
    &["Liberation Mono", "DejaVu Sans Mono", "Noto Sans Mono", "Ubuntu Mono"]
};

static FONT_FAMILY: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();

/// The default terminal font: the first installed candidate, with symbol, CJK and emoji fallbacks.
pub fn terminal_font(window: &Window) -> Font {
    let family = FONT_FAMILY
        .get_or_init(|| {
            let installed = window.text_system().all_font_names();
            FONT_CANDIDATES
                .iter()
                .find(|candidate| installed.iter().any(|name| name == *candidate))
                .copied()
                .unwrap_or(FONT_CANDIDATES[0])
                .into()
        })
        .clone();
    Font {
        family,
        features: FontFeatures::disable_ligatures(),
        fallbacks: Some(gpui_kit::FontFallbacks::from_fonts(vec![
            "DejaVu Sans Mono".into(),
            "Symbols Nerd Font Mono".into(),
            "Noto Sans CJK JP".into(),
            "Noto Sans CJK SC".into(),
            "PingFang SC".into(),
            "Hiragino Sans".into(),
            "Microsoft YaHei".into(),
            "Malgun Gothic".into(),
            "Apple Color Emoji".into(),
            "Segoe UI Emoji".into(),
            "Noto Color Emoji".into(),
        ])),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// One shaped stretch of a row.
struct Segment {
    col: u16,
    /// Columns covered: 2 for a wide glyph, which is centred in its cells.
    wide: bool,
    line: ShapedLine,
}

/// A row ready to paint.
#[derive(Default)]
struct RowLayout {
    backgrounds: Vec<(u16, u16, Hsla)>,
    segments: Vec<Segment>,
}

/// Painted rows, kept by the surface between frames.
#[derive(Default)]
pub struct GridLayout {
    rows: Vec<RowLayout>,
    metrics: Option<CellMetrics>,
    palette_generation: u64,
}

/// Where the grid sits on screen, for mapping mouse positions to cells.
#[derive(Clone, Copy, Debug)]
pub struct GridGeometry {
    pub origin: gpui_kit::Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub size: GridSize,
}

impl GridGeometry {
    /// The viewport cell under a window position, clamped to the grid, and
    /// whether the position is in the cell's right half.
    pub fn cell_at(&self, position: gpui_kit::Point<Pixels>) -> (usize, usize, bool) {
        let x = (position.x - self.origin.x).max(px(0.));
        let y = (position.y - self.origin.y).max(px(0.));
        let col = (x / self.cell_width).floor() as usize;
        let row = (y / self.line_height).floor() as usize;
        let right_half = x - self.cell_width * col as f32 > self.cell_width / 2.;
        let past_end = col >= self.size.cols as usize;
        (
            row.min(self.size.rows as usize - 1),
            col.min(self.size.cols as usize - 1),
            right_half || past_end,
        )
    }
}

/// Text attributes that decide whether neighbouring cells share a text run.
#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    color: Hsla,
    bold: bool,
    italic: bool,
    underline: Option<bool>,
    strike: bool,
}

impl GridLayout {
    /// Rebuilds rows the core reports damaged (all of them when metrics or theme changed).
    fn update(&mut self, core: &mut TerminalCore, metrics: &CellMetrics, palette: &TerminalPalette, palette_generation: u64, window: &Window) {
        let rows = core.size().rows as usize;
        let mut full = self.metrics.as_ref() != Some(metrics) || self.palette_generation != palette_generation;
        if self.rows.len() != rows {
            self.rows.resize_with(rows, RowLayout::default);
            full = true;
        }
        let damage = core.take_damage();
        self.metrics = Some(metrics.clone());
        self.palette_generation = palette_generation;
        match damage {
            Damage::Rows(dirty) if !full => {
                for row in dirty {
                    if row < rows {
                        build_row(&mut self.rows[row], row, core, metrics, palette, window);
                    }
                }
            }
            _ => {
                for row in 0..rows {
                    build_row(&mut self.rows[row], row, core, metrics, palette, window);
                }
            }
        }
    }
}

fn cell_colors(cell: &Cell, core: &TerminalCore, palette: &TerminalPalette) -> (Hsla, Option<Hsla>) {
    let overrides = core.term().colors();
    let mut fg = palette.resolve(cell.fg, overrides);
    let mut bg = palette.resolve(cell.bg, overrides);
    let default_bg = cell.bg == Color::Named(NamedColor::Background);
    let mut has_bg = !default_bg;
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
        has_bg = true;
    }
    if cell.flags.contains(Flags::DIM) {
        fg = blend(fg, bg);
    }
    (to_hsla(fg), has_bg.then(|| to_hsla(bg)))
}

fn build_row(layout: &mut RowLayout, row: usize, core: &TerminalCore, metrics: &CellMetrics, palette: &TerminalPalette, window: &Window) {
    layout.backgrounds.clear();
    layout.segments.clear();
    let grid = core.term().grid();
    let line = Line(row as i32 - grid.display_offset() as i32);
    let cols = grid.columns();
    let grid_row = &grid[line];

    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut segment_start: Option<u16> = None;
    let mut last_style: Option<CellStyle> = None;
    // Text after the last visible glyph is trailing blanks; it is trimmed when the segment ends.
    let mut visible_len = 0usize;
    let mut visible_runs = 0usize;

    let flush = |text: &mut String, runs: &mut Vec<TextRun>, start: &mut Option<u16>, visible_len: &mut usize, visible_runs: &mut usize, last_style: &mut Option<CellStyle>, segments: &mut Vec<Segment>| {
        if let Some(col) = start.take() {
            text.truncate(*visible_len);
            runs.truncate(*visible_runs);
            let covered: usize = runs.iter().rev().skip(1).map(|run| run.len).sum();
            if let Some(last) = runs.last_mut() {
                last.len = text.len() - covered;
            }
            if !text.is_empty() {
                let line = window.text_system().shape_line(
                    SharedString::from(std::mem::take(text)),
                    metrics.font_size,
                    runs,
                    Some(metrics.cell_width),
                );
                segments.push(Segment { col, wide: false, line });
            }
        }
        text.clear();
        runs.clear();
        *visible_len = 0;
        *visible_runs = 0;
        *last_style = None;
    };

    for col in 0..cols {
        let cell = &grid_row[Column(col)];
        if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let (fg, bg) = cell_colors(cell, core, palette);
        if let Some(bg) = bg {
            let span = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
            match layout.backgrounds.last_mut() {
                Some((start, len, color)) if *color == bg && (*start + *len) as usize == col => *len += span,
                _ => layout.backgrounds.push((col as u16, span, bg)),
            }
        }

        let hidden = cell.flags.contains(Flags::HIDDEN);
        let underline = cell.flags.intersects(Flags::ALL_UNDERLINES).then(|| cell.flags.contains(Flags::UNDERCURL));
        let strike = cell.flags.contains(Flags::STRIKEOUT);
        let blank = (cell.c == ' ' || hidden) && underline.is_none() && !strike;
        let style = CellStyle {
            color: fg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline,
            strike,
        };

        if cell.flags.contains(Flags::WIDE_CHAR) {
            flush(&mut text, &mut runs, &mut segment_start, &mut visible_len, &mut visible_runs, &mut last_style, &mut layout.segments);
            if !blank {
                let mut glyph = String::new();
                glyph.push(cell.c);
                glyph.extend(cell.zerowidth().unwrap_or_default());
                let run = text_run(metrics, style, glyph.len());
                let line = window.text_system().shape_line(glyph.into(), metrics.font_size, &[run], None);
                layout.segments.push(Segment { col: col as u16, wide: true, line });
            }
            continue;
        }

        if blank {
            if segment_start.is_some() {
                // A plain space joins whatever run is open, so it does not split runs.
                text.push(' ');
                let plain = CellStyle { underline: None, strike: false, ..style };
                match runs.last_mut() {
                    // Decorations stop at the styled text; the space gets a plain run.
                    Some(run) if run.underline.is_none() && run.strikethrough.is_none() => run.len += 1,
                    _ => {
                        runs.push(text_run(metrics, plain, 1));
                        last_style = Some(plain);
                    }
                }
            }
            continue;
        }

        segment_start.get_or_insert(col as u16);
        let start_len = text.len();
        if hidden {
            text.push(' ');
        } else {
            text.push(cell.c);
            text.extend(cell.zerowidth().unwrap_or_default());
        }
        let added = text.len() - start_len;
        match runs.last_mut() {
            Some(run) if last_style == Some(style) => run.len += added,
            // An open run made of blanks only takes this cell's style.
            Some(run) if last_style.is_none() => {
                *run = text_run(metrics, style, run.len + added);
            }
            _ => runs.push(text_run(metrics, style, added)),
        }
        last_style = Some(style);
        visible_len = text.len();
        visible_runs = runs.len();
    }
    flush(&mut text, &mut runs, &mut segment_start, &mut visible_len, &mut visible_runs, &mut last_style, &mut layout.segments);
}

fn text_run(metrics: &CellMetrics, style: CellStyle, len: usize) -> TextRun {
    let mut font = metrics.font.clone();
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    TextRun {
        len,
        font,
        color: style.color,
        background_color: None,
        underline: style.underline.map(|wavy| UnderlineStyle { color: Some(style.color), thickness: px(1.), wavy }),
        strikethrough: style.strike.then(|| StrikethroughStyle { color: Some(style.color), thickness: px(1.) }),
    }
}

/// The element that paints a [`TerminalSurface`]'s grid and owns its text input.
pub struct TerminalGrid {
    surface: Entity<TerminalSurface>,
}

impl TerminalGrid {
    pub fn new(surface: Entity<TerminalSurface>) -> Self {
        Self { surface }
    }
}

/// What prepaint hands to paint: the row layout, moved out of the surface for the frame.
pub struct GridPaint {
    layout: GridLayout,
    geometry: GridGeometry,
    overlay: super::surface::FrameOverlay,
}

impl Element for TerminalGrid {
    type RequestLayoutState = ();
    type PrepaintState = Option<GridPaint>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Self::PrepaintState {
        self.surface.update(cx, |surface, cx| {
            let metrics = CellMetrics::measure(surface.font(), surface.font_size(), window);
            let size = metrics.grid_size(bounds);
            surface.resize_grid(size, &metrics, cx);
            let scale = window.scale_factor();
            let snap = |value: Pixels| px((f32::from(value) * scale).floor() / scale);
            let origin = point(snap(bounds.origin.x + CONTENT_PADDING), snap(bounds.origin.y + CONTENT_PADDING));
            let geometry = GridGeometry { origin, cell_width: metrics.cell_width, line_height: metrics.line_height, size };
            let (mut layout, palette, generation) = surface.take_layout();
            layout.update(surface.core_mut(), &metrics, &palette, generation, window);
            let overlay = surface.frame_overlay(&geometry, window, cx);
            surface.set_geometry(geometry);
            Some(GridPaint { layout, geometry, overlay })
        })
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), prepaint: &mut Self::PrepaintState, window: &mut Window, cx: &mut App) {
        let Some(frame) = prepaint.take() else { return };
        let GridPaint { layout, geometry, overlay } = frame;
        let focus = self.surface.read(cx).focus_handle().clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.surface.clone()), cx);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, overlay.background));
            let origin = geometry.origin;
            let cell = |row: usize, col: usize| point(origin.x + geometry.cell_width * col as f32, origin.y + geometry.line_height * row as f32);
            let row_rect = |row: usize, col: usize, cols: usize| {
                Bounds::new(cell(row, col), size((geometry.cell_width * cols as f32).ceil(), geometry.line_height))
            };

            for (row, row_layout) in layout.rows.iter().enumerate() {
                for &(col, cols, color) in &row_layout.backgrounds {
                    window.paint_quad(fill(row_rect(row, col as usize, cols as usize), color));
                }
            }
            for &(row, col, cols, color) in &overlay.highlights {
                window.paint_quad(fill(row_rect(row, col, cols), color));
            }
            for (row, row_layout) in layout.rows.iter().enumerate() {
                for segment in &row_layout.segments {
                    let mut at = cell(row, segment.col as usize);
                    if segment.wide {
                        at.x += (geometry.cell_width * 2. - segment.line.width) / 2.;
                    }
                    segment.line.paint(at, geometry.line_height, TextAlign::Left, None, window, cx).ok();
                }
            }
            for &(row, col, cols, color) in &overlay.underlines {
                let rect = row_rect(row, col, cols);
                window.paint_quad(fill(Bounds::new(point(rect.origin.x, rect.bottom() - px(2.)), size(rect.size.width, px(1.))), color));
            }

            if let Some(cursor) = &overlay.cursor {
                let width = geometry.cell_width * if cursor.wide { 2. } else { 1. };
                let rect = Bounds::new(cell(cursor.row, cursor.col), size(width, geometry.line_height));
                match cursor.shape {
                    CursorShape::Block => {
                        window.paint_quad(fill(rect, cursor.color));
                        if let Some(glyph) = &cursor.glyph {
                            let mut at = rect.origin;
                            if cursor.wide {
                                at.x += (width - glyph.width) / 2.;
                            }
                            glyph.paint(at, geometry.line_height, TextAlign::Left, None, window, cx).ok();
                        }
                    }
                    CursorShape::Beam => window.paint_quad(fill(Bounds::new(rect.origin, size(px(2.), rect.size.height)), cursor.color)),
                    CursorShape::Underline => window.paint_quad(fill(
                        Bounds::new(point(rect.origin.x, rect.bottom() - px(2.)), size(width, px(2.))),
                        cursor.color,
                    )),
                    CursorShape::HollowBlock | CursorShape::Hidden => window.paint_quad(gpui_kit::outline(rect, cursor.color, gpui_kit::BorderStyle::Solid)),
                }
            }

            if let Some(marked) = &overlay.marked_text {
                let at = cell(marked.row, marked.col);
                let rect = Bounds::new(at, size(marked.line.width.max(geometry.cell_width), geometry.line_height));
                window.paint_quad(fill(rect, overlay.background));
                marked.line.paint(at, geometry.line_height, TextAlign::Left, None, window, cx).ok();
            }

            if let Some((top, height, color)) = overlay.scrollbar {
                let thumb = Bounds::new(
                    point(bounds.right() - SCROLLBAR_WIDTH - px(2.), bounds.top() + top),
                    size(SCROLLBAR_WIDTH, height.max(MIN_SCROLLBAR_THUMB)),
                );
                window.paint_quad(fill(thumb, color).corner_radii(Corners::all(SCROLLBAR_WIDTH / 2.)));
            }
        });

        self.surface.update(cx, |surface, _| surface.restore_layout(layout));
    }
}
