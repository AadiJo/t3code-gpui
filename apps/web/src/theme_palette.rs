//! Theme documents and the colour roles the UI draws with. Ports
//! `themePalette.ts`: a theme is a JSON file whose colours are literal CSS
//! colours (canonically `oklch(L C H)`), with an optional variant for the
//! other appearance. Roles a theme omits come from the T3 Chat palette.

use std::sync::Arc;

use gpui_kit::{App, Global, Hsla, Window, WindowAppearance};
use serde::{Deserialize, Deserializer, de::Error as _};

use crate::Assets;
use crate::appearance_contrast::{AppearanceContrast, ContrastForegrounds, apply_contrast};
use crate::appearance_fonts::FontSettings;
use crate::theme_color::{Srgb8, parse_css_color};
use crate::theme_component::apply_component_theme;
use crate::theme_tokens::{DerivedColors, FixedColors, GlassOpacity, Motion, Radii, Shadows, Spacing};

/// Defines [`ThemePalette`] (every role resolved) and [`PaletteOverrides`]
/// (the roles one theme file sets) from one list of roles.
macro_rules! theme_roles {
    ($($field:ident),* $(,)?) => {
        /// Every colour role a theme sets. Field names are the theme JSON keys in snake_case.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct ThemePalette {
            $(pub $field: Hsla,)*
        }

        /// The roles one theme file sets; the rest fall back to a default palette.
        #[derive(Clone, Copy, Debug, Default, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub struct PaletteOverrides {
            $(#[serde(default, deserialize_with = "css_color")] pub $field: Option<Hsla>,)*
        }

        impl PaletteOverrides {
            /// The full palette: these roles over `defaults`.
            pub fn over(&self, defaults: &ThemePalette) -> ThemePalette {
                ThemePalette { $($field: self.$field.unwrap_or(defaults.$field),)* }
            }

            /// The palette when every role is set, else `None`.
            fn complete(&self) -> Option<ThemePalette> {
                Some(ThemePalette { $($field: self.$field?,)* })
            }
        }
    };
}

theme_roles! {
    canvas, chrome, toolbar, toolbar_foreground, toolbar_border, toolbar_control,
    toolbar_control_foreground, toolbar_control_hover, surface, surface_raised,
    surface_overlay, text, text_muted, border, input, focus, accent, accent_foreground,
    secondary, secondary_foreground, muted, muted_foreground, placeholder, secondary_label,
    icon_muted, error, error_foreground, error_surface, warning, warning_foreground,
    warning_surface, update, update_foreground, update_surface, accent_surface,
    accent_surface_foreground, message_surface, message_foreground, message_action,
    message_action_foreground, message_action_hover, code_background, code_foreground,
    search_match_background, search_match_foreground, search_match_active_background,
    search_match_active_foreground, sidebar, sidebar_foreground, sidebar_muted_foreground,
    sidebar_control_surface, sidebar_row_hover, sidebar_row_active, sidebar_row_selected,
    sidebar_border, terminal_background, terminal_foreground, terminal_cursor,
    terminal_selection, terminal_scrollbar, terminal_scrollbar_hover,
}

/// Light or dark: which half of a theme is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub fn is_dark(self) -> bool {
        self == Self::Dark
    }
}

impl From<WindowAppearance> for Appearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
        }
    }
}

/// The appearance setting: a fixed half, or whatever the OS uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
    #[default]
    System,
}

/// The resolved palettes of the other appearance, when a theme has one.
#[derive(Clone, Debug)]
pub struct ThemeVariants {
    pub dark: Option<ThemePalette>,
    pub light: Option<ThemePalette>,
}

/// A theme file, parsed with `parseThemeFile`'s rules: version 1, a name, a
/// base appearance, and each palette completed from the T3 Chat defaults.
#[derive(Clone, Debug)]
pub struct ThemeDocument {
    pub id: String,
    pub name: String,
    pub appearance: Appearance,
    pub colors: ThemePalette,
    pub variants: Option<ThemeVariants>,
}

/// Why a theme file was rejected.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("theme JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("theme files must be version 1")]
    Version,
    #[error("theme files need a name of 48 characters or fewer")]
    Name,
    #[error("theme ids may only contain lowercase letters, numbers and hyphens")]
    Id,
    #[error("theme variants must not repeat the base appearance")]
    RepeatedVariant,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawThemeDocument {
    version: u32,
    id: Option<String>,
    name: String,
    appearance: Appearance,
    colors: PaletteOverrides,
    #[serde(default)]
    variants: RawVariants,
}

