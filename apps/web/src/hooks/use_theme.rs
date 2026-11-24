//! The theme choice. Ports `hooks/useTheme.ts` and the theme library parts
//! of `themePalette.ts`: the chosen theme, the appearance mode, separate
//! light and dark halves, and custom themes, each in its web `localStorage`
//! key (here [`LocalStorage`]). [`ThemeStore`] turns them, plus the font
//! settings in [`ClientSettingsStore`], into [`ThemeSettings`].

use std::sync::{Arc, LazyLock};

use gpui_kit::{App, Global, SharedString, px};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Assets;
use crate::appearance_fonts::{FamilyOverrides, FontSettings, set_family_overrides};
use crate::client_persistence_storage::LocalStorage;
use crate::hooks::use_settings::{ClientSettings, ClientSettingsStore, ServerSettingsStore};
use crate::theme_palette::{Appearance, ThemeDocument, ThemeError, ThemeMode, ThemeSettings, ThemeVariants};

/// `t3code:theme`: `system`, `light`, `dark`, or a theme id.
pub const THEME_STORAGE_KEY: &str = "t3code:theme";
/// `t3code:theme-appearance-mode`: `system`, `light` or `dark`.
pub const APPEARANCE_MODE_STORAGE_KEY: &str = "t3code:theme-appearance-mode";
/// `t3code:theme-halves:v1`: `{ light?: id, dark?: id }`.
pub const THEME_HALVES_STORAGE_KEY: &str = "t3code:theme-halves:v1";
/// `t3code:themes:v1`: the custom theme files.
pub const CUSTOM_THEMES_STORAGE_KEY: &str = "t3code:themes:v1";
/// `t3code:typography-advanced`: the "Separate fonts" switch.
pub const TYPOGRAPHY_ADVANCED_STORAGE_KEY: &str = "t3code:typography-advanced";

/// The preference ids that mean the standard T3 Code theme.
const STANDARD_PREFERENCES: [&str; 3] = ["system", "light", "dark"];
/// This port's default theme: the owner's Graphite.
pub const DEFAULT_THEME_ID: &str = "graphite";

/// Theme ids the web renamed (`LEGACY_THEME_ID_ALIASES`).
fn canonical_theme_id(id: &str) -> &str {
    match id {
        "t3-chat-dark" => "t3-chat",
        "t3-grove" => "grove",
        "t3-ocean" => "ocean",
        "t3-ember" => "ember",
        "t3-iris" => "iris",
        other => other,
    }
}

/// Where a theme in the picker comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    BuiltIn,
    Custom,
    /// Published by a connected environment.
    Environment,
}

/// One theme the picker can show.
#[derive(Clone, Debug)]
pub struct CatalogTheme {
    pub document: Arc<ThemeDocument>,
    pub source: ThemeSource,
}

impl CatalogTheme {
    pub fn id(&self) -> &str {
        &self.document.id
    }

    /// Light, dark, or both, in that order.
    pub fn modes(&self) -> Vec<Appearance> {
        [Appearance::Light, Appearance::Dark]
            .into_iter()
            .filter(|mode| self.document.palette_for(*mode).is_some())
            .collect()
    }
}

const BUILT_IN_FILES: [&str; 7] = ["t3-code", "t3-chat", "grove", "ocean", "ember", "iris", "graphite"];

/// `BUILT_IN_THEMES`, with the standard T3 Code theme first. Graphite,
/// which this port bundles and the web imports as a custom theme, comes
/// last, where the web lists custom themes.
static BUILT_IN: LazyLock<Vec<CatalogTheme>> = LazyLock::new(|| {
    BUILT_IN_FILES
        .iter()
        .map(|name| {
            let bytes = Assets::get(&format!("themes/{name}.json")).expect("built-in theme is embedded");
            let document = ThemeDocument::parse(&bytes).expect("built-in theme parses");
            CatalogTheme { document: Arc::new(document), source: ThemeSource::BuiltIn }
        })
        .collect()
});

/// The standard theme drawn for `system`, `light` and `dark`.
pub fn standard_theme() -> &'static CatalogTheme {
    &BUILT_IN[0]
}

/// `ThemeHalves`: a theme id per appearance, overriding the chosen theme's half.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeHalves {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark: Option<String>,
}

impl ThemeHalves {
    pub fn get(&self, appearance: Appearance) -> Option<&str> {
        match appearance {
            Appearance::Light => self.light.as_deref(),
            Appearance::Dark => self.dark.as_deref(),
        }
    }

