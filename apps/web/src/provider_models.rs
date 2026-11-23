//! Provider model lists and capabilities. Ports `providerModels.ts`,
//! `providerSubProviders.ts`, `providerUpdateRequiredModels.ts`, the model
//! lookup in `shared/model.ts` and `shared/semver.ts` comparison.

use std::cmp::Ordering;
use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_contracts::{
    ServerConfigProvidersItemModelsItemCapabilities as ModelCapabilities,
    ServerConfigProvidersItemModelsItemCapabilitiesOptionDescriptorsItem as OptionDescriptor,
    ServerConfigProvidersItemSubProvidersItem as SubProvider,
};

use crate::i18n::{list_format::format_conjunction_list, t};
use crate::provider_instances::{
    DEFAULT_MODEL, ServerProvider, ServerProviderModel, default_instance_id_for_driver, default_model_for_driver,
    provider_display_name, title_case_words,
};

/// Drivers that discover models at runtime, so a refresh can briefly omit one.
pub fn has_runtime_model_catalog(driver: &str) -> bool {
    matches!(driver, "opencode" | "antigravity" | "cursor" | "pi" | "acpRegistry")
}

/// Markers that follow the session's current model rather than naming one.
pub fn is_runtime_model_default_alias(driver: &str, model: Option<&str>) -> bool {
    matches!(driver, "antigravity" | "acpRegistry") && model.is_some() && model == default_model_for_driver(driver)
}

/// `keepsUnavailableModelOptions`.
pub fn keeps_unavailable_model_options(driver: &str, model: Option<&str>) -> bool {
    driver != "antigravity" && !is_runtime_model_default_alias(driver, model) && has_runtime_model_catalog(driver)
}

/// `formatProviderDriverKindLabel`: `claudeAgent` → `Claude Agent`.
pub fn format_provider_driver_kind_label(driver: &str) -> String {
    let mut spaced = String::with_capacity(driver.len() + 4);
    let mut prev: Option<char> = None;
    for ch in driver.chars() {
        if prev.is_some_and(|p| p.is_ascii_lowercase()) && ch.is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(ch);
        prev = Some(ch);
    }
    let mut collapsed = String::new();
    let mut run = false;
    for ch in spaced.chars() {
        if ch == '_' || ch == '-' {
            if !run {
                collapsed.push(' ');
            }
            run = true;
        } else {
            collapsed.push(ch);
            run = false;
        }
    }
    title_case_words(collapsed.trim())
}

/// The models of a driver's default instance.
pub fn get_provider_models<'a>(providers: &'a [Arc<ServerProvider>], driver: &str) -> &'a [ServerProviderModel] {
    let id = default_instance_id_for_driver(driver);
    providers.iter().find(|p| p.instance_id == id).map(|p| p.models.as_slice()).unwrap_or(&[])
}

/// `resolveSelectableProvider`: the requested enabled driver, else the first enabled, else Codex.
pub fn resolve_selectable_provider(providers: &[Arc<ServerProvider>], requested: Option<&str>) -> String {
    if let Some(entry) = requested.and_then(|id| providers.iter().find(|p| p.instance_id == id))
        && entry.enabled
    {
        return entry.driver.clone();
    }
    providers.iter().find(|p| p.enabled).map(|p| p.driver.clone()).unwrap_or_else(|| "codex".into())
}

/// `MODEL_SLUG_ALIASES_BY_PROVIDER`.
fn model_slug_alias(driver: &str, slug: &str) -> Option<&'static str> {
    Some(match (driver, slug) {
        ("codex", "gpt-5-codex" | "5.4") => "gpt-5.4",
        ("codex", "5.3" | "gpt-5.3") => "gpt-5.3-codex",
        ("codex", "5.3-spark" | "gpt-5.3-spark") => "gpt-5.3-codex-spark",
        ("cursor", "composer") => "composer-2",
        ("cursor", "composer-1.5" | "composer-1") => "composer-1.5",
        ("cursor", "opus-4.6-thinking" | "opus-4.6") => "claude-opus-4-6",
        ("cursor", "sonnet-4.6-thinking" | "sonnet-4.6") => "claude-sonnet-4-6",
        ("cursor", "opus-4.5-thinking" | "opus-4.5") => "claude-opus-4-5",
        _ => return None,
    })
}

