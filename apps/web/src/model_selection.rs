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
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
    pub description: Option<SharedString>,
    pub is_default: bool,
}

/// A select option a model offers (effort, service tier, context window, an agent's mode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectDescriptor {
    pub id: SharedString,
    pub label: SharedString,
    pub options: Vec<OptionChoice>,
    /// Levels that work when typed into the prompt (Ultrathink) but no picker offers
    /// (`promptInjectedValues`).
    pub prompt_injected: Vec<SharedString>,
    /// The value in effect: the server's `currentValue`, or the selection's once resolved.
    pub current: Option<SharedString>,
    /// The agent marked it as the reasoning control (`role: "reasoning"`).
    pub reasoning_role: bool,
}

impl SelectDescriptor {
    /// The options a picker offers (`selectableProviderOptionChoices`).
    pub fn selectable(&self) -> Vec<OptionChoice> {
        self.options.iter().filter(|option| !self.prompt_injected.contains(&option.id)).cloned().collect()
    }

    /// The current value, else the default option (`getProviderOptionCurrentValue`).
    pub fn value(&self) -> Option<&SharedString> {
        self.current.as_ref().or_else(|| self.options.iter().find(|o| o.is_default).map(|o| &o.id))
    }

    /// The current option's label (`getProviderOptionCurrentLabel`).
    pub fn value_label(&self) -> Option<SharedString> {
        let value = self.value()?;
        self.options.iter().find(|o| &o.id == value).map(|o| o.label.clone())
    }

    /// `resolveDescriptorChoiceValue`: an offered id stays, a prompt-injected or unknown id falls
    /// back to the server's value or the default.
    fn resolve(&self, raw: Option<&str>) -> Option<SharedString> {
        let fallback = || self.current.clone().or_else(|| self.options.iter().find(|o| o.is_default).map(|o| o.id.clone()));
        let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else { return fallback() };
        if self.options.is_empty() {
            return Some(raw.to_owned().into());
        }
        if self.prompt_injected.iter().any(|v| v.as_ref() == raw) && self.options.iter().any(|o| o.id.as_ref() == raw) {
            return self.options.iter().find(|o| o.is_default).map(|o| o.id.clone());
        }
        if self.options.iter().any(|o| o.id.as_ref() == raw) {
            return Some(raw.to_owned().into());
        }
        fallback()
    }
}

/// A two-state option (Claude's Fast mode, thinking, Ultracode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BooleanDescriptor {
    pub id: SharedString,
    pub label: SharedString,
    pub current: Option<bool>,
}

/// One option a model offers, as the server's `optionDescriptors` carry it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionDescriptor {
    Select(SelectDescriptor),
    Boolean(BooleanDescriptor),
}

/// `REASONING_EFFORT_OPTION_IDS` plus `thinking`, the ids `isReasoningControl` knows.
const REASONING_IDS: &[&str] = &["reasoningEffort", "effort", "reasoning", "variant", "reasoning_effort", "thinking_level", "thinking"];

impl OptionDescriptor {
    pub fn id(&self) -> &SharedString {
        match self {
            Self::Select(s) => &s.id,
            Self::Boolean(b) => &b.id,
        }
    }

    pub fn label(&self) -> &SharedString {
        match self {
            Self::Select(s) => &s.label,
            Self::Boolean(b) => &b.label,
        }
    }

    /// `isReasoningControl`: a select the agent marked as reasoning, or a known effort id.
    pub fn is_reasoning(&self) -> bool {
        matches!(self, Self::Select(s) if s.reasoning_role) || REASONING_IDS.contains(&self.id().as_ref())
    }

    /// The value in effect, for the wire.
    pub fn value(&self) -> Option<OptionValue> {
        match self {
            Self::Select(s) => s.value().cloned().map(OptionValue::Text),
            Self::Boolean(b) => b.current.map(OptionValue::Bool),
        }
    }

    /// A copy with `value` in effect (`withDescriptorCurrentValue`).
    fn with_value(&self, value: Option<&OptionValue>) -> Self {
        match (self, value) {
            (Self::Boolean(b), Some(OptionValue::Bool(v))) => Self::Boolean(BooleanDescriptor { current: Some(*v), ..b.clone() }),
            (Self::Boolean(b), _) => Self::Boolean(b.clone()),
            (Self::Select(s), value) => {
                let raw = match value {
                    Some(OptionValue::Text(text)) => Some(text.as_ref()),
                    _ => s.current.as_deref(),
                };
                Self::Select(SelectDescriptor { current: s.resolve(raw), ..s.clone() })
            }
        }
    }
}

/// A selected option value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OptionValue {
    Text(SharedString),
    Bool(bool),
}

