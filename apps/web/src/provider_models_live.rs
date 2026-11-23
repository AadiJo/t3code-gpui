//! The model picker's catalog from an environment's provider snapshots (`providerModels.ts`,
//! the model parts of `modelSelection.ts`): instances in the fork's order (Claude before
//! OpenAI), models in the owner's saved order, the owner's favourites and hidden models, and
//! the reasoning-effort, speed and ultracode options each model offers. Also converts between
//! the picker's [`ModelSelection`] and the wire's.

use std::collections::{HashMap, HashSet};

use gpui_kit::SharedString;
use t3code_contracts::{
    ModelSelection as WireSelection, ProviderOptionSelection, ProviderOptionSelectionValue, ProviderOptionSelections,
    ServerConfig, ServerConfigProvidersItem, ServerConfigProvidersItemModelsItem,
    ServerConfigProvidersItemModelsItemCapabilitiesOptionDescriptorsItem as Descriptor, ServerProviderState,
};

use crate::model_ordering::order_claude_before_openai;
use crate::model_selection::{
    ModelCatalog, ModelOption, ModelSelection, OptionChoice, ProviderDriverKind, ProviderInstanceEntry, ProviderInstanceId,
    SelectDescriptor, provider_model_key,
};

/// The option ids the picker knows: reasoning effort, speed and the ultracode switch.
const EFFORT_IDS: &[&str] = &["reasoningEffort", "effort", "reasoning_effort"];
const SPEED_IDS: &[&str] = &["speed", "serviceTier", "fastMode"];
const ULTRACODE_ID: &str = "ultracode";

fn driver_kind(driver: &str) -> ProviderDriverKind {
    match driver {
        "codex" => ProviderDriverKind::Codex,
        "claudeAgent" => ProviderDriverKind::ClaudeAgent,
        "cursor" => ProviderDriverKind::Cursor,
        "opencode" => ProviderDriverKind::OpenCode,
        "grok" => ProviderDriverKind::Grok,
        "antigravity" => ProviderDriverKind::Antigravity,
        other => ProviderDriverKind::Other(SharedString::from(other.to_owned())),
    }
}

fn select(descriptor: &Descriptor) -> Option<SelectDescriptor> {
    let Descriptor::Select(select) = descriptor else { return None };
    Some(SelectDescriptor {
        label: SharedString::from(select.label.clone()),
        options: select
            .options
            .iter()
            .map(|option| OptionChoice {
                id: SharedString::from(option.id.clone()),
                label: SharedString::from(option.label.clone()),
                is_default: option.is_default == Some(true),
            })
            .collect(),
    })
}

fn descriptor_id(descriptor: &Descriptor) -> Option<&str> {
    match descriptor {
        Descriptor::Select(select) => Some(&select.id),
        Descriptor::Boolean(boolean) => Some(&boolean.id),
        Descriptor::Unknown(_) => None,
    }
}

fn model_option(model: &ServerConfigProvidersItemModelsItem, unavailable: bool) -> ModelOption {
    let descriptors = model.capabilities.as_ref().and_then(|c| c.option_descriptors.as_deref()).unwrap_or_default();
    let find = |ids: &[&str]| descriptors.iter().find(|d| descriptor_id(d).is_some_and(|id| ids.contains(&id)));
    let effort = descriptors
        .iter()
        .find(|d| matches!(d, Descriptor::Select(s) if s.role.is_some()))
        .or_else(|| find(EFFORT_IDS))
        .and_then(select);
    ModelOption {
        slug: SharedString::from(model.slug.clone()),
        name: SharedString::from(model.name.clone()),
        short_name: model.short_name.clone().map(SharedString::from),
        sub_provider: model.sub_provider.clone().map(SharedString::from),
        is_legacy: model.is_legacy == Some(true),
        is_unavailable: unavailable,
        effort,
        speed: find(SPEED_IDS).and_then(select),
        ultracode: find(&[ULTRACODE_ID]).is_some(),
    }
}

fn is_ready(provider: &ServerConfigProvidersItem) -> bool {
    provider.enabled && provider.installed && matches!(provider.status, ServerProviderState::Ready | ServerProviderState::Warning)
}

