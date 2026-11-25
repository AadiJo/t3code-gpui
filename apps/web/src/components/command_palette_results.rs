//! Palette rows. Ports `CommandPaletteResults.tsx`: group labels, item rows
//! with icon, title, second line, timestamp, shortcut and submenu chevron,
//! disabled rows, and the empty state.

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, FontWeight, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, StyledText, Window, div, prelude::FluentBuilder as _, px,
};
use t3code_shared::palette_search::{SearchMatchHint, char_ranges_to_bytes};

use super::command_palette_drag::PaletteThreadDrag;
use super::command_palette_lookups::PaletteThread;
use super::sidebar::model::SidebarProviderMark;
use super::command_palette_logic::{PaletteItem, PaletteItemKind, PaletteProjectRow, PaletteRow, PaletteTitle, PaletteTrailing, ShownItem};
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
        PaletteRow::Item { shown, .. } => item_height(shown, theme),
    }
}

/// An item row's height: `py-1.5` around the title and second line. A thread
/// subtitle is an inline-flex line, which the fork's line box makes 2px taller.
fn item_height(shown: &ShownItem, theme: &AppTheme) -> Pixels {
    let rem = theme.fonts.interface;
    let title = px(TextSize::Sm.line_height(rem));
    let second = if has_second_line(shown) { px(TextSize::Xs.line_height(rem)) } else { px(0.) };
    let inline_flex = if shown.hint.is_none() && (shown.item.thread.is_some() || shown.item.project.is_some()) && has_second_line(shown) {
        px(2.)
    } else {
        px(0.)
    };
    (theme.sp(3.) + title + second + inline_flex).max(theme.sp(7.))
}

fn has_second_line(shown: &ShownItem) -> bool {
    shown.item.description.is_some() || shown.hint.is_some()
}

