//! `ProviderAccentColorPicker` (inline layout): a 28px swatch button that
//! opens a popover with the saturation/value plane, the hue slider, a hex
//! field, and "Add color" or "Clear color". Ports
//! `ProviderAccentColorPicker.tsx` and the `ColorSaturationValuePlane` /
//! `ColorHueSlider` primitives of `ui/color-picker.tsx`.
//!
//! A pointer drag previews and commits once on release; a typed hex commits
//! as soon as it is a full `#rrggbb`.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, Bounds, Context, Entity, Hsla, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Render, SharedString, Subscription, Window, canvas, div, hsla, linear_color_stop, linear_gradient, prelude::*,
    px, rgb,
};

use crate::ActiveAppTheme as _;
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::components::ui::popover::{Popover, PopoverPadding, PopoverSurface};
use crate::i18n::t;
use crate::provider_instances::normalize_provider_accent_color;
use crate::theme_color::{parse_css_color, with_alpha};

const FALLBACK_ACCENT_COLOR: &str = "#2563eb";

/// A colour in hue (0–360), saturation and value (0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

/// `hexToHsv` from `lib/color.ts`.
pub fn hex_to_hsv(hex: &str) -> Hsv {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    let (r, g, b) = (((n >> 16) & 255) as f32 / 255., ((n >> 8) & 255) as f32 / 255., (n & 255) as f32 / 255.);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let mut h = 0.;
    if delta != 0. {
        h = if max == r {
            ((g - b) / delta) % 6.
        } else if max == g {
            (b - r) / delta + 2.
        } else {
            (r - g) / delta + 4.
        };
        h *= 60.;
        if h < 0. {
            h += 360.;
        }
    }
    Hsv { h, s: if max == 0. { 0. } else { delta / max }, v: max }
}

