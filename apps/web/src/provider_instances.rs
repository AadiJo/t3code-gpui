//! Provider instances as the pickers and the Providers settings panel see
//! them. Ports `providerInstances.ts`, `providerInstanceIds.ts`,
//! `state/providerInstanceDisplay.ts` and the instance-enable rules in
//! `contracts/settings.ts`, over the generated wire types.

use std::collections::HashSet;
use std::sync::Arc;

use t3code_contracts::{
    ModelSelection, ServerConfigProvidersItem, ServerConfigProvidersItemModelsItem, ServerProviderAuthStatus,
    ServerProviderAvailability, ServerProviderState, ServerSettings, ServerSettingsProviderInstancesValue,
};

/// A provider snapshot as `server.getConfig` / `subscribeServerConfig` send it.
pub type ServerProvider = ServerConfigProvidersItem;
/// One model in a provider snapshot.
pub type ServerProviderModel = ServerConfigProvidersItemModelsItem;
/// One configured instance in `ServerSettings.providerInstances`.
pub type ProviderInstanceConfig = ServerSettingsProviderInstancesValue;

/// The draft placeholder instance id. Never persist or dispatch it.
pub const NO_PROVIDER_INSTANCE_ID: &str = "t3code_no_provider";

/// `PROVIDER_DISPLAY_NAMES`.
pub fn provider_display_name(driver: &str) -> Option<&'static str> {
    Some(match driver {
        "antigravity" => "Antigravity",
        "codex" => "Codex",
        "claudeAgent" => "Claude",
        "cursor" => "Cursor",
        "grok" => "Grok",
        "fx" => "fx",
        "kiro" => "Kiro",
        "muse" => "Muse Code",
        "acpRegistry" => "ACP Registry",
        "pi" => "Pi",
        "opencode" => "OpenCode",
        _ => return None,
    })
}

/// `DEFAULT_MODEL`.
pub const DEFAULT_MODEL: &str = "gpt-6-astra";

/// `DEFAULT_MODEL_BY_PROVIDER`.
pub fn default_model_for_driver(driver: &str) -> Option<&'static str> {
    Some(match driver {
        "codex" => DEFAULT_MODEL,
        "claudeAgent" => "claude-opus-5-5",
        "cursor" => "auto",
        "grok" => "grok-build",
        "fx" | "kiro" | "muse" | "acpRegistry" | "pi" => "default",
        "opencode" => "openai/gpt-5",
        "antigravity" => "antigravity-default",
        _ => return None,
    })
}

/// The id of a driver's default instance: the driver name itself.
pub fn default_instance_id_for_driver(driver: &str) -> &str {
    driver
}

/// Drivers whose instances start off unless enabled (`DEFAULT_DISABLED_PROVIDER_DRIVERS`).
fn default_enabled_for_driver(driver: &str) -> bool {
    !matches!(driver, "cursor" | "grok" | "fx" | "kiro" | "muse" | "pi" | "opencode" | "antigravity")
}

/// Whether an unconfigured default instance counts as enabled.
pub fn is_unconfigured_default_instance_enabled(instance_id: &str) -> bool {
    matches!(instance_id, "codex" | "claudeAgent")
}

fn config_enabled_flag(config: Option<&serde_json::Value>) -> Option<bool> {
    config?.as_object()?.get("enabled")?.as_bool()
}

/// `resolveProviderInstanceEnabled`: an explicit `false` anywhere wins.
pub fn resolve_provider_instance_enabled(driver: &str, enabled: Option<bool>, config: Option<&serde_json::Value>) -> bool {
    let config_enabled = config_enabled_flag(config);
    if enabled == Some(false) || config_enabled == Some(false) {
        return false;
    }
    enabled.or(config_enabled).unwrap_or_else(|| default_enabled_for_driver(driver))
}

/// [`resolve_provider_instance_enabled`] over a stored instance.
pub fn instance_enabled(instance: &ProviderInstanceConfig) -> bool {
    resolve_provider_instance_enabled(&instance.driver, instance.enabled, instance.config.as_ref())
}

