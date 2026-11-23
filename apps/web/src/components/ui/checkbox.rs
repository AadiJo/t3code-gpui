//! Checkbox. Ports `checkbox.tsx` at desktop width (16px).

use gpui_kit::{App, BoxShadow, ClickEvent, ElementId, IntoElement, RenderOnce, Window, div, prelude::*, px, svg};

use super::{ControlState, DISABLED_OPACITY, control_highlight};
use crate::ActiveAppTheme as _;
use crate::theme_color::with_alpha;

/// Checked, unchecked, or the indeterminate dash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckState {
    #[default]
    Unchecked,
    Checked,
    Indeterminate,
}

/// A checkbox. `contrast` uses a `foreground/55` border.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: CheckState,
    contrast: bool,
    disabled: bool,
    state: Option<ControlState>,
    on_change: Option<Box<dyn Fn(bool, &mut Window, &mut App) + 'static>>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: CheckState) -> Self {
        Self { id: id.into(), checked, contrast: false, disabled: false, state: None, on_change: None }
    }

    pub fn contrast(mut self) -> Self {
        self.contrast = true;
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

    /// Called with the new checked value.
    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let size = theme.sp(4.);
        let radius = px(0.25 * theme.fonts.interface);
        let disabled = self.disabled || self.state == Some(ControlState::Disabled);
        let state = self.state.unwrap_or(if disabled { ControlState::Disabled } else { ControlState::Idle });
        let live = self.state.is_none() && !disabled;
        let focused = state == ControlState::FocusVisible;
        let is_checked = self.checked != CheckState::Unchecked;
        let border = if self.contrast { with_alpha(c.foreground(), 0.55) } else { c.input };
        let ring = BoxShadow::new(px(0.), px(0.), c.ring()).spread_radius(px(2.)).inset();
        let gap = BoxShadow::new(px(0.), px(0.), c.background()).spread_radius(px(3.)).inset();
        let indicator = (self.checked != CheckState::Unchecked).then(|| {
            let glyph = match self.checked {
                CheckState::Indeterminate => "icons/ui/checkbox-indeterminate.svg",
                _ => "icons/ui/checkbox-check.svg",
            };
            div()
                .absolute()
                .top(px(-1.))
                .left(px(-1.))
                .size(size)
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius)
                .when(self.checked == CheckState::Checked, |i| i.bg(c.primary()))
                .when(focused, |i| i.shadow(vec![ring.clone(), gap]))
                .child(svg().path(glyph).size(theme.sp(3.)).text_color(match self.checked {
                    CheckState::Indeterminate => c.foreground(),
                    _ => c.primary_foreground(),
                }))
        });
        let current = self.checked;
        div()
            .id(self.id)
            .relative()
            .flex_none()
            .size(size)
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(if is_checked && theme.is_dark() { c.background() } else { theme.derived.input_fill })
            .when(!is_checked && !disabled, |b| {
                b.shadow(theme.shadows.xs5.clone()).child(control_highlight(theme, radius))
            })
            .when(focused && !is_checked, |b| b.shadow(vec![ring]))
            .children(indicator)
            .when(disabled, |b| b.opacity(DISABLED_OPACITY))
            .when_some(self.on_change.filter(|_| live), |b, handler| {
                b.cursor_pointer()
                    .on_click(move |_: &ClickEvent, window, cx| handler(current != CheckState::Checked, window, cx))
            })
    }
}
