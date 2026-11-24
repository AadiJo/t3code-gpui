//! Ports `ThemeColorPicker.tsx`: the role label, the swatch that opens the
//! picker (saturation and value plane, hue slider, HEX and RGB fields) and
//! the compact hex field of one theme editor colour row.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, Bounds, Context, Entity, Hsla, Styled, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    SharedString, Subscription, Window, canvas, div, hsla, linear_color_stop, linear_gradient, prelude::*, px, rgb,
};

use super::provider_accent_color_picker::{Hsv, hex_to_hsv, hsv_to_hex};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::i18n::t;
use crate::theme_color::{parse_css_color, with_alpha};
use crate::vscode_theme_import::theme_color_to_hex;

/// `getThemeRoleLabel`: the editor's label for a role, or the role split into words.
pub fn theme_role_label(role: &str) -> SharedString {
    match role {
        "canvas" => t!("Background"),
        "toolbar" => t!("Toolbar background"),
        "toolbarForeground" => t!("Toolbar text"),
        "toolbarBorder" => t!("Toolbar border"),
        "toolbarControl" => t!("Toolbar control"),
        "toolbarControlForeground" => t!("Toolbar control text"),
        "toolbarControlHover" => t!("Toolbar control hover"),
        "accent" => t!("Accent color"),
        "errorForeground" => t!("Error text"),
        "errorSurface" => t!("Error background"),
        "warningForeground" => t!("Warning text"),
        "warningSurface" => t!("Warning background"),
        "updateForeground" => t!("Update text"),
        "updateSurface" => t!("Update background"),
        _ => {
            let mut words = String::new();
            for (ix, ch) in role.chars().enumerate() {
                if ix == 0 {
                    words.extend(ch.to_uppercase());
                } else if ch.is_ascii_uppercase() {
                    words.push(' ');
                    words.push(ch);
                } else {
                    words.push(ch);
                }
            }
            words.into()
        }
    }
}

/// The colour a swatch paints for a theme colour string.
pub fn swatch_color(value: &str) -> Hsla {
    theme_color_to_hex(value).and_then(|hex| parse_css_color(&hex)).map(|c| c.hsla()).unwrap_or_else(|| hsla(0., 0., 0., 1.))
}

/// `normalizeThemePickerColor` and `themePickerAlphaSuffix`: `#rrggbb` and the alpha suffix to keep.
fn picker_color(value: &str) -> (String, String) {
    let hex = theme_color_to_hex(value).unwrap_or_else(|| "#000000".into());
    let alpha = if hex.len() == 9 && &hex[7..] != "ff" { hex[7..].to_owned() } else { String::new() };
    (hex[..7].to_owned(), alpha)
}

fn rgb_text(hex: &str) -> String {
    let n = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    format!("{}, {}, {}", n >> 16, (n >> 8) & 255, n & 255)
}

/// `themeRgbToHex`: `rgb(1, 2, 3)` or `1 2 3` into `#010203`.
fn rgb_to_hex(text: &str) -> Option<String> {
    let lower = text.trim().to_ascii_lowercase();
    let body = lower.strip_prefix("rgb(").map_or(lower.as_str(), |rest| rest.trim_start()).trim_end_matches(')').trim_end();
    let channels: Vec<u8> = body.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()).map(str::parse).collect::<Result<_, _>>().ok()?;
    let [r, g, b] = channels.as_slice() else { return None };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    Plane,
    Hue,
}

/// The popover body (`ThemeColorPickerPanel`). Each change calls `on_change`
/// with the new colour, keeping the edited colour's alpha.
pub struct ThemeColorPickerPanel {
    label: SharedString,
    hsv: Hsv,
    alpha: String,
    drag: Option<Drag>,
    plane: Bounds<Pixels>,
    hue: Bounds<Pixels>,
    hex: Entity<InputState>,
    rgb: Entity<InputState>,
    on_change: Rc<dyn Fn(String, &mut Window, &mut App)>,
    _subscriptions: [Subscription; 2],
}

