//! Fonts and text sizes. Ports `appearanceFonts.ts`: the UI and mono font
//! stacks resolved per platform, the bundled Inter Variable, and the four
//! size settings (interface, prompt, code, terminal).

use std::sync::{LazyLock, RwLock};

use gpui_kit::{App, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, SharedString};

use crate::Assets;

/// The web app's Inter is `@fontsource-variable/inter@5.3.0`, split by script.
/// Variable fonts cannot be merged, so each subset is its own family and the
/// UI font lists the others as fallbacks, as the browser does with `unicode-range`.
const INTER_SUBSETS: [(&str, &str); 7] = [
    ("Inter Variable", "latin"),
    ("Inter Variable Latin Ext", "latin-ext"),
    ("Inter Variable Cyrillic", "cyrillic"),
    ("Inter Variable Cyrillic Ext", "cyrillic-ext"),
    ("Inter Variable Greek", "greek"),
    ("Inter Variable Greek Ext", "greek-ext"),
    ("Inter Variable Vietnamese", "vietnamese"),
];

/// The UI font family. CSS stack `-apple-system, BlinkMacSystemFont, "Inter
/// Variable", ...` resolves to SF Pro on macOS and the bundled Inter elsewhere.
pub fn ui_font_family() -> SharedString {
    if cfg!(target_os = "macos") {
        ".SystemUIFont".into()
    } else {
        INTER_SUBSETS[0].0.into()
    }
}

/// The mono stack, in the order the browser tries it.
const MONO_STACK: [&str; 6] = ["SF Mono", "SFMono-Regular", "Menlo", "Consolas", "Liberation Mono", "DejaVu Sans Mono"];

/// The first family of [`MONO_STACK`] that is installed, set by [`load_fonts`].
static MONO_FAMILY: std::sync::OnceLock<SharedString> = std::sync::OnceLock::new();

/// The default mono family: the first installed family of
/// `ui-monospace, "SF Mono", ..., Consolas, "Liberation Mono"`, as the browser
/// resolves it. A missing family would lose its bold face to the fallback.
pub fn mono_font_family() -> SharedString {
    MONO_FAMILY.get().cloned().unwrap_or_else(|| {
        if cfg!(target_os = "macos") {
            "SF Mono".into()
        } else if cfg!(target_os = "windows") {
            "Consolas".into()
        } else {
            "Liberation Mono".into()
        }
    })
}

static UI_FALLBACKS: LazyLock<FontFallbacks> = LazyLock::new(|| {
    let mut families: Vec<String> = INTER_SUBSETS[1..].iter().map(|(name, _)| (*name).into()).collect();
    if !cfg!(target_os = "macos") {
        families.push("Segoe UI".into());
    }
    FontFallbacks::from_fonts(families)
});

static MONO_FALLBACKS: LazyLock<FontFallbacks> = LazyLock::new(|| {
    FontFallbacks::from_fonts(
        ["SF Mono", "SFMono-Regular", "Menlo", "Consolas", "Liberation Mono", "DejaVu Sans Mono"]
            .into_iter()
            .map(String::from)
            .collect(),
    )
});

/// The font family settings (`fontFamilySans`, `fontFamilyCode`,
/// `fontFamilyComposer`, `fontFamilyTerminal`): a family name each, or
/// `None` for the default stack. Set by the theme store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FamilyOverrides {
    pub sans: Option<SharedString>,
    pub code: Option<SharedString>,
    /// Only while "Separate fonts" is on; otherwise the composer uses sans.
    pub composer: Option<SharedString>,
    /// Only while "Separate fonts" is on; otherwise the terminal uses code.
    pub terminal: Option<SharedString>,
}

/// A custom family with the default stack behind it, built once per change
/// so the per-frame font helpers only clone `Arc`s.
type Resolved = Option<(SharedString, FontFallbacks)>;

struct ResolvedFamilies {
    sans: Resolved,
    code: Resolved,
    composer: Resolved,
    terminal: Resolved,
}

static FAMILY_OVERRIDES: RwLock<ResolvedFamilies> =
    RwLock::new(ResolvedFamilies { sans: None, code: None, composer: None, terminal: None });

/// `appearanceFontStack`: the custom family first, then the default family and its fallbacks.
fn resolve(custom: Option<SharedString>, family: SharedString, fallbacks: &FontFallbacks) -> Resolved {
    let custom = custom?;
    let mut list = vec![family.to_string()];
    list.extend(fallbacks.fallback_list().iter().cloned());
    Some((custom, FontFallbacks::from_fonts(list)))
}

/// Replaces the family settings every font helper reads.
pub fn set_family_overrides(overrides: FamilyOverrides) {
    let resolved = ResolvedFamilies {
        sans: resolve(overrides.sans, ui_font_family(), &UI_FALLBACKS),
        code: resolve(overrides.code, mono_font_family(), &MONO_FALLBACKS),
        composer: resolve(overrides.composer, ui_font_family(), &UI_FALLBACKS),
        terminal: resolve(overrides.terminal, mono_font_family(), &MONO_FALLBACKS),
    };
    *FAMILY_OVERRIDES.write().expect("font overrides lock") = resolved;
}