/// `resolveProjectProviderInstanceEnabled`: a project override, then the
/// machine instance, then the unconfigured default.
pub fn resolve_project_provider_instance_enabled(settings: &ServerSettings, project_id: Option<&str>, instance_id: &str) -> bool {
    let override_enabled = project_id.and_then(|project| {
        settings
            .project_settings_overrides
            .as_ref()?
            .get(project)?
            .provider_instances
            .as_ref()?
            .get(instance_id)?
            .enabled
    });
    if let Some(enabled) = override_enabled {
        return enabled;
    }
    match settings.provider_instances.as_ref().and_then(|instances| instances.get(instance_id)) {
        Some(instance) => instance_enabled(instance),
        None => is_unconfigured_default_instance_enabled(instance_id),
    }
}

fn humanize_slug(slug: &str) -> String {
    // `replace(/([a-z])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").trim()` then title-case each word.
    let mut spaced = String::with_capacity(slug.len() + 4);
    let mut prev: Option<char> = None;
    for ch in slug.chars() {
        if prev.is_some_and(|p| p.is_ascii_lowercase()) && ch.is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(ch);
        prev = Some(ch);
    }
    let mut collapsed = String::with_capacity(spaced.len());
    let mut in_run = false;
    for ch in spaced.chars() {
        if ch == '_' || ch == '-' {
            if !in_run {
                collapsed.push(' ');
            }
            in_run = true;
        } else {
            collapsed.push(ch);
            in_run = false;
        }
    }
    title_case_words(collapsed.trim())
}

/// `.replace(/\b\w/g, c => c.toUpperCase())` for ASCII word characters.
pub(crate) fn title_case_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_word = false;
    for ch in text.chars() {
        let word = ch.is_ascii_alphanumeric() || ch == '_';
        if word && !prev_word {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push(ch);
        }
        prev_word = word;
    }
    out
}

/// `resolveProviderInstanceDisplayName`.
pub fn resolve_provider_instance_display_name(instance_id: &str, driver: &str, display_name: Option<&str>) -> String {
    let trimmed = display_name.map(str::trim).filter(|name| !name.is_empty());
    let kind_label = provider_display_name(driver).map(str::to_owned).unwrap_or_else(|| humanize_slug(driver));
    if let Some(name) = trimmed
        && name != kind_label
    {
        return name.to_owned();
    }
    if instance_id != default_instance_id_for_driver(driver) {
        let humanized = humanize_slug(instance_id);
        if !humanized.is_empty() {
            return humanized;
        }
    }
    trimmed.map(str::to_owned).unwrap_or(kind_label)
}

/// `providerInstanceInitials`: two letters for an instance without a mark.
pub fn provider_instance_initials(label: &str) -> String {
    let to_words = |text: &str| -> Vec<String> {
        text.split_whitespace()
            .map(|word| word.trim_matches(|c| c == '\'' || c == '\u{2019}').to_owned())
            .filter(|word| !word.is_empty())
            .collect()
    };
    // Punctuation other than apostrophes separates words.
    let cleaned: String = label
        .chars()
        .map(|c| if is_punctuation(c) && c != '\'' && c != '\u{2019}' { ' ' } else { c })
        .collect();
    let mut words = to_words(&cleaned);
    if words.len() == 1 {
        let mut split = String::new();
        let mut prev: Option<char> = None;
        for ch in words[0].chars() {
            if prev.is_some_and(char::is_lowercase) && ch.is_uppercase() {
                split.push(' ');
            }
            split.push(ch);
            prev = Some(ch);
        }
        words = to_words(&split);
    }
    match words.len() {
        0 => String::new(),
        1 => words[0].chars().take(2).collect::<String>().to_uppercase(),
        _ => words.iter().take(2).filter_map(|word| word.chars().next()).flat_map(char::to_uppercase).collect(),
    }
}

/// Unicode `\p{P}`: the general categories Pc, Pd, Ps, Pe, Pi, Pf and Po.
fn is_punctuation(c: char) -> bool {
    use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
    c.general_category_group() == GeneralCategoryGroup::Punctuation
}

