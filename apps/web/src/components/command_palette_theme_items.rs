//! Change theme, Change appearance and the theme editor row. Ports
//! `CommandPaletteThemeItems.tsx`, and the `appearance.cycle` toast from
//! `shortcuts/commandPaletteCommands.ts`.

use std::rc::Rc;

use gpui_kit::{App, SharedString};
use t3code_shared::keybindings::StaticCommand;

use super::command_palette_logic::{PaletteGroup, PaletteItem, PaletteRun, PaletteTitle, PaletteTrailing, PaletteView};
use super::command_palette_lookups::PaletteTheme;
use super::icons::IconName;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::theme_palette::{ThemeMode, ThemeSettings};

/// The appearance modes in cycle order (`APPEARANCE_OPTIONS`).
pub const APPEARANCE_OPTIONS: [(ThemeMode, IconName); 3] =
    [(ThemeMode::System, IconName::Monitor), (ThemeMode::Light, IconName::Sun), (ThemeMode::Dark, IconName::Moon)];

fn appearance_label(mode: ThemeMode) -> SharedString {
    match mode {
        ThemeMode::System => t!("System"),
        ThemeMode::Light => t!("Light"),
        ThemeMode::Dark => t!("Dark"),
    }
}

fn mode_key(mode: ThemeMode) -> &'static str {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
}

/// Moves to the next appearance and says so in a short toast (`appearance.cycle`).
pub fn cycle_appearance(cx: &mut App) {
    let current = cx.global::<ThemeSettings>().mode;
    let index = APPEARANCE_OPTIONS.iter().position(|(mode, _)| *mode == current).unwrap_or(0);
    let next = APPEARANCE_OPTIONS[(index + 1) % APPEARANCE_OPTIONS.len()].0;
    ThemeSettings::set_mode(next, cx);
    let title = t!(
        "{mode, select, system {Appearance: System} light {Appearance: Light} other {Appearance: Dark}}",
        mode = mode_key(next)
    );
    Toaster::push(ToastData::new(ToastKind::Plain, title), cx);
}

/// The Change theme page.
pub fn change_theme_item(themes: &[PaletteTheme], cx: &App) -> PaletteItem {
    let current_id = cx.global::<ThemeSettings>().document.id.clone();
    let items = themes
        .iter()
        .map(|theme| {
            let value = match &theme.id {
                Some(id) => format!("theme:palette:{id}"),
                None => "theme:standard".to_owned(),
            };
            let document = theme.document.clone();
            let run = PaletteRun::Local(Rc::new(move |_, cx| ThemeSettings::set_document((*document).clone(), cx)));
            let mut item = PaletteItem::action(value, PaletteTitle::Text(theme.label.clone()), IconName::Palette, run, &[]);
            item.set_terms([theme.label.clone(), "theme".into(), "appearance".into()]);
            if let [(appearance, _)] = theme.previews.as_slice() {
                item.description = Some(if appearance.is_dark() { t!("For dark mode") } else { t!("For light mode") });
            }
            let current = theme.id.as_deref() == Some(current_id.as_str());
            Rc::new(item.trailing(PaletteTrailing::Swatches {
                current,
                colors: theme.previews.iter().map(|(_, colors)| *colors).collect(),
            }))
        })
        .collect();
    let view = PaletteView {
        addon_icon: IconName::Palette,
        groups: vec![PaletteGroup { value: "themes".into(), label: t!("Change theme"), items }],
        initial_query: None,
    };
    PaletteItem::submenu("action:change-theme", t!("Change theme"), IconName::Palette, view, &["change theme", "appearance", "colors", "palette"])
        .shortcut(StaticCommand::ThemeSelect)
}

/// The Change appearance page.
pub fn change_appearance_item(cx: &App) -> PaletteItem {
    let current = cx.global::<ThemeSettings>().mode;
    let items = APPEARANCE_OPTIONS
        .iter()
        .map(|(mode, icon)| {
            let mode = *mode;
            let label = appearance_label(mode);
            let run = PaletteRun::Local(Rc::new(move |_, cx| ThemeSettings::set_mode(mode, cx)));
            let mut item = PaletteItem::action(format!("appearance:{}", mode_key(mode)), PaletteTitle::Text(label.clone()), *icon, run, &[]);
            item.set_terms([label, mode_key(mode).into(), "appearance".into(), "mode".into()]);
            if mode == current {
                item.trailing = Some(PaletteTrailing::Current);
            }
            Rc::new(item)
        })
        .collect();
    let view = PaletteView {
        addon_icon: IconName::Monitor,
        groups: vec![PaletteGroup { value: "appearance".into(), label: t!("Change appearance"), items }],
        initial_query: None,
    };
    PaletteItem::submenu(
        "action:change-appearance",
        t!("Change appearance"),
        IconName::Monitor,
        view,
        &["change appearance", "light", "dark", "system", "mode", "toggle"],
    )
    .shortcut(StaticCommand::AppearanceCycle)
}

/// The theme editor toggle row.
pub fn theme_editor_item() -> PaletteItem {
    PaletteItem::action(
        "action:theme-editor",
        PaletteTitle::Text(t!("Toggle theme editor")),
        IconName::Palette,
        PaletteRun::Command(StaticCommand::ThemeEditorToggle.into()),
        &["theme", "appearance", "colors", "palette", "customize"],
    )
    .shortcut(StaticCommand::ThemeEditorToggle)
}
