//! The command-palette frame go to file and content search sit in. Ports the
//! parts of `CommandPaletteContent.tsx` and `ui/command.tsx` they use: a
//! `dialog-glass` popup up to 36rem wide, a borderless search field with a
//! leading search icon, the list panel, and the footer of key hints.

use gpui_kit::{rems, 
    AnyElement, App, Div, Entity, IntoElement, MouseButton, ParentElement as _, Pixels, SharedString, Styled as _,
    Window, anchored, component::input::{Input, InputState},
    deferred, div, point, prelude::*, px,
};

use crate::{
    AppTheme,
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
        .w_full()
        .max_w(width)
        .max_h(theme.rem(26.25).max(height.unwrap_or(px(0.))))
        .when_some(height, |el, height| el.h(height))
        .overflow_hidden()
}

/// `CommandInput`: `size="lg"` field with a muted search icon, no border.
pub fn palette_input(theme: &AppTheme, input: &Entity<InputState>, accessory: Option<AnyElement>) -> Div {
    div().px(rems(0.5)).py(rems(0.375)).flex_none().child(
        div()
            .relative()
            .h(rems(2.25))
            .flex()
            .items_center()
            .gap(rems(0.5))
            .pl(rems(0.75))
            .pr(rems(0.625))
            .child(Icon::new(IconName::Search).size(IconSize::Md).color(theme.colors.icon_muted))
            .child(div().flex_1().min_w_0().child(Input::new(input).appearance(false).bordered(false).focus_bordered(false).p_0().h(px(20.)).text_size(px(14.)).w_full()))
            .children(accessory),
    )
}

/// `CommandFooter` with Navigate, the action and the escape hint.
pub fn palette_footer(theme: &AppTheme, action: SharedString, escape: SharedString) -> Div {
    let c = &theme.colors;
    let group = |keys: Vec<AnyElement>, label: SharedString| {
        div().flex().items_center().gap(rems(0.375)).children(keys).child(label)
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(rems(0.75))
        .px(rems(1.))
        .py(rems(0.625))
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

/// `CommandDialogViewport` and its backdrop: the palette centred across,
/// `max(16px, 10vh)` from the top, 16px from the sides. A press on the
/// backdrop runs `on_close`; the palette handles Escape itself. Unlike the
/// shared `Modal`, this leaves focus where the palette put it (its field).
pub fn palette_overlay(content: AnyElement, on_close: impl Fn(&mut Window, &mut App) + 'static, window: &Window, theme: &AppTheme) -> impl IntoElement {
    let viewport = window.viewport_size();
    deferred(
        anchored().position(point(px(0.), px(0.))).child(
            div()
                .id("palette-overlay")
                .occlude()
                .relative()
                .w(viewport.width)
                .h(viewport.height)
                .flex()
                .flex_col()
                .items_center()
                .px(rems(1.))
                .pt((viewport.height * 0.1).max(px(16.)))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(theme.derived.dialog_backdrop)
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| on_close(window, cx)),
                )
                .child(div().relative().w_full().flex().justify_center().child(content)),
        ),
    )
    .with_priority(2)
}
