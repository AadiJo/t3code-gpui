//! Ports `usagePriceTable.ts` and `usagePriceTargets.ts`: what each cell of
//! the custom model prices table shows across the selected environments,
//! and the changes and errors a set of edited rows makes on each.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_contracts::{ClientServerSettingsPatch, EnvironmentId, UsageModelPriceOverride};

use super::usage_price_form::{UsagePriceField, parse_usage_price_form, usage_price_form};
use crate::i18n::t;

/// One environment the dialog can write to (`UsagePriceTarget`).
#[derive(Clone, Debug, PartialEq)]
pub struct UsagePriceTarget {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    /// `None` until the server settings load.
    pub prices: Option<Arc<BTreeMap<String, UsageModelPriceOverride>>>,
    /// `None` when the server cannot map models, or its settings are not loaded.
    pub aliases: Option<Arc<BTreeMap<String, String>>>,
    /// Why this environment cannot be written ("Offline", "Read-only access").
    pub unavailable: Option<SharedString>,
}

impl UsagePriceTarget {
    fn price(&self, model: &str) -> Option<&UsageModelPriceOverride> {
        self.prices.as_ref()?.get(model)
    }

    fn alias(&self, model: &str) -> Option<&str> {
        self.aliases.as_ref()?.get(model).map(String::as_str)
    }
}

/// One edited or added row (`UsagePriceDraft`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsagePriceDraft {
    pub id: SharedString,
    pub model: String,
    pub is_new: bool,
    /// Only the cells the user changed.
    pub values: BTreeMap<UsagePriceField, String>,
    /// The edited "Map to" model. `None` keeps each environment's mapping; blank removes it.
    pub alias: Option<String>,
    pub removed: bool,
}

impl UsagePriceDraft {
    /// `isEmptyUsagePriceDraft`: an added row nothing has been typed into.
    pub fn is_empty(&self) -> bool {
        self.is_new
            && self.model.trim().is_empty()
            && self.alias.as_deref().unwrap_or("").trim().is_empty()
            && self.values.values().all(|v| v.trim().is_empty())
    }
}

/// What a cell shows when the row has no edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsagePriceCell {
    pub value: String,
    pub placeholder: SharedString,
}

/// `usageAliasCell`.
pub fn usage_alias_cell(targets: &[UsagePriceTarget], model: &str) -> UsagePriceCell {
    if targets.iter().any(|t| t.aliases.is_none()) {
        return UsagePriceCell { value: String::new(), placeholder: t!("Unavailable") };
    }
    let values: Vec<&str> = targets.iter().map(|t| t.alias(model).unwrap_or("")).collect();
    if values.iter().any(|v| Some(v) != values.first()) {
        return UsagePriceCell { value: String::new(), placeholder: t!("Mixed") };
    }
    UsagePriceCell { value: values.first().copied().unwrap_or("").to_owned(), placeholder: t!("None") }
}

/// `usagePriceCell`.
pub fn usage_price_cell(targets: &[UsagePriceTarget], model: &str, field: UsagePriceField) -> UsagePriceCell {
    let values: Vec<Option<String>> =
        targets.iter().map(|t| t.price(model).map(|p| usage_price_form(model, Some(p)).get(field).to_owned())).collect();
    if targets.iter().any(|t| t.prices.is_none()) {
        return UsagePriceCell { value: String::new(), placeholder: t!("Unavailable") };
    }
    if values.iter().any(|v| Some(v) != values.first()) {
        return UsagePriceCell { value: String::new(), placeholder: t!("Mixed") };
    }
    let first = values.into_iter().next().flatten();
    let placeholder = match (&first, field.optional()) {
        (None, _) => t!("Automatic"),
        (Some(_), true) => t!("Input rate"),
        (Some(_), false) => "0.00".into(),
    };
    UsagePriceCell { value: first.unwrap_or_default(), placeholder }
}

/// One write to an environment's settings (`UsagePriceChange`); `None` removes it.
#[derive(Clone, Debug, PartialEq)]
pub enum UsagePriceChange {
    Price { model: String, price: Option<UsageModelPriceOverride> },
    Alias { model: String, alias: Option<String> },
}

