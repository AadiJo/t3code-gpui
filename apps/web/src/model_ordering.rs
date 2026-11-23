//! Picker ordering. Ports `modelOrdering.ts` and the fork's
//! `packages/client-runtime/src/providerPickerOrder.ts`.

use crate::model_selection::{ProviderDriverKind, ProviderInstanceId, provider_model_key};

/// Moves every Claude entry directly before the first OpenAI entry when a
/// Claude entry comes after it; all other entries keep server order. Applies
/// to the rail, lists and favourites. Ports `orderClaudeBeforeOpenAi`.
pub fn order_claude_before_openai<T: Clone>(items: &[T], driver_of: impl Fn(&T) -> &ProviderDriverKind) -> Vec<T> {
    let first_openai = items.iter().position(|i| *driver_of(i) == ProviderDriverKind::Codex);
    let first_claude = items.iter().position(|i| *driver_of(i) == ProviderDriverKind::ClaudeAgent);
    let (Some(openai), Some(claude)) = (first_openai, first_claude) else {
        return items.to_vec();
    };
    if claude < openai {
        return items.to_vec();
    }
    let (claudes, rest): (Vec<_>, Vec<_>) =
        items.iter().cloned().partition(|i| *driver_of(i) == ProviderDriverKind::ClaudeAgent);
    let mut out = Vec::with_capacity(items.len());
    out.extend_from_slice(&rest[..openai]);
    out.extend(claudes);
    out.extend_from_slice(&rest[openai..]);
    out
}

/// An item the picker sorts.
pub trait ProviderModelItem {
    fn instance_id(&self) -> &ProviderInstanceId;
    fn slug(&self) -> &str;
}

/// Favourites first (when grouped), then instance order, then original order.
/// Ports `sortProviderModelItems`.
pub fn sort_provider_model_items<T: ProviderModelItem + Clone>(
    items: &[T],
    favorites: &std::collections::HashSet<String>,
    group_favorites: bool,
    instance_order: &[ProviderInstanceId],
) -> Vec<T> {
    let mut indexed: Vec<(usize, &T)> = items.iter().enumerate().collect();
    indexed.sort_by_key(|(original, item)| {
        let favorite = group_favorites && favorites.contains(&provider_model_key(item.instance_id(), item.slug()));
        let instance_rank = instance_order.iter().position(|id| id == item.instance_id()).unwrap_or(usize::MAX);
        (!favorite, instance_rank, *original)
    });
    indexed.into_iter().map(|(_, item)| item.clone()).collect()
}
