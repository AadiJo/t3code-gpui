//! The settings range input (`.settings-slider` in `index.css`) with its
//! value chip: a 6px pill track filled with the primary colour up to the
//! thumb's centre, the rest `muted-foreground/22%` with a 1px inset
//! `border/55%` edge, and a 16px thumb with a 2px primary border on the
//! background colour. Click or drag anywhere on the 24px row to set the
//! value; Left/Down and Right/Up step it while it has focus.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, Bounds, ElementId, Empty, FocusHandle, IntoElement, Pixels, RenderOnce, SharedString, Window,
    canvas, div, prelude::*, px, relative,
};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::theme_color::with_alpha;

/// The value a drag belongs to, so two sliders never answer each other's drag.
struct SliderDrag(ElementId);

/// A `.settings-slider` range input.
#[derive(IntoElement)]
pub struct SettingsSlider {
    id: ElementId,
    value: u64,
    min: u64,
    max: u64,
    step: u64,
    on_change: Rc<dyn Fn(u64, &mut Window, &mut App)>,
}

impl SettingsSlider {
    /// `range` is (min, max, step).
    pub fn new(
        id: impl Into<ElementId>,
        value: u64,
        range: (u64, u64, u64),
        on_change: impl Fn(u64, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self { id: id.into(), value, min: range.0, max: range.1, step: range.2.max(1), on_change: Rc::new(on_change) }
    }
}

/// The stepped value at `x` across `bounds`, where the thumb's centre
/// travels from 8px in from each end.
fn value_at(x: Pixels, bounds: Bounds<Pixels>, thumb: Pixels, (min, max, step): (u64, u64, u64)) -> u64 {
    let span = (bounds.size.width - thumb).max(px(1.));
    let ratio = ((x - bounds.origin.x - thumb / 2.) / span).clamp(0., 1.);
    let raw = ratio * (max - min) as f32;
    let stepped = (raw / step as f32).round() as u64 * step + min;
    stepped.clamp(min, max)
}

impl RenderOnce for SettingsSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let bounds = window.use_keyed_state((self.id.clone(), "bounds"), cx, |_, _| Rc::new(Cell::new(Bounds::default())));
        let bounds = bounds.read(cx).clone();
        let focus = window.use_keyed_state((self.id.clone(), "focus"), cx, |_, cx| cx.focus_handle());
        let focus: FocusHandle = focus.read(cx).clone();
        let theme = cx.app_theme();
        let c = &theme.colors;
        let range = (self.min, self.max, self.step);
        let ratio = self.value.saturating_sub(self.min) as f32 / (self.max - self.min).max(1) as f32;
        let thumb = theme.rem(1.);
        let focused = focus.is_focused(window);
        let primary = c.primary();
        let (press, drag_move, keys) = (self.on_change.clone(), self.on_change.clone(), self.on_change.clone());
        let (store, press_bounds, own_id, value) = (bounds.clone(), bounds, self.id.clone(), self.value);
        let press_focus = focus.clone();
        div()
            .id(self.id.clone())
            .track_focus(&focus)
            .relative()
            .flex_1()
            .min_w_0()
            .h(theme.rem(1.5))
            .cursor_pointer()
            .child(canvas(move |b, _, _| store.set(b), |_, _, _, _| {}).absolute().size_full())
            .on_mouse_down(gpui_kit::MouseButton::Left, move |event, window, cx| {
                press_focus.focus(window, cx);
                let next = value_at(event.position.x, press_bounds.get(), thumb, range);
                if next != value {
                    press(next, window, cx);
                }
            })
            .on_drag(SliderDrag(self.id.clone()), |_, _, _, cx| cx.new(|_| Empty))
            .on_drag_move::<SliderDrag>(move |event, window, cx| {
                if event.drag(cx).0 == own_id {
                    drag_move(value_at(event.event.position.x, event.bounds, thumb, range), window, cx);
                }
            })
            .on_key_down(move |event, window, cx| {
                let delta: i64 = match event.keystroke.key.as_str() {
                    "left" | "down" => -(range.2 as i64),
                    "right" | "up" => range.2 as i64,
                    _ => return,
                };
                let next = (value as i64 + delta).clamp(range.0 as i64, range.1 as i64) as u64;
                cx.stop_propagation();
                if next != value {
                    keys(next, window, cx);
                }
            })
            // The 6px track, centred in the 24px row.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(theme.rem(0.5625))
                    .h(theme.rem(0.375))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(with_alpha(c.muted_foreground, 0.22))
                    .child(
                        // Filled to the thumb's centre: progress + (0.5 - progress) rem.
                        div().absolute().left_0().top_0().bottom_0().right(relative(1. - ratio)).mr(theme.rem(ratio - 0.5)).bg(primary),
                    )
                    .child(div().absolute().size_full().rounded_full().border_1().border_color(with_alpha(c.border, 0.55))),
            )
            .child(
                div()
                    .absolute()
                    .top(theme.rem(0.25))
                    .left(relative(ratio))
                    .ml(-(thumb * ratio))
                    .size(thumb)
                    .rounded_full()
                    .border_2()
                    .border_color(primary)
                    .bg(c.background())
                    .when(focused, |d| d.shadow(vec![crate::components::ui::input_focus_ring(theme.derived.input_focus_ring)])),
            )
    }
}

/// The `<output>` beside a slider: `rounded-md bg-muted px-2 py-1
/// text-center font-mono text-xs font-medium tabular-nums`, at least
/// `min_width_rem` wide.
pub fn slider_value(text: impl Into<SharedString>, min_width_rem: f32, cx: &App) -> impl IntoElement {
    let theme = cx.app_theme();
    let c = &theme.colors;
    let rem = theme.fonts.interface;
    div()
        .flex_none()
        .min_w(theme.rem(min_width_rem))
        .flex()
        .justify_center()
        .rounded(theme.radii.md)
        .bg(c.muted)
        .px(theme.sp(2.))
        .py(theme.sp(1.))
        .mono_text(px(TextSize::Xs.size(rem)), weight::MEDIUM)
        .line_height(px(TextSize::Xs.line_height(rem)))
        .text_color(c.foreground())
        .child(text.into())
}
