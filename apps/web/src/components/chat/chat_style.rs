//! Colours, fonts and type sizes the chat view draws with. Theme roles come
//! from the active [`AppTheme`]; the few colours the web client takes from
//! Tailwind instead of the theme (info, success, diff counts, alert accents)
//! are fixed per appearance, as in `graphite-tokens.json` → `nonThemeable`.
//!
//! The shared UI primitives will own most of this once they exist.

use std::sync::OnceLock;

use gpui_kit::{App, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Rgba, SharedString, px};

use crate::{
    ActiveAppTheme as _,
    theme_palette::{Appearance, ThemePalette},
};

/// The interface font (`Inter Variable`, bundled on Linux and Windows).
pub const SANS_FAMILY: &str = "Inter";
/// The web client's mono stack after a custom code font (`appearanceFonts.ts:17-18`),
/// plus the Linux default; the first one installed wins.
const MONO_STACK: [&str; 6] = ["SF Mono", "SFMono-Regular", "Menlo", "Consolas", "Liberation Mono", "DejaVu Sans Mono"];
static MONO_FAMILY: OnceLock<SharedString> = OnceLock::new();

/// The code font family resolved against installed fonts.
pub fn mono_family() -> SharedString {
    MONO_FAMILY.get().cloned().unwrap_or_else(|| SharedString::new_static(MONO_STACK[4]))
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
        family: mono_family(),
        features: FontFeatures::default(),
        fallbacks: None,
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
    /// The composer card's glass at the user's glass opacity.
    pub composer_glass: crate::theme_tokens::Glass,
}

impl ChatColors {
    pub fn get(cx: &App) -> Self {
        MONO_FAMILY.get_or_init(|| {
            let installed = cx.text_system().all_font_names();
            let family = MONO_STACK.iter().find(|f| installed.iter().any(|i| i == *f)).unwrap_or(&MONO_STACK[5]);
            SharedString::new_static(family)
        });
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
