//! Palette rows. Ports `CommandPaletteResults.tsx`: group labels, item rows
//! with icon, title, second line, timestamp, shortcut and submenu chevron,
//! disabled rows, and the empty state.

use gpui_kit::{
    AnyElement, App, AppContext as _, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Window, div, prelude::FluentBuilder as _, px,
};
use t3code_shared::palette_search::{SearchMatchHint, char_ranges_to_bytes};

use super::command_palette_drag::PaletteThreadDrag;
use super::command_palette_logic::{PaletteItem, PaletteItemKind, PaletteRow, PaletteTitle, PaletteTrailing, ShownItem};
use super::icons::{Icon, IconName, IconSize};
use super::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, weight};
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// The height a row lays out at, so the list can size itself without measuring.
pub fn row_height(row: &PaletteRow, theme: &AppTheme) -> Pixels {
    let rem = theme.fonts.interface;
    let xs = px(TextSize::Xs.line_height(rem));
    match row {
        PaletteRow::Label { first, .. } => theme.sp(3.) + xs + if *first { px(0.) } else { theme.sp(1.5) },
        PaletteRow::Item { shown, .. } => {
            let title = px(TextSize::Sm.line_height(rem));
            let second = if has_second_line(shown) { xs } else { px(0.) };
            (theme.sp(3.) + title + second).max(theme.sp(7.))
        }
    }
}

fn has_second_line(shown: &ShownItem) -> bool {
    shown.item.description.is_some() || shown.hint.is_some()
}

/// What a row reports back to the palette.
pub struct RowCallbacks {
    pub on_hover: Box<dyn Fn(&mut Window, &mut App)>,
    pub on_click: Box<dyn Fn(&mut Window, &mut App)>,
    pub on_drag_start: Box<dyn Fn(&mut Window, &mut App)>,
}

/// A group label (`CommandListHeading`): `px-2 py-1.5 text-xs font-medium`, `pt-1.5` after the first.
pub fn render_label(label: SharedString, first: bool, theme: &AppTheme) -> AnyElement {
    div()
        .w_full()
        .when(!first, |d| d.pt(theme.sp(1.5)))
        .child(
            div()
                .px(theme.sp(2.))
                .py(theme.sp(1.5))
                .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                .text_color(theme.colors.muted_foreground)
                .child(label),
        )
        .into_any_element()
}

fn title_element(item: &PaletteItem, theme: &AppTheme) -> AnyElement {
    match &item.title {
        PaletteTitle::Text(text) => div().truncate().child(text.clone()).into_any_element(),
        PaletteTitle::NewThreadIn { project } => {
            let full = item.search_name();
            let start = full.find(project.as_ref()).unwrap_or(full.len());
            let bold = HighlightStyle { font_weight: Some(FontWeight(600.)), ..Default::default() };
            let _ = theme;
            div()
                .truncate()
                .child(StyledText::new(full.clone()).with_highlights([(start..start + project.len(), bold)]))
                .into_any_element()
        }
    }
}

/// `SearchMatchHintLine`: "Branch: feature/x", the matched letters bold.
fn hint_line(hint: &SearchMatchHint, theme: &AppTheme) -> AnyElement {
    let c = &theme.colors;
    let label = match hint.label {
        "PR" => t!("PR", context = "abbreviation of pull request"),
        "Branch" => t!("Branch"),
        "Project" => t!("Project"),
        "Environment" => t!("Environment"),
        _ => t!("ID or link"),
    };
    let strong = HighlightStyle { color: Some(c.foreground()), font_weight: Some(FontWeight(600.)), ..Default::default() };
    let highlights: Vec<_> = char_ranges_to_bytes(&hint.text, &hint.ranges).into_iter().map(|r| (r, strong)).collect();
    div()
        .flex()
        .gap(px(4.))
        .min_w_0()
        .truncate()
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(with_alpha(c.muted_foreground, 0.85))
        .child(div().flex_none().text_color(with_alpha(c.muted_foreground, 0.7)).child(t!("{label}:", label = label)))
        .child(div().truncate().child(StyledText::new(SharedString::from(hint.text.to_string())).with_highlights(highlights)))
        .into_any_element()
}

fn trailing_element(trailing: &PaletteTrailing, theme: &AppTheme) -> AnyElement {
    let muted = with_alpha(theme.colors.muted_foreground, 0.7);
    let current = || div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(muted).child(t!("Current", context = "the theme or mode in use now"));
    match trailing {
        PaletteTrailing::Current => current().into_any_element(),
        PaletteTrailing::Swatches { current: is_current, colors } => div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.sp(2.))
            .when(*is_current, |d| d.child(current()))
            .child(div().flex().items_center().gap(theme.sp(1.)).children(colors.iter().map(|[background, accent]| {
                // `ThemePreviewCircle` at `size-3`: the background with the accent's half.
                div()
                    .size(theme.sp(3.))
                    .rounded_full()
                    .overflow_hidden()
                    .flex()
                    .child(div().w_1_2().h_full().bg(*background))
                    .child(div().w_1_2().h_full().bg(*accent))
            })))
            .into_any_element(),
    }
}

