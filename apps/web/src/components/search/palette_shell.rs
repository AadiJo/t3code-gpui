//! The command-palette frame go to file and content search sit in. Ports the
//! parts of `CommandPaletteContent.tsx` and `ui/command.tsx` they use: a
//! `dialog-glass` popup up to 36rem wide, a borderless search field with a
//! leading search icon, the list panel, and the footer of key hints.

use gpui_kit::{
    AnyElement, Div, Entity, IntoElement, ParentElement as _, Pixels, SharedString, Styled as _,
    component::input::{Input, InputState},
    div, prelude::*, px,
};

use crate::{
    ActiveAppTheme as _, AppTheme,
    components::{
        icons::{Icon, IconName, IconSize},
        ui::{dialog::dialog_glass, kbd::Kbd},
    },
    i18n::t,
};

/// `max-w-xl`.
pub const PALETTE_WIDTH: f32 = 576.;

/// The popup frame; `height` fixes it (`h-105` for content search).
pub fn palette_frame(theme: &AppTheme, width: Pixels, height: Option<Pixels>) -> Div {
    dialog_glass(theme)
        .w(width)
        .max_h(px(420.).max(height.unwrap_or(px(0.))))
        .when_some(height, |el, height| el.h(height))
        .overflow_hidden()
}

/// `CommandInput`: `size="lg"` field with a muted search icon, no border.
pub fn palette_input(theme: &AppTheme, input: &Entity<InputState>, accessory: Option<AnyElement>) -> Div {
    div().px(px(8.)).py(px(6.)).flex_none().child(
        div()
            .relative()
            .h(px(36.))
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(10.))
            .child(Icon::new(IconName::Search).size(IconSize::Md).color(theme.colors.icon_muted))
            .child(div().flex_1().min_w_0().text_size(px(14.)).child(Input::new(input).appearance(false).w_full()))
            .children(accessory),
    )
}

/// `CommandFooter` with Navigate, the action and the escape hint.
pub fn palette_footer(theme: &AppTheme, action: SharedString, escape: SharedString) -> Div {
    let c = &theme.colors;
    let group = |keys: Vec<AnyElement>, label: SharedString| {
        div().flex().items_center().gap(px(6.)).children(keys).child(label)
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(16.))
        .py(px(10.))
        .border_t_1()
        .border_color(c.foreground().opacity(0.07))
        .bg(c.foreground().opacity(0.025))
        .rounded_b(theme.radii.xxl - px(1.))
        .text_size(px(13.))
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .text_color(c.muted_foreground)
        .child(group(vec![Kbd::new("↑").into_any_element(), Kbd::new("↓").into_any_element()], t!("Navigate")))
        .child(group(vec![Kbd::new("Enter").into_any_element()], action))
        .child(group(vec![Kbd::new("Esc").into_any_element()], escape))
}

/// Draws `value` with the characters at `indices` in the foreground and bold,
/// as `HighlightedFuzzyText` does.
pub fn highlighted_text(value: &SharedString, indices: &[usize], strong: gpui_kit::Hsla) -> gpui_kit::StyledText {
    let highlights = indices
        .iter()
        .filter_map(|&i| {
            let end = value[i..].chars().next().map(|c| i + c.len_utf8())?;
            Some((i..end, gpui_kit::HighlightStyle { color: Some(strong), font_weight: Some(gpui_kit::FontWeight::SEMIBOLD), ..Default::default() }))
        })
        .collect::<Vec<_>>();
    gpui_kit::StyledText::new(value.clone()).with_highlights(highlights)
}