/// `hsvToHex` from `lib/color.ts`.
pub fn hsv_to_hex(hsv: Hsv) -> String {
    let h = ((hsv.h % 360.) + 360.) % 360.;
    let c = hsv.v * hsv.s;
    let x = c * (1. - (((h / 60.) % 2.) - 1.).abs());
    let m = hsv.v - c;
    let (r, g, b) = match h {
        h if h < 60. => (c, x, 0.),
        h if h < 120. => (x, c, 0.),
        h if h < 180. => (0., c, x),
        h if h < 240. => (0., x, c),
        h if h < 300. => (x, 0., c),
        _ => (c, 0., x),
    };
    let channel = |v: f32| ((v + m) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", channel(r), channel(g), channel(b))
}

fn hex_hsla(hex: &str) -> Hsla {
    parse_css_color(hex).map(|c| c.hsla()).unwrap_or_else(|| hsla(0., 0., 0., 1.))
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Plane,
    Hue,
}

/// The picker for one provider instance. The panel keeps one per instance and
/// calls [`ProviderAccentColorPicker::sync`] each render.
pub struct ProviderAccentColorPicker {
    display_name: SharedString,
    /// The saved colour, normalized, or `None`.
    value: Option<String>,
    hsv: Hsv,
    drag: Option<Drag>,
    /// The colour of a drag not yet committed.
    preview: Option<String>,
    plane: Bounds<Pixels>,
    hue: Bounds<Pixels>,
    hex: Entity<InputState>,
    on_commit: Rc<dyn Fn(String, &mut Window, &mut App)>,
    _subscription: Subscription,
}

impl ProviderAccentColorPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hex = cx.new(|cx| InputState::new(window, cx).default_value(FALLBACK_ACCENT_COLOR));
        let subscription = cx.subscribe_in(&hex, window, |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let text = state.read(cx).value().to_string();
                if text.len() == 7 && text.starts_with('#') && text[1..].chars().all(|c| c.is_ascii_hexdigit()) {
                    this.hsv = hex_to_hsv(&text);
                    this.commit(text.to_lowercase(), window, cx);
                }
            }
            InputEvent::Blur => this.sync_hex(window, cx),
            _ => {}
        });
        Self {
            display_name: SharedString::default(),
            value: None,
            hsv: hex_to_hsv(FALLBACK_ACCENT_COLOR),
            drag: None,
            preview: None,
            plane: Bounds::default(),
            hue: Bounds::default(),
            hex,
            on_commit: Rc::new(|_, _, _| {}),
            _subscription: subscription,
        }
    }

    /// Updates the saved value, the name for labels, and the commit callback.
    pub fn sync(
        &mut self,
        display_name: SharedString,
        value: Option<&str>,
        on_commit: impl Fn(String, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.display_name = display_name;
        self.on_commit = Rc::new(on_commit);
        let value = normalize_provider_accent_color(value);
        if value != self.value {
            self.value = value;
            self.preview = None;
            let shown = self.value.clone().unwrap_or_else(|| FALLBACK_ACCENT_COLOR.into());
            if hsv_to_hex(self.hsv) != shown {
                self.hsv = hex_to_hsv(&shown);
                self.sync_hex(window, cx);
            }
        }
    }

    fn current(&self) -> String {
        hsv_to_hex(self.hsv)
    }

    fn sync_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current();
        if self.hex.read(cx).value().as_ref() != text {
            self.hex.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    fn commit(&mut self, color: String, window: &mut Window, cx: &mut Context<Self>) {
        let normalized = normalize_provider_accent_color(Some(&color)).unwrap_or_default();
        self.preview = None;
        (self.on_commit)(normalized, window, cx);
        cx.notify();
    }

    fn change(&mut self, hsv: Hsv, window: &mut Window, cx: &mut Context<Self>) {
        self.hsv = hsv;
        self.sync_hex(window, cx);
        if self.drag.is_some() {
            self.preview = Some(self.current());
            cx.notify();
        } else {
            self.commit(self.current(), window, cx);
        }
    }

    fn drag_to(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let fraction = |pos: Pixels, origin: Pixels, size: Pixels| {
            if size <= px(0.) { 0. } else { (f32::from(pos - origin) / f32::from(size)).clamp(0., 1.) }
        };
        match self.drag {
            Some(Drag::Plane) => {
                let s = fraction(position.x, self.plane.origin.x, self.plane.size.width);
                let v = 1. - fraction(position.y, self.plane.origin.y, self.plane.size.height);
                self.change(Hsv { s, v, ..self.hsv }, window, cx);
            }
            Some(Drag::Hue) => {
                let h = fraction(position.x, self.hue.origin.x, self.hue.size.width) * 360.;
                self.change(Hsv { h, ..self.hsv }, window, cx);
            }
            None => {}
        }
    }

    fn end_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            let color = self.preview.take().unwrap_or_else(|| self.current());
            self.commit(color, window, cx);
        }
    }

    /// The popover body: plane, hue slider, hex field, add or clear.
    fn render_panel(&self, close: crate::components::ui::popover::PopupHandle, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let entity = cx.entity();
        let hue_color = hex_hsla(&hsv_to_hex(Hsv { h: self.hsv.h, s: 1., v: 1. }));
        let current = hex_hsla(&self.current());
        let thumb = |size: f32| {
            div().absolute().size(px(size)).rounded_full().border_2().border_color(hsla(0., 0., 1., 1.)).shadow(vec![gpui_kit::BoxShadow {
                color: hsla(0., 0., 0., 0.4),
                offset: gpui_kit::point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(1.),
                inset: false,
            }])
        };
        let measure = |entity: Entity<Self>, which: Drag| {
            canvas(
                move |bounds, _, cx| {
                    entity.update(cx, |this, _| match which {
                        Drag::Plane => this.plane = bounds,
                        Drag::Hue => this.hue = bounds,
                    })
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        };
        let plane_w = theme.rem(14.);
        let plane_h = theme.sp(36.);
        let plane = div()
            .id("accent-plane")
            .relative()
            .w(plane_w)
            .h(plane_h)
            .cursor_crosshair()
            .bg(hue_color)
            .child(measure(entity.clone(), Drag::Plane))
            .child(div().absolute().size_full().bg(linear_gradient(90., linear_color_stop(hsla(0., 0., 1., 1.), 0.), linear_color_stop(hsla(0., 0., 1., 0.), 1.))))
            .child(div().absolute().size_full().bg(linear_gradient(0., linear_color_stop(hsla(0., 0., 0., 1.), 0.), linear_color_stop(hsla(0., 0., 0., 0.), 1.))))
            .child(
                thumb(12.)
                    .left(plane_w * self.hsv.s - px(6.))
                    .top(plane_h * (1. - self.hsv.v) - px(6.))
                    .bg(current),
            )
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &gpui_kit::MouseDownEvent, window, cx| {
                this.drag = Some(Drag::Plane);
                this.drag_to(e.position, window, cx);
            }));
        let stops: [u32; 7] = [0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xff0000];
        let track = div().flex().w_full().h(px(10.)).children((0..6).map(|ix| {
            div()
                .flex_1()
                .h_full()
                .when(ix == 0, |d| d.rounded_l_full())
                .when(ix == 5, |d| d.rounded_r_full())
                .bg(linear_gradient(90., linear_color_stop(rgb(stops[ix]), 0.), linear_color_stop(rgb(stops[ix + 1]), 1.)))
        }));
        let hue = div()
            .id("accent-hue")
            .relative()
            .flex()
            .items_center()
            .h(px(24.))
            .w_full()
            .cursor_pointer()
            .child(measure(entity.clone(), Drag::Hue))
            .child(track)
            .child(thumb(16.).left(relative_left(self.hsv.h / 360., self.hue.size.width) - px(8.)).top(px(4.)).bg(hue_color))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &gpui_kit::MouseDownEvent, window, cx| {
                this.drag = Some(Drag::Hue);
                this.drag_to(e.position, window, cx);
            }));
        // While dragging, follow the pointer anywhere in the window.
        let tracker = self.drag.is_some().then(|| {
            let entity = entity.clone();
            canvas(|_, _, _| {}, move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, _, window, cx| {
                    moved.update(cx, |this, cx| this.drag_to(e.position, window, cx));
                });
                let up = entity.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, window, cx| {
                    up.update(cx, |this, cx| this.end_drag(window, cx));
                });
            })
            .absolute()
            .size_0()
        });
        let saved = self.value.is_some();
        let add_close = close.clone();
        let commit_entity = entity.clone();
        let clear_entity = entity;
        PopoverSurface::new()
            .padding(PopoverPadding::None)
            .child(
                div()
                    .w(plane_w)
                    .flex()
                    .flex_col()
                    .children(tracker)
                    .child(plane)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(theme.sp(3.))
                            .p(theme.sp(3.))
                            .child(hue)
                            .child(TextInput::new(&self.hex).size(InputSize::Compact).mono())
                            .when(!saved, |d| {
                                d.child(Button::new("accent-add").size(ButtonSize::Compact).label(t!("Add color")).on_click(move |_, window, cx| {
                                    commit_entity.update(cx, |this, cx| this.commit(this.current(), window, cx));
                                    add_close.close(cx);
                                }))
                            }),
                    ),
            )
            .when(saved, |s| {
                s.child(
                    div().border_t_1().border_color(with_alpha(c.border, 0.6)).p(theme.sp(1.)).child(
                        Button::new("accent-clear")
                            .variant(ButtonVariant::GhostMuted)
                            .size(ButtonSize::Compact)
                            .icon(IconName::X)
                            .label(t!("Clear color"))
                            .on_click(move |_, window, cx| {
                                clear_entity.update(cx, |this, cx| this.commit(String::new(), window, cx));
                                close.close(cx);
                            }),
                    ),
                )
            })
    }
}

fn relative_left(fraction: f32, width: Pixels) -> Pixels {
    width * fraction.clamp(0., 1.)
}

impl Render for ProviderAccentColorPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let shown = self.preview.clone().or_else(|| self.value.clone());
        let trigger = div()
            .id("accent-trigger")
            .size(theme.sp(7.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.radii.lg)
            .cursor_pointer()
            .when_some(shown.as_deref().map(hex_hsla), |d, color| d.bg(color))
            .when(shown.is_none(), |d| {
                d.text_color(c.muted_foreground)
                    .hover(|d| d.bg(c.accent).text_color(c.foreground()))
                    .child(Icon::new(IconName::Plus).size(IconSize::Md).color(c.muted_foreground))
            });
        let entity = cx.entity();
        Popover::new("accent-popover", trigger, move |close, _, cx| {
            entity.update(cx, |this, cx| this.render_panel(close, cx).into_any_element())
        })
    }
}
