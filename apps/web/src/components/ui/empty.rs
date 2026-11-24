//! Empty state. Ports `empty.tsx`: an optional icon tile, a title, a muted
//! description and actions, centred in a column.

use gpui_kit::{AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use super::{TextStyleExt as _, ControlHighlightExt as _};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};

/// `emptySizeClassName`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmptySize {
    Compact,
    #[default]
    Default,
    Hero,
}

/// A centred empty state. Children are the actions under the text.
#[derive(IntoElement)]
pub struct Empty {
    size: EmptySize,
    icon: Option<IconName>,
    media: Option<AnyElement>,
    title: SharedString,
    description: Option<SharedString>,
    actions: Vec<AnyElement>,
}

impl Empty {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { size: EmptySize::Default, icon: None, media: None, title: title.into(), description: None, actions: Vec::new() }
    }

    pub fn size(mut self, size: EmptySize) -> Self {
        self.size = size;
        self
    }

    /// Shows the icon in a 36px card tile (`EmptyMedia variant="icon"`).
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// An illustration above the header, as its own row of the column (a direct `Empty` child).
    pub fn media(mut self, media: impl IntoElement) -> Self {
        self.media = Some(media.into_any_element());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl ParentElement for Empty {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.actions.extend(elements);
    }
}

impl RenderOnce for Empty {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let (gap, padding) = match self.size {
            EmptySize::Compact => (sp(4.), sp(10.)),
            _ => (sp(6.), sp(12.)),
        };
        let title = div().ui_text(theme, TextSize::Xl, weight::SEMIBOLD).text_color(c.foreground());
        let title = match self.size {
            EmptySize::Compact => title.text_size(px(17.)).line_height(sp(6.)),
            EmptySize::Default => title,
            EmptySize::Hero => title.ui_text(theme, TextSize::Xl3, weight::SEMIBOLD),
        };
        let description = |text: SharedString| {
            let line = div().mt(sp(1.)).ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground);
            let line = match self.size {
                EmptySize::Compact => line.text_size(px(13.)).line_height(px(18.)),
                _ => line,
            };
            line.child(text)
        };
        let media = self.icon.map(|icon| {
            div()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .size(sp(9.))
                .when(self.size != EmptySize::Compact, |m| m.mb(sp(6.)))
                .rounded(theme.radii.md)
                .border_1()
                .border_color(c.border)
                .bg(c.card())
                .shadow(theme.shadows.sm5.clone())
                .control_highlight(theme)
                .child(Icon::new(icon).size(IconSize::Lg).color(c.foreground()))
        });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_center()
            .gap(gap)
            .p(padding)
            .when(self.size == EmptySize::Compact, |e| e.min_h(sp(64.)))
            .children(self.media)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .max_w(sp(96.))
                    .children(media)
                    .child(title.child(self.title))
                    .children(self.description.map(description)),
            )
            .when(!self.actions.is_empty(), |e| {
                e.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(sp(4.))
                        .w_full()
                        .max_w(sp(96.))
                        .children(self.actions),
                )
            })
    }
}
