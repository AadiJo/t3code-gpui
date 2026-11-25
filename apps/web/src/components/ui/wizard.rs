//! `wizard.tsx`: the popup, header, panel and footer every wizard dialog is built from.

use gpui_kit::{AnyElement, Div, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::FluentBuilder as _, px};

use super::TextStyleExt as _;
use super::dialog::dialog_glass;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::theme_color::with_alpha;

/// `WizardPopup`: a `dialog-glass` card, `max-w-xl` (or `max-w-3xl` when wide), whose content
/// scrolls. Add the header, panel and footer as children.
pub fn wizard_popup(theme: &AppTheme, wide: bool) -> Div {
    dialog_glass(theme).occlude().w_full().max_w(theme.rem(if wide { 48. } else { 36. })).overflow_hidden()
}

/// `WizardHeader` (`DialogHeader`: `gap-2 p-6`, `pb-3` above a panel; the title is
/// `text-xl font-semibold leading-snug`, the description `text-sm text-muted-foreground`).
pub fn wizard_header(theme: &AppTheme, title: SharedString, description: Option<SharedString>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.sp(2.))
        .p(theme.sp(6.))
        .pb(theme.sp(3.))
        .child(
            div()
                .pr(theme.sp(6.))
                .ui_text(theme, TextSize::Xl, weight::SEMIBOLD)
                .line_height(px(TextSize::Xl.size(theme.fonts.interface) * 1.375))
                .child(title),
        )
        .children(description.map(|description| {
            div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.muted_foreground).child(description)
        }))
        .into_any_element()
}

/// A 1px ring outside the box (`ring-1`).
fn ring(color: gpui_kit::Hsla) -> gpui_kit::BoxShadow {
    gpui_kit::BoxShadow { color, offset: gpui_kit::point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.), inset: false }
}

/// `WizardPanel`: `space-y-4 px-6 py-5`, white/2 with a white/5 ring (`bg-background/80` and
/// black/5 in light). `last` rounds its foot to the popup's corners when no footer follows.
pub fn wizard_panel(theme: &AppTheme, last: bool, children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    let dark = theme.appearance.is_dark();
    div()
        .when(last, |d| d.rounded_b(theme.radii.xxl - px(1.)))
        .flex()
        .flex_col()
        .gap(theme.sp(4.))
        .min_w_0()
        .px(theme.sp(6.))
        .py(theme.sp(5.))
        .bg(if dark { with_alpha(gpui_kit::white(), 0.02) } else { with_alpha(theme.colors.background(), 0.8) })
        .shadow(vec![ring(if dark { with_alpha(gpui_kit::white(), 0.05) } else { with_alpha(gpui_kit::black(), 0.05) })])
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .children(children)
        .into_any_element()
}

/// `WizardFooter` (`DialogFooter variant="bare"`: `gap-2 px-6 py-4`, buttons at the end). A
/// `leading` element sits at the start and pushes the rest to the end (`sm:justify-between`).
pub fn wizard_footer(theme: &AppTheme, leading: Option<AnyElement>, children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    let buttons = div().flex().gap(theme.sp(2.)).children(children);
    div()
        .flex()
        .gap(theme.sp(2.))
        .px(theme.sp(6.))
        .py(theme.sp(4.))
        .rounded_b(theme.radii.xxl - px(1.))
        .map(|d| match leading {
            Some(leading) => d.justify_between().child(leading).child(buttons),
            None => d.justify_end().child(buttons),
        })
        .into_any_element()
}