    fn slot(&mut self, appearance: Appearance) -> &mut Option<String> {
        match appearance {
            Appearance::Light => &mut self.light,
            Appearance::Dark => &mut self.dark,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.light.is_none() && self.dark.is_none()
    }
}

/// A custom theme: the parsed document and the JSON it came from, which is
/// what gets saved.
#[derive(Clone, Debug)]
struct CustomTheme {
    document: Arc<ThemeDocument>,
    source: Value,
}

/// The theme choice. See the module docs.
pub struct ThemeStore {
    preference: SharedString,
    appearance_mode: ThemeMode,
    halves: ThemeHalves,
    custom: Vec<CustomTheme>,
    /// What [`Self::apply`] last pushed, to skip rebuilding the theme when
    /// an unrelated client setting changes.
    applied: Option<(FontSettings, FamilyOverrides)>,
}

impl Global for ThemeStore {}

/// Why a theme could not be added.
#[derive(Debug, thiserror::Error)]
pub enum ThemeImportError {
    #[error(transparent)]
    Invalid(#[from] ThemeError),
    #[error("A built-in theme already uses the id \"{0}\".")]
    ReservedId(String),
}

impl ThemeStore {
    /// Reads the theme keys from [`LocalStorage`], applies them, and keeps
    /// the theme in step with the font settings. Call after
    /// [`ClientSettingsStore::init`].
    pub fn init(cx: &mut App) {
        let preference: String = LocalStorage::get(cx, THEME_STORAGE_KEY).unwrap_or_else(|| DEFAULT_THEME_ID.into());
        let appearance_mode = LocalStorage::get::<String>(cx, APPEARANCE_MODE_STORAGE_KEY)
            .and_then(|mode| parse_mode(&mode))
            .unwrap_or_else(|| match preference.as_str() {
                "light" => ThemeMode::Light,
                "dark" => ThemeMode::Dark,
                _ => ThemeMode::System,
            });
        let custom = LocalStorage::get::<Vec<Value>>(cx, CUSTOM_THEMES_STORAGE_KEY)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|source| {
                let document = ThemeDocument::parse(source.to_string().as_bytes()).ok()?;
                Some(CustomTheme { document: Arc::new(document), source })
            })
            .collect();
        let halves = LocalStorage::get(cx, THEME_HALVES_STORAGE_KEY).unwrap_or_default();
        let first = !cx.has_global::<Self>();
        cx.set_global(Self {
            preference: canonical_theme_id(&preference).to_owned().into(),
            appearance_mode,
            halves,
            custom,
            applied: None,
        });
        Self::apply(true, cx);
        if !first {
            return;
        }
        cx.observe_global::<ClientSettingsStore>(|cx| Self::apply(false, cx)).detach();
        cx.observe_global::<LocalStorage>(|cx| Self::apply(false, cx)).detach();
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The chosen theme: `system`, `light`, `dark` or a theme id.
    pub fn preference(&self) -> &SharedString {
        &self.preference
    }

    pub fn appearance_mode(&self) -> ThemeMode {
        self.appearance_mode
    }

    pub fn halves(&self) -> &ThemeHalves {
        &self.halves
    }

    /// Every theme the picker offers: built-in, custom, then each connected
    /// environment's published themes.
    pub fn catalog(cx: &App) -> Vec<CatalogTheme> {
        let mut themes = BUILT_IN.clone();
        if let Some(this) = cx.try_global::<Self>() {
            // A custom theme with a bundled theme's id (an imported Graphite) replaces it.
            themes.retain(|theme| this.custom.iter().all(|custom| custom.document.id != theme.document.id));
            themes.extend(
                this.custom
                    .iter()
                    .map(|theme| CatalogTheme { document: theme.document.clone(), source: ThemeSource::Custom }),
            );
        }
        for id in ServerSettingsStore::environment_ids(cx) {
            let Some(config) = ServerSettingsStore::config(cx, &id) else { continue };
            for theme in config.environment_themes.iter().flatten() {
                let Ok(Value::Object(mut json)) = serde_json::to_value(theme) else { continue };
                json.insert("version".into(), 1.into());
                json.entry("colors").or_insert_with(|| Value::Object(Default::default()));
                let Ok(json) = serde_json::to_vec(&json) else { continue };
                let Ok(document) = ThemeDocument::parse(&json) else { continue };
                if themes.iter().all(|existing| existing.document.id != document.id) {
                    themes.push(CatalogTheme { document: Arc::new(document), source: ThemeSource::Environment });
                }
            }
        }
        themes
    }

    /// A theme by id, `system`/`light`/`dark` meaning the standard theme.
    pub fn find(id: &str, cx: &App) -> Option<CatalogTheme> {
        if STANDARD_PREFERENCES.contains(&id) {
            return Some(standard_theme().clone());
        }
        let id = canonical_theme_id(id);
        Self::catalog(cx).into_iter().find(|theme| theme.document.id == id)
    }