impl ThemeColorPickerPanel {
    pub fn new(label: SharedString, value: &str, on_change: Rc<dyn Fn(String, &mut Window, &mut App)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (hex_value, alpha) = picker_color(value);
        let hex = cx.new(|cx| InputState::new(window, cx).default_value(hex_value.clone()));
        let rgb = cx.new(|cx| InputState::new(window, cx).default_value(rgb_text(&hex_value)));
        let on_hex = cx.subscribe_in(&hex, window, |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let text = state.read(cx).value().to_string();
                let valid = text.len() == 7 && text.starts_with('#') && text[1..].chars().all(|c| c.is_ascii_hexdigit());
                if valid {
                    let color = text.to_ascii_lowercase();
                    this.hsv = hex_to_hsv(&color);
                    let rgb = rgb_text(&color);
                    this.rgb.update(cx, |input, cx| input.set_value(rgb, window, cx));
                    this.emit(color, window, cx);
                }
            }
            InputEvent::Blur => this.sync_fields(window, cx),
            _ => {}
        });
        let on_rgb = cx.subscribe_in(&rgb, window, |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                if let Some(color) = rgb_to_hex(&state.read(cx).value()) {
                    this.hsv = hex_to_hsv(&color);
                    let hex = color.clone();
                    this.hex.update(cx, |input, cx| input.set_value(hex, window, cx));
                    this.emit(color, window, cx);
                }
            }
            InputEvent::Blur => this.sync_fields(window, cx),
            _ => {}
        });
        Self {
            label,
            hsv: hex_to_hsv(&hex_value),
            alpha,
            drag: None,
            plane: Bounds::default(),
            hue: Bounds::default(),
            hex,
            rgb,
            on_change,
            _subscriptions: [on_hex, on_rgb],
        }
    }

    fn current(&self) -> String {
        hsv_to_hex(self.hsv)
    }

    fn emit(&mut self, color: String, window: &mut Window, cx: &mut Context<Self>) {
        (self.on_change)(format!("{color}{}", self.alpha), window, cx);
        cx.notify();
    }

    /// Shows the current colour in both fields, as blurring a field does.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let color = self.current();
        let rgb = rgb_text(&color);
        self.hex.update(cx, |input, cx| input.set_value(color, window, cx));
        self.rgb.update(cx, |input, cx| input.set_value(rgb, window, cx));
    }

    fn drag_to(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let fraction = |pos: Pixels, origin: Pixels, size: Pixels| {
            if size <= px(0.) { 0. } else { (f32::from(pos - origin) / f32::from(size)).clamp(0., 1.) }
        };
        match self.drag {
            Some(Drag::Plane) => {
                self.hsv.s = fraction(position.x, self.plane.origin.x, self.plane.size.width);
                self.hsv.v = 1. - fraction(position.y, self.plane.origin.y, self.plane.size.height);
            }
            Some(Drag::Hue) => self.hsv.h = fraction(position.x, self.hue.origin.x, self.hue.size.width) * 360.,
            None => return,
        }
        self.sync_fields(window, cx);
        self.emit(self.current(), window, cx);
    }
}

