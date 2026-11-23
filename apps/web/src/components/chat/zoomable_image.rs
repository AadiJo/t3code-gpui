//! An image that fits the window and zooms up to 8x. Ports `ZoomableImage.tsx`:
//! click toggles 2x at the pointer, Ctrl+scroll (and trackpad pinch) zooms
//! around the pointer, scroll or drag pans, Enter or Space toggles zoom, `+`
//! and `-` step by 1.5x, `0` fits, arrow keys pan by 40px.

use gpui_kit::{
    Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, ParentElement as _, Pixels, Point, Render, ScrollEvent, Size, Styled as _,
    StyledImage as _, Window, div, img, point, prelude::*, px, size,
};

use crate::{ActiveAppTheme as _, components::media::media_source::DecodedImage};

const MAX_ZOOM: f32 = 8.;
/// Pointer travel that turns a press into a drag instead of a click.
const DRAG_THRESHOLD: f32 = 4.;

struct Drag {
    start: Point<Pixels>,
    pan: Point<Pixels>,
    moved: bool,
}

/// The zoomable image view.
pub struct ZoomableImage {
    image: DecodedImage,
    svg: bool,
    zoom: f32,
    /// Offset of the visible box into the zoomed image.
    pan: Point<Pixels>,
    drag: Option<Drag>,
    /// Viewer size the zoom was set for; a resize refits.
    viewer: Size<Pixels>,
    /// Window position of the visible box, from the last paint.
    origin: Point<Pixels>,
    focus_handle: FocusHandle,
}

impl Focusable for ZoomableImage {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ZoomableImage {
    pub fn new(image: DecodedImage, cx: &mut Context<Self>) -> Self {
        let svg = image.format == gpui_kit::ImageFormat::Svg;
        Self { image, svg, zoom: 1., pan: Point::default(), drag: None, viewer: Size::default(), origin: Point::default(), focus_handle: cx.focus_handle() }
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn image(&self) -> &DecodedImage {
        &self.image
    }

    /// The fitted size at zoom 1 and the most room the box may take.
    fn layout(&self) -> (Size<Pixels>, Size<Pixels>) {
        let (w, h) = (self.viewer.width.as_f32(), self.viewer.height.as_f32());
        let max_h = (h * 0.86).min(h - 160.).max(1.);
        let max_w = (w * 0.92 - if w >= 640. { 96. } else { 0. }).max(1.);
        let (nw, nh) = (self.image.width.max(1) as f32, self.image.height.max(1) as f32);
        let fit = (if self.svg { f32::INFINITY } else { 1. }).min(max_w / nw).min(max_h / nh);
        (size(px(nw * fit), px(nh * fit)), size(px(max_w), px(max_h)))
    }

    fn clamp_pan(&mut self) {
        let (fitted, room) = self.layout();
        let image = size(fitted.width * self.zoom, fitted.height * self.zoom);
        let visible = size(image.width.min(room.width), image.height.min(room.height));
        self.pan.x = self.pan.x.clamp(px(0.), (image.width - visible.width).max(px(0.)));
        self.pan.y = self.pan.y.clamp(px(0.), (image.height - visible.height).max(px(0.)));
    }

    /// Zooms to `next`, keeping the image point under `at` (box-relative) in place.
    pub fn set_zoom(&mut self, next: f32, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let clamped = next.clamp(1., MAX_ZOOM);
        if (clamped - self.zoom).abs() < f32::EPSILON {
            return;
        }
        let (fitted, room) = self.layout();
        let visible = size(
            (fitted.width * self.zoom).min(room.width),
            (fitted.height * self.zoom).min(room.height),
        );
        let at = at.unwrap_or(point(visible.width / 2., visible.height / 2.));
        let anchor = point((self.pan.x + at.x) / self.zoom, (self.pan.y + at.y) / self.zoom);
        self.zoom = clamped;
        self.pan = point(anchor.x * clamped - at.x, anchor.y * clamped - at.y);
        self.clamp_pan();
        cx.notify();
    }

    /// Pans for an arrow key when zoomed; false lets the gallery take the key.
    pub fn pan_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.zoom <= 1. {
            return false;
        }
        let (fitted, room) = self.layout();
        let wider = fitted.width * self.zoom > room.width;
        let delta = match key {
            "left" if wider => point(px(-40.), px(0.)),
            "right" if wider => point(px(40.), px(0.)),
            "up" => point(px(0.), px(-40.)),
            "down" => point(px(0.), px(40.)),
            _ => return false,
        };
        self.pan = self.pan + delta;
        self.clamp_pan();
        cx.notify();
        true
    }

