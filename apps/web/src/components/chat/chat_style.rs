//! Colours, fonts and type sizes the chat view draws with. Theme roles come
//! from the active [`AppTheme`]; the few colours the web client takes from
//! Tailwind instead of the theme (info, success, diff counts, alert accents)
//! are fixed per appearance, as in `graphite-tokens.json` → `nonThemeable`.
//!
//! The shared UI primitives will own most of this once they exist.

use gpui_kit::{App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Rgba, px};

use crate::{
    ActiveAppTheme as _,
    theme_palette::{Appearance, ThemePalette},
};

/// The interface font (`Inter Variable`, bundled on Linux and Windows).
pub const SANS_FAMILY: &str = "Inter";
/// The first mono family of the web stack that exists here, then fallbacks.
pub const MONO_FAMILY: &str = "Liberation Mono";
const MONO_FALLBACKS: [&str; 4] = ["SF Mono", "Menlo", "Consolas", "DejaVu Sans Mono"];

/// A Tailwind type step: font size and line height, in pixels at a 16px root.
#[derive(Clone, Copy, Debug)]
pub struct TypeStep {
    pub size: Pixels,
    pub line_height: Pixels,
}

pub const TEXT_3XS: TypeStep = TypeStep { size: px(10.), line_height: px(14.) };
pub const TEXT_2XS: TypeStep = TypeStep { size: px(11.), line_height: px(16.) };
pub const TEXT_XS: TypeStep = TypeStep { size: px(12.), line_height: px(16.) };
pub const TEXT_SM: TypeStep = TypeStep { size: px(14.), line_height: px(20.) };
/// `text-sm leading-relaxed`: chat prose, work-log lines.
pub const TEXT_SM_RELAXED: TypeStep = TypeStep { size: px(14.), line_height: px(22.75) };
/// Code blocks: `--font-size-code` 13px with `leading-snug`.
pub const TEXT_CODE: TypeStep = TypeStep { size: px(13.), line_height: px(17.875) };

/// The interface font at `weight`.
pub fn sans(weight: FontWeight) -> Font {
    Font {
        family: SANS_FAMILY.into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

/// The code font.
pub fn mono() -> Font {
    Font {
        family: MONO_FAMILY.into(),
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(MONO_FALLBACKS.iter().map(|f| f.to_string()).collect())),
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

/// `0xRRGGBB` as a colour.
pub fn rgb_hsla(rgb: u32) -> Hsla {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.,
        g: ((rgb >> 8) & 0xff) as f32 / 255.,
        b: (rgb & 0xff) as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// `color` at `alpha` (Tailwind's `/NN` suffix).
pub fn alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: color.a * alpha, ..color }
}

/// The resolved palette for chat surfaces.
#[derive(Clone, Copy, Debug)]
pub struct ChatColors {
    pub appearance: Appearance,
    pub theme: ThemePalette,
    pub info: Hsla,
    pub info_foreground: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub diff_addition: Hsla,
    pub diff_deletion: Hsla,
    pub guest: Hsla,
    pub selection: Hsla,
    pub scrollbar: Hsla,
    pub scrollbar_hover: Hsla,
}

impl ChatColors {
    pub fn get(cx: &App) -> Self {
        let theme = cx.app_theme();
        let dark = theme.appearance == Appearance::Dark;
        let pick = |dark_rgb: u32, light_rgb: u32| rgb_hsla(if dark { dark_rgb } else { light_rgb });
        Self {
            appearance: theme.appearance,
            theme: theme.colors,
            info: rgb_hsla(0x3280ff),
            info_foreground: pick(0x56a2ff, 0x1447e6),
            success: rgb_hsla(0x00b981),
            success_foreground: pick(0x00d294, 0x007857),
            diff_addition: pick(0x00d294, 0x007857),
            diff_deletion: pick(0xff6568, 0xbf000f),
            guest: rgb_hsla(0xe12afb),
            selection: alpha(theme.colors.focus, 0.32),
            scrollbar: if dark { alpha(rgb_hsla(0xffffff), 0.36) } else { rgb_hsla(0xd9d9d9) },
            scrollbar_hover: if dark { alpha(rgb_hsla(0xffffff), 0.45) } else { rgb_hsla(0xbfbfbf) },
        }
    }

    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }

    /// Chat prose colour: `text-foreground/90`.
    pub fn prose(&self) -> Hsla {
        alpha(self.theme.text, 0.9)
    }

    /// `destructive` / `error`.
    pub fn destructive(&self) -> Hsla {
        self.theme.error
    }

    /// The code-block frame fill: `codeBackground` under Graphite.
    pub fn code_background(&self) -> Hsla {
        self.theme.code_background
    }

    /// Tailwind accent colours for GitHub alerts: (border at 70%, title).
    pub fn alert(&self, kind: crate::markdown_pipeline::AlertKind) -> (Hsla, Hsla) {
        use crate::markdown_pipeline::AlertKind::*;
        let dark = self.is_dark();
        let (border, title_dark, title_light) = match kind {
            Insight => return (self.info, self.info),
            Note => (0x3080ff, 0x54a2ff, 0x155dfc),
            Tip => (0x00bc7d, 0x00d294, 0x009966),
            Important => (0xad46ff, 0xc27aff, 0x9810fa),
            Warning => (0xfe9a00, 0xfe9a00, 0xe17100),
            Caution => (0xfb2c36, 0xff6467, 0xe7000b),
        };
        (alpha(rgb_hsla(border), 0.7), rgb_hsla(if dark { title_dark } else { title_light }))
    }
}
