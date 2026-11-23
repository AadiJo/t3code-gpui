//! Selectable rich text: one laid-out paragraph with per-run fonts and
//! colours, rounded backgrounds behind inline code and file chips, and
//! clickable links. It joins the window's text selection the same way
//! gpui-base's `SelectableText` does, so a drag across paragraphs, code and
//! tables selects and copies as one document.

use std::{ops::Range, rc::Rc};

use gpui_kit::{
    App, Bounds, Corners, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, LayoutId, MouseButton, MouseUpEvent,
    Pixels, SharedString, StyledText, Subscription, TextRun, Window, fill, point, px,
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
}

/// Called with a link target when a link is clicked.
pub type LinkHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;

/// A paragraph of styled, selectable text.
pub struct RichText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    boxes: Rc<[TextBox]>,
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
            links: Rc::from([]),
            on_link: None,
            selection_color: Hsla::transparent_black(),
        }
    }

    pub fn boxes(mut self, boxes: impl Into<Rc<[TextBox]>>) -> Self {
        self.boxes = boxes.into();
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
        for text_box in self.boxes.iter() {
            for quad in range_quads(&layout, text_box.range.clone(), line_height) {
                let quad = Bounds::from_corners(
                    point(quad.left() - text_box.pad_x, quad.top()),
                    point(quad.right() + text_box.pad_x, quad.bottom()),
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
