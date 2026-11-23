//! Code colours for chat code blocks: `pierre-dark` in dark mode and
//! `t3-light` in light mode, as `lib/diffRendering.ts` picks them. `t3-light`
//! is `pierre-light` with each syntax colour darkened until it reads on the
//! darkest light code surface (`lib/readableSyntaxTheme.ts`).

use std::{collections::HashMap, sync::LazyLock};

use gpui_kit::{FontStyle, FontWeight, HighlightStyle, Hsla, Rgba};

use super::syntax_theme_data::{PIERRE_DARK, PIERRE_LIGHT};
use crate::theme_palette::Appearance;

/// One theme entry: colour as `0xRRGGBB`, plus weight and slant.
#[derive(Clone, Copy, Debug)]
pub struct TokenStyle {
    pub rgb: Option<u32>,
    pub bold: bool,
    pub italic: bool,
}

/// A syntax theme keyed by Zed highlight name.
pub struct SyntaxTheme {
    styles: HashMap<&'static str, HighlightStyle>,
}

static PIERRE_DARK_THEME: LazyLock<SyntaxTheme> = LazyLock::new(|| SyntaxTheme::new(PIERRE_DARK, |rgb| rgb));
static T3_LIGHT_THEME: LazyLock<SyntaxTheme> =
    LazyLock::new(|| SyntaxTheme::new(PIERRE_LIGHT, readable_syntax_color));

impl SyntaxTheme {
    /// The theme for an appearance.
    pub fn for_appearance(appearance: Appearance) -> &'static Self {
        match appearance {
            Appearance::Dark => &PIERRE_DARK_THEME,
            Appearance::Light => &T3_LIGHT_THEME,
        }
    }

    fn new(entries: &'static [(&'static str, TokenStyle)], map_color: fn(u32) -> u32) -> Self {
        let styles = entries
            .iter()
            .map(|(name, style)| {
                let highlight = HighlightStyle {
                    color: style.rgb.map(|rgb| rgb_to_hsla(map_color(rgb))),
                    font_weight: style.bold.then_some(FontWeight::BOLD),
                    font_style: style.italic.then_some(FontStyle::Italic),
                    ..Default::default()
                };
                (*name, highlight)
            })
            .collect();
        Self { styles }
    }

    /// The style for a highlight name, falling back to its parents the way
    /// Zed does: `function.method.call` → `function.method` → `function`.
    pub fn style(&self, name: &str) -> Option<HighlightStyle> {
        let mut name = name;
        loop {
            if let Some(style) = self.styles.get(name) {
                return Some(*style);
            }
            name = name.rsplit_once('.')?.0;
        }
    }
}

fn rgb_to_hsla(rgb: u32) -> Hsla {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.,
        g: ((rgb >> 8) & 0xff) as f32 / 255.,
        b: (rgb & 0xff) as f32 / 255.,
        a: 1.,
    }
    .into()
}

const MIN_CONTRAST: f32 = 4.5;
/// The darkest light code surface (a changed word on a deleted line), rounded down.
const DARKEST_SURFACE_LUMINANCE: f32 = 0.69;
/// Darker members of colour pairs that would otherwise merge at one floor.
const FURTHER_COLORS: [u32; 3] = [0x1ca1c7, 0x636363, 0x1a85d4];
const FURTHER_MIN_CONTRAST: f32 = 6.;

fn channel_luminance(channel: u32) -> f32 {
    let value = channel as f32 / 255.;
    if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

fn contrast_on_light_code([r, g, b]: [u32; 3]) -> f32 {
    let luminance = 0.2126 * channel_luminance(r) + 0.7152 * channel_luminance(g) + 0.0722 * channel_luminance(b);
    (DARKEST_SURFACE_LUMINANCE + 0.05) / (luminance + 0.05)
}

/// The `t3-light` colour for a `pierre-light` colour (`readableSyntaxColor`).
fn readable_syntax_color(rgb: u32) -> u32 {
    let min = if FURTHER_COLORS.contains(&rgb) { FURTHER_MIN_CONTRAST } else { MIN_CONTRAST };
    let mut channels = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff];
    while contrast_on_light_code(channels) < min {
        channels = channels.map(|c| (c as f32 * 0.96).floor() as u32);
    }
    (channels[0] << 16) | (channels[1] << 8) | channels[2]
}