#[derive(Default, Deserialize)]
struct RawVariants {
    dark: Option<PaletteOverrides>,
    light: Option<PaletteOverrides>,
}

impl ThemeDocument {
    /// Parses a theme file. Roles it omits come from the T3 Chat palette.
    pub fn parse(json: &[u8]) -> Result<Self, ThemeError> {
        let raw: RawThemeDocument = serde_json::from_slice(json)?;
        let name = raw.name.trim();
        if raw.version != 1 {
            return Err(ThemeError::Version);
        }
        if name.is_empty() || name.chars().count() > 48 {
            return Err(ThemeError::Name);
        }
        let id = raw.id.unwrap_or_else(|| theme_id_from_name(name));
        let valid_id = !id.is_empty()
            && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid_id {
            return Err(ThemeError::Id);
        }
        let repeats = match raw.appearance {
            Appearance::Dark => raw.variants.dark.is_some(),
            Appearance::Light => raw.variants.light.is_some(),
        };
        if repeats {
            return Err(ThemeError::RepeatedVariant);
        }
        let dark = raw.variants.dark.map(|o| o.over(&default_palette(Appearance::Dark)));
        let light = raw.variants.light.map(|o| o.over(&default_palette(Appearance::Light)));
        Ok(Self {
            id,
            name: name.to_owned(),
            appearance: raw.appearance,
            colors: raw.colors.over(&default_palette(raw.appearance)),
            variants: (dark.is_some() || light.is_some()).then_some(ThemeVariants { dark, light }),
        })
    }

    /// The bundled Graphite theme.
    pub fn graphite() -> Self {
        let bytes = Assets::get("themes/graphite.json").expect("graphite theme is embedded");
        Self::parse(&bytes).expect("graphite theme parses")
    }

    /// The palette for an appearance, or `None` when the theme has no such half.
    pub fn palette_for(&self, appearance: Appearance) -> Option<ThemePalette> {
        if appearance == self.appearance {
            return Some(self.colors);
        }
        let variants = self.variants.as_ref()?;
        match appearance {
            Appearance::Dark => variants.dark,
            Appearance::Light => variants.light,
        }
    }

    /// The palette for an appearance, or the base colours when that half is missing.
    pub fn palette(&self, appearance: Appearance) -> ThemePalette {
        self.palette_for(appearance).unwrap_or(self.colors)
    }

    /// The appearance actually drawn for a mode (`resolveThemeAppearance`):
    /// a single-appearance theme always draws its own half.
    pub fn resolve_appearance(&self, mode: ThemeMode, system: Appearance) -> Appearance {
        let wanted = match mode {
            ThemeMode::Dark => Appearance::Dark,
            ThemeMode::Light => Appearance::Light,
            ThemeMode::System => system,
        };
        if self.palette_for(wanted).is_some() { wanted } else { self.appearance }
    }
}

/// `themeIdFromName`: lowercase, runs of other characters become one hyphen.
fn theme_id_from_name(name: &str) -> String {
    let mut id = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            id.push(c);
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').to_owned()
}

/// The T3 Chat palette for an appearance (`getDefaultThemeColors`).
pub fn default_palette(appearance: Appearance) -> ThemePalette {
    #[derive(Deserialize)]
    struct Defaults {
        colors: PaletteOverrides,
        variants: RawVariants,
    }
    let bytes = Assets::get("themes/t3-chat.json").expect("t3 chat theme is embedded");
    let defaults: Defaults = serde_json::from_slice(&bytes).expect("t3 chat theme parses");
    let overrides = match appearance {
        Appearance::Light => Some(defaults.colors),
        Appearance::Dark => defaults.variants.dark,
    };
    overrides
        .and_then(|o| o.complete())
        .expect("t3 chat theme sets every role in both appearances")
}

/// The theme the app is drawing with: one appearance of one theme document,
/// plus every value derived from it. Read it through [`ActiveAppTheme`].
#[derive(Clone, Debug)]
pub struct AppTheme {
    pub id: String,
    pub name: String,
    pub appearance: Appearance,
    /// The palette with the Contrast setting applied: what every view draws with.
    pub colors: ThemePalette,
    /// The theme file's palette before the Contrast setting.
    pub raw_colors: ThemePalette,
    /// Text recomputed against the card and popover surfaces for the Contrast setting.
    pub foregrounds: ContrastForegrounds,
    pub contrast: AppearanceContrast,
    /// The stylesheet's `color-mix` and opacity results for this palette.
    pub derived: DerivedColors,
    /// Tailwind palette colours the theme cannot change.
    pub fixed: FixedColors,
    pub fonts: FontSettings,
    /// The Glass opacity setting every glass fill is derived from.
    pub glass_opacity: GlassOpacity,
    pub radii: Radii,
    pub shadows: Shadows,
    pub spacing: Spacing,
    pub motion: Motion,
}

