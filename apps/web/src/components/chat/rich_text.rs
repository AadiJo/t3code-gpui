//! Selectable rich text: one laid-out paragraph with per-run fonts and
//! colours, rounded backgrounds behind inline code and file chips, and
//! clickable links. It joins the window's text selection the same way
//! gpui-base's `SelectableText` does, so a drag across paragraphs, code and
//! tables selects and copies as one document.
//!
//! A paragraph with inline formulas ([`TextAtom`]) lays out row by row: each
//! formula reserves its width in the line (a run of spacer glyphs), and a row
//! holding one grows to fit its box, as a CSS line box grows around an
//! `inline-block`. Each row is then its own text layout, shifted to the row's
//! baseline, and all rows stay one selection participant whose copy puts each
//! formula's TeX in place of its slot.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, Corners, CursorStyle, DispatchPhase, Element, ElementId, Font,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement as _, LayoutId, MouseButton,
    MouseUpEvent, Pixels, SharedString, Size, StyledText, Subscription, TextLayout, TextRun, WhiteSpace, Window, fill,
    line_baseline_offset, point, px, size,
    base::{TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun},
};

use crate::math::{
    self,
    atom::{InlineBox, InlineMath},
};

/// A rounded background behind a byte range, such as inline code.
#[derive(Clone, Debug)]
pub struct TextBox {
    pub range: Range<usize>,
    pub color: Hsla,
    /// Horizontal padding outside the glyphs.
    pub pad_x: Pixels,
    pub radius: Pixels,
    /// Box height in ems of the paragraph's font, centred on the line; `None` fills the line.
    pub height_em: Option<f32>,
    /// When set, the box is an inline element's background in this font instead, as Blink
    /// draws one: the font's rounded ascent above the line's baseline and rounded descent
    /// below it, plus padding and border; `pad_x` is then unused.
    pub inline_font: Option<InlineFontBox>,
}

/// The font and box of an inline element such as inline code (see [`TextBox::inline_font`]).
#[derive(Clone, Debug)]
pub struct InlineFontBox {
    pub font: Font,
    /// The font size in ems of the paragraph's font (`font-size: 0.85em`).
    pub size_em: f32,
    /// Horizontal padding in ems of the element's own font size (`padding-inline: 0.35em`).
    pub pad_x_em: f32,
    /// Vertical padding in ems of the element's own font size (`padding-block: 0.1em`).
    pub pad_y_em: f32,
    pub border: Pixels,
}

/// An icon painted over a placeholder glyph, such as the file icon of a file chip or a
/// link's favicon.
#[derive(Clone)]
pub struct TextIcon {
    /// Byte offset of the placeholder the icon covers.
    pub index: usize,
    pub size: Pixels,
    /// Space between the placeholder's start and the icon.
    pub inset: Pixels,
    /// `None` centres the icon on the line; `Some(drop)` sets its bottom this far below the
    /// baseline (`vertical-align: -0.125em`).
    pub below_baseline: Option<Pixels>,
    pub paint: IconPaint,
}

/// What a [`TextIcon`] draws.
#[derive(Clone)]
pub enum IconPaint {
    /// A monochrome SVG in `color`.
    Svg { path: SharedString, color: Hsla },
    /// A colour image, with rounded corners.
    Image { image: std::sync::Arc<gpui_kit::RenderImage>, radius: Pixels },
    /// Keeps the space and draws nothing (an icon still loading).
    Empty,
}

/// An inline formula over a [`math::SPACER_LEN`]-byte slot of the text whose run uses
/// [`math::spacer_font`]; the slot's glyphs are rewritten to the formula's width.
#[derive(Clone, Debug)]
pub struct TextAtom {
    pub range: Range<usize>,
    pub math: Rc<InlineMath>,
}

/// Called with a link target when a link is clicked.
pub type LinkHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// A paragraph of styled, selectable text.
pub struct RichText {
    id: ElementId,
    text: SharedString,
    runs: Rc<[TextRun]>,
    styled: StyledText,
    boxes: Rc<[TextBox]>,
    icons: Rc<[TextIcon]>,
    atoms: Rc<[TextAtom]>,
    links: Rc<[(Range<usize>, SharedString)]>,
    on_link: Option<LinkHandler>,
    selection_color: Hsla,
}