/// `resolveSelectableModel`: slug, then name, then alias, then the driver's slug aliases.
pub fn resolve_selectable_model<'a>(driver: &str, value: Option<&str>, models: &'a [ServerProviderModel]) -> Option<&'a str> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(model) = models.iter().find(|m| m.slug == trimmed) {
        return Some(&model.slug);
    }
    let lower = trimmed.to_lowercase();
    if let Some(model) = models.iter().find(|m| m.name.to_lowercase() == lower) {
        return Some(&model.slug);
    }
    if let Some(model) =
        models.iter().find(|m| m.aliases.iter().flatten().any(|alias| alias.to_lowercase() == lower))
    {
        return Some(&model.slug);
    }
    let normalized = model_slug_alias(driver, trimmed).unwrap_or(trimmed);
    models.iter().find(|m| m.slug == normalized).map(|m| m.slug.as_str())
}

/// `getProviderModelCapabilities`: the plan agent is left out unless asked for.
pub fn get_provider_model_capabilities(
    models: &[ServerProviderModel],
    model: Option<&str>,
    driver: &str,
    include_plan_agent: bool,
) -> ModelCapabilities {
    let slug = resolve_selectable_model(driver, model, models);
    let caps = slug
        .and_then(|slug| models.iter().find(|m| m.slug == slug))
        .and_then(|m| m.capabilities.clone())
        .unwrap_or(ModelCapabilities { option_descriptors: Some(Vec::new()) });
    if include_plan_agent { caps } else { without_plan_agent_option(caps) }
}

fn without_plan_agent_option(caps: ModelCapabilities) -> ModelCapabilities {
    let descriptors = caps
        .option_descriptors
        .unwrap_or_default()
        .into_iter()
        .filter_map(|descriptor| match descriptor {
            OptionDescriptor::Select(mut select) if select.id == "agent" => {
                select.options.retain(|option| option.id != "plan");
                if select.options.is_empty() {
                    return None;
                }
                let keeps_current =
                    select.current_value.as_ref().is_some_and(|value| select.options.iter().any(|o| &o.id == value));
                if !keeps_current {
                    select.current_value = select
                        .options
                        .iter()
                        .find(|o| o.is_default == Some(true))
                        .or_else(|| select.options.first())
                        .map(|o| o.id.clone());
                }
                Some(OptionDescriptor::Select(select))
            }
            other => Some(other),
        })
        .collect();
    ModelCapabilities { option_descriptors: Some(descriptors) }
}

/// `getDefaultServerModel`.
pub fn get_default_server_model(providers: &[Arc<ServerProvider>], driver: &str) -> String {
    let models = get_provider_models(providers, driver);
    models
        .iter()
        .find(|m| m.is_default == Some(true) && !m.is_custom)
        .or_else(|| models.iter().find(|m| !m.is_custom))
        .or_else(|| models.first())
        .map(|m| m.slug.clone())
        .or_else(|| default_model_for_driver(driver).map(str::to_owned))
        .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
}

/// One sub-provider and its models.
#[derive(Clone, Debug, PartialEq)]
pub struct SubProviderGroup<'a, M> {
    pub sub_provider: &'a SubProvider,
    pub models: Vec<&'a M>,
}

/// `groupModelsBySubProvider`: models outside any sub-provider, then one group
/// per sub-provider in the provider's order (a signed-out one keeps its group).
pub fn group_models_by_sub_provider<'a, M>(
    models: &'a [M],
    sub_providers: &'a [SubProvider],
    sub_provider_of: impl Fn(&M) -> Option<&str>,
) -> (Vec<&'a M>, Vec<SubProviderGroup<'a, M>>) {
    let ungrouped =
        models.iter().filter(|m| !sub_providers.iter().any(|s| Some(s.name.as_str()) == sub_provider_of(m))).collect();
    let groups = sub_providers
        .iter()
        .map(|sub_provider| SubProviderGroup {
            sub_provider,
            models: models.iter().filter(|m| sub_provider_of(m) == Some(sub_provider.name.as_str())).collect(),
        })
        .collect();
    (ungrouped, groups)
}

/// A parsed `major.minor.patch-pre` version.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Semver {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Vec<String>,
}

