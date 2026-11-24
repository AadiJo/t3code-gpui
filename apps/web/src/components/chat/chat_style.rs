//! Colours, fonts and type sizes the chat view draws with. Theme roles come
//! from the active [`AppTheme`]; the few colours the web client takes from
//! Tailwind instead of the theme (info, success, diff counts, alert accents)
//! are fixed per appearance, as in `graphite-tokens.json` → `nonThemeable`.
//!
//! The shared UI primitives will own most of this once they exist.

use gpui_kit::{AbsoluteLength, App, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Rgba, SharedString, px, rems};

use crate::{
    ActiveAppTheme as _,
    theme_palette::{Appearance, ThemePalette},
};

/// The interface font (`Inter`, bundled on Linux and Windows).
pub const SANS_FAMILY: &str = "Inter";

/// The code font family, as the Appearance settings resolve it (the owner's custom family included).
pub fn mono_family() -> SharedString {
    crate::appearance_fonts::mono_font(FontWeight::NORMAL).family
}

/// A Tailwind type step: font size and line height. Rem steps resolve against the window's
/// rem (the Interface size); the code step is the Code font size setting.
#[derive(Clone, Copy, Debug)]
pub struct TypeStep {
    pub size: AbsoluteLength,
    pub line_height: AbsoluteLength,
}

const fn step(size: f32, line_height: f32) -> TypeStep {
    TypeStep { size: AbsoluteLength::Rems(rems(size / 16.)), line_height: AbsoluteLength::Rems(rems(line_height / 16.)) }
}

pub const TEXT_3XS: TypeStep = step(10., 14.);
pub const TEXT_2XS: TypeStep = step(11., 16.);
pub const TEXT_XS: TypeStep = step(12., 16.);
pub const TEXT_SM: TypeStep = step(14., 20.);
/// `text-sm leading-relaxed`: chat prose, work-log lines.
pub const TEXT_SM_RELAXED: TypeStep = step(14., 22.75);

/// Code blocks: `--font-size-code` (the Code font size setting) with `leading-snug`.
pub fn text_code(cx: &App) -> TypeStep {
    let size = cx.app_theme().fonts.code;
    TypeStep { size: px(size).into(), line_height: px(size * 1.375).into() }
}

/// The interface font at `weight`.
pub fn sans(weight: FontWeight) -> Font {
    Font { family: SANS_FAMILY.into(), features: FontFeatures::default(), fallbacks: None, weight, style: FontStyle::Normal }
}

/// The code font.
pub fn mono() -> Font {
    crate::appearance_fonts::mono_font(FontWeight::NORMAL)
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
    /// The composer card's glass fill at the user's glass opacity.
    pub composer_glass: Hsla,
    /// The rem against 16px (Interface size / 16): lengths written at a 16px root scale by it.
    pub rem: f32,
}

impl ChatColors {
    /// A length the web writes in rem, given at a 16px root.
    pub fn r(&self, at16: Pixels) -> Pixels {
        at16 * self.rem
    }

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
            composer_glass: theme.derived.composer_glass_opaque,
            rem: theme.fonts.interface / 16.,
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
