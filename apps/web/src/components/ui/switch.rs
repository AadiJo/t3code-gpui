//! On/off switch. Ports `switch.tsx` at desktop width.

use gpui_kit::{TestSupportExt as _, App, ClickEvent, ElementId, IntoElement, RenderOnce, Window, div, prelude::*, px};

use super::{ControlState, DISABLED_OPACITY, focus_ring_inset};
use crate::ActiveAppTheme as _;

/// `default` has a 16px thumb, `sm` 14px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwitchSize {
    #[default]
    Default,
    Sm,
}

/// A switch. The thumb travels `thumb - 4px`; pressing stretches it 10%.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    checked: bool,
    size: SwitchSize,
    disabled: bool,
    state: Option<ControlState>,
    on_change: Option<Box<dyn Fn(bool, &mut Window, &mut App) + 'static>>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self { id: id.into(), checked, size: SwitchSize::Default, disabled: false, state: None, on_change: None }
    }

    pub fn size(mut self, size: SwitchSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn state(mut self, state: ControlState) -> Self {
        self.state = Some(state);
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let thumb = match self.size {
            SwitchSize::Default => theme.sp(4.),
            SwitchSize::Sm => theme.sp(3.5),
        };
        let disabled = self.disabled || self.state == Some(ControlState::Disabled);
        let state = self.state.unwrap_or(if disabled { ControlState::Disabled } else { ControlState::Idle });
        let live = self.state.is_none() && !disabled;
        let pressed = state == ControlState::Active;
        let knob = thumb - px(2.);
        let knob_width = if pressed { knob * 1.1 } else { knob };
        let travel = thumb - px(4.);
        let offset = if self.checked { travel - (knob_width - knob) } else { px(0.) };
        let thumb_color = if theme.is_dark() && !self.checked { c.muted_foreground } else { c.background() };
        let checked = self.checked;
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .w(thumb * 2. - px(2.))
            .h(thumb + px(2.))
            .p(px(2.))
            .rounded(theme.radii.full)
            .bg(if checked { c.primary() } else { c.input })
            .when(state == ControlState::FocusVisible, |s| s.shadow(focus_ring_inset(theme, px(0.))))
            .child(
                div()
                    .ml(offset)
                    .w(knob_width)
                    .h(knob)
                    .rounded(theme.radii.full)
                    .bg(thumb_color)
                    .shadow(theme.shadows.sm5.clone()),
            )
            .when(disabled, |s| s.opacity(DISABLED_OPACITY))
            .when_some(self.on_change.filter(|_| live), |s, handler| {
                s.cursor_pointer().on_click(move |_: &ClickEvent, window, cx| handler(!checked, window, cx))
            })
            .test_support()
    }
}