impl RichText {
    /// Text laid out with `runs`, which must cover `text` exactly.
    pub fn new(id: impl Into<ElementId>, text: SharedString, runs: Vec<TextRun>) -> Self {
        let runs: Rc<[TextRun]> = runs.into();
        Self {
            id: id.into(),
            styled: StyledText::new(text.clone()).with_runs(runs.to_vec()),
            runs,
            text,
            boxes: Rc::from([]),
            icons: Rc::from([]),
            atoms: Rc::from([]),
            links: Rc::from([]),
            on_link: None,
            selection_color: Hsla::transparent_black(),
        }
    }

    pub fn boxes(mut self, boxes: impl Into<Rc<[TextBox]>>) -> Self {
        self.boxes = boxes.into();
        self
    }

    pub fn icons(mut self, icons: impl Into<Rc<[TextIcon]>>) -> Self {
        self.icons = icons.into();
        self
    }

    /// Inline formulas, which switch the paragraph to row-by-row layout.
    pub fn atoms(mut self, atoms: impl Into<Rc<[TextAtom]>>) -> Self {
        self.atoms = atoms.into();
        self
    }

    /// Clickable ranges and the targets they open through `handler`.
    pub fn links(mut self, links: impl Into<Rc<[(Range<usize>, SharedString)]>>, handler: LinkHandler) -> Self {
        self.links = links.into();
        self.on_link = Some(handler);
        self
    }

    pub fn selection_color(mut self, color: Hsla) -> Self {
        self.selection_color = color;
        self
    }
}

/// What a paragraph keeps between frames: its selection participant, and for a
/// paragraph with formulas, what its last paint selected (read by its copy).
struct OwnedSelection {
    handle: TextSelectionHandle,
    copy: Option<Rc<RefCell<AtomCopy>>>,
    _refresh: Subscription,
}

/// The selected byte ranges of a paragraph with formulas and the TeX each slot copies as.
#[derive(Default)]
struct AtomCopy {
    text: SharedString,
    selected: Vec<Range<usize>>,
    atoms: Vec<(Range<usize>, SharedString)>,
}

impl AtomCopy {
    fn text(&self) -> String {
        let mut out = String::new();
        for range in &self.selected {
            let mut cursor = range.start;
            for (slot, tex) in self.atoms.iter().filter(|(slot, _)| slot.start < range.end && slot.end > range.start) {
                out.push_str(&self.text[cursor..slot.start.max(cursor)]);
                out.push_str(tex);
                cursor = slot.end.min(range.end).max(cursor);
            }
            out.push_str(&self.text[cursor..range.end.max(cursor)]);
        }
        out
    }
}

/// A paragraph with formulas, measured: its text with the slots filled in, its rows and
/// each formula's box.
#[derive(Default)]
struct RowLayout {
    text: SharedString,
    rows: Vec<Row>,
    boxes: Vec<InlineBox>,
    wrap_width: Option<Pixels>,
    size: Size<Pixels>,
    font: Option<Font>,
    font_size: Pixels,
}

/// One laid-out row of a paragraph with formulas.
struct Row {
    range: Range<usize>,
    top: Pixels,
    height: Pixels,
    /// The row's baseline below its top.
    baseline: Pixels,
}

pub struct RichTextPrepaint {
    hitbox: Hitbox,
    /// The rows of a paragraph with formulas, each its own text layout.
    rows: Vec<PreparedRow>,
}

struct PreparedRow {
    element: AnyElement,
    layout: TextLayout,
    range: Range<usize>,
    bounds: Bounds<Pixels>,
}

pub struct RichTextLayout {
    handle: TextSelectionHandle,
    copy: Option<Rc<RefCell<AtomCopy>>>,
    rows: Option<Rc<RefCell<RowLayout>>>,
}