/// `normalizeProviderAccentColor`: `#rrggbb` or nothing.
pub fn normalize_provider_accent_color(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (trimmed.len() == 7 && trimmed.starts_with('#') && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| trimmed.to_owned())
}

/// `providerAccentForegroundColor`: black or white text on an accent badge.
pub fn provider_accent_foreground_color(accent: &str) -> &'static str {
    let channel = |offset: usize| {
        let value = u8::from_str_radix(accent.get(offset..offset + 2).unwrap_or("00"), 16).unwrap_or(0) as f64 / 255.;
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
    let black = (luminance + 0.05) / 0.05;
    let white = 1.05 / (luminance + 0.05);
    if black >= white { "#000000" } else { "#ffffff" }
}

/// What a picker or the panel knows about one instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderInstanceEntry {
    pub instance_id: String,
    pub driver_kind: String,
    pub display_name: String,
    pub accent_color: Option<String>,
    pub acp_registry_agent_id: Option<String>,
    pub acp_registry_icon_url: Option<String>,
    pub continuation_group_key: Option<String>,
    pub enabled: bool,
    pub installed: bool,
    pub status: ServerProviderState,
    pub is_default: bool,
    pub is_available: bool,
    pub snapshot: Arc<ServerProvider>,
}

impl ProviderInstanceEntry {
    pub fn models(&self) -> &[ServerProviderModel] {
        &self.snapshot.models
    }
}

/// `shouldShowInstanceBadge`: an accent, or another instance of the same driver.
pub fn should_show_instance_badge(entry: &ProviderInstanceEntry, entries: &[ProviderInstanceEntry]) -> bool {
    if normalize_provider_accent_color(entry.accent_color.as_deref()).is_some() {
        return true;
    }
    let mut shared = 0;
    for candidate in entries {
        if candidate.driver_kind != entry.driver_kind || (entry.enabled && !candidate.enabled) {
            continue;
        }
        if entry.driver_kind == "acpRegistry" && candidate.acp_registry_agent_id != entry.acp_registry_agent_id {
            continue;
        }
        shared += 1;
        if shared > 1 {
            return true;
        }
    }
    false
}

/// `hasInstanceBadge`.
pub fn has_instance_badge(entries: &[ProviderInstanceEntry]) -> bool {
    entries.iter().any(|entry| should_show_instance_badge(entry, entries))
}

fn is_provider_instance_healthy(entry: &ProviderInstanceEntry) -> bool {
    if !entry.enabled || !entry.is_available {
        return false;
    }
    match entry.status {
        ServerProviderState::Ready => true,
        ServerProviderState::Warning => {
            let usable: HashSet<&str> = entry
                .snapshot
                .sub_providers
                .iter()
                .flatten()
                .filter(|sub| sub.unavailable_reason.is_none())
                .map(|sub| sub.name.as_str())
                .collect();
            entry.models().iter().any(|model| model.sub_provider.as_deref().is_some_and(|sub| usable.contains(sub)))
        }
        _ => false,
    }
}

/// `isProviderInstancePickerReady`.
pub fn is_provider_instance_picker_ready(entry: &ProviderInstanceEntry) -> bool {
    if is_provider_instance_healthy(entry) {
        return true;
    }
    let snapshot = &entry.snapshot;
    if !entry.enabled || !entry.is_available || snapshot.sub_providers.is_some() {
        return false;
    }
    if snapshot.models_retained == Some(true) {
        return true;
    }
    entry.status == ServerProviderState::Warning
        && entry.installed
        && snapshot.auth.status == ServerProviderAuthStatus::Authenticated
        && !entry.models().is_empty()
}