/// What a row reports back to the palette.
pub struct RowCallbacks {
    pub on_hover: Box<dyn Fn(&mut Window, &mut App)>,
    pub on_click: Box<dyn Fn(&mut Window, &mut App)>,
    /// The project row's pin button.
    pub on_pin: Box<dyn Fn(&mut Window, &mut App)>,
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
            // The project name is `font-semibold` inside the translated sentence.
            let full = item.search_name();
            let start = full.find(project.as_ref()).unwrap_or(full.len());
            let end = (start + project.len()).min(full.len());
            let _ = theme;
            div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(div().flex_none().child(SharedString::from(full[..start].replace(' ', "\u{a0}"))))
                .child(div().flex_none().font(crate::appearance_fonts::ui_font(weight::SEMIBOLD)).child(SharedString::from(full[start..end].to_owned())))
                .child(div().min_w_0().truncate().child(SharedString::from(full[end..].replace(' ', "\u{a0}"))))
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
        .gap(theme.rem(0.25))
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
        // `CheckIcon ms-auto size-4 text-muted-foreground/70`.
        PaletteTrailing::Check => div().flex_none().ml_auto().child(Icon::new(IconName::Check).size(IconSize::Md).color(muted)).into_any_element(),
        PaletteTrailing::Chevron => {
            div().flex_none().ml_auto().mr(px(-2.)).child(Icon::new(IconName::ChevronRight).size(IconSize::Md).color(muted)).into_any_element()
        }
        PaletteTrailing::Checkbox(checked) => div()
            .flex_none()
            .ml_auto()
            .child(crate::components::ui::checkbox::Checkbox::new("command-palette-checkbox", if *checked { crate::components::ui::checkbox::CheckState::Checked } else { crate::components::ui::checkbox::CheckState::Unchecked }))
            .into_any_element(),
        PaletteTrailing::Swatches { current: is_current, colors } => div()
            .flex()
            .flex_none()
            .items_center()
            .gap(theme.sp(2.))
            .when(*is_current, |d| d.child(current()))
            .child(div().flex().items_center().gap(theme.sp(1.)).children(colors.iter().map(|(mode, colors)| {
                // `ThemePreviewCircle` at `size-3 border-0`, over the popover.
                crate::components::settings::theme_sphere::borderless_theme_sphere(*colors, *mode, theme.colors.popover(), theme.sp(3.))
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
    cx: &gpui_kit::App,
) -> AnyElement {
    let c = &theme.colors;
    let item = &shown.item;
    let (callbacks, on_pin) = match callbacks {
        Some(mut callbacks) => {
            let on_pin = std::mem::replace(&mut callbacks.on_pin, Box::new(|_, _| {}));
            (Some(callbacks), Some(on_pin))
        }
        None => (None, None),
    };
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
            d.child(div().flex_none().max_w(gpui_kit::relative(0.45)).truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(muted_70).child(context))
        });
    let content = if has_second_line(shown) {
        let second = match (&shown.hint, &item.description) {
            (Some(hint), _) => hint_line(hint, theme),
            (None, _) if item.project.is_some() => project_location(item.project.as_ref().expect("checked"), theme),
            (None, _) if item.thread.is_some() => {
                thread_subtitle(item.thread.as_ref().expect("checked"), item.secondary, theme, cx)
            }
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
        // A fixed height keeps the 30px pitch where the line height rounds.
        .h(item_height(shown, theme))
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
                .child(match &item.project {
                    Some(project) => crate::components::project_favicon::ProjectFavicon::new(project.title.clone())
                        .size(theme.rem(1.))
                        .into_any_element(),
                    None => Icon::new(item.icon).size(IconSize::Md).color(c.icon_muted).into_any_element(),
                }),
        )
        .child(content)
        .when_some(item.trailing.as_ref(), |d, trailing| d.child(trailing_element(trailing, theme)))
        .when(item.unavailable, |d| {
            d.child(div().flex_none().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(muted_70).child(t!("Unavailable")))
        })
        .when_some(item.project.as_ref().zip(on_pin), |d, (project, on_pin)| d.child(pin_button(ix, project, active, theme, on_pin)))
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
        .when(item.is_submenu() && !item.hide_chevron, |d| {
            d.child(div().flex_none().ml_auto().mr(px(-2.)).child(Icon::new(IconName::ChevronRight).size(IconSize::Md).color(muted_70)))
        });
    let Some(callbacks) = callbacks.filter(|_| enabled) else {
        return row.into_any_element();
    };
    let RowCallbacks { on_hover, on_click, on_drag_start, .. } = callbacks;
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
            .test_support()
            .into_any_element()
        }
        _ => row.test_support().into_any_element(),
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

/// A summary above the list (`NewProjectSummary`, `RemoteRepositorySummary`): an icon, a title
/// and a `text-muted-foreground/85` second line in a `min-h-8 px-2 py-1.5` row.
pub fn render_summary_row(icon: IconName, title: SharedString, description: SharedString, theme: &AppTheme) -> AnyElement {
    let c = &theme.colors;
    let rem = theme.fonts.interface;
    div()
        .flex()
        .items_center()
        .gap(theme.sp(2.))
        .h(theme.sp(3.) + px(TextSize::Sm.line_height(rem)) + px(TextSize::Xs.line_height(rem)))
        .px(theme.sp(2.))
        .py(theme.sp(1.5))
        .rounded(theme.radii.sm)
        .child(div().flex().flex_none().h(theme.sp(5.)).items_center().self_start().pt(theme.sp(0.5)).child(Icon::new(icon).size(IconSize::Md).color(c.icon_muted)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().truncate().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.foreground()).child(title))
                .child(div().truncate().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(with_alpha(c.muted_foreground, 0.85)).child(description)),
        )
        .into_any_element()
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

/// `ThreadCommandSubtitle`: the project's icon and name · environment, the
/// workspace icon and branch, the provider's mark, and "Current thread", in
/// `text-xs text-muted-foreground/70` with dots between.
fn thread_subtitle(thread: &PaletteThread, is_current: bool, theme: &AppTheme, cx: &gpui_kit::App) -> AnyElement {
    let muted = with_alpha(theme.colors.muted_foreground, 0.7);
    let small = theme.rem(0.75);
    let dot = || div().flex_none().child("·");
    let mut parts: Vec<AnyElement> = Vec::new();
    if let Some(project) = thread.project_title.clone() {
        parts.push(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(theme.rem(0.25))
                .child(crate::components::project_favicon::ProjectFavicon::new(project.clone()).size(small))
                .child(div().min_w_0().truncate().child(project))
                .child(dot())
                .child(div().flex_none().child(thread.environment_label.clone()))
                .into_any_element(),
        );
    }
    if let Some(branch) = thread.branch.clone() {
        parts.push(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(theme.rem(0.25))
                .child(Icon::new(if thread.in_worktree { IconName::FolderGit2 } else { IconName::Folder }).size(IconSize::Custom(small)).color(muted))
                .child(div().min_w_0().truncate().child(branch))
                .into_any_element(),
        );
    }
    if let Some(provider) = &thread.provider {
        let mark = match provider.mark {
            SidebarProviderMark::Brand(mark) => {
                let composer = crate::components::chat::composer_theme::ComposerColors::new(cx);
                crate::components::chat::composer_icons::provider_icon(mark, small, &provider.initials, &composer, cx)
            }
            SidebarProviderMark::AcpRegistry => {
                Icon::new(crate::components::icons::BrandIcon::AcpRegistry).size(IconSize::Custom(small)).color(muted).into_any_element()
            }
        };
        parts.push(div().flex_none().opacity(0.7).child(mark).into_any_element());
    }
    if is_current {
        parts.push(div().flex_none().child(t!("Current thread")).into_any_element());
    }
    let mut row = div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(theme.rem(0.25))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(muted);
    for (index, part) in parts.into_iter().enumerate() {
        if index > 0 {
            row = row.child(dot());
        }
        row = row.child(part);
    }
    row.into_any_element()
}

/// `ProjectLocationDescription`: "Current · Local · /path", the path truncating.
fn project_location(project: &PaletteProjectRow, theme: &AppTheme) -> AnyElement {
    let muted = with_alpha(theme.colors.muted_foreground, 0.7);
    let dot = || div().flex_none().child("·");
    div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(theme.rem(0.25))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(muted)
        .when(project.current, |d| d.child(div().flex_none().child(t!("Current", context = "this project is the current one"))).child(dot()))
        .child(div().flex_none().truncate().child(project.location.clone().unwrap_or_else(|| t!("Remote", context = "machine location"))))
        .child(dot())
        .child(div().flex_1().min_w_0().truncate().child(project.workspace_root.clone()))
        .into_any_element()
}

/// `ProjectPinButton`: a pinned project always shows its pin (crossed on
/// hover); an unpinned one shows it on the highlighted row only.
fn pin_button(ix: usize, project: &PaletteProjectRow, active: bool, theme: &AppTheme, on_pin: Box<dyn Fn(&mut Window, &mut App)>) -> AnyElement {
    let group: SharedString = format!("palette-pin-{ix}").into();
    let muted = theme.colors.muted_foreground;
    let hover = theme.colors.highlight();
    let size = IconSize::Custom(theme.rem(0.875));
    let title = if project.pinned {
        t!("Unpin {name}", name = project.name.as_ref())
    } else {
        t!("Pin {name}", name = project.name.as_ref())
    };
    div()
        .id(("palette-pin", ix))
        .group(group.clone())
        .flex()
        .flex_none()
        .ml_auto()
        .size(theme.rem(1.5))
        .items_center()
        .justify_center()
        .rounded(theme.radii.md)
        .when(!project.pinned && !active, |d| d.invisible())
        .hover(move |s| s.bg(hover))
        .tooltip(crate::components::ui::tooltip::tooltip(title))
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_pin(window, cx)
        })
        .map(|d| {
            if project.pinned {
                d.child(div().group_hover(group.clone(), |d| d.invisible()).child(Icon::new(IconName::Pin).size(size).color(muted)))
                    .child(div().absolute().invisible().group_hover(group, |d| d.visible()).child(Icon::new(IconName::PinOff).size(size).color(muted)))
            } else {
                d.child(Icon::new(IconName::Pin).size(size).color(muted))
            }
        })
        .into_any_element()
}