impl Element for RichText {
    type RequestLayoutState = RichTextLayout;
    type PrepaintState = RichTextPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, RichTextLayout) {
        let text = self.text.clone();
        let with_atoms = !self.atoms.is_empty();
        let (handle, copy) = window.with_element_state(
            global_id.expect("RichText has an element id"),
            |retained: Option<OwnedSelection>, window| {
                let mut retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(text, cx);
                    let refresh = handle.refresh_window_on_change(window, cx);
                    OwnedSelection { handle, copy: None, _refresh: refresh }
                });
                if with_atoms && retained.copy.is_none() {
                    let copy = Rc::new(RefCell::new(AtomCopy::default()));
                    let state = copy.clone();
                    retained.handle.copy_with(move |_| state.borrow().text(), cx);
                    retained.copy = Some(copy);
                }
                ((retained.handle.clone(), retained.copy.clone()), retained)
            },
        );
        if !with_atoms {
            let (layout_id, ()) = self.styled.request_layout(global_id, inspector_id, window, cx);
            return (layout_id, RichTextLayout { handle, copy, rows: None });
        }
        let rows = Rc::new(RefCell::new(RowLayout::default()));
        let layout_id = self.request_row_layout(rows.clone(), window);
        (layout_id, RichTextLayout { handle, copy, rows: Some(rows) })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut RichTextLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> RichTextPrepaint {
        let rows = match &layout.rows {
            None => {
                self.styled.prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
                Vec::new()
            }
            Some(rows) => self.prepaint_rows(&rows.borrow(), bounds, window, cx),
        };
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let text_bounds = if rows.is_empty() { vec![bounds] } else { rows.iter().map(|row| row.bounds).collect() };
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(text_bounds)
            .with_rendered_element(&layout.handle, window, cx);
        layout.handle.register(registration, window, cx);
        RichTextPrepaint { hitbox, rows }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut RichTextLayout,
        prepaint: &mut RichTextPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let em = window.text_style().font_size.to_pixels(window.rem_size());
        if let Some(rows) = &layout.rows {
            self.paint_rows(&rows.borrow(), layout, prepaint, em, window, cx);
        } else {
            let text_layout = self.styled.layout().clone();
            let segment = Segment { layout: &text_layout, range: 0..self.text.len() };
            self.paint_boxes(&segment, em, window);
            let projection =
                layout.handle.update_runs(&[TextSelectionRun::new(self.text.clone(), text_layout.clone(), bounds)], cx);
            for range in projection.ranges().iter().flatten() {
                for quad in range_quads(&text_layout, range.clone(), text_layout.line_height()) {
                    window.paint_quad(fill(quad, self.selection_color));
                }
            }
            self.styled.paint(global_id, inspector_id, bounds, &mut (), &mut (), window, cx);
            self.paint_icons(&segment, window, cx);
        }
        self.listen_for_links(prepaint, window);
    }
}

/// A laid-out stretch of the paragraph: the whole of it, or one row.
struct Segment<'a> {
    layout: &'a TextLayout,
    range: Range<usize>,
}

impl Segment<'_> {
    /// `range` in this segment's own bytes, if they overlap.
    fn local(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let start = range.start.max(self.range.start);
        let end = range.end.min(self.range.end);
        (start < end || (range.is_empty() && self.range.contains(&range.start)))
            .then(|| start - self.range.start..end - self.range.start)
    }
}

