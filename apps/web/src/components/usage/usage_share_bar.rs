//! A labelled bar split by share, with a legend. Ports `UsageShareBar.tsx`.

use gpui_kit::{AnyElement, App, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px, relative};

use super::usage_breakdown::ShareSegment;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::tooltip::tooltip;
use crate::i18n::number_format::format_percent;

/// "Cost by type" and friends. Renders nothing when every segment is zero.
#[derive(IntoElement)]
pub struct UsageShareBar {
    id: SharedString,
    label: SharedString,
    segments: Vec<ShareSegment>,
    format: Box<dyn Fn(f64) -> String>,
    aside: Option<AnyElement>,
}

impl UsageShareBar {
    pub fn new(id: impl Into<SharedString>, label: SharedString, segments: Vec<ShareSegment>, format: impl Fn(f64) -> String + 'static) -> Self {
        Self { id: id.into(), label, segments, format: Box::new(format), aside: None }
    }

    /// Content on the right of the title, such as the speed premium.
    pub fn aside(mut self, aside: impl IntoElement) -> Self {
        self.aside = Some(aside.into_any_element());
        self
    }
}

impl RenderOnce for UsageShareBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let visible: Vec<ShareSegment> = self.segments.into_iter().filter(|s| s.value > 0.0).collect();
        let total: f64 = visible.iter().map(|s| s.value).sum();
        if total <= 0.0 {
            return div().into_any_element();
        }
        let last = visible.len() - 1;
        let bar = div().flex().h(theme.sp(2.)).gap(theme.sp(0.5)).children(visible.iter().enumerate().map(|(index, segment)| {
            let text = format!("{} · {} · {}", segment.label, (self.format)(segment.value), format_percent(segment.value / total, 1));
            let (full, small) = (theme.sp(2.), px(2.));
            div()
                .id((self.id.clone(), index))
                .h_full()
                .min_w(px(4.))
                .flex_basis(relative((segment.value / total) as f32))
                .flex_shrink(1.)
                .bg(segment.color)
                .rounded_tl(if index == 0 { full } else { small })
                .rounded_bl(if index == 0 { full } else { small })
                .rounded_tr(if index == last { full } else { small })
                .rounded_br(if index == last { full } else { small })
                .tooltip(tooltip(SharedString::from(text)))
        }));
        let legend = div().flex().flex_wrap().gap_x(theme.sp(4.)).gap_y(theme.sp(1.)).ui_text(theme, TextSize::Xs, weight::NORMAL).children(
            visible.iter().map(|segment| {
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(1.5))
                    .child(div().size(theme.sp(2.)).rounded(px(2.)).bg(segment.color))
                    .child(div().text_color(c.muted_foreground).child(segment.label.clone()))
                    .child(div().text_color(c.foreground()).child((self.format)(segment.value)))
            }),
        );
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap(theme.sp(2.5))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .gap(theme.sp(3.))
                    .child(div().ui_text(theme, TextSize::Sm, weight::MEDIUM).text_color(c.foreground()).child(self.label))
                    .children(self.aside),
            )
            .child(bar)
            .child(legend)
            .into_any_element()
    }
}
