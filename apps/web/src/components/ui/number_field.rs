//! Number field. Ports `number-field.tsx` at desktop width: one bordered
//! group with a "−" button, the centred value and a "+" button.

use std::rc::Rc;

use gpui_kit::{
    App, ElementId, Entity, Focusable as _, IntoElement, RenderOnce, SharedString, Styled, TextAlign, Window,
    component::input::{Input, InputState},
    div, prelude::*, px,
};

use super::{ControlHighlightExt as _, DISABLED_OPACITY, TextStyleExt as _, input_focus_ring};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};

/// `NumberField` with `size="sm"`: `−`, the value, `+` in one `border-input`
/// group. Typing commits through the field's own draft binding; the buttons
/// call `on_step` with the next value, clamped to `min..=max`.
#[derive(IntoElement)]
pub struct NumberField {
    id: ElementId,
    field: Entity<InputState>,
    value: u64,
    min: u64,
    max: u64,
    disabled: bool,
    on_step: Option<Rc<dyn Fn(u64, &mut Window, &mut App)>>,
}

impl NumberField {
    /// `field` holds the typed text; `value` is the committed number.
    pub fn new(id: impl Into<ElementId>, field: &Entity<InputState>, value: u64, min: u64, max: u64) -> Self {
        Self { id: id.into(), field: field.clone(), value, min, max, disabled: false, on_step: None }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_step(mut self, handler: impl Fn(u64, &mut Window, &mut App) + 'static) -> Self {
        self.on_step = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for NumberField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let focused = self.field.read(cx).focus_handle(cx).is_focused(window);
        let inner_radius = theme.radii.lg - px(1.);
        let pad = sp(2.5) - px(1.);
        let step_button = |name: &str, icon: IconName, next: Option<u64>, left: bool| {
            let on_step = self.on_step.clone();
            let enabled = !self.disabled && next.is_some();
            div()
                .id(SharedString::from(format!("{}-{name}", self.id)))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .px(pad)
                .when(left, |d| d.rounded_l(inner_radius))
                .when(!left, |d| d.rounded_r(inner_radius))
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(c.accent)))
                .child(Icon::new(icon).size(IconSize::Custom(sp(4.))).color(c.foreground()))
                .when_some(next.filter(|_| enabled).zip(on_step), |d, (next, on_step)| {
                    d.on_click(move |_, window, cx| on_step(next, window, cx))
                })
        };
        let down = self.value.checked_sub(1).filter(|n| *n >= self.min);
        let up = Some(self.value + 1).filter(|n| *n <= self.max);
        let input = Input::new(&self.field).appearance(false).disabled(self.disabled);
        let input = Styled::h(input, sp(6.5)).flex_1().min_w_0().px(pad).text_align(TextAlign::Center).ui_text(theme, TextSize::Sm, weight::NORMAL);
        div()
            .id(self.id.clone())
            .flex()
            .w_full()
            .justify_between()
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(if focused { c.ring() } else { c.input })
            .bg(theme.derived.input_fill)
            .text_color(c.foreground())
            .when(focused, |d| d.shadow(vec![input_focus_ring(theme.derived.input_focus_ring)]))
            .when(!focused && !self.disabled, |d| d.shadow(theme.shadows.xs5.clone()).control_highlight(theme))
            .child(step_button("dec", IconName::Minus, down, true))
            .child(input)
            .child(step_button("inc", IconName::Plus, up, false))
            .when(self.disabled, |d| d.opacity(DISABLED_OPACITY))
    }
}