impl Render for ThemeColorPickerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let entity = cx.entity();
        let sp = |n: f32| theme.sp(n);
        let current = swatch_color(&self.current());
        let hue_color = swatch_color(&hsv_to_hex(Hsv { h: self.hsv.h, s: 1., v: 1. }));
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
        let plane_h = sp(32.);
        let plane = div()
            .id("theme-picker-plane")
            .relative()
            .w_full()
            .h(plane_h)
            .overflow_hidden()
            .rounded(theme.radii.lg)
            .cursor_crosshair()
            .bg(hue_color)
            .child(measure(entity.clone(), Drag::Plane))
            .child(div().absolute().size_full().bg(linear_gradient(90., linear_color_stop(hsla(0., 0., 1., 1.), 0.), linear_color_stop(hsla(0., 0., 1., 0.), 1.))))
            .child(div().absolute().size_full().bg(linear_gradient(0., linear_color_stop(hsla(0., 0., 0., 1.), 0.), linear_color_stop(hsla(0., 0., 0., 0.), 1.))))
            .child(thumb(12.).left(self.plane.size.width * self.hsv.s - px(6.)).top(plane_h * (1. - self.hsv.v) - px(6.)).bg(current))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &gpui_kit::MouseDownEvent, window, cx| {
                this.drag = Some(Drag::Plane);
                this.drag_to(e.position, window, cx);
            }));
        let stops: [u32; 7] = [0xff0000, 0xffff00, 0x00ff00, 0x00ffff, 0x0000ff, 0xff00ff, 0xff0000];
        let track = div().flex().w_full().h(sp(2.5)).children((0..6).map(|ix| {
            div()
                .flex_1()
                .h_full()
                .when(ix == 0, |d| d.rounded_l_full())
                .when(ix == 5, |d| d.rounded_r_full())
                .bg(linear_gradient(90., linear_color_stop(rgb(stops[ix]), 0.), linear_color_stop(rgb(stops[ix + 1]), 1.)))
        }));
        let hue = div()
            .id("theme-picker-hue")
            .relative()
            .flex()
            .items_center()
            .h(sp(6.))
            .w_full()
            .cursor_pointer()
            .child(measure(entity.clone(), Drag::Hue))
            .child(track)
            .child(thumb(16.).left(self.hue.size.width * (self.hsv.h / 360.) - px(8.)).top(sp(3.) - px(8.)).bg(hue_color))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &gpui_kit::MouseDownEvent, window, cx| {
                this.drag = Some(Drag::Hue);
                this.drag_to(e.position, window, cx);
            }));
        // While dragging, follow the pointer anywhere in the window.
        let tracker = self.drag.is_some().then(|| {
            canvas(|_, _, _| {}, move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, _, window, cx| moved.update(cx, |this, cx| this.drag_to(e.position, window, cx)));
                let up = entity.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                    up.update(cx, |this, cx| {
                        this.drag = None;
                        cx.notify();
                    })
                });
            })
            .absolute()
            .size_0()
        });
        let field = |caption: &'static str, input: &Entity<InputState>, dot: bool| {
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap(sp(1.))
                .child(div().px(sp(1.)).ui_text(&theme, TextSize::Xs3, weight::SEMIBOLD).text_color(c.muted_foreground).child(caption))
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(sp(2.))
                        .rounded(theme.radii.lg)
                        .border_1()
                        .border_color(c.input)
                        .bg(c.background())
                        .px(sp(2.))
                        .when(dot, |d| d.child(div().flex_none().size(sp(3.5)).rounded_full().bg(current)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(sp(8.))
                                .child(Styled::h(gpui_kit::component::input::Input::new(input).appearance(false), sp(8.)).mono_text(px(TextSize::Xs.size(theme.fonts.interface)), weight::NORMAL)),
                        ),
                )
        };
        div()
            .w(theme.rem(18.))
            .children(tracker)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(with_alpha(c.border, 0.7))
                    .px(sp(4.))
                    .py(sp(3.))
                    .child(
                        div()
                            .min_w_0()
                            .child(div().truncate().ui_text(&theme, TextSize::Xs, weight::SEMIBOLD).text_color(c.foreground()).child(self.label.clone()))
                            .child(div().ui_text(&theme, TextSize::Xs2, weight::NORMAL).text_color(c.muted_foreground).child(t!("Choose a color"))),
                    )
                    .child(div().flex_none().size(sp(7.)).rounded_full().bg(current).shadow(theme.shadows.sm5.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(3.))
                    .p(sp(3.))
                    .child(plane)
                    .child(hue)
                    .child(
                        // `grid-cols-[1fr_1.2fr] gap-2` across the 16.5rem body.
                        div()
                            .flex()
                            .gap(sp(2.))
                            .child(field("HEX", &self.hex, true).w(theme.rem(16. / 2.2)))
                            .child(field("RGB", &self.rgb, false).flex_1()),
                    ),
            )
    }
}
