//! Hands client settings to the views and stores that predate the settings
//! store and keep their own copies, so a change reaches them in the same
//! frame: the diff panel's settings, the composer's send shortcut and rich
//! text switch, and the timestamp format.

use gpui_kit::{App, BorrowAppContext as _};
use t3code_contracts::{ClientSettingsSchemaSendShortcut, DiffColorScheme, DiffLayout, TimestampFormat};

use super::use_settings::{ClientSettings, ClientSettingsStore};
use crate::composer_logic::SendShortcut;
use crate::diff_panel_store::{self, DiffPanelStore, DiffSettings};
use crate::timestamp_format;

/// The diff panel's copy of the diff settings.
pub fn diff_settings(settings: &ClientSettings) -> DiffSettings {
    DiffSettings {
        diff_layout: match settings.diff_layout {
            DiffLayout::Split => diff_panel_store::DiffLayout::Split,
            _ => diff_panel_store::DiffLayout::Stacked,
        },
        word_wrap: settings.word_wrap,
        diff_ignore_whitespace: settings.diff_ignore_whitespace,
        diff_files_collapsed: settings.diff_files_collapsed,
        diff_color_scheme: match settings.diff_color_scheme {
            DiffColorScheme::BlueOrange => diff_panel_store::DiffColorScheme::BlueOrange,
            _ => diff_panel_store::DiffColorScheme::RedGreen,
        },
    }
}

/// The diff toolbar's layout toggle saves the setting, as the web's does.
pub fn set_diff_layout(layout: diff_panel_store::DiffLayout, cx: &mut App) {
    let layout = match layout {
        diff_panel_store::DiffLayout::Split => DiffLayout::Split,
        diff_panel_store::DiffLayout::Stacked => DiffLayout::Stacked,
    };
    ClientSettingsStore::update(cx, |s| s.diff_layout = layout);
}

/// The composer's send shortcut setting.
pub fn composer_send_shortcut(cx: &App) -> SendShortcut {
    match ClientSettingsStore::get(cx).send_shortcut {
        ClientSettingsSchemaSendShortcut::ModEnter => SendShortcut::ModEnter,
        ClientSettingsSchemaSendShortcut::ModEnterMultiline => SendShortcut::ModEnterMultiline,
        _ => SendShortcut::Enter,
    }
}

/// Whether the composer formats Markdown as you type.
pub fn composer_rich_text(cx: &App) -> bool {
    ClientSettingsStore::get(cx).composer_rich_text_enabled
}

/// The timestamp format setting, for the formatting helpers.
pub fn timestamp_format(cx: &App) -> timestamp_format::TimestampFormat {
    match ClientSettingsStore::get(cx).timestamp_format {
        TimestampFormat::N12Hour => timestamp_format::TimestampFormat::TwelveHour,
        TimestampFormat::N24Hour => timestamp_format::TimestampFormat::TwentyFourHour,
        _ => timestamp_format::TimestampFormat::Locale,
    }
}

/// Copies settings into the stores that keep their own copy. Runs on
/// every client settings change.
pub fn apply(cx: &mut App) {
    let diff = diff_settings(ClientSettingsStore::get(cx));
    if cx.try_global::<DiffPanelStore>().is_none_or(|store| store.settings != diff) {
        DiffPanelStore::global(cx);
        cx.update_global::<DiffPanelStore, _>(|store, _| store.settings = diff);
        cx.refresh_windows();
    }
}