/// The changes `drafts` make on `target`, and an error per row that cannot apply there.
pub fn usage_price_table_changes(
    target: &UsagePriceTarget,
    drafts: &[UsagePriceDraft],
) -> (Vec<UsagePriceChange>, HashMap<SharedString, SharedString>) {
    let mut changes = Vec::new();
    let mut errors: HashMap<SharedString, SharedString> = HashMap::new();
    for draft in drafts {
        if draft.is_empty() {
            continue;
        }
        let model = draft.model.trim().to_owned();
        let current_alias = target.alias(&model).map(str::to_owned);
        let has_price = target.price(&model).is_some();
        if draft.removed {
            if has_price {
                changes.push(UsagePriceChange::Price { model: model.clone(), price: None });
            }
            if current_alias.is_some() {
                changes.push(UsagePriceChange::Alias { model, alias: None });
            }
            continue;
        }
        let alias = draft.alias.clone().or_else(|| current_alias.clone()).unwrap_or_default().trim().to_owned();
        if !alias.is_empty() {
            if draft.alias.is_none() && !draft.values.is_empty() {
                errors.insert(
                    draft.id.clone(),
                    t!("{target} maps this model. Remove it from \"Apply to\" or clear its mapping first.", target = target.label.clone()),
                );
                continue;
            }
            if model.is_empty() {
                errors.insert(draft.id.clone(), t!("Enter a model ID."));
            } else if alias == model {
                errors.insert(draft.id.clone(), t!("Map to a different model."));
            } else {
                if Some(&alias) != current_alias.as_ref() {
                    changes.push(UsagePriceChange::Alias { model: model.clone(), alias: Some(alias) });
                }
                if has_price {
                    changes.push(UsagePriceChange::Price { model, price: None });
                }
            }
            continue;
        }
        if current_alias.is_some() {
            changes.push(UsagePriceChange::Alias { model: model.clone(), alias: None });
            if !has_price && draft.values.values().all(|v| v.trim().is_empty()) {
                continue;
            }
        }
        let original = usage_price_form(&model, target.price(&model));
        let mut form = original.clone();
        for (field, value) in &draft.values {
            form.set(*field, value.clone());
        }
        let Some((model, price)) = parse_usage_price_form(&form) else {
            let missing = UsagePriceField::ALL.into_iter().find(|f| !f.optional() && form.get(*f).trim().is_empty());
            let error = if model.is_empty() {
                t!("Enter a model ID.")
            } else if let Some(missing) = missing {
                t!("{field} is required on {target}.", field = missing.label(), target = target.label.clone())
            } else {
                t!("Use non-negative numbers for prices.")
            };
            errors.insert(draft.id.clone(), error);
            continue;
        };
        if usage_price_form(&model, Some(&price)) != original {
            changes.push(UsagePriceChange::Price { model, price: Some(price) });
        }
    }
    // Checks the final batch so clearing a mapping and reversing it in one save stays valid.
    let mut aliases: BTreeMap<String, String> = target.aliases.as_deref().cloned().unwrap_or_default();
    for change in &changes {
        if let UsagePriceChange::Alias { model, alias } = change {
            match alias {
                Some(alias) => aliases.insert(model.clone(), alias.clone()),
                None => aliases.remove(model),
            };
        }
    }
    for draft in drafts {
        if draft.removed || errors.contains_key(&draft.id) {
            continue;
        }
        let mut model = draft.model.trim().to_owned();
        let mut seen = BTreeSet::new();
        while let Some(next) = aliases.get(&model) {
            if !seen.insert(model.clone()) {
                errors.insert(draft.id.clone(), t!("Model mappings cannot form a cycle on {target}.", target = target.label.clone()));
                break;
            }
            model = next.clone();
        }
    }
    (changes, errors)
}

/// `usagePriceTableErrors`: errors from every writable target with loaded prices.
pub fn usage_price_table_errors(targets: &[UsagePriceTarget], drafts: &[UsagePriceDraft]) -> HashMap<SharedString, SharedString> {
    targets
        .iter()
        .filter(|t| t.unavailable.is_none() && t.prices.is_some())
        .flat_map(|t| usage_price_table_changes(t, drafts).1)
        .collect()
}

/// How one destination's save ended (`UsagePriceWriteResult`), or that it is still saving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsagePriceWriteResult {
    Saving,
    Saved,
    Failed(SharedString),
}

/// The settings patch for `changes`, or why `target` cannot take it
/// (`writeUsagePrices` before its write). `Ok(None)` means nothing to write.
pub fn usage_price_patch(target: &UsagePriceTarget, changes: &[UsagePriceChange]) -> Result<Option<ClientServerSettingsPatch>, SharedString> {
    if let Some(reason) = &target.unavailable {
        return Err(reason.clone());
    }
    let mut prices = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    for change in changes {
        match change {
            UsagePriceChange::Price { model, price } => prices.insert(model.clone(), price.clone()).map(|_| ()),
            UsagePriceChange::Alias { model, alias } => aliases.insert(model.clone(), alias.clone()).map(|_| ()),
        };
    }
    if !aliases.is_empty() && target.aliases.is_none() {
        return Err(t!("Update server to map models"));
    }
    if changes.is_empty() {
        return Ok(None);
    }
    Ok(Some(ClientServerSettingsPatch {
        usage_price_overrides: (!prices.is_empty()).then_some(prices),
        usage_model_aliases: (!aliases.is_empty()).then_some(aliases),
        ..Default::default()
    }))
}