    /// Zoom and pan keys; true when the key was used.
    pub fn handle_key(&mut self, key: &str, held: bool, cx: &mut Context<Self>) -> bool {
        match key {
            "enter" | "space" if !held => self.set_zoom(if self.zoom > 1. { 1. } else { 2. }, None, cx),
            "+" | "=" => self.set_zoom(self.zoom * 1.5, None, cx),
            "-" => self.set_zoom(self.zoom / 1.5, None, cx),
            "0" => self.set_zoom(1., None, cx),
            _ => return self.pan_key(key, cx),
        }
        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return;
        }
        if self.handle_key(&event.keystroke.key, event.is_held, cx) {
            cx.stop_propagation();
        }
    }
}

impl Render for ZoomableImage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewer = window.viewport_size();
        if viewer != self.viewer {
            self.viewer = viewer;
            self.zoom = 1.;
            self.pan = Point::default();
        }
        let theme = cx.app_theme();
        let (fitted, room) = self.layout();
        let image = size(fitted.width * self.zoom, fitted.height * self.zoom);
        let visible = size(image.width.min(room.width), image.height.min(room.height));
        let entity = cx.entity();
        let cursor = if self.zoom > 1. {
            if self.drag.is_some() { gpui_kit::CursorStyle::ClosedHand } else { gpui_kit::CursorStyle::OpenHand }
        } else {
            gpui_kit::CursorStyle::PointingHand
        };
        div()
            .id("zoomable-image")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .w(visible.width)
            .h(visible.height)
            .overflow_hidden()
            .rounded(theme.radii.lg)
            .bg(theme.colors.canvas)
            .border_1()
            .border_color(theme.colors.border.opacity(0.7))
            .shadow(theme.shadows.dialog.clone())
            .cursor(cursor)
            .child(
                img(self.image.render.clone())
                    .absolute()
                    .left(-self.pan.x)
                    .top(-self.pan.y)
                    .w(image.width)
                    .h(image.height)
                    .object_fit(ObjectFit::Fill),
            )
            .child(gpui_kit::canvas(
                move |bounds, _, cx| entity.update(cx, |this, _| this.origin = bounds.origin),
                |_, _, _, _| {},
            ).absolute().size_full())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                    this.drag = Some(Drag { start: event.position, pan: this.pan, moved: false });
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some(drag) = &mut this.drag else { return };
                let delta = event.position - drag.start;
                if (delta.x.as_f32().powi(2) + delta.y.as_f32().powi(2)).sqrt() > DRAG_THRESHOLD {
                    drag.moved = true;
                }
                if drag.moved && this.zoom > 1. {
                    this.pan = drag.pan - delta;
                    this.clamp_pan();
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    let Some(drag) = this.drag.take() else { return };
                    if !drag.moved && event.click_count <= 1 {
                        let at = event.position - this.origin;
                        this.set_zoom(if this.zoom > 1. { 1. } else { 2. }, Some(at), cx);
                    }
                    cx.notify();
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let delta = event.delta.pixel_delta(px(16.));
                if event.modifiers.control || event.modifiers.platform {
                    if delta.y != px(0.) {
                        let at = event.position - this.origin;
                        this.set_zoom(this.zoom * (delta.y.as_f32() * 0.01).exp(), Some(at), cx);
                    }
                } else if this.zoom > 1. {
                    this.pan = this.pan - delta;
                    this.clamp_pan();
                    cx.notify();
                }
            }))
    }
}
