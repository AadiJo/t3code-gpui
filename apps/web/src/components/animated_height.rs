//! Ports `AnimatedHeight.tsx`: a box whose height follows its content's,
//! easing over 200ms (`ease-out`) and clipping the content while it moves.
//! Instant under reduced motion (`motion-reduce:transition-none`).

use gpui_kit::{AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, Styled, Window, canvas, div, prelude::*, px};

use crate::components::ui::motion::{SizeTween, Transition, prefers_reduced_motion};

/// Wraps content whose height changes. `hold_height` freezes the box at its
/// last height (the content fills it) while the caller swaps content.
#[derive(IntoElement)]
pub struct AnimatedHeight {
    id: ElementId,
    content: AnyElement,
    hold_height: bool,
}

impl AnimatedHeight {
    pub fn new(id: impl Into<ElementId>, content: impl IntoElement) -> Self {
        Self { id: id.into(), content: content.into_any_element(), hold_height: false }
    }

    pub fn hold_height(mut self, hold: bool) -> Self {
        self.hold_height = hold;
        self
    }
}

impl RenderOnce for AnimatedHeight {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SizeTween::default());
        let now = cx.background_executor().now();
        let current = state.update(cx, |tween, _| tween.current(Transition::HEIGHT, window, now));
        let hold = self.hold_height;
        let measure = state.clone();
        let animate = !prefers_reduced_motion(cx);
        let probe = canvas(
            move |bounds, window, cx| {
                let now = cx.background_executor().now();
                if hold {
                    return;
                }
                let height = f32::from(bounds.size.height);
                if measure.update(cx, |tween, _| tween.measure(height, animate, Transition::HEIGHT, now)) {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let content = div().relative().flex_none().when(hold, |c| c.h_full()).child(self.content).child(probe);
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .when_some(current, |d, (height, clipping)| {
                // `[overflow-clip-margin:2px]`: focus rings may reach 2px past the box.
                d.h(px(height)).when(clipping || hold, |d| d.overflow_hidden())
            })
            .child(content)
    }
}
