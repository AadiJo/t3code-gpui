//! Ports `AnimatedWidth.tsx`: an inline box whose width follows its
//! one-line content's, easing over 150ms (`ease-out`), clipped.
//! Instant under reduced motion (`motion-reduce:transition-none`).

use gpui_kit::{AnyElement, App, ElementId, IntoElement, ParentElement, RenderOnce, Styled, Window, canvas, div, prelude::*, px};

use crate::components::ui::motion::{SizeTween, Transition, prefers_reduced_motion};

/// Wraps one line of content whose width changes (a label that swaps text).
#[derive(IntoElement)]
pub struct AnimatedWidth {
    id: ElementId,
    content: AnyElement,
}

impl AnimatedWidth {
    pub fn new(id: impl Into<ElementId>, content: impl IntoElement) -> Self {
        Self { id: id.into(), content: content.into_any_element() }
    }
}

impl RenderOnce for AnimatedWidth {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SizeTween::default());
        let now = cx.background_executor().now();
        let current = state.update(cx, |tween, _| tween.current(Transition::WIDTH, window, now));
        let animate = !prefers_reduced_motion(cx);
        let probe = canvas(
            move |bounds, window, cx| {
                let now = cx.background_executor().now();
                let width = f32::from(bounds.size.width);
                if width > 0. && state.update(cx, |tween, _| tween.measure(width, animate, Transition::WIDTH, now)) {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .overflow_hidden()
            .when_some(current, |d, (width, _)| d.w(px(width)))
            .child(div().relative().flex_none().whitespace_nowrap().child(self.content).child(probe))
    }
}
