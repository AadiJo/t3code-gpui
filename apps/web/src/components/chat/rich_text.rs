//! Selectable rich text: one laid-out paragraph with per-run fonts and
//! colours, rounded backgrounds behind inline code and file chips, and
//! clickable links. It joins the window's text selection the same way
//! gpui-base's `SelectableText` does, so a drag across paragraphs, code and
//! tables selects and copies as one document.

use std::{ops::Range, rc::Rc};

use gpui_kit::{
    App, Bounds, Corners, CursorStyle, DispatchPhase, Element, ElementId, Font, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, LayoutId, MouseButton, MouseUpEvent,
    Pixels, SharedString, StyledText, Subscription, TextRun, Window, fill, line_baseline_offset, point, px,
    base::{TextSelection, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun},
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

/// Called with a link target when a link is clicked.
pub type LinkHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// A paragraph of styled, selectable text.
pub struct RichText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    boxes: Rc<[TextBox]>,
    icons: Rc<[TextIcon]>,
    links: Rc<[(Range<usize>, SharedString)]>,
    on_link: Option<LinkHandler>,
    selection_color: Hsla,
}

impl RichText {
    /// Text laid out with `runs`, which must cover `text` exactly.
    pub fn new(id: impl Into<ElementId>, text: SharedString, runs: Vec<TextRun>) -> Self {
        Self {
            id: id.into(),
            styled: StyledText::new(text.clone()).with_runs(runs),
            text,
            boxes: Rc::from([]),
            icons: Rc::from([]),
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

/// What a paragraph keeps between frames: its selection participant.
struct OwnedSelection {
    handle: TextSelectionHandle,
    _refresh: Subscription,
}

pub struct RichTextPrepaint {
    hitbox: Hitbox,
}

impl Element for RichText {
    type RequestLayoutState = TextSelectionHandle;
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
    ) -> (LayoutId, TextSelectionHandle) {
        let text = self.text.clone();
        let handle = window.with_element_state(
            global_id.expect("RichText has an element id"),
            |retained: Option<OwnedSelection>, window| {
                let retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(text, cx);
                    let refresh = handle.refresh_window_on_change(window, cx);
                    OwnedSelection { handle, _refresh: refresh }
                });
                (retained.handle.clone(), retained)
            },
        );
        let (layout_id, ()) = self.styled.request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut TextSelectionHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> RichTextPrepaint {
        self.styled.prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        RichTextPrepaint { hitbox }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut TextSelectionHandle,
        prepaint: &mut RichTextPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let line_height = layout.line_height();
        let em = window.text_style().font_size.to_pixels(window.rem_size());
        for text_box in self.boxes.iter() {
            let inline = text_box.inline_font.as_ref().map(|inline| {
                let size = em * inline.size_em;
                let text_system = window.text_system();
                let font_id = text_system.resolve_font(&inline.font);
                let ascent = text_system.ascent(font_id, size).round();
                let descent = text_system.descent(font_id, size).abs().round();
                let extra = size * inline.pad_y_em + inline.border;
                let baseline = layout.line_layout_for_index(text_box.range.start).map_or(line_height, |line| {
                    line_baseline_offset(line_height, line.ascent(), line.descent(), window.scale_factor())
                });
                (baseline - ascent - extra, baseline + descent + extra, size * inline.pad_x_em + inline.border)
            });
            for quad in layout.fragment_bounds(text_box.range.clone()) {
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
                let quad = Bounds::from_corners(
                    point(quad.left() - pad_left, top),
                    point(quad.right() + pad_right, bottom),
                );
                window.paint_quad(fill(quad, text_box.color).corner_radii(Corners::all(text_box.radius)));
            }
        }
        let projection = handle.update_runs(&[TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)], cx);
        for range in projection.ranges().iter().flatten() {
            for quad in range_quads(&layout, range.clone(), line_height) {
                window.paint_quad(fill(quad, self.selection_color));
            }
        }
        self.styled.paint(global_id, inspector_id, bounds, &mut (), &mut (), window, cx);
        for text_icon in self.icons.iter() {
            let Some(origin) = layout.position_for_index(text_icon.index) else {
                continue;
            };
            let top = match text_icon.below_baseline {
                None => origin.y + (line_height - text_icon.size) / 2.,
                Some(drop) => {
                    let baseline = layout.line_layout_for_index(text_icon.index).map_or(line_height, |line| {
                        line_baseline_offset(line_height, line.ascent(), line.descent(), window.scale_factor())
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

        if self.links.is_empty() {
            return;
        }
        let link_at = {
            let links = self.links.clone();
            let layout = layout.clone();
            move |position| {
                let index = layout.index_for_position(position).ok()?;
                links.iter().find(|(range, _)| range.contains(&index)).map(|(_, target)| target.clone())
            }
        };
        if prepaint.hitbox.is_hovered(window)
            && let Some(position) = Some(window.mouse_position())
            && link_at(position).is_some()
        {
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

/// One rectangle per laid-out row covered by `range`.
fn range_quads(layout: &gpui_kit::TextLayout, range: Range<usize>, line_height: Pixels) -> Vec<Bounds<Pixels>> {
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
