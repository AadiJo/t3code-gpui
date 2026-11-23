//! The diff panel frame: a 40px header row over the body, and the loading
//! skeleton. Ports `DiffPanelShell.tsx` for the non-drag-region modes.

use gpui_kit::*;

use crate::ActiveAppTheme as _;

/// Header row and body of the diff panel.
#[derive(IntoElement)]
pub struct DiffPanelShell {
    header: AnyElement,
    body: AnyElement,
}

impl DiffPanelShell {
    pub fn new(header: impl IntoElement, body: impl IntoElement) -> Self {
        Self { header: header.into_any_element(), body: body.into_any_element() }
    }
}

impl RenderOnce for DiffPanelShell {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(colors.canvas)
            .text_color(colors.text)
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.6))
                    .child(self.header),
            )
            .child(div().flex_1().min_h_0().flex().flex_col().child(self.body))
    }
}

/// Placeholder rows while a diff loads (`DiffPanelLoadingState`). Static: no shimmer.
#[derive(IntoElement)]
pub struct DiffPanelLoadingState {
    label: SharedString,
}

impl DiffPanelLoadingState {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into() }
    }
}

impl RenderOnce for DiffPanelLoadingState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let bone = colors.muted;
        let pill = |w: f32, h: f32| div().w(px(w)).h(px(h)).rounded_full().bg(bone);
        let header = |title: f32| {
            div()
                .h(px(32.))
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .pr(px(12.))
                .child(div().size(px(20.)).flex().items_center().justify_center().child(div().size(px(10.)).rounded(px(2.)).bg(bone)))
                .child(div().size(px(20.)).rounded(px(4.)).bg(bone))
                .child(pill(title, 12.))
                .child(div().ml_auto().flex().gap(px(8.)).child(pill(20., 12.)).child(pill(20., 12.)))
        };
        let line = |w: f32| div().flex().items_center().gap(px(12.)).child(pill(20., 10.)).child(pill(w, 10.));
        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(colors.canvas)
            .child(header(220.))
            .child(
                div()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .child(div().h(px(1.)).flex_1().bg(colors.border.opacity(0.4)))
                    .child(pill(96., 10.))
                    .child(div().h(px(1.)).flex_1().bg(colors.border.opacity(0.4))),
            )
            .child(div().px(px(12.)).py(px(8.)).flex().flex_col().gap(px(8.)).child(line(240.)).child(line(300.)).child(line(180.)))
            .child(header(160.))
            .child(header(260.))
            .child(div().absolute().invisible().child(self.label))
    }
}