/// An item row (`CommandPaletteResultRow` / `DisabledCommandPaletteResultRow`).
pub fn render_item(
    ix: usize,
    shown: &ShownItem,
    active: bool,
    shortcut_label: Option<SharedString>,
    theme: &AppTheme,
    callbacks: Option<RowCallbacks>,
) -> AnyElement {
    let c = &theme.colors;
    let item = &shown.item;
    let muted_70 = with_alpha(c.muted_foreground, 0.7);
    let title_line = div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(theme.sp(1.5))
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .text_color(c.foreground())
        .child(title_element(item, theme))
        // Project and environment, when the second line shows a hint instead.
        .when_some(shown.hint.as_ref().and_then(|hint| {
            let thread = item.thread.as_ref()?;
            let parts: Vec<&str> = [thread.subject.project_title.as_ref(), thread.subject.environment_label.as_ref()]
                .into_iter()
                .flatten()
                .map(|t| t.as_str())
                .filter(|t| *t != &*hint.text)
                .collect();
            (!parts.is_empty()).then(|| parts.join(" · "))
        }), |d, context| {
            d.child(div().flex_none().max_w(px(220.)).truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(muted_70).child(context))
        });
    let content = if has_second_line(shown) {
        let second = match (&shown.hint, &item.description) {
            (Some(hint), _) => hint_line(hint, theme),
            (None, Some(description)) => div()
                .min_w_0()
                .truncate()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(muted_70)
                .child(description.clone())
                .into_any_element(),
            (None, None) => div().into_any_element(),
        };
        div().flex().flex_col().flex_1().min_w_0().child(title_line).child(second)
    } else {
        div().flex().flex_1().min_w_0().child(title_line)
    };
    let enabled = item.is_enabled();
    let row = div()
        .id(("command-palette-row", ix))
        .w_full()
        .flex()
        .items_center()
        .gap(theme.sp(2.))
        .min_h(theme.sp(7.))
        .px(theme.sp(2.))
        .py(theme.sp(1.5))
        .rounded(theme.radii.sm)
        .when(active, |d| d.bg(c.highlight()).text_color(c.highlight_foreground()))
        .when(!enabled, |d| d.opacity(DISABLED_OPACITY))
        // `CommandPaletteTitleIcon`: a 20px slot, the icon 2px down from the top.
        .child(
            div()
                .flex()
                .flex_none()
                .h(theme.sp(5.))
                .items_center()
                .self_start()
                .pt(theme.sp(0.5))
                .child(Icon::new(item.icon).size(IconSize::Md).color(c.icon_muted)),
        )
        .child(content)
        .when_some(item.trailing.as_ref(), |d, trailing| d.child(trailing_element(trailing, theme)))
        .when(item.unavailable, |d| {
            d.child(div().flex_none().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(muted_70).child(t!("Unavailable")))
        })
        .when_some(item.timestamp.clone(), |d, timestamp| {
            d.child(
                div()
                    .flex_none()
                    .min_w(theme.sp(12.))
                    .flex()
                    .justify_end()
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(muted_70)
                    .child(timestamp),
            )
        })
        .when_some(shortcut_label.filter(|_| enabled), |d, label| {
            // `CommandShortcut`: `ms-auto text-xs font-medium tracking-widest text-secondary-label`.
            d.child(
                div()
                    .flex_none()
                    .ml_auto()
                    .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                    .text_color(c.secondary_label)
                    .child(spaced(&label)),
            )
        })
        .when(item.is_submenu(), |d| {
            d.child(div().flex_none().ml_auto().mr(px(-2.)).child(Icon::new(IconName::ChevronRight).size(IconSize::Md).color(muted_70)))
        });
    let Some(callbacks) = callbacks.filter(|_| enabled) else {
        return row.into_any_element();
    };
    let RowCallbacks { on_hover, on_click, on_drag_start } = callbacks;
    let row = row
        .cursor_pointer()
        .on_mouse_move(move |_, window, cx| on_hover(window, cx))
        .on_click(move |_, window, cx| on_click(window, cx));
    match (&item.thread, &item.kind) {
        (Some(thread), PaletteItemKind::Action { .. }) => {
            let drag = PaletteThreadDrag { thread: thread.key.clone(), title: thread.title.clone() };
            row.on_drag(drag, move |drag, _, window, cx| {
                on_drag_start(window, cx);
                cx.new(|_| drag.clone())
            })
            .into_any_element()
        }
        _ => row.into_any_element(),
    }
}

/// `tracking-widest` (0.1em) has no GPUI equivalent; a hair space between
/// glyphs comes close at 12px.
fn spaced(label: &str) -> SharedString {
    let mut out = String::with_capacity(label.len() * 3);
    for (i, c) in label.chars().enumerate() {
        if i > 0 {
            out.push('\u{200A}');
        }
        out.push(c);
    }
    out.into()
}

/// The empty state: `py-10 text-center text-sm text-muted-foreground`.
pub fn render_empty(message: SharedString, theme: &AppTheme) -> AnyElement {
    div()
        .w_full()
        .py(theme.sp(10.))
        .flex()
        .justify_center()
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .text_color(theme.colors.muted_foreground)
        .child(message)
        .into_any_element()
}