impl RichText {
    /// Measures a paragraph with formulas: wraps it with each formula's slot as wide as
    /// its box, then sizes each row to the union of its strut and the boxes on it.
    fn request_row_layout(&self, rows: Rc<RefCell<RowLayout>>, window: &mut Window) -> LayoutId {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = text_style.line_height.to_pixels(font_size.into(), window.rem_size());
        let font = text_style.font();
        let text = self.text.clone();
        let runs = self.runs.clone();
        let atoms = self.atoms.clone();
        let wraps = text_style.white_space == WhiteSpace::Normal;
        window.request_measured_layout(Default::default(), move |known, available, window, cx| {
            let wrap_width = wraps
                .then(|| {
                    known.width.or(match available.width {
                        AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    })
                })
                .flatten();
            if let Some(size) = rows.borrow().size.ne(&Size::default()).then(|| rows.borrow().size)
                && rows.borrow().wrap_width == wrap_width
            {
                return size;
            }
            math::ensure_fonts(cx);
            let text_system = window.text_system().clone();
            let boxes: Vec<InlineBox> = atoms
                .iter()
                .map(|atom| atom.math.measure(&font, font_size, line_height, wrap_width, &text_system, cx))
                .collect();
            let mut filled = text.to_string();
            for (atom, measured) in atoms.iter().zip(&boxes) {
                if filled.is_char_boundary(atom.range.start) && atom.range.len() == math::SPACER_LEN {
                    filled.replace_range(atom.range.clone(), &math::spacer_text(measured.width, font_size));
                }
            }
            let filled: SharedString = filled.into();
            let Ok(lines) = text_system.shape_text(filled.clone(), font_size, &runs, wrap_width, None) else {
                return Size::default();
            };
            let mut laid_out = Vec::new();
            let mut top = px(0.);
            let mut width = px(0.);
            let mut line_start = 0;
            for line in &lines {
                let layout = &line.unwrapped_layout;
                let glyph_index = |run_ix: usize, glyph_ix: usize| layout.runs[run_ix].glyphs[glyph_ix].index;
                let mut starts: Vec<usize> = vec![0];
                starts.extend(line.wrap_boundaries.iter().map(|b| glyph_index(b.run_ix, b.glyph_ix)));
                starts.push(line.len());
                for pair in starts.windows(2) {
                    let range = pair[0]..pair[1];
                    let start_x = layout.x_for_index(range.start);
                    width = width.max(layout.x_for_index(range.end) - start_x);
                    // The strut: the row's fonts, centred on the line height as GPUI draws them.
                    let (mut ascent, mut descent) = (px(0.), px(0.));
                    for run in &layout.runs {
                        if run.glyphs.iter().any(|glyph| range.contains(&glyph.index)) {
                            ascent = ascent.max(text_system.ascent(run.font_id, font_size));
                            descent = descent.max(text_system.descent(run.font_id, font_size).abs());
                        }
                    }
                    if ascent == px(0.) {
                        (ascent, descent) = (layout.ascent, layout.descent);
                    }
                    let strut_above = line_baseline_offset(line_height, ascent, descent);
                    let mut above = strut_above;
                    let mut below = line_height - strut_above;
                    let absolute = line_start + range.start..line_start + range.end;
                    for (atom, measured) in atoms.iter().zip(&boxes) {
                        if absolute.contains(&atom.range.start) {
                            above = above.max(measured.extent.above);
                            below = below.max(measured.extent.below);
                        }
                    }
                    laid_out.push(Row { range: absolute, top, height: above + below, baseline: above });
                    top += above + below;
                }
                line_start += line.len() + 1;
            }
            let size = size(wrap_width.unwrap_or(px(((f32::from(width) * 64.).ceil()) / 64.)), top);
            *rows.borrow_mut() = RowLayout {
                text: filled,
                rows: laid_out,
                boxes,
                wrap_width,
                size,
                font: Some(font.clone()),
                font_size,
            };
            size
        })
    }

    /// Lays out and places each row's text so its baseline lands on the row's.
    fn prepaint_rows(&self, rows: &RowLayout, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) -> Vec<PreparedRow> {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = text_style.line_height.to_pixels(font_size.into(), window.rem_size());
        rows.rows
            .iter()
            .map(|row| {
                let text: SharedString = rows.text[row.range.clone()].trim_end_matches('\n').to_owned().into();
                let styled = StyledText::new(text.clone()).with_runs(slice_runs(&self.runs, row.range.start..row.range.start + text.len()));
                let layout = styled.layout().clone();
                let mut element = styled.into_any_element();
                element.layout_as_root(size(AvailableSpace::MaxContent, AvailableSpace::MinContent), window, cx);
                // The row's own text baseline, which GPUI centres on the line height.
                let shaped = window.text_system().shape_line(text.clone(), font_size, &slice_runs(&self.runs, row.range.start..row.range.start + text.len()), None);
                let own_baseline = line_baseline_offset(line_height, shaped.ascent, shaped.descent);
                let origin = point(bounds.left(), bounds.top() + row.top + row.baseline - own_baseline);
                element.prepaint_at(origin, window, cx);
                PreparedRow {
                    element,
                    layout,
                    range: row.range.start..row.range.start + text.len(),
                    bounds: Bounds::new(point(bounds.left(), bounds.top() + row.top), size(bounds.size.width, row.height)),
                }
            })
            .collect()
    }

