//! The syntax palettes the web app highlights with: `pierre-dark` and
//! `t3-light` (`pierre-light` darkened until it reads on light code,
//! `lib/readableSyntaxTheme.ts`). `pierre_palettes.json` holds both, resolved
//! from `@pierre/highlights`' Zed-style themes with the same prefix fallback
//! the web resolver uses, so every token here has its final colour.

use std::{collections::HashMap, sync::LazyLock};

use gpui_kit::{FontStyle, FontWeight, HighlightStyle, Hsla, Rgba};
use serde::Deserialize;

use crate::theme_palette::Appearance;

use super::SyntaxToken;

#[derive(Deserialize)]
struct TokenStyleJson {
    color: Option<String>,
    #[serde(default)]
    italic: bool,
    weight: Option<u16>,
}

#[derive(Deserialize)]
struct PaletteJson {
    background: String,
    foreground: String,
    created: String,
    deleted: String,
    modified: String,
    syntax: HashMap<String, TokenStyleJson>,
}

#[derive(Deserialize)]
struct PalettesJson {
    #[serde(rename = "pierre-dark")]
    dark: PaletteJson,
    #[serde(rename = "t3-light")]
    light: PaletteJson,
}

/// One appearance's syntax colours, indexed by [`SyntaxToken`].
#[derive(Debug)]
pub struct SyntaxPalette {
    pub background: Hsla,
    pub foreground: Hsla,
    pub created: Hsla,
    pub deleted: Hsla,
    pub modified: Hsla,
    styles: [HighlightStyle; SyntaxToken::ALL.len()],
}

impl SyntaxPalette {
    /// The palette for the app's appearance: `pierre-dark` or `t3-light`.
    pub fn for_appearance(appearance: Appearance) -> &'static SyntaxPalette {
        static PALETTES: LazyLock<(SyntaxPalette, SyntaxPalette)> = LazyLock::new(|| {
            let json: PalettesJson = serde_json::from_str(include_str!("pierre_palettes.json"))
                .expect("embedded syntax palettes parse");
            (SyntaxPalette::from_json(json.dark), SyntaxPalette::from_json(json.light))
        });
        match appearance {
            Appearance::Dark => &PALETTES.0,
            Appearance::Light => &PALETTES.1,
        }
    }

    /// The style for a token. Tokens the theme leaves unset draw in the foreground.
    pub fn style(&self, token: SyntaxToken) -> HighlightStyle {
        self.styles[token as usize]
    }

    fn from_json(json: PaletteJson) -> Self {
        let styles = SyntaxToken::ALL.map(|token| {
            // Zed's theme fallback: `tag.jsx` takes `tag` when it has no style of its own.
            let mut name = token.name();
            let style = loop {
                if let Some(style) = json.syntax.get(name) {
                    break Some(style);
                }
                match name.rfind('.') {
                    Some(dot) => name = &name[..dot],
                    None => break None,
                }
            };
            let Some(style) = style else {
                return HighlightStyle::default();
            };
            HighlightStyle {
                color: style.color.as_deref().and_then(hex),
                font_style: style.italic.then_some(FontStyle::Italic),
                font_weight: style.weight.map(|w| FontWeight(w as f32)),
                ..Default::default()
            }
        });
        Self {
            background: hex(&json.background).unwrap_or_default(),
            foreground: hex(&json.foreground).unwrap_or_default(),
            created: hex(&json.created).unwrap_or_default(),
            deleted: hex(&json.deleted).unwrap_or_default(),
            modified: hex(&json.modified).unwrap_or_default(),
            styles,
        }
    }
}

/// Parses `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn hex(value: &str) -> Option<Hsla> {
    let digits = value.strip_prefix('#')?;
    let expanded: String = match digits.len() {
        3 | 4 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => digits.to_owned(),
        _ => return None,
    };
    let number = u32::from_str_radix(&expanded, 16).ok()?;
    let (rgb, alpha) = if expanded.len() == 8 {
        (number >> 8, (number & 0xff) as f32 / 255.)
    } else {
        (number, 1.)
    };
    let rgba = Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.,
        g: ((rgb >> 8) & 0xff) as f32 / 255.,
        b: (rgb & 0xff) as f32 / 255.,
        a: alpha,
    };
    Some(rgba.into())
}
