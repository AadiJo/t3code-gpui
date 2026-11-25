//! The agent's pointer over a streamed page. Ports `AgentCursorMark` from
//! `AgentBrowserCursor.tsx`: a primary `MousePointer2` filled with the background at
//! the point the server's `pointer` message names, a click burst, and a fade to 35%
//! 700ms after the last move or click.
//!
//! Every frame here is finite: the fade is one 150ms transition and a click is one
//! eight-step burst over 800ms, started by the pointer event that caused it.

use gpui_kit::{App, IntoElement, Pixels, RenderOnce, Window, div, prelude::*, px, svg};

use crate::ActiveAppTheme as _;
use crate::components::ui::motion::Transition;
use crate::theme_color::with_alpha;
use crate::theme_tokens::CubicBezier;

/// `transition-[opacity,transform] duration-150 ease-out`.
const FADE: Transition = Transition::new(150, CubicBezier(0., 0., 0.58, 1.));
/// `status-ping`: the burst is the first 40% of its 2s cycle, in eight steps.
const BURST: Transition = Transition::new(800, CubicBezier(0., 0., 1., 1.));

/// `agentBrowserCursorOpacity` when inactive: a human driving (0.18) or not (0.35).
pub fn inactive_opacity(human_controls: bool) -> f32 {
    if human_controls { 0.18 } else { 0.35 }
}

/// The pointer at `left`/`top` in the surface's pixels.
#[derive(IntoElement)]
pub struct AgentCursorMark {
    pub click: bool,
    pub sequence: u64,
    pub left: Pixels,
    pub top: Pixels,
    /// False once 700ms passed since `sequence` arrived.
    pub active: bool,
    pub human_controls: bool,
}

impl RenderOnce for AgentCursorMark {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let size = theme.sp(5.);
        let low = inactive_opacity(self.human_controls);
        // `size-5 -translate-x-0.5 -translate-y-0.5`, filled with the background, primary stroke.
        let mark = div()
            .absolute()
            .left(-theme.sp(0.5))
            .top(-theme.sp(0.5))
            .size(size)
            .child(svg().absolute().inset_0().size(size).path("icons/ui/agent-cursor-fill.svg").text_color(c.background()))
            .child(svg().absolute().inset_0().size(size).path("icons/ui/agent-cursor-stroke.svg").text_color(c.primary()));
        let burst = self.click.then(|| {
            let (center, base) = (theme.sp(0.5) + theme.sp(2.), theme.sp(4.));
            let ring = div().absolute().rounded_full().bg(with_alpha(c.primary(), 0.25));
            BURST.run(("agent-cursor-burst", self.sequence as usize), ring, move |el, t| {
                let step = (t * 8.).floor().min(8.) / 8.;
                let scale = 0.75 + 1.25 * step;
                let half = base * scale / 2.;
                el.left(center - half).top(center - half).size(half * 2.).opacity(0.9 * (1. - step))
            })
        });
        let root = div().absolute().left(self.left).top(self.top).size(px(0.)).children(burst).child(mark);
        if self.active {
            root.into_any_element()
        } else {
            FADE.run(("agent-cursor-fade", self.sequence as usize), root, move |el, t| el.opacity(1. - (1. - low) * t))
        }
    }
}
