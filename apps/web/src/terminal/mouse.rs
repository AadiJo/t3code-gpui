//! Mouse reports for programs that enable mouse tracking (DECSET 1000/1002/1003
//! with 1006 SGR or 1005 UTF-8 coordinates), and alternate-scroll arrow keys.

use alacritty_terminal::term::TermMode;
use gpui_kit::{Modifiers, MouseButton};

/// What happened to the mouse, in the terms a report needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    /// Motion with the held button, if any.
    Move(Option<MouseButton>),
    WheelUp,
    WheelDown,
}

/// Whether a mouse event goes to the program instead of local selection.
/// Shift always keeps the mouse local, like xterm.
pub fn reports_mouse(mode: TermMode, modifiers: Modifiers) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !modifiers.shift
}

/// The report for an action at a viewport cell, or `None` when the mode does not ask for it.
pub fn mouse_report(action: MouseAction, row: usize, col: usize, modifiers: Modifiers, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button_code = |button: MouseButton| match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        _ => None,
    };
    let (code, pressed) = match action {
        MouseAction::Press(button) => (button_code(button)?, true),
        MouseAction::Release(button) => (button_code(button)?, false),
        MouseAction::WheelUp => (64, true),
        MouseAction::WheelDown => (65, true),
        MouseAction::Move(button) => {
            if !mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG) {
                return None;
            }
            // Drag mode (1002) reports motion only while a button is held.
            if button.is_none() && !mode.contains(TermMode::MOUSE_MOTION) {
                return None;
            }
            (32 + button.map_or(Some(3), button_code)?, true)
        }
    };
    let mods = (modifiers.shift as u8) * 4 + (modifiers.alt as u8) * 8 + (modifiers.control as u8) * 16;

    if mode.contains(TermMode::SGR_MOUSE) {
        let final_char = if pressed { 'M' } else { 'm' };
        return Some(format!("\x1b[<{};{};{}{final_char}", code + mods, col + 1, row + 1).into_bytes());
    }

    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let limit = if utf8 { 2015 } else { 223 };
    if row >= limit || col >= limit {
        return None;
    }
    // X10 encoding has no release button; it reports button 3.
    let button = if pressed { code } else { 3 };
    let mut report = vec![0x1b, b'[', b'M', 32 + button + mods];
    for position in [col, row] {
        let value = 32 + 1 + position;
        if utf8 && position >= 95 {
            report.push((0xC0 + value / 64) as u8);
            report.push((0x80 + (value & 63)) as u8);
        } else {
            report.push(value as u8);
        }
    }
    Some(report)
}

/// Arrow keys for wheel motion on the alternate screen when the program asked
/// for alternate scroll (DECSET 1007): pagers and editors scroll their content.
pub fn alternate_scroll(lines: i32, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) || lines == 0 {
        return None;
    }
    let arrow = if lines > 0 { b'A' } else { b'B' };
    let prefix: &[u8] = if mode.contains(TermMode::APP_CURSOR) { b"\x1bO" } else { b"\x1b[" };
    let mut bytes = Vec::with_capacity(lines.unsigned_abs() as usize * 3);
    for _ in 0..lines.unsigned_abs() {
        bytes.extend_from_slice(prefix);
        bytes.push(arrow);
    }
    Some(bytes)
}
