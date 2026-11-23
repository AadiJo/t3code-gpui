//! What the main pane shows with no thread open. Ports `NoActiveThreadState.tsx`
//! (desktop variant): a quiet header row and a centred empty state.

use gpui_kit::*;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::empty::Empty;
use crate::i18n::t;

/// The no-thread pane. `topbar` is the window's topbar height and
/// `leading_inset` clears the floating sidebar toggle.
#[derive(IntoElement)]
pub struct NoActiveThreadState {
    pub topbar: Pixels,
    pub leading_inset: Pixels,
}

impl RenderOnce for NoActiveThreadState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.canvas)
            .child(
                div()
                    .flex_none()
                    .h(self.topbar)
                    .flex()
                    .items_center()
                    .px_5()
                    .pl(px(20.) + self.leading_inset)
                    .border_b_1()
                    .border_color(c.border)
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(c.muted_foreground.opacity(0.5))
                    .child(t!("No active thread")),
            )
            .child(
                div().flex_1().flex().items_center().justify_center().child(
                    div().w_full().max_w(px(512.)).px_8().py_12().child(
                        Empty::new(t!("Pick a thread to continue"))
                            .description(t!("Select an existing thread or create a new one to get started.")),
                    ),
                ),
            )
    }
}