/// The picker's catalog for one environment's config.
pub fn model_catalog(config: &ServerConfig) -> ModelCatalog {
    let settings = &config.settings;
    let preferences = settings.provider_model_preferences.as_ref();
    let instances: Vec<ProviderInstanceEntry> = config
        .providers
        .iter()
        .filter(|provider| provider.enabled)
        .map(|provider| ProviderInstanceEntry {
            instance_id: ProviderInstanceId(SharedString::from(provider.instance_id.clone())),
            driver_kind: driver_kind(&provider.driver),
            display_name: SharedString::from(provider.display_name.clone().unwrap_or_else(|| provider.instance_id.clone())),
            ready: is_ready(provider),
            unavailable_reason: provider.unavailable_reason.clone().or_else(|| provider.message.clone()).map(SharedString::from),
        })
        .collect();
    let instances = order_claude_before_openai(&instances, |entry| &entry.driver_kind);
    let mut models = HashMap::new();
    let mut hidden = HashMap::new();
    for provider in &config.providers {
        let id = ProviderInstanceId(SharedString::from(provider.instance_id.clone()));
        let prefs = preferences.and_then(|prefs| prefs.get(&provider.instance_id));
        let unavailable = !is_ready(provider);
        let mut list: Vec<ModelOption> = provider.models.iter().map(|model| model_option(model, unavailable)).collect();
        if let Some(order) = prefs.and_then(|p| p.model_order.as_ref()) {
            list.sort_by_key(|model| order.iter().position(|slug| slug.as_str() == model.slug.as_ref()).unwrap_or(usize::MAX));
        }
        if let Some(hidden_models) = prefs.and_then(|p| p.hidden_models.as_ref()) {
            hidden.insert(id.clone(), hidden_models.iter().map(|slug| SharedString::from(slug.clone())).collect::<HashSet<_>>());
        }
        models.insert(id, list);
    }
    let favorites = settings
        .favorites
        .iter()
        .flatten()
        .map(|favorite| provider_model_key(&ProviderInstanceId(SharedString::from(favorite.provider.to_string())), &favorite.model))
        .collect();
    ModelCatalog { instances, models, favorites, hidden }
}

fn option_value(options: Option<&ProviderOptionSelections>, ids: &[&str]) -> Option<String> {
    match options? {
        ProviderOptionSelections::List(list) => list.iter().find(|o| ids.contains(&o.id.as_str())).and_then(|o| match &o.value {
            ProviderOptionSelectionValue::Text(value) => Some(value.clone()),
            ProviderOptionSelectionValue::Bool(value) => Some(value.to_string()),
        }),
        ProviderOptionSelections::Map(map) => ids.iter().find_map(|id| map.get(*id)).map(|value| match value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        }),
    }
}

/// The picker's selection from the wire's.
pub fn from_wire(selection: &WireSelection) -> ModelSelection {
    let options = selection.options.as_ref();
    ModelSelection {
        instance_id: ProviderInstanceId(SharedString::from(selection.instance_id.clone())),
        model: SharedString::from(selection.model.clone()),
        effort: option_value(options, EFFORT_IDS).map(SharedString::from),
        speed: option_value(options, SPEED_IDS).map(SharedString::from),
        ultracode: option_value(options, &[ULTRACODE_ID]).is_some_and(|value| value == "true"),
    }
}

/// The wire's selection from the picker's. Option ids come from the model's descriptors.
pub fn to_wire(selection: &ModelSelection, catalog: &ModelCatalog, config: Option<&ServerConfig>) -> WireSelection {
    let descriptors = config
        .and_then(|config| config.providers.iter().find(|p| p.instance_id == selection.instance_id.0.as_ref()))
        .and_then(|provider| provider.models.iter().find(|m| m.slug == selection.model.as_ref()))
        .and_then(|model| model.capabilities.as_ref())
        .and_then(|capabilities| capabilities.option_descriptors.clone())
        .unwrap_or_default();
    let id_for = |ids: &[&str]| descriptors.iter().filter_map(descriptor_id).find(|id| ids.contains(id)).map(str::to_owned);
    let mut options = Vec::new();
    let effort_id = descriptors
        .iter()
        .find(|d| matches!(d, Descriptor::Select(s) if s.role.is_some()))
        .and_then(descriptor_id)
        .map(str::to_owned)
        .or_else(|| id_for(EFFORT_IDS));
    if let (Some(id), Some(effort)) = (effort_id, &selection.effort) {
        options.push(ProviderOptionSelection { id, value: ProviderOptionSelectionValue::Text(effort.to_string()) });
    }
    if let (Some(id), Some(speed)) = (id_for(SPEED_IDS), &selection.speed) {
        options.push(ProviderOptionSelection { id, value: ProviderOptionSelectionValue::Text(speed.to_string()) });
    }
    if selection.ultracode && catalog.model(&selection.instance_id, &selection.model).is_some_and(|m| m.ultracode) {
        options.push(ProviderOptionSelection { id: ULTRACODE_ID.to_owned(), value: ProviderOptionSelectionValue::Bool(true) });
    }
    WireSelection {
        instance_id: selection.instance_id.0.to_string().into(),
        model: selection.model.to_string(),
        options: (!options.is_empty()).then_some(ProviderOptionSelections::List(options)),
    }
}