    /// `setTheme`: chooses a whole theme, which clears any separate halves.
    /// The appearance mode stays as it was.
    pub fn set_theme(id: &str, cx: &mut App) {
        let id: SharedString = canonical_theme_id(id).to_owned().into();
        let mode = cx.global::<Self>().appearance_mode;
        LocalStorage::set(cx, APPEARANCE_MODE_STORAGE_KEY, Some(&mode_name(mode)));
        LocalStorage::set::<ThemeHalves>(cx, THEME_HALVES_STORAGE_KEY, None);
        LocalStorage::set(cx, THEME_STORAGE_KEY, Some(&id.to_string()));
        let this = cx.global_mut::<Self>();
        this.preference = id;
        this.halves = ThemeHalves::default();
        Self::apply(true, cx);
    }

    /// `setAppearanceMode`: light, dark, or follow the OS.
    pub fn set_appearance_mode(mode: ThemeMode, cx: &mut App) {
        LocalStorage::set(cx, APPEARANCE_MODE_STORAGE_KEY, Some(&mode_name(mode)));
        cx.global_mut::<Self>().appearance_mode = mode;
        Self::apply(true, cx);
    }

    /// `setThemeHalf`: draws `appearance` with another theme's half, or
    /// clears that half with `None`.
    pub fn set_half(appearance: Appearance, id: Option<&str>, cx: &mut App) {
        let mut halves = cx.global::<Self>().halves.clone();
        *halves.slot(appearance) = id.map(|id| canonical_theme_id(id).to_owned());
        LocalStorage::set(cx, THEME_HALVES_STORAGE_KEY, (!halves.is_empty()).then_some(&halves));
        cx.global_mut::<Self>().halves = halves;
        Self::apply(true, cx);
    }

    /// Adds or replaces a custom theme from a T3 theme file. Returns its id.
    pub fn add_custom(json: &[u8], cx: &mut App) -> Result<String, ThemeImportError> {
        let document = ThemeDocument::parse(json)?;
        if crate::vscode_theme_import::RESERVED_THEME_IDS.contains(&document.id.as_str()) {
            return Err(ThemeImportError::ReservedId(document.id));
        }
        let mut source: Value = serde_json::from_slice(json).map_err(ThemeError::from)?;
        if let Value::Object(map) = &mut source {
            map.insert("id".into(), Value::String(document.id.clone()));
        }
        let id = document.id.clone();
        let mut custom = cx.global::<Self>().custom.clone();
        custom.retain(|theme| theme.document.id != id);
        custom.push(CustomTheme { document: Arc::new(document), source });
        Self::save_custom(custom, cx);
        Ok(id)
    }

    /// Adds a copy of a theme as a custom theme named "<name> copy" and
    /// returns its id.
    pub fn duplicate(id: &str, cx: &mut App) -> Result<String, ThemeImportError> {
        let custom = cx.global::<Self>().custom.iter().find(|theme| theme.document.id == id).map(|theme| theme.source.clone());
        let mut source = match custom {
            Some(source) => source,
            None => {
                let file = BUILT_IN_FILES.iter().zip(BUILT_IN.iter()).find(|(_, theme)| theme.document.id == id).map(|(file, _)| file);
                let bytes = file.and_then(|file| Assets::get(&format!("themes/{file}.json"))).ok_or_else(|| {
                    ThemeImportError::Invalid(ThemeError::from(serde_json::Error::io(std::io::Error::other("unknown theme"))))
                })?;
                serde_json::from_slice(&bytes).map_err(ThemeError::from)?
            }
        };
        let name = source.get("name").and_then(Value::as_str).unwrap_or("Theme").to_owned();
        let taken: Vec<String> = Self::catalog(cx).iter().map(|t| t.document.id.clone()).collect();
        let base = format!("{id}-copy");
        let new_id = (1..).map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") }).find(|c| !taken.contains(c)).expect("an id is free");
        if let Value::Object(map) = &mut source {
            map.insert("id".into(), Value::String(new_id));
            map.insert("name".into(), Value::String(format!("{name} copy")));
        }
        Self::add_custom(&serde_json::to_vec(&source).map_err(ThemeError::from)?, cx)
    }

    /// Deletes a custom theme. A deleted theme in use falls back to the
    /// standard theme, as the web does.
    pub fn delete_custom(id: &str, cx: &mut App) {
        let mut custom = cx.global::<Self>().custom.clone();
        custom.retain(|theme| theme.document.id != id);
        Self::save_custom(custom, cx);
        let this = cx.global::<Self>();
        if this.preference.as_ref() == id {
            Self::set_theme("system", cx);
        }
        for appearance in [Appearance::Light, Appearance::Dark] {
            if cx.global::<Self>().halves.get(appearance) == Some(id) {
                Self::set_half(appearance, None, cx);
            }
        }
    }