impl Global for AppTheme {}

impl AppTheme {
    /// One appearance of a theme with the default font settings.
    pub fn new(document: &ThemeDocument, appearance: Appearance) -> Self {
        Self::with_fonts(document, appearance, FontSettings::default())
    }

    /// One appearance of a theme with the given font settings.
    pub fn with_fonts(document: &ThemeDocument, appearance: Appearance, fonts: FontSettings) -> Self {
        Self::with_settings(document, appearance, fonts, GlassOpacity::default())
    }

    /// One appearance of a theme with the given font and glass settings.
    pub fn with_settings(
        document: &ThemeDocument,
        appearance: Appearance,
        fonts: FontSettings,
        glass_opacity: GlassOpacity,
    ) -> Self {
        Self::from_settings(&ThemeSettings {
            document: Arc::new(document.clone()),
            mode: match appearance {
                Appearance::Dark => ThemeMode::Dark,
                Appearance::Light => ThemeMode::Light,
            },
            system: appearance,
            fonts,
            glass_opacity,
            ..ThemeSettings::default()
        })
    }

    /// The theme every setting in `settings` describes.
    pub fn from_settings(settings: &ThemeSettings) -> Self {
        let document = &settings.document;
        let appearance = document.resolve_appearance(settings.mode, settings.system);
        let raw_colors = document.palette(appearance);
        let (colors, foregrounds) = apply_contrast(&raw_colors, appearance, settings.contrast);
        let fonts = settings.fonts.clamped();
        let glass_opacity = settings.glass_opacity;
        Self {
            id: document.id.clone(),
            name: document.name.clone(),
            appearance,
            colors,
            raw_colors,
            foregrounds,
            contrast: settings.contrast,
            derived: DerivedColors::new(&colors, appearance, glass_opacity),
            fixed: FixedColors::new(appearance),
            fonts,
            glass_opacity,
            radii: Radii::new(fonts.interface),
            shadows: Shadows::new(appearance),
            spacing: Spacing::new(fonts.interface),
            motion: Motion { panel: settings.panel_animation, ..Motion::default() },
        }
    }

    pub fn is_dark(&self) -> bool {
        self.appearance.is_dark()
    }

    /// `n` root ems in pixels. Tailwind sizes are rem-based, so every metric
    /// scales with the Interface size setting.
    pub fn rem(&self, n: f32) -> gpui_kit::Pixels {
        gpui_kit::px(n * self.fonts.interface)
    }