/// One selected option, as the wire's `ProviderOptionSelection`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OptionSelection {
    pub id: SharedString,
    pub value: OptionValue,
}

fn selected<'a>(selections: &'a [OptionSelection], id: &str) -> Option<&'a OptionValue> {
    selections.iter().find(|s| s.id.as_ref() == id).map(|s| &s.value)
}

/// `withLegacyUltracodeEffort`: Ultracode was a Claude effort level that ran at xhigh before it
/// became its own switch.
fn with_legacy_ultracode(descriptors: &[OptionDescriptor], selections: &[OptionSelection]) -> Vec<OptionSelection> {
    let legacy = matches!(selected(selections, "effort"), Some(OptionValue::Text(v)) if v.as_ref() == "ultracode");
    let has_toggle = descriptors.iter().any(|d| matches!(d, OptionDescriptor::Boolean(b) if b.id.as_ref() == "ultracode"));
    if !legacy || !has_toggle {
        return selections.to_vec();
    }
    let mut out: Vec<_> = selections
        .iter()
        .map(|s| match (&s.id, &s.value) {
            (id, OptionValue::Text(v)) if id.as_ref() == "effort" && v.as_ref() == "ultracode" => {
                OptionSelection { id: id.clone(), value: OptionValue::Text("xhigh".into()) }
            }
            _ => s.clone(),
        })
        .collect();
    if selected(selections, "ultracode").is_none() {
        out.push(OptionSelection { id: "ultracode".into(), value: OptionValue::Bool(true) });
    }
    out
}

/// A model's descriptors with the selection's values in effect (`getProviderOptionDescriptors`).
pub fn resolve_descriptors(base: &[OptionDescriptor], selections: &[OptionSelection]) -> Vec<OptionDescriptor> {
    let selections = with_legacy_ultracode(base, selections);
    base.iter().map(|d| d.with_value(selected(&selections, d.id()))).collect()
}

/// Every descriptor's value in effect (`buildProviderOptionSelectionsFromDescriptors`).
pub fn selections_from(descriptors: &[OptionDescriptor]) -> Vec<OptionSelection> {
    descriptors.iter().filter_map(|d| Some(OptionSelection { id: d.id().clone(), value: d.value()? })).collect()
}

/// The options a turn carries: the explicitly chosen ones, normalised against the model, plus
/// Fast mode off when the model has it and nothing chose it (`withImplicitFastModeDefault`,
/// `buildExplicitProviderOptionSelectionsFromDescriptors`).
pub fn dispatch_options(base: &[OptionDescriptor], selections: &[OptionSelection]) -> Vec<OptionSelection> {
    let mut selections = selections.to_vec();
    let fast = base.iter().any(|d| matches!(d, OptionDescriptor::Boolean(b) if b.id.as_ref() == "fastMode"));
    if fast && selected(&selections, "fastMode").is_none() {
        selections.push(OptionSelection { id: "fastMode".into(), value: OptionValue::Bool(false) });
    }
    let explicit: HashSet<SharedString> = with_legacy_ultracode(base, &selections).into_iter().map(|s| s.id).collect();
    selections_from(&resolve_descriptors(base, &selections)).into_iter().filter(|s| explicit.contains(&s.id)).collect()
}

/// A model's options sorted the way the picker places them (`splitSpeed`, `columnControls`):
/// speed and Ultracode in the footer, the reasoning control at the head of the column, and
/// every other option below it, selects before switches.
#[derive(Clone, Debug, Default)]
pub struct ModelTraits {
    /// All descriptors with the selection's values in effect, in server order.
    pub descriptors: Vec<OptionDescriptor>,
}

impl ModelTraits {
    /// A model's traits under a selection's options.
    pub fn new(model: &ModelOption, selections: &[OptionSelection]) -> Self {
        Self { descriptors: resolve_descriptors(&model.options, selections) }
    }

    fn select(&self, id: &str) -> Option<&SelectDescriptor> {
        self.descriptors.iter().find_map(|d| match d {
            OptionDescriptor::Select(s) if s.id.as_ref() == id => Some(s),
            _ => None,
        })
    }

    fn boolean(&self, id: &str) -> Option<&BooleanDescriptor> {
        self.descriptors.iter().find_map(|d| match d {
            OptionDescriptor::Boolean(b) if b.id.as_ref() == id => Some(b),
            _ => None,
        })
    }

    /// The first select: the one a typed Ultrathink owns.
    pub fn primary_select(&self) -> Option<&SelectDescriptor> {
        self.descriptors.iter().find_map(|d| match d {
            OptionDescriptor::Select(s) => Some(s),
            OptionDescriptor::Boolean(_) => None,
        })
    }

