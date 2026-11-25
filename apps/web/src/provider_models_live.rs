//! The model picker's catalog from an environment's provider snapshots (`providerModels.ts`,
//! the model parts of `modelSelection.ts`): instances in the fork's order (Claude before
//! OpenAI), models in the owner's saved order, the owner's favourites and hidden models, and
//! every option each model offers (`capabilities.optionDescriptors`). Also converts between
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
    BooleanDescriptor, ModelCatalog, ModelOption, ModelSelection, OptionChoice, OptionDescriptor, OptionSelection, OptionValue,
    ProviderDriverKind, ProviderInstanceEntry, ProviderInstanceId, SelectDescriptor, dispatch_options, provider_model_key,
};

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

fn descriptor(descriptor: &Descriptor) -> Option<OptionDescriptor> {
    let text = |value: &String| SharedString::from(value.clone());
    Some(match descriptor {
        Descriptor::Select(select) => OptionDescriptor::Select(SelectDescriptor {
            id: text(&select.id),
            label: text(&select.label),
            options: select
                .options
                .iter()
                .map(|option| OptionChoice {
                    id: text(&option.id),
                    label: text(&option.label),
                    description: option.description.as_ref().map(text),
                    is_default: option.is_default == Some(true),
                })
                .collect(),
            prompt_injected: select.prompt_injected_values.iter().flatten().map(text).collect(),
            current: select.current_value.as_ref().map(text),
            reasoning_role: select.role.is_some(),
        }),
        Descriptor::Boolean(boolean) => {
            OptionDescriptor::Boolean(BooleanDescriptor { id: text(&boolean.id), label: text(&boolean.label), current: boolean.current_value })
        }
        Descriptor::Unknown(_) => return None,
    })
}

fn model_option(model: &ServerConfigProvidersItemModelsItem, driver: &str, unavailable: bool) -> ModelOption {
    let descriptors = model.capabilities.as_ref().and_then(|c| c.option_descriptors.as_deref()).unwrap_or_default();
    ModelOption {
        slug: SharedString::from(model.slug.clone()),
        name: SharedString::from(model.name.clone()),
        // `resolveModelShortName`: triggers and the effort column name "Sonnet 4.6", not "Claude Sonnet 4.6".
        short_name: crate::state::thread_relationships::resolve_model_short_name(&model.name, model.short_name.as_deref(), driver).map(SharedString::from),
        sub_provider: model.sub_provider.clone().map(SharedString::from),
        is_legacy: model.is_legacy == Some(true),
        is_unavailable: unavailable,
        options: descriptors.iter().filter_map(descriptor).collect(),
    }
}

fn is_ready(provider: &ServerConfigProvidersItem) -> bool {
    provider.enabled && provider.installed && matches!(provider.status, ServerProviderState::Ready | ServerProviderState::Warning)
}

/// The picker's catalog for one environment's config.
pub fn model_catalog(config: &ServerConfig) -> ModelCatalog {
    model_catalog_for(&config.providers, &config.settings)
}

/// The picker's catalog from an environment's providers and settings.
pub fn model_catalog_for<'a>(
    providers: impl IntoIterator<Item = &'a ServerConfigProvidersItem> + Clone,
    settings: &t3code_contracts::ServerSettings,
) -> ModelCatalog {
    let preferences = settings.provider_model_preferences.as_ref();
    let instances: Vec<ProviderInstanceEntry> = providers
        .clone()
        .into_iter()
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
    for provider in providers {
        let id = ProviderInstanceId(SharedString::from(provider.instance_id.clone()));
        let prefs = preferences.and_then(|prefs| prefs.get(&provider.instance_id));
        let unavailable = !is_ready(provider);
        let mut list: Vec<ModelOption> = provider.models.iter().map(|model| model_option(model, &provider.driver, unavailable)).collect();
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

/// The picker's selection from the wire's. An old object-form option list becomes a list.
pub fn from_wire(selection: &WireSelection) -> ModelSelection {
    let options = match &selection.options {
        None => Vec::new(),
        Some(ProviderOptionSelections::List(list)) => list
            .iter()
            .map(|o| OptionSelection {
                id: SharedString::from(o.id.clone()),
                value: match &o.value {
                    ProviderOptionSelectionValue::Text(value) => OptionValue::Text(SharedString::from(value.clone())),
                    ProviderOptionSelectionValue::Bool(value) => OptionValue::Bool(*value),
                },
            })
            .collect(),
        Some(ProviderOptionSelections::Map(map)) => map
            .iter()
            .filter_map(|(id, value)| {
                let value = match value {
                    serde_json::Value::String(text) => OptionValue::Text(SharedString::from(text.clone())),
                    serde_json::Value::Bool(value) => OptionValue::Bool(*value),
                    _ => return None,
                };
                Some(OptionSelection { id: SharedString::from(id.clone()), value })
            })
            .collect(),
    };
    ModelSelection {
        instance_id: ProviderInstanceId(SharedString::from(selection.instance_id.clone())),
        model: SharedString::from(selection.model.clone()),
        options,
    }
}

/// The wire's selection from the picker's, with the options a turn carries for the model
/// (`modelOptionsForDispatch`). A model the catalog does not know keeps its options as they are.
pub fn to_wire(selection: &ModelSelection, catalog: &ModelCatalog, _config: Option<&ServerConfig>) -> WireSelection {
    let options = match catalog.model(&selection.instance_id, &selection.model) {
        Some(model) => dispatch_options(&model.options, &selection.options),
        None => selection.options.clone(),
    };
    let options: Vec<_> = options
        .into_iter()
        .map(|o| ProviderOptionSelection {
            id: o.id.to_string(),
            value: match o.value {
                OptionValue::Text(text) => ProviderOptionSelectionValue::Text(text.to_string()),
                OptionValue::Bool(value) => ProviderOptionSelectionValue::Bool(value),
            },
        })
        .collect();
    WireSelection {
        instance_id: selection.instance_id.0.to_string().into(),
        model: selection.model.to_string(),
        options: (!options.is_empty()).then_some(ProviderOptionSelections::List(options)),
    }
}
