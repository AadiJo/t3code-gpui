//! With `T3CODE_MATH_DUMP=<file>`, every formula painted appends its glyphs and
//! rules, in window pixels, as JSON lines: how the math scene's run is compared
//! with KaTeX in the fork. Off, it costs one cached environment lookup per formula.

use std::{io::Write as _, sync::OnceLock};

use gpui_kit::{Font, Pixels, Point, Size};
use serde_json::{Value, json};

fn path() -> Option<&'static str> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| std::env::var("T3CODE_MATH_DUMP").ok()).as_deref()
}

pub fn enabled() -> bool {
    path().is_some()
}

/// Appends one record.
pub fn write(record: Value) {
    let Some(path) = path() else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{record}");
    }
}

/// A glyph at `at` from the formula's origin, in the KaTeX face `font` names.
pub(super) fn glyph(text: &str, index: usize, font: &Font, at: Point<Pixels>, size: Pixels) -> Value {
    let face = font.family.trim_start_matches("T3Math_");
    let (family, style) = face.split_once('-').unwrap_or((face, "Regular"));
    json!({
        "kind": "glyph",
        "ch": text[index..].chars().next().map(String::from),
        "family": format!("KaTeX_{family}"),
        "bold": style.contains("Bold"),
        "italic": style.contains("Italic"),
        "x": f32::from(at.x),
        "y": f32::from(at.y),
        "size": f32::from(size),
    })
}

/// A rule, or a path's clip box, from the formula's origin.
pub(super) fn rect(at: Point<Pixels>, size: Size<Pixels>) -> Value {
    json!({ "kind": "rect", "x": f32::from(at.x), "y": f32::from(at.y), "w": f32::from(size.width), "h": f32::from(size.height) })
}
