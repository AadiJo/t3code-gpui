//! The Contrast setting. Ports `appearanceContrast.ts` and the `--contrast-*`
//! block of `index.css`: below 100 every text role fades toward the surface it
//! sits on and every border role fades toward transparent; above 100 text
//! moves toward white (dark) or black (light) and borders toward the text.

use gpui_kit::{Hsla, black, white};

use crate::theme_color::{mix_oklab_twice, mix_srgb};
use crate::theme_palette::{Appearance, ThemePalette};

/// The `appearanceContrast` client setting, a percentage from 50 to 200.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppearanceContrast(u8);

impl AppearanceContrast {
    pub const MIN: u8 = 50;
    pub const MAX: u8 = 200;

    /// A setting value, clamped to 50..=200 as the settings schema does.
    pub fn new(percent: u32) -> Self {
        Self(percent.clamp(Self::MIN.into(), Self::MAX.into()) as u8)
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    /// `--appearance-contrast-base`: `min(contrast, 100)%`.
    fn base(self) -> f32 {
        f32::from(self.0.min(100)) / 100.
    }

    /// `--appearance-contrast-boost`: `max(contrast - 100, 0)%`.
    fn boost(self) -> f32 {
        f32::from(self.0.saturating_sub(100)) / 100.
    }

    /// `--appearance-contrast-border-boost`: a quarter of the boost.
    fn border_boost(self) -> f32 {
        self.boost() / 4.
    }
}

impl Default for AppearanceContrast {
    /// `DEFAULT_APPEARANCE_CONTRAST`.
    fn default() -> Self {
        Self(100)
    }
}

/// The two `--contrast-*` foregrounds that have no palette role of their own:
/// the theme's `text` recomputed against the card and popover surfaces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContrastForegrounds {
    /// `--contrast-card-foreground`, text on `surface`.
    pub card: Hsla,
    /// `--contrast-popover-foreground`, text on `surfaceOverlay` (menus, popovers, selects).
    pub popover: Hsla,
}

/// Applies the setting to a palette: the roles `index.css` maps to
/// `--contrast-*` variables are replaced, every other role is unchanged.
pub fn apply_contrast(
    raw: &ThemePalette,
    appearance: Appearance,
    contrast: AppearanceContrast,
) -> (ThemePalette, ContrastForegrounds) {
    if contrast == AppearanceContrast::default() {
        // Every `--contrast-*` variable is its source token at 100.
        let foregrounds = ContrastForegrounds { card: raw.text, popover: raw.text };
        return (*raw, foregrounds);
    }
    let (base, boost, border_boost) = (contrast.base(), contrast.boost(), contrast.border_boost());
    let target = if appearance.is_dark() { white() } else { black() };
    let text = |fg: Hsla, on: Hsla| mix_oklab_twice(fg, base, on, target, boost);
    let edge = |border: Hsla, toward: Hsla| {
        mix_srgb(mix_srgb(border, base, Hsla::transparent_black()), 1. - border_boost, toward)
    };
    let palette = ThemePalette {
        text: text(raw.text, raw.canvas),
        muted_foreground: text(raw.muted_foreground, raw.canvas),
        placeholder: text(raw.placeholder, raw.canvas),
        secondary_label: text(raw.secondary_label, raw.canvas),
        icon_muted: text(raw.icon_muted, raw.canvas),
        accent_surface_foreground: text(raw.accent_surface_foreground, raw.accent_surface),
        secondary_foreground: text(raw.secondary_foreground, raw.secondary),
        message_foreground: text(raw.message_foreground, raw.message_surface),
        sidebar_foreground: text(raw.sidebar_foreground, raw.sidebar),
        sidebar_muted_foreground: text(raw.sidebar_muted_foreground, raw.sidebar),
        toolbar_foreground: text(raw.toolbar_foreground, raw.toolbar),
        toolbar_control_foreground: text(raw.toolbar_control_foreground, raw.toolbar_control),
        input: edge(raw.input, raw.text),
        border: edge(raw.border, raw.text),
        sidebar_border: edge(raw.sidebar_border, raw.sidebar_foreground),
        toolbar_border: edge(raw.toolbar_border, raw.toolbar_foreground),
        ..*raw
    };
    let foregrounds = ContrastForegrounds {
        card: text(raw.text, raw.surface),
        popover: text(raw.text, raw.surface_overlay),
    };
    (palette, foregrounds)
}