fn font_with(pick: fn(&ResolvedFamilies) -> &Resolved, weight: FontWeight) -> Option<Font> {
    let families = FAMILY_OVERRIDES.read().expect("font overrides lock");
    let (family, fallbacks) = pick(&families).clone()?;
    Some(Font { family, features: FontFeatures::default(), fallbacks: Some(fallbacks), weight, style: FontStyle::Normal })
}

fn default_font(family: SharedString, fallbacks: &FontFallbacks, weight: FontWeight) -> Font {
    Font { family, features: FontFeatures::default(), fallbacks: Some(fallbacks.clone()), weight, style: FontStyle::Normal }
}

/// The UI font at a weight, with the Inter subsets as fallbacks.
pub fn ui_font(weight: FontWeight) -> Font {
    font_with(|f| &f.sans, weight).unwrap_or_else(|| default_font(ui_font_family(), &UI_FALLBACKS, weight))
}

/// The mono font at a weight.
pub fn mono_font(weight: FontWeight) -> Font {
    font_with(|f| &f.code, weight).unwrap_or_else(|| default_font(mono_font_family(), &MONO_FALLBACKS, weight))
}

/// The composer's font: its own family when fonts are separate, else the UI font.
pub fn composer_font(weight: FontWeight) -> Font {
    font_with(|f| &f.composer, weight).unwrap_or_else(|| ui_font(weight))
}

/// The terminal's font: its own family when fonts are separate, else the code font.
pub fn terminal_font(weight: FontWeight) -> Font {
    font_with(|f| &f.terminal, weight).unwrap_or_else(|| mono_font(weight))
}
/// Registers the bundled Inter subsets with GPUI's text system.
pub fn load_fonts(cx: &mut App) -> anyhow::Result<()> {
    let fonts = INTER_SUBSETS
        .iter()
        .map(|(_, subset)| {
            Assets::get(&format!("fonts/inter/inter-{subset}-wght-normal.ttf"))
                .ok_or_else(|| anyhow::anyhow!("missing Inter subset {subset}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    cx.text_system().add_fonts(fonts)?;
    let installed = cx.text_system().all_font_names();
    if let Some(family) = MONO_STACK.iter().find(|f| installed.iter().any(|i| i == *f)) {
        MONO_FAMILY.get_or_init(|| SharedString::new_static(family));
    }
    Ok(())
}

/// The client's font size settings in pixels (`packages/contracts/src/settings.ts:168-198`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSettings {
    /// The root font size every rem scales with. Default 16, range 12 to 20.
    pub interface: f32,
    /// The composer prompt, when "separate fonts" is on. Default 14, range 12 to 20.
    pub prompt: f32,
    /// Code blocks and diffs. Default 13, range 10 to 18.
    pub code: f32,
    /// The terminal. Default 12, range 8 to 20.
    pub terminal: f32,
    /// Whether the prompt uses its own size; otherwise it is `text-sm`.
    pub separate_prompt_font: bool,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self { interface: 16., prompt: 14., code: 13., terminal: 12., separate_prompt_font: false }
    }
}

impl FontSettings {
    /// Clamps every size to the range the settings schema allows.
    pub fn clamped(self) -> Self {
        Self {
            interface: self.interface.clamp(12., 20.),
            prompt: self.prompt.clamp(12., 20.),
            code: self.code.clamp(10., 18.),
            terminal: self.terminal.clamp(8., 20.),
            ..self
        }
    }

    /// The prompt font size in pixels: its own setting, or `text-sm`.
    pub fn prompt_size(&self) -> f32 {
        if self.separate_prompt_font { self.prompt } else { TextSize::Sm.size(self.interface) }
    }
}

/// Tailwind's type scale plus the app's additions (`index.css:186-193`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    /// 7/7px.
    Xs5,
    /// 8/8px.
    Xs4,
    /// 10/14px.
    Xs3,
    /// 11/16px.
    Xs2,
    /// 12/16px.
    Xs,
    /// 14/20px.
    Sm,
    /// 16/24px.
    Base,
    /// 18/28px.
    Lg,
    /// 20/28px.
    Xl,
    /// 24/32px.
    Xl2,
    /// 30/36px.
    Xl3,
}

impl TextSize {
    /// Font size and line height in rem.
    fn rems(self) -> (f32, f32) {
        match self {
            Self::Xs5 => (0.4375, 0.4375),
            Self::Xs4 => (0.5, 0.5),
            Self::Xs3 => (0.625, 0.875),
            Self::Xs2 => (0.6875, 1.),
            Self::Xs => (0.75, 1.),
            Self::Sm => (0.875, 1.25),
            Self::Base => (1., 1.5),
            Self::Lg => (1.125, 1.75),
            Self::Xl => (1.25, 1.75),
            Self::Xl2 => (1.5, 2.),
            Self::Xl3 => (1.875, 2.25),
        }
    }

    /// Font size in pixels at a root size.
    pub fn size(self, rem: f32) -> f32 {
        self.rems().0 * rem
    }

    /// Default line height in pixels at a root size.
    pub fn line_height(self, rem: f32) -> f32 {
        self.rems().1 * rem
    }
}

/// Weights the shell uses.
pub mod weight {
    use gpui_kit::FontWeight;
    pub const NORMAL: FontWeight = FontWeight(400.);
    pub const MEDIUM: FontWeight = FontWeight(500.);
    pub const SEMIBOLD: FontWeight = FontWeight(600.);
    pub const BOLD: FontWeight = FontWeight(700.);
}
