//! The model list the composer's picker shows, and the selection it edits.
//! Typed stand-ins for the provider snapshots the server sends; the chat view
//! fills a [`ModelCatalog`] from the shell state, scenes fill it from fixtures.
//! Ports the parts of `modelSelection.ts`, `providerIconUtils.ts` and
//! `EffortGlyph.tsx` the picker needs.

use std::collections::{HashMap, HashSet};

use gpui_kit::SharedString;

/// A provider driver. The picker's ordering and icons depend on it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProviderDriverKind {
    Codex,
    ClaudeAgent,
    Cursor,
    OpenCode,
    Grok,
    Antigravity,
    Other(SharedString),
}

impl ProviderDriverKind {
    /// The wire name, as in `ProviderDriverKind` in contracts.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeAgent => "claudeAgent",
            Self::Cursor => "cursor",
            Self::OpenCode => "opencode",
            Self::Grok => "grok",
            Self::Antigravity => "antigravity",
            Self::Other(name) => name,
        }
    }
}

/// A configured provider instance id, such as `codex` or `claude-work`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderInstanceId(pub SharedString);

/// One provider instance as the picker rail shows it.
#[derive(Clone, Debug)]
pub struct ProviderInstanceEntry {
    pub instance_id: ProviderInstanceId,
    pub driver_kind: ProviderDriverKind,
    pub display_name: SharedString,
    /// Installed, authenticated and enabled; otherwise the rail dims it.
    pub ready: bool,
    pub unavailable_reason: Option<SharedString>,
}

/// A level of a select option such as reasoning effort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionChoice {
    pub id: SharedString,
    pub label: SharedString,
    pub is_default: bool,
}

/// A select option a model offers (effort, speed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectDescriptor {
    pub label: SharedString,
    pub options: Vec<OptionChoice>,
    /// Levels that work when typed into the prompt (Ultrathink) but no picker offers
    /// (`promptInjectedValues`).
    pub prompt_injected: Vec<SharedString>,
}

impl SelectDescriptor {
    /// The options a picker offers (`selectableProviderOptionChoices`).
    pub fn selectable(&self) -> Vec<OptionChoice> {
        self.options.iter().filter(|option| !self.prompt_injected.contains(&option.id)).cloned().collect()
    }
}

/// `isClaudeUltrathinkPrompt`: the word "ultrathink" anywhere in the prompt.
pub fn is_ultrathink_prompt(text: &str) -> bool {
    static WORD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)\bultrathink\b").expect("valid"));
    WORD.is_match(text)
}

/// The prompt without a leading `Ultrathink:` and the whitespace after it.
pub fn strip_ultrathink_prefix(text: &str) -> &str {
    static PREFIX: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)^Ultrathink:\s*").expect("valid"));
    PREFIX.find(text).map_or(text, |m| &text[m.end()..])
}

/// One model of an instance.
#[derive(Clone, Debug)]
pub struct ModelOption {
    pub slug: SharedString,
    pub name: SharedString,
    pub short_name: Option<SharedString>,
    pub sub_provider: Option<SharedString>,
    pub is_legacy: bool,
    pub is_unavailable: bool,
    pub effort: Option<SelectDescriptor>,
    pub speed: Option<SelectDescriptor>,
    pub ultracode: bool,
}

/// The picker's whole input: instances in server order, their models, and the
/// user's model preferences.
#[derive(Clone, Debug, Default)]
pub struct ModelCatalog {
    pub instances: Vec<ProviderInstanceEntry>,
    pub models: HashMap<ProviderInstanceId, Vec<ModelOption>>,
    /// `instance:slug` keys, see [`provider_model_key`].
    pub favorites: HashSet<String>,
    /// Hidden slugs per instance (Settings → Providers).
    pub hidden: HashMap<ProviderInstanceId, HashSet<SharedString>>,
}

/// What the composer sends with: a model and its options.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelSelection {
    pub instance_id: ProviderInstanceId,
    pub model: SharedString,
    pub effort: Option<SharedString>,
    pub speed: Option<SharedString>,
    pub ultracode: bool,
}

/// `instance:slug`, the key favourites use. Ports `providerModelKey`.
pub fn provider_model_key(instance: &ProviderInstanceId, slug: &str) -> String {
    format!("{}:{slug}", instance.0)
}

impl ModelCatalog {
    /// An instance by id.
    pub fn instance(&self, id: &ProviderInstanceId) -> Option<&ProviderInstanceEntry> {
        self.instances.iter().find(|entry| &entry.instance_id == id)
    }

    /// A model by instance and slug.
    pub fn model(&self, instance: &ProviderInstanceId, slug: &str) -> Option<&ModelOption> {
        self.models.get(instance)?.iter().find(|m| m.slug.as_ref() == slug)
    }

    /// An instance's models minus hidden ones (a hidden model stays when selected).
    /// Ports `filterHiddenModelOptions` in `modelSelection.ts`.
    pub fn visible_models(&self, instance: &ProviderInstanceId, selected: Option<&str>) -> Vec<&ModelOption> {
        let hidden = self.hidden.get(instance);
        self.models
            .get(instance)
            .into_iter()
            .flatten()
            .filter(|m| hidden.is_none_or(|h| !h.contains(&m.slug)) || selected == Some(m.slug.as_ref()))
            .collect()
    }

