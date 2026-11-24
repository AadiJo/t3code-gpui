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
                    .h(rems(2.5))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(rems(0.5))
                    .px(rems(1.))
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
                .h(rems(2.))
                .flex()
                .items_center()
                .gap(rems(0.5))
                .px(rems(0.5))
                .pr(rems(0.75))
                .child(div().size(rems(1.25)).flex().items_center().justify_center().child(div().size(rems(0.625)).rounded(rems(0.125)).bg(bone)))
                .child(div().size(rems(1.25)).rounded(rems(0.25)).bg(bone))
                .child(pill(title, 12.))
                .child(div().ml_auto().flex().gap(rems(0.5)).child(pill(20., 12.)).child(pill(20., 12.)))
        };
        let line = |w: f32| div().flex().items_center().gap(rems(0.75)).child(pill(20., 10.)).child(pill(w, 10.));
        div()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(colors.canvas)
            .child(header(220.))
            .child(
                div()
                    .h(rems(1.5))
                    .flex()
                    .items_center()
                    .gap(rems(0.5))
                    .px(rems(0.5))
                    .child(div().h(px(1.)).flex_1().bg(colors.border.opacity(0.4)))
                    .child(pill(96., 10.))
                    .child(div().h(px(1.)).flex_1().bg(colors.border.opacity(0.4))),
            )
            .child(div().px(rems(0.75)).py(rems(0.5)).flex().flex_col().gap(rems(0.5)).child(line(240.)).child(line(300.)).child(line(180.)))
            .child(header(160.))
            .child(header(260.))
            .child(div().absolute().invisible().child(self.label))
    }
}
