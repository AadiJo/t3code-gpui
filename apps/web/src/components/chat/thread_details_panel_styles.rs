//! The row styles every control in the thread details card shares. Ports
//! `threadDetailsPanelStyles.ts` and `ThreadDetailsControl.tsx`: 32px rows with
//! a 16px muted icon, 13px medium label at 80%, a faint hover fill, and split
//! rows whose hairline shows only while the pair is in use.

use gpui_kit::{
    AnyElement, Div, ElementId, Hsla, IntoElement, ParentElement as _, Pixels, SharedString, Stateful, Styled as _, div,
    prelude::*, px,
};

use crate::AppTheme;
use crate::appearance_fonts::{ui_font, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::DISABLED_OPACITY;
use crate::theme_color::with_alpha;

/// Row height (`h-8`).
pub const ROW_HEIGHT: Pixels = px(32.);
/// Label size: `text-[13px]`, a pixel literal that does not scale.
pub const ROW_TEXT: Pixels = px(13.);
/// Label line height: the cap-trimmed label sits in an 18px box.
pub const ROW_LINE_HEIGHT: Pixels = px(18.);
/// The secondary half of a split row (`w-8`).
pub const SPLIT_SECONDARY_WIDTH: Pixels = px(32.);

/// `hover:bg-black/[0.035] dark:hover:bg-white/[0.08]`.
pub fn row_hover(theme: &AppTheme) -> Hsla {
    if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.08) } else { gpui_kit::hsla(0., 0., 0., 0.035) }
}

/// The running action's resting fill: `bg-black/[0.035] dark:bg-white/[0.055]`.
pub fn row_busy_fill(theme: &AppTheme) -> Hsla {
    if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.055) } else { gpui_kit::hsla(0., 0., 0., 0.035) }
}

/// The row text colour, `text-foreground/80`.
pub fn row_text(theme: &AppTheme) -> Hsla {
    with_alpha(theme.colors.foreground(), 0.8)
}

/// The 13px medium label font.
pub fn row_font(el: Div) -> Div {
    el.font(ui_font(weight::MEDIUM)).text_size(ROW_TEXT).line_height(ROW_LINE_HEIGHT)
}

/// `THREAD_DETAILS_PANEL_ICON_CLASS`: 16px, muted.
pub fn row_icon(theme: &AppTheme, name: IconName) -> Icon {
    Icon::new(name).size(IconSize::Md).color(theme.colors.muted_foreground)
}

/// The base of every row control: 32px tall, 8px radius, 10px gaps and padding.
/// `open` keeps the hover fill while its popup is open.
pub fn row_base(id: impl Into<ElementId>, theme: &AppTheme, disabled: bool, open: bool) -> Stateful<Div> {
    let hover = row_hover(theme);
    row_font(div())
        .id(id)
        .relative()
        .flex()
        .items_center()
        .min_w_0()
        .h(ROW_HEIGHT)
        .gap(px(10.))
        .px(px(10.))
        .rounded(theme.radii.lg)
        .text_color(row_text(theme))
        .when(open, |r| r.bg(hover))
        .when(disabled, |r| r.opacity(DISABLED_OPACITY))
        .when(!disabled, |r| r.cursor_pointer().hover(move |s| s.bg(hover)))
}

/// A full-width row: icon, then a truncating label.
pub fn row(id: impl Into<ElementId>, theme: &AppTheme, icon: Option<Icon>, label: impl Into<SharedString>, disabled: bool) -> Stateful<Div> {
    row_base(id, theme, disabled, false)
        .w_full()
        .children(icon)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
}

/// A split row: the primary half grows, the 32px secondary half holds a
/// chevron, and a 16px hairline between them shows while the pair is hovered
/// or its popup is open (`group/details-split`).
pub fn split_row(
    group: &'static str,
    theme: &AppTheme,
    primary: Stateful<Div>,
    secondary: Stateful<Div>,
    open: bool,
    busy: bool,
) -> Div {
    let hairline = with_alpha(theme.colors.border, 0.65);
    let hover = row_hover(theme);
    div()
        .group(group)
        .flex()
        .w_full()
        .items_center()
        .rounded(theme.radii.lg)
        // The pair keeps a soft fill while each half stacks its own on hover.
        .when(busy, |g| g.bg(row_busy_fill(theme)))
        .when(open, |g| g.bg(hover))
        .when(!busy, |g| g.hover(move |s| s.bg(hover)))
        .child(primary.flex_1().rounded_r(px(0.)))
        .child(
            div()
                .h(px(16.))
                .w(px(1.))
                .flex_none()
                .when(open, |h| h.bg(hairline))
                .group_hover(group, move |h| h.bg(hairline)),
        )
        .child(secondary)
}

/// The chevron half of a split row.
pub fn split_secondary(id: impl Into<ElementId>, theme: &AppTheme, disabled: bool, open: bool) -> Stateful<Div> {
    row_base(id, theme, disabled, open)
        .flex_none()
        .w(SPLIT_SECONDARY_WIDTH)
        .px(px(0.))
        .justify_center()
        .rounded_l(px(0.))
        .child(row_icon(theme, IconName::ChevronDown))
}

/// A select-style row (`part="select"`): the label half and a 32px chevron
/// half, each with its own hover fill, as one control.
pub fn select_row(id: impl Into<ElementId>, theme: &AppTheme, content: impl IntoElement, disabled: bool, open: bool) -> Stateful<Div> {
    let hover = row_hover(theme);
    let hairline = with_alpha(theme.colors.border, 0.65);
    let id: ElementId = id.into();
    let group: SharedString = format!("select-{id}").into();
    row_base(id, theme, disabled, open)
        .w_full()
        .gap(px(0.))
        .px(px(0.))
        .group(group.clone())
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .rounded_l(theme.radii.lg)
                .when(!disabled, |d| d.hover(move |s| s.bg(hover)))
                .child(content),
        )
        .child(
            div()
                .relative()
                .flex()
                .flex_none()
                .h_full()
                .w(SPLIT_SECONDARY_WIDTH)
                .items_center()
                .justify_center()
                .rounded_r(theme.radii.lg)
                .when(!disabled, |d| d.hover(move |s| s.bg(hover)))
                .child(
                    div()
                        .absolute()
                        .left(px(-1.))
                        .top(px(8.))
                        .h(px(16.))
                        .w(px(1.))
                        .when(open, |h| h.bg(hairline))
                        .group_hover(group, move |h| h.bg(hairline)),
                )
                .child(row_icon(theme, IconName::ChevronDown)),
        )
}

/// A plain-text row for a locked control (`THREAD_DETAILS_PANEL_LOCKED_ROW_CLASS`).
pub fn locked_row(theme: &AppTheme, content: impl IntoElement) -> Div {
    row_font(div())
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(ROW_HEIGHT)
        .gap(px(10.))
        .px(px(10.))
        .text_color(row_text(theme))
        .child(content)
}

/// A section of the card (`ThreadDetailsSection`): `px-2 pt-2 pb-2.5`, with a
/// top rule when it follows another section.
pub fn section(theme: &AppTheme, separated: bool, rows: Vec<AnyElement>) -> Option<AnyElement> {
    if rows.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_col()
            .px(px(8.))
            .pt(px(8.))
            .pb(px(10.))
            .when(separated, |s| s.border_t_1().border_color(with_alpha(theme.colors.border, 0.65)))
            .children(rows)
            .into_any_element(),
    )
}