    fn paint_rows(
        &self,
        rows: &RowLayout,
        layout: &RichTextLayout,
        prepaint: &mut RichTextPrepaint,
        em: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        for row in &prepaint.rows {
            self.paint_boxes(&Segment { layout: &row.layout, range: row.range.clone() }, em, window);
        }
        let runs: Vec<TextSelectionRun> = prepaint
            .rows
            .iter()
            .map(|row| TextSelectionRun::new(rows.text[row.range.clone()].to_owned(), row.layout.clone(), row.bounds))
            .collect();
        let projection = layout.handle.update_runs(&runs, cx);
        let mut selected = Vec::new();
        for (row, range) in prepaint.rows.iter().zip(projection.ranges()) {
            let Some(range) = range else { continue };
            for quad in range_quads(&row.layout, range.clone(), row.layout.line_height()) {
                window.paint_quad(fill(quad, self.selection_color));
            }
            selected.push(row.range.start + range.start..row.range.start + range.end);
        }
        if let Some(copy) = &layout.copy {
            *copy.borrow_mut() = AtomCopy {
                text: rows.text.clone(),
                selected,
                atoms: self.atoms.iter().map(|atom| (atom.range.clone(), atom.math.copy_text())).collect(),
            };
        }
        for row in &mut prepaint.rows {
            row.element.paint(window, cx);
        }
        for row in &prepaint.rows {
            self.paint_icons(&Segment { layout: &row.layout, range: row.range.clone() }, window, cx);
        }
        let Some(font) = rows.font.as_ref() else { return };
        for ((atom, measured), row) in self.atoms.iter().zip(&rows.boxes).filter_map(|(atom, measured)| {
            let row = rows.rows.iter().position(|row| row.range.contains(&atom.range.start))?;
            Some(((atom, measured), row))
        }) {
            let (Some(prepared), Some(laid)) = (prepaint.rows.get(row), rows.rows.get(row)) else { continue };
            let Some(start) = prepared.layout.position_for_index(atom.range.start - prepared.range.start) else { continue };
            let baseline = prepared.bounds.top() + laid.baseline;
            let origin = point(start.x, baseline - measured.extent.above);
            if math::dump::enabled() {
                math::dump::write(serde_json::json!({
                    "kind": "inline", "tex": atom.math.tex.as_ref(),
                    "row": [f32::from(prepared.bounds.top()), f32::from(laid.height)],
                    "box": [f32::from(origin.x), f32::from(origin.y), f32::from(measured.width), f32::from(measured.extent.height())],
                }));
            }
            atom.math.paint(measured, origin, font, rows.font_size, window, cx);
        }
    }

    fn paint_boxes(&self, segment: &Segment, em: Pixels, window: &mut Window) {
        let layout = segment.layout;
        let line_height = layout.line_height();
        for text_box in self.boxes.iter() {
            let Some(range) = segment.local(&text_box.range) else { continue };
            let inline = text_box.inline_font.as_ref().map(|inline| {
                let size = em * inline.size_em;
                let text_system = window.text_system();
                let font_id = text_system.resolve_font(&inline.font);
                let ascent = text_system.ascent(font_id, size).round();
                let descent = text_system.descent(font_id, size).abs().round();
                let extra = size * inline.pad_y_em + inline.border;
                let baseline = layout.line_layout_for_index(range.start).map_or(line_height, |line| {
                    line_baseline_offset(line_height, line.ascent(), line.descent())
                });
                (baseline - ascent - extra, baseline + descent + extra, size * inline.pad_x_em + inline.border)
            });
            for quad in range_quads(layout, range.clone(), line_height) {
                // An inline element's padding already takes space in the line: the range's
                // first glyph sits after it and its end is the next glyph, past the trailing one.
                let (pad_left, pad_right) = inline.map_or((text_box.pad_x, text_box.pad_x), |(_, _, pad_x)| (pad_x, px(0.)));
                let (top, bottom) = match (inline, text_box.height_em) {
                    (Some((above, below, _)), _) => (quad.top() + above, quad.top() + below),
                    (None, Some(h)) => {
                        let h = em * h;
                        let top = quad.top() + (quad.size.height - h) / 2. + px(0.5);
                        (top, top + h)
                    }
                    (None, None) => (quad.top(), quad.bottom()),
                };
                let quad = Bounds::from_corners(point(quad.left() - pad_left, top), point(quad.right() + pad_right, bottom));
                window.paint_quad(fill(quad, text_box.color).corner_radii(Corners::all(text_box.radius)));
            }
        }
    }

