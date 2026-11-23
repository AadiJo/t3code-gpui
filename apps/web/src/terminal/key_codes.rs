//! Encodes keystrokes as the bytes a terminal program expects. Ports the role
//! of `terminal/ghostty/keyCodes.ts`: xterm legacy sequences with modifier
//! parameters, application cursor mode, and the kitty keyboard protocol
//! (disambiguate, report all keys, report event types) when a program enables it.

use std::borrow::Cow;

use alacritty_terminal::term::TermMode;
use gpui_kit::Keystroke;

/// Press, autorepeat or release, for kitty's event-type reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

/// The bytes for a keystroke, or `None` when the key should reach the text
/// input path (plain characters) or the app's own shortcuts (platform-key chords).
pub fn encode_key(keystroke: &Keystroke, mode: TermMode, kind: KeyEventKind) -> Option<Cow<'static, str>> {
    if keystroke.modifiers.platform {
        return None;
    }
    if mode.intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC) {
        return encode_kitty(keystroke, mode, kind);
    }
    if kind == KeyEventKind::Release {
        return None;
    }
    encode_legacy(keystroke, mode)
}

/// xterm modifier parameter: 1 + shift(1) + alt(2) + ctrl(4) + super(8).
fn modifier_param(keystroke: &Keystroke) -> u32 {
    let m = &keystroke.modifiers;
    1 + m.shift as u32 + ((m.alt as u32) << 1) + ((m.control as u32) << 2) + ((m.platform as u32) << 3)
}

/// The CSI tail for keys that are the same in legacy and kitty encodings.
fn functional_key(key: &str) -> Option<(&'static str, char)> {
    // (number, final): `CSI number ; mods final`, number empty means `1` when modified.
    Some(match key {
        "up" => ("", 'A'),
        "down" => ("", 'B'),
        "right" => ("", 'C'),
        "left" => ("", 'D'),
        "home" => ("", 'H'),
        "end" => ("", 'F'),
        "f1" => ("", 'P'),
        "f2" => ("", 'Q'),
        "f3" => ("", 'R'),
        "f4" => ("", 'S'),
        "insert" => ("2", '~'),
        "delete" => ("3", '~'),
        "pageup" => ("5", '~'),
        "pagedown" => ("6", '~'),
        "f5" => ("15", '~'),
        "f6" => ("17", '~'),
        "f7" => ("18", '~'),
        "f8" => ("19", '~'),
        "f9" => ("20", '~'),
        "f10" => ("21", '~'),
        "f11" => ("23", '~'),
        "f12" => ("24", '~'),
        "f13" => ("25", '~'),
        "f14" => ("26", '~'),
        "f15" => ("28", '~'),
        "f16" => ("29", '~'),
        "f17" => ("31", '~'),
        "f18" => ("32", '~'),
        "f19" => ("33", '~'),
        "f20" => ("34", '~'),
        _ => return None,
    })
}

