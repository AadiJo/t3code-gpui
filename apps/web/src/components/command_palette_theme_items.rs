//! Change theme, Change appearance and the theme editor row. Ports
//! `CommandPaletteThemeItems.tsx`, and the `appearance.cycle` toast from
//! `shortcuts/commandPaletteCommands.ts`.

use std::rc::Rc;

use gpui_kit::{App, SharedString};
use t3code_shared::keybindings::StaticCommand;

use super::command_palette_logic::{PaletteGroup, PaletteItem, PaletteRun, PaletteTitle, PaletteTrailing, PaletteView};
use super::settings::theme_sphere::{SphereColors, standard_sphere_colors};
use super::icons::IconName;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::hooks::use_theme::{ThemeStore, standard_theme};
use crate::theme_palette::{Appearance, ThemeMode, ThemeSettings};

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

/// The Change theme page (`useThemeCards`): the standard T3 Code theme, then every other theme
/// in the catalog (built-in, custom, environment). A one-mode theme sets that half; the others
/// set the whole theme.
pub fn change_theme_item(cx: &App) -> PaletteItem {
    let settings = cx.global::<ThemeSettings>();
    let resolved = match settings.mode {
        ThemeMode::System => settings.system,
        ThemeMode::Light => Appearance::Light,
        ThemeMode::Dark => Appearance::Dark,
    };
    let store = cx.try_global::<ThemeStore>();
    let standard_id = standard_theme().document.id.clone();
    // `themeHalves[resolvedTheme] ?? getThemeDefinition(theme)?.id`; the standard theme is `None`.
    let current_id = store.and_then(|store| {
        store.halves().get(resolved).map(str::to_owned).or_else(|| ThemeStore::find(store.preference(), cx).map(|t| t.document.id.clone()))
    });
    let current_id = current_id.filter(|id| *id != standard_id);
    let items = ThemeStore::catalog(cx)
        .into_iter()
        .map(|theme| {
            let standard = theme.document.id == standard_id;
            let id: Option<SharedString> = (!standard).then(|| theme.document.id.clone().into());
            let value = match &id {
                Some(id) => format!("theme:palette:{id}"),
                None => "theme:standard".to_owned(),
            };
            let modes = theme.modes();
            let single = (modes.len() == 1).then(|| modes[0]);
            let label: SharedString = theme.document.name.clone().into();
            let run_id = id.clone();
            let run = PaletteRun::Local(Rc::new(move |_, cx| match (&run_id, single) {
                (Some(id), Some(appearance)) => ThemeStore::set_half(appearance, Some(id), cx),
                (Some(id), None) => ThemeStore::set_theme(id, cx),
                (None, _) => {
                    let mode = cx.global::<ThemeSettings>().mode;
                    ThemeStore::set_theme(mode_key(mode), cx)
                }
            }));
            let mut item = PaletteItem::action(value, PaletteTitle::Text(label.clone()), IconName::Palette, run, &[]);
            item.set_terms([label, "theme".into(), "appearance".into()]);
            if let Some(appearance) = single {
                item.description = Some(if appearance.is_dark() { t!("For dark mode") } else { t!("For light mode") });
            }
            let colors = modes
                .iter()
                .map(|&mode| {
                    let colors = if standard {
                        standard_sphere_colors(mode)
                    } else {
                        let palette = theme.document.palette(mode);
                        SphereColors { canvas: palette.canvas, accent: palette.accent, message_action: palette.message_action }
                    };
                    (mode, colors)
                })
                .collect();
            let current = current_id.as_deref() == id.as_deref();
            Rc::new(item.trailing(PaletteTrailing::Swatches { current, colors }))
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
