//! Colours, fonts and type sizes the chat view draws with. Theme roles come
//! from the active [`AppTheme`]; the few colours the web client takes from
//! Tailwind instead of the theme (info, success, diff counts, alert accents)
//! are fixed per appearance, as in `graphite-tokens.json` → `nonThemeable`.
//!
//! The shared UI primitives will own most of this once they exist.

use gpui_kit::{App, Font, FontWeight, Hsla, Pixels, Rgba, SharedString, px};

use crate::{
    ActiveAppTheme as _, appearance_fonts,
    theme_palette::{Appearance, ThemePalette},
};

/// The code font family, from the code font setting and the web's mono stack.
pub fn mono_family() -> SharedString {
    mono().family
}

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

/// The interface font at `weight`, as the font family setting resolves it.
pub fn sans(weight: FontWeight) -> Font {
    appearance_fonts::ui_font(weight)
}

/// The code font, as the code font setting resolves it.
pub fn mono() -> Font {
    appearance_fonts::mono_font(FontWeight::NORMAL)
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
    /// Code blocks: the code font size setting (`--font-size-code`) with `leading-snug`.
    pub code: TypeStep,
    /// The composer card's glass at the user's glass opacity.
    pub composer_glass: crate::theme_tokens::Glass,
}

impl ChatColors {
    pub fn get(cx: &App) -> Self {
        let theme = cx.app_theme();
        let dark = theme.appearance == Appearance::Dark;
        Self {
            appearance: theme.appearance,
            theme: theme.colors,
            info: theme.fixed.info,
            info_foreground: theme.fixed.info_foreground,
            success: theme.fixed.success,
            success_foreground: theme.fixed.success_foreground,
            diff_addition: theme.fixed.diff_addition_foreground,
            diff_deletion: theme.fixed.diff_deletion_foreground,
            guest: theme.fixed.guest,
            selection: alpha(theme.colors.focus, 0.32),
            scrollbar: if dark { alpha(rgb_hsla(0xffffff), 0.36) } else { rgb_hsla(0xd9d9d9) },
            scrollbar_hover: if dark { alpha(rgb_hsla(0xffffff), 0.45) } else { rgb_hsla(0xbfbfbf) },
            composer_glass: theme.derived.glass_composer,
            code: TypeStep { size: px(theme.fonts.code), line_height: px(theme.fonts.code * 1.375) },
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

/// The stroke width the chat draws `icon` at: work-log tool glyphs use 2
/// (`TimelineToolActivityIcon.tsx`), everything else the app's 1.75.
pub fn chat_icon_stroke(icon: crate::components::icons::IconName) -> f32 {
    use crate::components::icons::IconName::*;
    match icon {
        Bot | Brain | CodeXml | Eye | GitPullRequest | Globe | Hammer | ListTodo | MessageCircle | Search | Smartphone
        | SquarePen | Terminal | Wrench | Zap => 2.,
        _ => 1.75,
    }
}