    /// The service tier select, shown as footer segments.
    pub fn speed_select(&self) -> Option<&SelectDescriptor> {
        self.select("serviceTier")
    }

    /// Claude's Fast mode, shown as Standard / Fast footer segments.
    pub fn fast_toggle(&self) -> Option<&BooleanDescriptor> {
        self.boolean("fastMode")
    }

    /// The Ultracode switch, a footer toggle.
    pub fn ultracode_toggle(&self) -> Option<&BooleanDescriptor> {
        self.boolean("ultracode")
    }

    /// The context window select, which the trigger also names.
    pub fn context_window(&self) -> Option<&SelectDescriptor> {
        self.select("contextWindow")
    }

    /// The column's controls: selects then switches, without the footer's.
    fn column(&self) -> Vec<&OptionDescriptor> {
        let footer = |d: &OptionDescriptor| match d {
            OptionDescriptor::Select(s) => s.id.as_ref() == "serviceTier",
            OptionDescriptor::Boolean(b) => matches!(b.id.as_ref(), "fastMode" | "ultracode"),
        };
        let selects = self.descriptors.iter().filter(|d| matches!(d, OptionDescriptor::Select(_)) && !footer(d));
        let switches = self.descriptors.iter().filter(|d| matches!(d, OptionDescriptor::Boolean(_)) && !footer(d));
        selects.chain(switches).collect()
    }

    /// The reasoning control the column lists first.
    pub fn reasoning(&self) -> Option<&OptionDescriptor> {
        self.column().into_iter().find(|d| d.is_reasoning())
    }

    /// The reasoning control when it is a select (the levels).
    pub fn effort(&self) -> Option<&SelectDescriptor> {
        match self.reasoning()? {
            OptionDescriptor::Select(s) => Some(s),
            OptionDescriptor::Boolean(_) => None,
        }
    }

    /// Every other column option (an agent's mode, a context window), one row each.
    pub fn others(&self) -> Vec<&OptionDescriptor> {
        let reasoning = self.reasoning().map(|d| d.id().clone());
        self.column().into_iter().filter(|d| Some(d.id()) != reasoning.as_ref()).collect()
    }

    /// Whether the model has any option to show (`hasAnyControls`).
    pub fn any(&self) -> bool {
        !self.descriptors.is_empty()
    }

    /// The selection's options after one option changes (`replaceDescriptorCurrentValue` then
    /// `buildProviderOptionSelectionsFromDescriptors`).
    pub fn with_value(&self, id: &str, value: OptionValue) -> Vec<OptionSelection> {
        let next: Vec<_> = self.descriptors.iter().map(|d| if d.id().as_ref() == id { d.with_value(Some(&value)) } else { d.clone() }).collect();
        selections_from(&next)
    }

    /// `"Fast"` or `"Ultrafast"` when a speed option is on (`buildTraitsTriggerDisplay`'s
    /// `speedLabel`).
    pub fn speed_label(&self) -> Option<&'static str> {
        if let Some(fast) = self.fast_toggle() {
            return (fast.current == Some(true)).then_some("Fast");
        }
        let tier = self.speed_select()?;
        let current = tier.value()?;
        let by_label = |label: &str| tier.options.iter().find(|o| o.label.as_ref() == label);
        if by_label("Ultrafast").is_some_and(|o| &o.id == current) {
            Some("Ultrafast")
        } else if by_label("Fast").is_some_and(|o| &o.id == current) {
            Some("Fast")
        } else {
            None
        }
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
    /// The options the model offers, in server order (`capabilities.optionDescriptors`).
    pub options: Vec<OptionDescriptor>,
}

impl ModelOption {
    /// The model's traits under a selection's options.
    pub fn traits(&self, selections: &[OptionSelection]) -> ModelTraits {
        ModelTraits::new(self, selections)
    }
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

/// What the composer sends with: a model and the options chosen for it, as the wire carries
/// them. A model's [`ModelTraits`] say which option is effort, speed or a mode.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModelSelection {
    pub instance_id: ProviderInstanceId,
    pub model: SharedString,
    pub options: Vec<OptionSelection>,
}

impl ModelSelection {
    /// The model's traits in a catalog under this selection, when the catalog has the model.
    pub fn traits(&self, catalog: &ModelCatalog) -> Option<ModelTraits> {
        Some(catalog.model(&self.instance_id, &self.model)?.traits(&self.options))
    }

    /// The reasoning level in effect, when the model has levels.
    pub fn effort(&self, catalog: &ModelCatalog) -> Option<SharedString> {
        self.traits(catalog)?.effort()?.value().cloned()
    }
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
