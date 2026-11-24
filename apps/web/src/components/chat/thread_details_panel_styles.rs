//! The row styles every control in the thread details card shares. Ports
//! `threadDetailsPanelStyles.ts` and `ThreadDetailsControl.tsx`: 2rem rows with
//! a 1rem muted icon, 13px medium label at 80%, a faint hover fill, and split
//! rows whose hairline shows only while the pair is in use. Sizes the web gives
//! in rem follow the interface font size through [`AppTheme::rem`].

use gpui_kit::{
    AnyElement, Div, ElementId, Hsla, IntoElement, ParentElement as _, Pixels, SharedString, Stateful, Styled as _,
    TestSupportExt as _, div,
    prelude::*, px,
};

use crate::AppTheme;
use crate::appearance_fonts::{ui_font, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::DISABLED_OPACITY;
use crate::theme_color::with_alpha;

/// Row height (`h-8`).
pub fn row_height(theme: &AppTheme) -> Pixels {
    theme.rem(2.)
}

/// Row gap and side padding (`gap-2.5 px-2.5`).
pub fn row_inset(theme: &AppTheme) -> Pixels {
    theme.rem(0.625)
}

/// Label size: `text-[13px]`, a pixel literal that does not scale.
pub const ROW_TEXT: Pixels = px(13.);
/// Label line height: the cap-trimmed label sits in an 18px box.
pub const ROW_LINE_HEIGHT: Pixels = px(18.);
/// A full row's width: the card less its 1px border and the section's `px-2`.
/// Menus and pickers the rows open take this width (`--anchor-width`).
pub fn row_width(theme: &AppTheme) -> Pixels {
    theme.spacing.thread_details_panel_width - px(2.) - theme.rem(1.)
}

/// The secondary half of a split row (`w-8`).
pub fn split_secondary_width(theme: &AppTheme) -> Pixels {
    theme.rem(2.)
}

/// `size-4` icons.
pub fn row_icon_size(theme: &AppTheme) -> IconSize {
    IconSize::Custom(theme.rem(1.))
}

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

/// `THREAD_DETAILS_PANEL_ICON_CLASS`: 1rem, muted.
pub fn row_icon(theme: &AppTheme, name: impl Into<crate::components::icons::IconSource>) -> Icon {
    Icon::new(name).size(row_icon_size(theme)).color(theme.colors.muted_foreground)
}

/// The base of every row control: 2rem tall, `rounded-lg`, 0.625rem gaps and padding.
/// `open` keeps the hover fill while its popup is open.
pub fn row_base(id: impl Into<ElementId>, theme: &AppTheme, disabled: bool, open: bool) -> Stateful<Div> {
    row_base_hovering(id, theme, disabled, open, true)
}

/// [`row_base`], with its hover fill off while `hover_fill` is false (a control over the row has
/// the pointer, and in the web the row is not hovered then).
pub fn row_base_hovering(id: impl Into<ElementId>, theme: &AppTheme, disabled: bool, open: bool, hover_fill: bool) -> Stateful<Div> {
    let hover = row_hover(theme);
    row_font(div())
        .id(id)
        .relative()
        .flex()
        .items_center()
        .min_w_0()
        .h(row_height(theme))
        .gap(row_inset(theme))
        .px(row_inset(theme))
        .rounded(theme.radii.lg)
        .text_color(row_text(theme))
        .when(open, |r| r.bg(hover))
        .when(disabled, |r| r.opacity(DISABLED_OPACITY))
        .when(!disabled, |r| r.cursor_pointer())
        .when(!disabled && hover_fill, |r| r.hover(move |s| s.bg(hover)))
}

/// A full-width row: icon, then a truncating label.
pub fn row(id: impl Into<ElementId>, theme: &AppTheme, icon: Option<Icon>, label: impl Into<SharedString>, disabled: bool) -> Stateful<Div> {
    row_base(id, theme, disabled, false)
        .w_full()
        .children(icon)
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
}

/// A split row: the primary half grows, the 2rem secondary half holds a
/// chevron, and a 1rem hairline between them shows while the pair is hovered
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
        .child(primary.flex_1().rounded_r(px(0.)).test_support())
        .child(
            div()
                .h(theme.rem(1.))
                .w(px(1.))
                .flex_none()
                .when(open, |h| h.bg(hairline))
                .group_hover(group, move |h| h.bg(hairline)),
        )
        .child(secondary.test_support())
}

/// The chevron half of a split row.
pub fn split_secondary(id: impl Into<ElementId>, theme: &AppTheme, disabled: bool, open: bool) -> Stateful<Div> {
    row_base(id, theme, disabled, open)
        .flex_none()
        .w(split_secondary_width(theme))
        .px(px(0.))
        .justify_center()
        .rounded_l(px(0.))
        .child(row_icon(theme, IconName::ChevronDown))
}

/// A select-style row (`part="select"`): the label half and a 2rem chevron
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
                .gap(row_inset(theme))
                .px(row_inset(theme))
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
                .w(split_secondary_width(theme))
                .items_center()
                .justify_center()
                .rounded_r(theme.radii.lg)
                .when(!disabled, |d| d.hover(move |s| s.bg(hover)))
                .child(
                    div()
                        .absolute()
                        .left(px(-1.))
                        .top(theme.rem(0.5))
                        .h(theme.rem(1.))
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
        .h(row_height(theme))
        .gap(row_inset(theme))
        .px(row_inset(theme))
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
            .px(theme.rem(0.5))
            .pt(theme.rem(0.5))
            .pb(theme.rem(0.625))
            .when(separated, |s| s.border_t_1().border_color(with_alpha(theme.colors.border, 0.65)))
            .children(rows)
            .into_any_element(),
    )
}

/// Wraps a control drawn by a primitive (a [`Button`](crate::components::ui::button::Button)
/// or checkbox) in an observed element, so scripted checks can find and click it.
/// Without `test-support` the wrapper is a plain div.
pub fn observed(id: impl Into<ElementId>, control: impl IntoElement) -> AnyElement {
    div().id(id).child(control).test_support().into_any_element()
}

/// Floats a popup just below the row before it. `below` positions at its
/// parent's origin, so a zero-height spacer after the row gives it that origin.
pub fn below_row(content: impl IntoElement) -> AnyElement {
    div().w_full().h(px(0.)).child(crate::components::ui::popover::below(content)).into_any_element()
}

/// Remembers the press that closed a popup from outside, so the trigger's own
/// click at that spot does not reopen it (the `PopupOpen` rule of `popover.rs`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DismissPress(Option<gpui_kit::Point<Pixels>>);

impl DismissPress {
    pub fn record(&mut self, at: gpui_kit::Point<Pixels>) {
        self.0 = Some(at);
    }

    /// Whether this click is the press that just closed the popup.
    pub fn consumes(&mut self, click: &gpui_kit::ClickEvent) -> bool {
        self.0.take() == Some(crate::components::ui::popover::press_position(click))
    }
}