    /// Whether a model is a favourite.
    pub fn is_favorite(&self, instance: &ProviderInstanceId, slug: &str) -> bool {
        self.favorites.contains(&provider_model_key(instance, slug))
    }

    /// Adds or removes a favourite.
    pub fn toggle_favorite(&mut self, instance: &ProviderInstanceId, slug: &str) {
        let key = provider_model_key(instance, slug);
        if !self.favorites.remove(&key) {
            self.favorites.insert(key);
        }
    }
}

/// The mark a provider or model is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProviderIcon {
    OpenAi,
    Claude,
    Cursor,
    OpenCode,
    Grok,
    Gemini,
    Meta,
    /// No bundled mark; draw the instance initials.
    Initials,
}

impl ProviderIcon {
    /// The icon for a driver, with the fork's model overrides: OpenCode
    /// `opencode/muse-spark-*` → Meta, Cursor `grok-*` → Grok, Cursor `gemini-*` →
    /// Gemini. Ports `ProviderInstanceIcon` and `providerIconUtils.ts`.
    pub fn for_model(driver: &ProviderDriverKind, model: Option<&str>) -> Self {
        let model = model.unwrap_or("");
        match driver {
            ProviderDriverKind::OpenCode if model.starts_with("opencode/muse-spark-") => Self::Meta,
            ProviderDriverKind::Cursor if model.starts_with("grok-") => Self::Grok,
            ProviderDriverKind::Cursor if model.starts_with("gemini-") => Self::Gemini,
            ProviderDriverKind::Codex => Self::OpenAi,
            ProviderDriverKind::ClaudeAgent => Self::Claude,
            ProviderDriverKind::Cursor => Self::Cursor,
            ProviderDriverKind::OpenCode => Self::OpenCode,
            ProviderDriverKind::Grok => Self::Grok,
            ProviderDriverKind::Antigravity | ProviderDriverKind::Other(_) => Self::Initials,
        }
    }
}

/// Drops a leading sub-provider qualifier from a model name ("OpenAI: GPT-5" under
/// OpenAI → "GPT-5"). Ports `getDisplayModelName`.
pub fn display_model_name(model: &ModelOption, prefer_short: bool) -> SharedString {
    let name = match (&model.short_name, prefer_short) {
        (Some(short), true) => short.clone(),
        _ => model.name.clone(),
    };
    let Some(qualifier) = model.sub_provider.as_ref().map(|q| q.trim()).filter(|q| !q.is_empty()) else {
        return name;
    };
    let lower = name.to_lowercase();
    if !lower.starts_with(&qualifier.to_lowercase()) {
        return name;
    }
    let Some(rest) = name.get(qualifier.len()..) else { return name };
    let trimmed = rest.trim_start();
    let stripped = match trimmed.chars().next() {
        Some('.' | ':' | '/' | '-') => trimmed[1..].trim_start(),
        _ if trimmed.len() < rest.len() => trimmed,
        _ => return name,
    };
    if stripped.trim().is_empty() { name } else { stripped.trim().to_owned().into() }
}

/// Bars for an effort select: one per ordinary level, filled up to the current
/// one. Max, Default and Ultracode levels are named instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortGlyph {
    pub count: usize,
    pub filled: usize,
    /// The level's name when it is a named level instead of a bar count.
    pub special: Option<SharedString>,
    pub label: SharedString,
}

const NAMED_LEVELS: [&str; 3] = ["ultracode", "max", "default"];
const MAX_BARS: usize = 7;

/// Ports `resolveEffortGlyph`.
pub fn resolve_effort_glyph(descriptor: &SelectDescriptor, current: Option<&str>) -> Option<EffortGlyph> {
    let current = current?;
    let levels: Vec<_> = descriptor.options.iter().filter(|o| !NAMED_LEVELS.contains(&o.id.as_ref())).collect();
    if levels.len() < 2 || levels.len() > MAX_BARS {
        return None;
    }
    let starts_at_off = matches!(levels[0].id.as_ref(), "off" | "none");
    let count = if starts_at_off { levels.len() - 1 } else { levels.len() };
    if NAMED_LEVELS.contains(&current) {
        let label = descriptor.options.iter().find(|o| o.id.as_ref() == current).map_or_else(|| current.to_owned().into(), |o| o.label.clone());
        return Some(EffortGlyph { count, filled: count, special: Some(label.clone()), label });
    }
    let index = levels.iter().position(|o| o.id.as_ref() == current)?;
    Some(EffortGlyph {
        count,
        filled: if starts_at_off { index } else { index + 1 },
        special: None,
        label: levels[index].label.clone(),
    })
}

/// Bar heights rising by one whole-pixel step from about half height to `max_bar`.
/// Ports `effortBarHeights`.
pub fn effort_bar_heights(count: usize, max_bar: f32) -> Vec<f32> {
    let step = (max_bar / 2.0 / (count.saturating_sub(1).max(1)) as f32).round().max(1.0);
    (0..count).map(|i| max_bar - (count - 1 - i) as f32 * step).collect()
}