    /// Tailwind's spacing unit: `--spacing(n)` = n x 0.25rem.
    pub fn sp(&self, n: f32) -> gpui_kit::Pixels {
        self.rem(n * 0.25)
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

/// The user's theme choice and what the OS reports. Changing it rebuilds
/// [`AppTheme`] and refreshes every window.
#[derive(Clone, Debug)]
pub struct ThemeSettings {
    pub document: Arc<ThemeDocument>,
    pub mode: ThemeMode,
    pub system: Appearance,
    pub fonts: FontSettings,
    /// The Glass opacity client setting (`glassOpacity`, 40 to 100, default 80).
    pub glass_opacity: GlassOpacity,
    /// The Contrast client setting (`appearanceContrast`, 50 to 200, default 100).
    pub contrast: AppearanceContrast,
    /// The Panel animations client setting (`panelAnimationDurationMs`, 0 to 400).
    pub panel_animation: std::time::Duration,
}

impl Global for ThemeSettings {}

impl Default for ThemeSettings {
    /// Graphite, dark, with every client setting at its default.
    fn default() -> Self {
        Self {
            document: Arc::new(ThemeDocument::graphite()),
            mode: ThemeMode::System,
            system: Appearance::Dark,
            fonts: FontSettings::default(),
            glass_opacity: GlassOpacity::default(),
            contrast: AppearanceContrast::default(),
            panel_animation: std::time::Duration::from_millis(DEFAULT_PANEL_ANIMATION_MS),
        }
    }
}

/// `DEFAULT_PANEL_ANIMATION_DURATION_MS` in `packages/contracts/src/settings.ts`.
pub const DEFAULT_PANEL_ANIMATION_MS: u64 = 0;

impl ThemeSettings {
    /// Applies a change to the theme settings, then rebuilds the theme.
    pub fn update(cx: &mut App, change: impl FnOnce(&mut ThemeSettings)) {
        let mut settings = cx.global::<ThemeSettings>().clone();
        change(&mut settings);
        install(settings, cx);
    }

    /// Switches dark, light, or follow-system at runtime.
    pub fn set_mode(mode: ThemeMode, cx: &mut App) {
        Self::update(cx, |settings| settings.mode = mode);
    }

    /// Applies the Glass opacity client setting; every glass fill follows it.
    pub fn set_glass_opacity(glass_opacity: GlassOpacity, cx: &mut App) {
        Self::update(cx, |settings| settings.glass_opacity = glass_opacity);
    }

    /// Applies the owner-setting overrides the headless tools take from the
    /// environment: `T3CODE_CONTRAST`, `T3CODE_GLASS_OPACITY`,
    /// `T3CODE_INTERFACE_SIZE` and `T3CODE_PANEL_MS`. Call after opening the
    /// window so its rem follows the Interface size. Captures show settled
    /// frames, so motion is off unless `T3CODE_MOTION=1`.
    pub fn apply_env_overrides(cx: &mut App) {
        cx.set_reduce_motion(std::env::var("T3CODE_MOTION").map_or(true, |v| v != "1"));
        let var = |name: &str| std::env::var(name).ok().and_then(|v| v.trim().parse::<u32>().ok());
        let (contrast, glass, interface, panel) =
            (var("T3CODE_CONTRAST"), var("T3CODE_GLASS_OPACITY"), var("T3CODE_INTERFACE_SIZE"), var("T3CODE_PANEL_MS"));
        Self::update(cx, |settings| {
            if let Some(contrast) = contrast {
                settings.contrast = AppearanceContrast::new(contrast);
            }
            if let Some(glass) = glass {
                settings.glass_opacity = GlassOpacity::new(glass.min(100) as u8);
            }
            if let Some(interface) = interface {
                settings.fonts.interface = interface as f32;
            }
            if let Some(panel) = panel {
                settings.panel_animation = std::time::Duration::from_millis(panel.min(400).into());
            }
        });
    }

    /// Pins one appearance of the current theme, keeping every other setting.
    pub fn set_appearance(appearance: Appearance, cx: &mut App) {
        let mode = match appearance {
            Appearance::Dark => ThemeMode::Dark,
            Appearance::Light => ThemeMode::Light,
        };
        Self::set_mode(mode, cx);
    }

    /// Replaces the theme document, keeping the mode.
    pub fn set_document(document: ThemeDocument, cx: &mut App) {
        Self::update(cx, |settings| settings.document = Arc::new(document));
    }

    /// Tracks a window's OS appearance so follow-system mode switches with it,
    /// and sets the window's rem to the Interface size. Call once per window
    /// after it opens.
    pub fn follow_window(window: &mut Window, cx: &mut App) {
        window.set_rem_size(gpui_kit::px(cx.global::<ThemeSettings>().fonts.clamped().interface));
        let system = window.appearance().into();
        if cx.global::<ThemeSettings>().system != system {
            Self::update(cx, |settings| settings.system = system);
        }
        window
            .observe_window_appearance(|window, cx| {
                let system = window.appearance().into();
                ThemeSettings::update(cx, |settings| settings.system = system);
            })
            .detach();
    }
}

/// Installs settings as the app-wide theme, mirrored onto gpui-component's theme.
/// Every open window's rem follows the Interface size, as the web's root font size does.
pub fn install(settings: ThemeSettings, cx: &mut App) {
    let theme = AppTheme::from_settings(&settings);
    let rem = gpui_kit::px(theme.fonts.interface);
    cx.set_global(settings);
    for window in cx.windows() {
        _ = window.update(cx, |_, window, _| window.set_rem_size(rem));
    }
    set_app_theme(theme, cx);
}

/// Installs a theme as the app-wide theme.
pub fn set_app_theme(theme: AppTheme, cx: &mut App) {
    apply_component_theme(&theme, cx);
    cx.set_global(theme);
    cx.refresh_windows();
}

/// Parses a literal CSS colour into a GPUI colour, using the app's own
/// OKLCH conversion so each role lands on the same 8-bit value as the browser.
pub fn parse_oklch(value: &str) -> Option<Hsla> {
    parse_css_color(value).map(Srgb8::hsla)
}

fn css_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Hsla>, D::Error> {
    let value = String::deserialize(deserializer)?;
    parse_oklch(&value)
        .map(Some)
        .ok_or_else(|| D::Error::custom(format!("not a literal CSS colour: {value}")))
}