    fn paint_icons(&self, segment: &Segment, window: &mut Window, cx: &mut App) {
        let layout = segment.layout;
        let line_height = layout.line_height();
        for text_icon in self.icons.iter() {
            let Some(range) = segment.local(&(text_icon.index..text_icon.index)) else { continue };
            let Some(origin) = layout.position_for_index(range.start) else { continue };
            let top = match text_icon.below_baseline {
                None => origin.y + (line_height - text_icon.size) / 2.,
                Some(drop) => {
                    let baseline = layout.line_layout_for_index(range.start).map_or(line_height, |line| {
                        line_baseline_offset(line_height, line.ascent(), line.descent())
                    });
                    origin.y + baseline + drop - text_icon.size
                }
            };
            let icon_bounds = Bounds::new(point(origin.x + text_icon.inset, top), gpui_kit::size(text_icon.size, text_icon.size));
            match &text_icon.paint {
                IconPaint::Svg { path, color } => {
                    let _ = window.paint_svg(icon_bounds, path.clone(), None, gpui_kit::TransformationMatrix::unit(), *color, cx);
                }
                IconPaint::Image { image, radius } => {
                    let _ = window.paint_image(icon_bounds, icon_bounds, Corners::all(*radius), image.clone(), 0, false);
                }
                IconPaint::Empty => {}
            }
        }
    }

    /// The pointing cursor over links and a click handler that opens them.
    fn listen_for_links(&self, prepaint: &RichTextPrepaint, window: &mut Window) {
        if self.links.is_empty() {
            return;
        }
        let segments: Vec<(TextLayout, usize)> = if prepaint.rows.is_empty() {
            vec![(self.styled.layout().clone(), 0)]
        } else {
            prepaint.rows.iter().map(|row| (row.layout.clone(), row.range.start)).collect()
        };
        let link_at = {
            let links = self.links.clone();
            move |position| {
                let index = segments
                    .iter()
                    .find_map(|(layout, offset)| layout.index_for_position(position).ok().map(|index| index + offset))?;
                links.iter().find(|(range, _)| range.contains(&index)).map(|(_, target)| target.clone())
            }
        };
        if prepaint.hitbox.is_hovered(window) && link_at(window.mouse_position()).is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, &prepaint.hitbox);
        }
        let hitbox = prepaint.hitbox.clone();
        let on_link = self.on_link.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left || !hitbox.is_hovered(window) {
                return;
            }
            if TextSelection::has_selection(window, cx) {
                return;
            }
            if let (Some(target), Some(on_link)) = (link_at(event.position), on_link.as_ref()) {
                on_link(&target, window, cx);
            }
        });
    }
}

/// The runs covering `range` of the text `runs` cover.
fn slice_runs(runs: &[TextRun], range: Range<usize>) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let (from, to) = (start.max(range.start), end.min(range.end));
        if from < to {
            out.push(TextRun { len: to - from, ..run.clone() });
        }
        start = end;
    }
    out
}

/// One rectangle per laid-out row covered by `range`.
fn range_quads(layout: &TextLayout, range: Range<usize>, line_height: Pixels) -> Vec<Bounds<Pixels>> {
    let (Some(start), Some(end)) = (layout.position_for_index(range.start), layout.position_for_index(range.end)) else {
        return Vec::new();
    };
    let bounds = layout.bounds();
    if start.y == end.y {
        return vec![Bounds::from_corners(start, point(end.x, end.y + line_height))];
    }
    let mut quads = vec![Bounds::from_corners(start, point(bounds.right(), start.y + line_height))];
    if end.y > start.y + line_height {
        quads.push(Bounds::from_corners(point(bounds.left(), start.y + line_height), point(bounds.right(), end.y)));
    }
    if end.x > bounds.left() + px(0.5) {
        quads.push(Bounds::from_corners(point(bounds.left(), end.y), point(end.x, end.y + line_height)));
    }
    quads
}