/// `normalizeSemverVersion`: drops build metadata and pads to three parts.
pub fn normalize_semver_version(version: &str) -> String {
    let without_build = version.trim().split('+').next().unwrap_or("");
    let mut parts = without_build.split('-');
    let main = parts.next().unwrap_or("");
    let prerelease = parts.collect::<Vec<_>>().join("-");
    let mut segments: Vec<String> =
        main.split('.').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect();
    while !segments.is_empty() && segments.len() < 3 {
        segments.push("0".into());
    }
    if prerelease.is_empty() { segments.join(".") } else { format!("{}-{prerelease}", segments.join(".")) }
}

fn is_number_segment(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn parse_semver(value: &str) -> Option<Semver> {
    let normalized = normalize_semver_version(value);
    let normalized = normalized.strip_prefix('v').unwrap_or(&normalized);
    let mut parts = normalized.split('-');
    let main = parts.next().unwrap_or("");
    let prerelease = parts.collect::<Vec<_>>().join("-");
    let segments: Vec<&str> = main.split('.').collect();
    let [major, minor, patch] = segments.as_slice() else { return None };
    if ![major, minor, patch].iter().all(|s| is_number_segment(s)) {
        return None;
    }
    Some(Semver {
        major: major.parse().ok()?,
        minor: minor.parse().ok()?,
        patch: patch.parse().ok()?,
        prerelease: if prerelease.is_empty() {
            Vec::new()
        } else {
            prerelease.split('.').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
        },
    })
}

fn compare_prerelease_identifier(left: &str, right: &str) -> Ordering {
    match (is_number_segment(left), is_number_segment(right)) {
        (true, true) => {
            let (l, r) = (left.trim_start_matches('0'), right.trim_start_matches('0'));
            l.len().cmp(&r.len()).then_with(|| l.cmp(r))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        // JavaScript `<` on strings compares UTF-16 code units.
        (false, false) => left.encode_utf16().cmp(right.encode_utf16()),
    }
}

/// `compareSemverVersions`. Unparseable versions fall back to a plain string order.
pub fn compare_semver_versions(left: &str, right: &str) -> Ordering {
    let (Some(l), Some(r)) = (parse_semver(left), parse_semver(right)) else {
        return left.cmp(right);
    };
    l.major
        .cmp(&r.major)
        .then(l.minor.cmp(&r.minor))
        .then(l.patch.cmp(&r.patch))
        .then_with(|| match (l.prerelease.is_empty(), r.prerelease.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for index in 0..l.prerelease.len().max(r.prerelease.len()) {
                    match (l.prerelease.get(index), r.prerelease.get(index)) {
                        (None, _) => return Ordering::Less,
                        (_, None) => return Ordering::Greater,
                        (Some(a), Some(b)) => match compare_prerelease_identifier(a, b) {
                            Ordering::Equal => continue,
                            other => return other,
                        },
                    }
                }
                Ordering::Equal
            }
        })
}

/// `formatProviderUpdateRequiredNotice`: which models need a newer CLI, filtered by a search.
pub fn format_provider_update_required_notice(provider: &ServerProvider, search_query: &str, locale: &str) -> Option<SharedString> {
    let query = search_query.trim().to_lowercase();
    let models: Vec<_> = provider
        .update_required_models
        .iter()
        .flatten()
        .filter(|m| query.is_empty() || m.name.to_lowercase().contains(&query) || m.slug.to_lowercase().contains(&query))
        .collect();
    let first = models.first()?;
    let min_version = models
        .iter()
        .map(|m| m.min_version.as_str())
        .fold(first.min_version.as_str(), |highest, v| if compare_semver_versions(v, highest).is_gt() { v } else { highest });
    let provider_name = if provider.driver == "claudeAgent" {
        "Claude Code".to_owned()
    } else {
        provider_display_name(&provider.driver).map(str::to_owned).unwrap_or_else(|| provider.driver.clone())
    };
    let version = if min_version.starts_with('v') { min_version.to_owned() } else { format!("v{min_version}") };
    let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
    let names = format_conjunction_list(&names, locale);
    let runtime = if provider.driver == "codex" { "codex" } else { "other" };
    Some(t!(
        "{runtime, select, codex {Update the Codex CLI to {version} or newer to use {names}.} other {Update {providerName} to {version} or newer to use {names}.}}",
        runtime = runtime,
        version = version,
        names = names,
        providerName = provider_name
    ))
}