/// `deriveProviderInstanceEntries`.
pub fn derive_provider_instance_entries(providers: &[Arc<ServerProvider>]) -> Vec<ProviderInstanceEntry> {
    providers
        .iter()
        .map(|snapshot| {
            let driver = snapshot.driver.clone();
            ProviderInstanceEntry {
                is_default: snapshot.instance_id == default_instance_id_for_driver(&driver),
                display_name: resolve_provider_instance_display_name(
                    &snapshot.instance_id,
                    &driver,
                    snapshot.display_name.as_deref(),
                ),
                accent_color: normalize_provider_accent_color(snapshot.accent_color.as_deref()),
                acp_registry_agent_id: None,
                acp_registry_icon_url: (driver == "acpRegistry")
                    .then(|| snapshot.icon_url.clone().filter(|url| !url.is_empty()))
                    .flatten(),
                continuation_group_key: snapshot.continuation.as_ref().map(|c| c.group_key.clone()),
                enabled: snapshot.enabled,
                installed: snapshot.installed,
                status: snapshot.status.clone(),
                is_available: snapshot.availability != Some(ServerProviderAvailability::Unavailable),
                instance_id: snapshot.instance_id.clone(),
                driver_kind: driver,
                snapshot: snapshot.clone(),
            }
        })
        .collect()
}

fn config_string<'a>(config: Option<&'a serde_json::Value>, key: &str) -> Option<&'a str> {
    config?.as_object()?.get(key)?.as_str()
}

fn apply_provider_instance_branding(mut entry: ProviderInstanceEntry, settings: &ServerSettings) -> ProviderInstanceEntry {
    let explicit = settings.provider_instances.as_ref().and_then(|instances| instances.get(&entry.instance_id));
    let Some(explicit) = explicit else { return entry };
    if entry.driver_kind != "acpRegistry" {
        return entry;
    }
    let config = explicit.config.as_ref().filter(|c| c.is_object());
    if config_string(config, "source") == Some("local") {
        entry.acp_registry_agent_id = None;
        entry.acp_registry_icon_url = None;
        return entry;
    }
    if let Some(agent) = config_string(config, "agentId").map(str::trim).filter(|s| !s.is_empty()) {
        entry.acp_registry_agent_id = Some(agent.to_owned());
    }
    if let Some(icon) = config_string(config, "registryIconUrl").map(str::trim).filter(|s| !s.is_empty()) {
        entry.acp_registry_icon_url = Some(icon.to_owned());
    }
    entry
}

/// `brandProviderInstanceEntries`.
pub fn brand_provider_instance_entries(entries: Vec<ProviderInstanceEntry>, settings: &ServerSettings) -> Vec<ProviderInstanceEntry> {
    entries.into_iter().map(|entry| apply_provider_instance_branding(entry, settings)).collect()
}

/// `applyProviderInstanceSettings`: project overrides when `project_id` is set.
pub fn apply_provider_instance_settings(
    entries: Vec<ProviderInstanceEntry>,
    settings: &ServerSettings,
    project_id: Option<&str>,
) -> Vec<ProviderInstanceEntry> {
    entries
        .into_iter()
        .map(|mut entry| {
            entry.enabled = resolve_project_provider_instance_enabled(settings, project_id, &entry.instance_id);
            apply_provider_instance_branding(entry, settings)
        })
        .collect()
}

/// Picker order: drivers in first-seen order, each driver's default instance first.
fn sort_provider_instance_entries(entries: Vec<ProviderInstanceEntry>) -> Vec<ProviderInstanceEntry> {
    let mut kinds: Vec<String> = Vec::new();
    for entry in &entries {
        if !kinds.contains(&entry.driver_kind) {
            kinds.push(entry.driver_kind.clone());
        }
    }
    let mut sorted = Vec::with_capacity(entries.len());
    for kind in kinds {
        let bucket = entries.iter().filter(|entry| entry.driver_kind == kind);
        sorted.extend(bucket.clone().filter(|entry| entry.is_default).cloned());
        sorted.extend(bucket.filter(|entry| !entry.is_default).cloned());
    }
    sorted
}

/// `providerInstanceEntriesFor`.
pub fn provider_instance_entries_for(
    providers: &[Arc<ServerProvider>],
    settings: &ServerSettings,
    project_id: Option<&str>,
) -> Vec<ProviderInstanceEntry> {
    sort_provider_instance_entries(apply_provider_instance_settings(
        derive_provider_instance_entries(providers),
        settings,
        project_id,
    ))
}

