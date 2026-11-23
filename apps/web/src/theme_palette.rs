//! Theme documents and the colour roles the UI draws with. Ports
//! `themePalette.ts`: a theme is a JSON file whose colours are `oklch(L C H)`
//! strings, with an optional variant for the other appearance.

use gpui_kit::{App, Global, Hsla, component::oklch};
use serde::{Deserialize, Deserializer, de::Error as _};

use crate::Assets;

/// Every colour role a theme sets. Field names are the theme JSON keys in snake_case.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePalette {
    #[serde(deserialize_with = "oklch_color")]
    pub canvas: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub chrome: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub toolbar: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub toolbar_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub toolbar_border: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub toolbar_control: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub toolbar_control_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub toolbar_control_hover: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub surface_raised: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub surface_overlay: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub text: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub text_muted: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub border: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub input: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub focus: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub accent: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub accent_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub secondary: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub secondary_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub muted: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub muted_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub placeholder: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub secondary_label: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub icon_muted: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub error: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub error_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub error_surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub warning: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub warning_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub warning_surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub update: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub update_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub update_surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub accent_surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub accent_surface_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub message_surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub message_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub message_action: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub message_action_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub message_action_hover: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub code_background: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub code_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub search_match_background: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub search_match_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub search_match_active_background: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub search_match_active_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_muted_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_control_surface: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_row_hover: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_row_active: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_row_selected: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub sidebar_border: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub terminal_background: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub terminal_foreground: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub terminal_cursor: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub terminal_selection: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub terminal_scrollbar: Hsla,
    #[serde(deserialize_with = "oklch_color")]
    pub terminal_scrollbar_hover: Hsla,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ThemeVariants {
    pub dark: Option<ThemePalette>,
    pub light: Option<ThemePalette>,
}

/// A theme file as written on disk.
#[derive(Clone, Debug, Deserialize)]
pub struct ThemeDocument {
    pub id: String,
    pub name: String,
    pub appearance: Appearance,
    pub colors: ThemePalette,
    pub variants: Option<ThemeVariants>,
}

impl ThemeDocument {
    /// The bundled Graphite theme.
    pub fn graphite() -> Self {
        let bytes = Assets::get("themes/graphite.json").expect("graphite theme is embedded");
        serde_json::from_slice(&bytes).expect("graphite theme parses")
    }

    /// The palette for an appearance: the base colours, or the matching variant.
    pub fn palette(&self, appearance: Appearance) -> ThemePalette {
        if appearance == self.appearance {
            return self.colors;
        }
        let variant = self.variants.as_ref().and_then(|v| match appearance {
            Appearance::Dark => v.dark,
            Appearance::Light => v.light,
        });
        variant.unwrap_or(self.colors)
    }
}

/// The theme the app is drawing with. Read it through [`ActiveAppTheme`].
#[derive(Clone, Debug)]
pub struct AppTheme {
    pub id: String,
    pub name: String,
    pub appearance: Appearance,
    pub colors: ThemePalette,
}

impl Global for AppTheme {}

impl AppTheme {
    pub fn new(document: &ThemeDocument, appearance: Appearance) -> Self {
        Self {
            id: document.id.clone(),
            name: document.name.clone(),
            appearance,
            colors: document.palette(appearance),
        }
    }
}

/// `cx.app_theme()` from any context that derefs to [`App`].
pub trait ActiveAppTheme {
    fn app_theme(&self) -> &AppTheme;
}

impl ActiveAppTheme for App {
    fn app_theme(&self) -> &AppTheme {
        self.global::<AppTheme>()
    }
}

/// Installs a theme as the app-wide theme.
pub fn set_app_theme(theme: AppTheme, cx: &mut App) {
    cx.set_global(theme);
}

/// Parses `oklch(L C H)` where L is 0..1 or a percentage.
pub fn parse_oklch(value: &str) -> Option<Hsla> {
    let inner = value.trim().strip_prefix("oklch(")?.strip_suffix(')')?;
    let mut parts = inner.split_whitespace();
    let lightness = parts.next()?;
    let lightness = match lightness.strip_suffix('%') {
        Some(percent) => percent.parse::<f32>().ok()? / 100.0,
        None => lightness.parse::<f32>().ok()?,
    };
    let chroma = parts.next()?.parse::<f32>().ok()?;
    let hue = parts.next()?.parse::<f32>().ok()?;
    Some(oklch(lightness, chroma, hue))
}

fn oklch_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Hsla, D::Error> {
    let value = String::deserialize(deserializer)?;
    parse_oklch(&value).ok_or_else(|| D::Error::custom(format!("not an oklch colour: {value}")))
}