fn encode_legacy(keystroke: &Keystroke, mode: TermMode) -> Option<Cow<'static, str>> {
    let m = &keystroke.modifiers;
    let plain = !m.shift && !m.alt && !m.control;
    let key = keystroke.key.as_str();

    let fixed = match (key, m.shift, m.alt, m.control) {
        ("tab", false, false, false) => Some("\t"),
        ("tab", true, false, false) => Some("\x1b[Z"),
        ("escape", false, false, false) => Some("\x1b"),
        ("enter", false, false, false) => Some("\r"),
        ("enter", true, false, false) => Some("\n"),
        ("enter", false, true, false) => Some("\x1b\r"),
        ("backspace", _, false, false) => Some("\x7f"),
        ("backspace", false, false, true) => Some("\x08"),
        ("backspace", false, true, false) => Some("\x1b\x7f"),
        ("space", false, false, true) => Some("\0"),
        _ => None,
    };
    if let Some(fixed) = fixed {
        return Some(Cow::Borrowed(fixed));
    }

    if let Some((number, final_char)) = functional_key(key) {
        let app_cursor = mode.contains(TermMode::APP_CURSOR);
        if plain {
            return Some(Cow::Owned(match (number, final_char) {
                ("", 'A' | 'B' | 'C' | 'D' | 'H' | 'F') if app_cursor => format!("\x1bO{final_char}"),
                ("", 'P' | 'Q' | 'R' | 'S') => format!("\x1bO{final_char}"),
                ("", _) => format!("\x1b[{final_char}"),
                (number, _) => format!("\x1b[{number}{final_char}"),
            }));
        }
        let number = if number.is_empty() { "1" } else { number };
        return Some(Cow::Owned(format!("\x1b[{number};{}{final_char}", modifier_param(keystroke))));
    }

    // Caret notation: ctrl+letter and the C0 punctuation keys.
    if m.control && !m.alt {
        if let Some(byte) = control_byte(key) {
            return Some(Cow::Owned((byte as char).to_string()));
        }
    }

    // Alt as meta: ESC prefix. On macOS Option composes characters instead,
    // so only chords that produced no character are treated as meta.
    let alt_is_meta = !cfg!(target_os = "macos") || keystroke.key_char.is_none();
    if m.alt && alt_is_meta && key.len() == 1 && key.is_ascii() {
        let ch = key.chars().next()?;
        if m.control {
            return control_byte(key).map(|byte| Cow::Owned(format!("\x1b{}", byte as char)));
        }
        let ch = if m.shift { ch.to_ascii_uppercase() } else { ch };
        return Some(Cow::Owned(format!("\x1b{ch}")));
    }
    None
}

fn control_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match ch.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' | '-' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Unicode key code kitty uses for text-producing and C0 keys.
fn kitty_key_code(key: &str) -> Option<u32> {
    Some(match key {
        "escape" => 27,
        "enter" => 13,
        "tab" => 9,
        "backspace" => 127,
        "space" => 32,
        _ => {
            let mut chars = key.chars();
            let ch = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            ch.to_lowercase().next()? as u32
        }
    })
}

fn encode_kitty(keystroke: &Keystroke, mode: TermMode, kind: KeyEventKind) -> Option<Cow<'static, str>> {
    let report_all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let report_events = mode.contains(TermMode::REPORT_EVENT_TYPES);
    if kind == KeyEventKind::Release && !report_events {
        return None;
    }
    let m = &keystroke.modifiers;
    let mods = modifier_param(keystroke);
    let event = match kind {
        KeyEventKind::Press => "",
        KeyEventKind::Repeat if report_events => ":2",
        KeyEventKind::Repeat => "",
        KeyEventKind::Release => ":3",
    };
    let params = |number: &str| -> String {
        if mods == 1 && event.is_empty() {
            number.to_string()
        } else {
            format!("{number};{mods}{event}")
        }
    };
    let key = keystroke.key.as_str();

    if let Some((number, final_char)) = functional_key(key) {
        let number = if number.is_empty() && (mods != 1 || !event.is_empty()) { "1" } else { number };
        return Some(Cow::Owned(format!("\x1b[{}{final_char}", params(number))));
    }

    let code = kitty_key_code(key)?;
    let is_text_key = !matches!(key, "escape" | "enter" | "tab" | "backspace");
    // In disambiguate-only mode, unmodified text and Enter/Tab/Backspace keep
    // their legacy bytes; text arrives through the input path.
    if !report_all {
        let modified = m.control || m.alt;
        if kind == KeyEventKind::Release {
            return (modified || !is_text_key).then(|| Cow::Owned(format!("\x1b[{}u", params(&code.to_string()))));
        }
        if key == "escape" {
            return Some(Cow::Owned(format!("\x1b[{}u", params("27"))));
        }
        if !modified {
            return if is_text_key { None } else { encode_legacy(keystroke, mode) };
        }
    }
    Some(Cow::Owned(format!("\x1b[{}u", params(&code.to_string()))))
}