    fn save_custom(custom: Vec<CustomTheme>, cx: &mut App) {
        let sources: Vec<Value> = custom.iter().map(|theme| theme.source.clone()).collect();
        LocalStorage::set(cx, CUSTOM_THEMES_STORAGE_KEY, Some(&sources));
        cx.global_mut::<Self>().custom = custom;
        Self::apply(true, cx);
    }

    /// The document drawn now: the chosen theme with any separate halves
    /// composed in.
    pub fn effective_document(cx: &App) -> ThemeDocument {
        let this = cx.global::<Self>();
        let base = Self::find(&this.preference, cx).unwrap_or_else(|| standard_theme().clone());
        let half = |appearance| {
            this.halves
                .get(appearance)
                .and_then(|id| Self::find(id, cx))
                .and_then(|theme| theme.document.palette_for(appearance))
        };
        let (light, dark) = (half(Appearance::Light), half(Appearance::Dark));
        if light.is_none() && dark.is_none() {
            return ThemeDocument::clone(&base.document);
        }
        let mut document = ThemeDocument::clone(&base.document);
        let dark = dark.or_else(|| base.document.palette_for(Appearance::Dark));
        let light = light.or_else(|| base.document.palette_for(Appearance::Light));
        match document.appearance {
            Appearance::Dark => {
                document.colors = dark.unwrap_or(document.colors);
                document.variants = light.map(|light| ThemeVariants { dark: None, light: Some(light) });
            }
            Appearance::Light => {
                document.colors = light.unwrap_or(document.colors);
                document.variants = dark.map(|dark| ThemeVariants { dark: Some(dark), light: None });
            }
        }
        document
    }

    /// Pushes the theme and fonts into [`ThemeSettings`]. `force` rebuilds
    /// even when the fonts did not change (the theme choice did).
    fn apply(force: bool, cx: &mut App) {
        let fonts = font_settings(ClientSettingsStore::get(cx), separate_fonts(cx));
        let families = family_overrides(ClientSettingsStore::get(cx), separate_fonts(cx));
        let unchanged = cx.global::<Self>().applied.as_ref() == Some(&(fonts, families.clone()));
        if unchanged && !force {
            return;
        }
        let rem_changed = cx.global::<Self>().applied.as_ref().is_none_or(|(old, _)| old.interface != fonts.interface);
        cx.global_mut::<Self>().applied = Some((fonts, families.clone()));
        set_family_overrides(families);
        let document = Arc::new(Self::effective_document(cx));
        let mode = cx.global::<Self>().appearance_mode;
        ThemeSettings::update(cx, |settings| {
            settings.document = document;
            settings.mode = mode;
            settings.fonts = fonts;
        });
        if rem_changed {
            let rem = px(fonts.clamped().interface);
            for window in cx.windows() {
                _ = window.update(cx, |_, window, _| window.set_rem_size(rem));
            }
        }
    }
}

/// Whether "Separate fonts" is on.
pub fn separate_fonts(cx: &App) -> bool {
    LocalStorage::get(cx, TYPOGRAPHY_ADVANCED_STORAGE_KEY).unwrap_or(false)
}

/// Turns "Separate fonts" on or off.
pub fn set_separate_fonts(on: bool, cx: &mut App) {
    LocalStorage::set(cx, TYPOGRAPHY_ADVANCED_STORAGE_KEY, Some(&on));
}

/// The four sizes, with the prompt size only when fonts are separate.
fn font_settings(settings: &ClientSettings, separate: bool) -> FontSettings {
    FontSettings {
        interface: settings.font_size_interface as f32,
        prompt: settings.font_size_prompt as f32,
        code: settings.font_size_code as f32,
        terminal: settings.font_size_terminal as f32,
        separate_prompt_font: separate,
    }
    .clamped()
}

/// `applyAppearanceFontVariables`: the composer and terminal families
/// apply only when fonts are separate.
fn family_overrides(settings: &ClientSettings, separate: bool) -> FamilyOverrides {
    let first = |value: &str| -> Option<SharedString> {
        let family = value.split(',').next()?.trim().trim_matches(['"', '\'']).trim();
        (!family.is_empty()).then(|| SharedString::from(family.to_owned()))
    };
    FamilyOverrides {
        sans: first(settings.font_family_sans.as_str()),
        code: first(settings.font_family_code.as_str()),
        composer: separate.then(|| first(settings.font_family_composer.as_str())).flatten(),
        terminal: separate.then(|| first(settings.font_family_terminal.as_str())).flatten(),
    }
}

fn parse_mode(mode: &str) -> Option<ThemeMode> {
    match mode {
        "system" => Some(ThemeMode::System),
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        _ => None,
    }
}

fn mode_name(mode: ThemeMode) -> String {
    match mode {
        ThemeMode::System => "system",
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
    }
    .into()
}
