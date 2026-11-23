//! An image in the file viewer. Ports `PreviewImageContent.tsx`: a bar with
//! the pixel size and zoom out, percentage, zoom in, fit and actual size; the
//! image centred with 16px of room, scrolling when larger than the panel, and
//! a click opening the expanded viewer. Dragging pans the scroll position.

use gpui_kit::{
    Context, EventEmitter, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ObjectFit, ParentElement as _,
    Pixels, Point, Render, ScrollHandle, SharedString, Size, Styled as _, StyledImage as _, Window, canvas, div, img,
    point, prelude::*, px,
};

use crate::{
    ActiveAppTheme as _,
    components::{
        icons::IconName,
        media::media_source::DecodedImage,
        ui::{
            button::{Button, ButtonSize, ButtonVariant},
            toggle::{Toggle, ToggleSize, ToggleVariant},
            tooltip::tooltip,
        },
    },
    i18n::t,
};

const MIN_SCALE: f32 = 0.05;
const MAX_SCALE: f32 = 8.;

/// Asks the owner to open the expanded viewer.
#[derive(Clone, Debug)]
pub struct ExpandImage;

/// The image surface.
pub struct PreviewImage {
    image: DecodedImage,
    name: SharedString,
    /// `None` fits the panel.
    zoom: Option<f32>,
    viewport: Size<Pixels>,
    scroll: ScrollHandle,
    drag: Option<(Point<Pixels>, Point<Pixels>, bool)>,
}

impl EventEmitter<ExpandImage> for PreviewImage {}

impl PreviewImage {
    pub fn new(image: DecodedImage, name: SharedString) -> Self {
        Self { image, name, zoom: None, viewport: Size::default(), scroll: ScrollHandle::new(), drag: None }
    }

    pub fn image(&self) -> &DecodedImage {
        &self.image
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    fn fit(&self) -> f32 {
        let (w, h) = (self.image.width.max(1) as f32, self.image.height.max(1) as f32);
        1f32.min((self.viewport.width.as_f32() - 32.).max(1.) / w).min((self.viewport.height.as_f32() - 32.).max(1.) / h)
    }

    /// The scale shown: the chosen zoom, or the fit.
    pub fn scale(&self) -> f32 {
        self.zoom.unwrap_or_else(|| self.fit())
    }

    /// `changeZoom`: multiplies the scale, clamped to 5% to 800%.
    pub fn change_zoom(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom = Some((self.scale() * factor).clamp(MIN_SCALE, MAX_SCALE));
        cx.notify();
    }

    pub fn set_zoom(&mut self, zoom: Option<f32>, cx: &mut Context<Self>) {
        self.zoom = zoom;
        cx.notify();
    }

    fn action(&self, id: &'static str, label: SharedString, icon: IconName, disabled: bool, cx: &mut Context<Self>, f: fn(&mut Self, &mut Context<Self>)) -> impl IntoElement {
        div().id(id).tooltip(tooltip(label)).child(
            Button::new(id)
                .variant(ButtonVariant::Ghost)
                .size(ButtonSize::IconSm)
                .icon(icon)
                .disabled(disabled)
                .on_click(cx.listener(move |this, _, _, cx| f(this, cx))),
        )
    }

    fn toggle(&self, id: &'static str, label: SharedString, icon: IconName, pressed: bool, cx: &mut Context<Self>, zoom: Option<f32>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        div().id(id).tooltip(tooltip(label)).child(
            Toggle::new(id).variant(ToggleVariant::Ghost).size(ToggleSize::Sm).icon(icon).pressed(pressed).on_toggle(move |_, _, cx| {
                view.update(cx, |this, cx| this.set_zoom(zoom, cx)).ok();
            }),
        )
    }
}

impl Render for PreviewImage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        let scale = self.scale();
        let (w, h) = (self.image.width as f32 * scale, self.image.height as f32 * scale);
        let dims = format!("{} × {}", crate::components::files::delimited_table::group_digits(self.image.width as usize), crate::components::files::delimited_table::group_digits(self.image.height as usize));
        let percent = format!("{}%", (scale * 100.).round() as i32);
        let entity = cx.entity();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .min_h(px(40.))
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(4.))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.6))
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(dims)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(self.action("image-zoom-out", t!("Zoom out"), IconName::Minus, scale <= MIN_SCALE, cx, |this, cx| this.change_zoom(1. / 1.25, cx)))
                            .child(div().min_w(px(40.)).text_center().child(percent))
                            .child(self.action("image-zoom-in", t!("Zoom in"), IconName::Plus, scale >= MAX_SCALE, cx, |this, cx| this.change_zoom(1.25, cx)))
                            .child(self.toggle("image-fit", t!("Fit image"), IconName::Scan, self.zoom.is_none(), cx, None))
                            .child(self.toggle("image-actual", t!("Actual image size"), IconName::Maximize, self.zoom == Some(1.), cx, Some(1.))),
                    ),
            )
            .child(
                div()
                    .id("preview-image-scroll")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .child(canvas(move |bounds, _, cx| entity.update(cx, |this, cx| {
                        if this.viewport != bounds.size {
                            this.viewport = bounds.size;
                            cx.notify();
                        }
                    }), |_, _, _, _| {}).absolute().size_full())
                    .child(
                        div()
                            .min_w_full()
                            .min_h_full()
                            .w(px(w + 32.))
                            .h(px(h + 32.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .p(px(16.))
                            .child(
                                div()
                                    .id("preview-image")
                                    .flex_none()
                                    .cursor(gpui_kit::CursorStyle::PointingHand)
                                    .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, _| {
                                        this.drag = Some((event.position, this.scroll.offset(), false));
                                    }))
                                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                                        let Some((start, offset, moved)) = &mut this.drag else { return };
                                        let delta = event.position - *start;
                                        if delta.x.as_f32().abs() + delta.y.as_f32().abs() > 4. {
                                            *moved = true;
                                        }
                                        if *moved {
                                            this.scroll.set_offset(point(offset.x + delta.x, offset.y + delta.y));
                                            cx.notify();
                                        }
                                    }))
                                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                        if let Some((_, _, false)) = this.drag.take() {
                                            cx.emit(ExpandImage);
                                        }
                                    }))
                                    .child(img(self.image.render.clone()).w(px(w)).h(px(h)).object_fit(ObjectFit::Fill)),
                            ),
                    ),
            )
    }
}