/// The model a new selection on `instance_id` starts with.
pub fn get_default_provider_instance_model(providers: &[Arc<ServerProvider>], instance_id: &str) -> Option<String> {
    let snapshot = providers.iter().find(|p| p.instance_id == instance_id)?;
    let models = &snapshot.models;
    models
        .iter()
        .find(|m| m.is_default == Some(true) && !m.is_custom)
        .or_else(|| models.iter().find(|m| !m.is_custom))
        .or_else(|| models.first())
        .map(|m| m.slug.clone())
        .or_else(|| default_model_for_driver(&snapshot.driver).map(str::to_owned))
}

/// `resolveSelectableProviderInstanceEntry`: the requested instance if usable,
/// else the first healthy one, else the first enabled one that is not failing.
pub fn resolve_selectable_provider_instance_entry<'a>(
    entries: &'a [ProviderInstanceEntry],
    instance_id: Option<&str>,
) -> Option<&'a ProviderInstanceEntry> {
    let selectable = |entry: &ProviderInstanceEntry| entry.enabled && entry.is_available;
    if let Some(requested) = instance_id.and_then(|id| entries.iter().find(|entry| entry.instance_id == id))
        && selectable(requested)
    {
        return Some(requested);
    }
    entries
        .iter()
        .find(|entry| is_provider_instance_healthy(entry))
        .or_else(|| entries.iter().find(|entry| selectable(entry) && entry.status != ServerProviderState::Error))
}

/// `resolveDefaultProviderModelSelection`.
pub fn resolve_default_provider_model_selection(
    providers: &[Arc<ServerProvider>],
    selection: Option<&ModelSelection>,
) -> Option<ModelSelection> {
    let entries = derive_provider_instance_entries(providers);
    let instance_id =
        resolve_selectable_provider_instance_entry(&entries, selection.map(|s| s.instance_id.as_str()))?.instance_id.clone();
    if let Some(selection) = selection.filter(|s| s.instance_id == instance_id) {
        return Some(selection.clone());
    }
    let model = get_default_provider_instance_model(providers, &instance_id)?;
    Some(ModelSelection { instance_id, model, options: None })
}

/// `resolveProjectDefaultProviderModelSelection`: the first driver group wins.
pub fn resolve_project_default_provider_model_selection(
    providers: &[Arc<ServerProvider>],
    settings: &ServerSettings,
    project_id: Option<&str>,
) -> Option<ModelSelection> {
    let entries = provider_instance_entries_for(providers, settings, project_id);
    let first_driver = entries.first().map(|entry| entry.driver_kind.clone());
    let group: Vec<ProviderInstanceEntry> =
        entries.iter().filter(|entry| Some(&entry.driver_kind) == first_driver.as_ref()).cloned().collect();
    let entry = resolve_selectable_provider_instance_entry(&group, None)
        .or_else(|| resolve_selectable_provider_instance_entry(&entries, None))?;
    let model = get_default_provider_instance_model(providers, &entry.instance_id)?;
    Some(ModelSelection { instance_id: entry.instance_id.clone(), model, options: None })
}

/// `slugifyLabel`: lowercase, `_` between runs, at most 48 characters.
fn slugify_label(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    let mut out = String::new();
    let mut gap = false;
    for ch in lower.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(ch);
        } else {
            gap = true;
        }
    }
    // Leading and trailing runs are already dropped; the cut comes after the trim, as in the source.
    out.chars().take(48).collect()
}

/// `deriveInstanceId`: `<driver>_<slug>`, or empty when the label has no letters or digits.
pub fn derive_instance_id(driver: &str, label: &str) -> String {
    let slug = slugify_label(label);
    if slug.is_empty() { String::new() } else { format!("{driver}_{slug}") }
}

/// `deriveAvailableInstanceId`: appends `_2`, `_3`… until the id is free, within 64 characters.
pub fn derive_available_instance_id(base: String, existing: &HashSet<String>) -> String {
    if base.is_empty() || !existing.contains(&base) {
        return base;
    }
    (2..)
        .map(|suffix| {
            let suffix = format!("_{suffix}");
            let keep = 64usize.saturating_sub(suffix.len());
            format!("{}{suffix}", base.chars().take(keep).collect::<String>())
        })
        .find(|candidate| !existing.contains(candidate))
        .unwrap_or_default()
}
