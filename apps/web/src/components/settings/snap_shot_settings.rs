//! SnapShots. Ports the rows of `SnapShotSettings.tsx`. Capture itself is
//! the desktop shell's native code, which this app does not have yet, so
//! the rows read and write the saved settings and the setup wizard,
//! shortcut recorder and sound preview are left out.

use gpui_kit::{AnyElement, App, IntoElement, SharedString, prelude::*};
use t3code_contracts::{SnapShotShortcut, SnapShotSound};

use super::settings_controls::{SelectWidth, client_row, client_switch, select_control};
use super::settings_layout::{SettingsRow, SettingsSection, page_container};
use super::settings_search::setting_title;
use crate::components::ui::kbd::Kbd;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;

/// The saved capture shortcut as a label.
fn shortcut_label(shortcut: &SnapShotShortcut) -> SharedString {
    match shortcut {
        SnapShotShortcut::BothShiftKeys(_) => "⇧ + ⇧".into(),
        SnapShotShortcut::ModifierPair(pair) => format!("{0} + {0}", pair.modifier.as_str()).into(),
        SnapShotShortcut::KeyChord(chord) => {
            let mut parts = Vec::new();
            for (on, name) in [(chord.mod_key, "Mod"), (chord.meta_key, "Meta"), (chord.ctrl_key, "Ctrl"), (chord.alt_key, "Alt"), (chord.shift_key, "Shift")] {
                if on {
                    parts.push(name.to_owned());
                }
            }
            parts.push(chord.key.to_uppercase());
            parts.join("+").into()
        }
        SnapShotShortcut::Unknown(_) => "?".into(),
    }
}

/// `SnapShotSettings`.
pub fn snap_shot_panel(cx: &App) -> AnyElement {
    let settings = ClientSettingsStore::get(cx).clone();
    let mut enabled = client_switch(
        "snap-shot-enabled",
        t!("Capture a window and attach it to your current draft."),
        |s| &s.snap_shot_enabled,
        |s, v| s.snap_shot_enabled = v,
        cx,
    );
    if !settings.snap_shot_enabled {
        enabled = enabled.status(t!("Turn this on to set up snapshots."));
    }
    let sound_index = if !settings.snap_shot_play_sound { 0 } else if settings.snap_shot_sound == SnapShotSound::CameraShutter { 2 } else { 1 };
    let sound = select_control(
        "select-snap-shot-sound".into(),
        SelectWidth::Fit,
        std::rc::Rc::from([
            t!("Off", context = "snapshot sound turned off"),
            t!("Whoosh (Default)"),
            t!("Click", context = "name of a camera-shutter sound"),
        ]),
        sound_index,
        |index, _, cx| {
            ClientSettingsStore::update(cx, |s| {
                s.snap_shot_play_sound = index != 0;
                if index != 0 {
                    s.snap_shot_sound = if index == 2 { SnapShotSound::CameraShutter } else { SnapShotSound::SoftPop };
                }
            })
        },
    );
    let mut section = SettingsSection::new(t!("SnapShots")).child(enabled);
    if settings.snap_shot_enabled {
        section = section
            .child(client_switch(
                "snap-shot-accessibility",
                t!("Include text and controls when the app makes them available."),
                |s| &s.snap_shot_include_accessibility,
                |s, v| s.snap_shot_include_accessibility = v,
                cx,
            ))
            .child(
                SettingsRow::new(setting_title("snap-shot-shortcut"))
                    .description(t!("Capture the window you're using without switching apps."))
                    .control(Kbd::new(shortcut_label(&settings.snap_shot_shortcut))),
            )
            .child(client_row(
                "snap-shot-sound",
                t!("Choose the sound played when capture starts."),
                |s| &s.snap_shot_play_sound,
                |s, v| s.snap_shot_play_sound = v,
                sound,
                cx,
            ))
            .child(client_switch("snap-shot-flash", t!("Show a gentle cue on the captured window."), |s| &s.snap_shot_flash, |s, v| s.snap_shot_flash = v, cx))
            .child(client_switch(
                "snap-shot-animations",
                t!("Animate captured windows into your draft."),
                |s| &s.snap_shot_animations,
                |s, v| s.snap_shot_animations = v,
                cx,
            ));
    }
    page_container(cx).child(section).into_any_element()
}
