//! Where a prompt link opens (`client-runtime/promptLinks.ts`): the projects of the repository
//! it names and the model selection for the provider it asks for.

use t3code_contracts::{ModelSelection, RepositoryIdentity, ServerConfigProvidersItem, ServerProviderAvailability, ServerProviderState};
use t3code_shared::prompt_links::PromptLinkRepository;

/// A link's `owner/name` is GitHub's. GitCafe syncs a repository two-way with GitHub under the
/// same path, so a checkout cloned from git.cafe counts as the linked repository.
const PROMPT_LINK_REPOSITORY_HOSTS: [&str; 3] = ["github.com", "git.cafe", "staging.git.cafe"];

/// `findPromptLinkProjects`: the indices of the repository's projects in the order given.
/// Callers pass projects most recently active first and open the first.
pub fn find_prompt_link_projects<'a>(
    identities: impl IntoIterator<Item = Option<&'a RepositoryIdentity>>,
    repository: &PromptLinkRepository,
) -> Vec<usize> {
    let path = repository.path().to_lowercase();
    identities
        .into_iter()
        .enumerate()
        .filter(|(_, identity)| {
            identity.is_some_and(|identity| {
                let key = identity.grouping_key().to_lowercase();
                PROMPT_LINK_REPOSITORY_HOSTS.iter().any(|host| key.strip_prefix(host).and_then(|rest| rest.strip_prefix('/')) == Some(path.as_str()))
            })
        })
        .map(|(index, _)| index)
        .collect()
}

/// A resolved link model: `exact` is false when the instance exists but the model does not, so
/// its default model stands in.
#[derive(Clone, Debug, PartialEq)]
pub struct PromptLinkModelSelection {
    pub selection: ModelSelection,
    pub exact: bool,
}

/// `resolvePromptLinkModelSelection`: the first ready (else warning) enabled, available
/// instance of the link's provider, with the link's model when it offers it. `resolve_model` is
/// `resolveSelectableModel` (driver, requested model, the instance's models). `None` when no
/// usable instance exists; the user's default applies then.
pub fn resolve_prompt_link_model_selection(
    providers: &[ServerConfigProvidersItem],
    provider: &str,
    model: Option<&str>,
    resolve_model: impl Fn(&ServerConfigProvidersItem, &str) -> Option<String>,
) -> Option<PromptLinkModelSelection> {
    // `PROVIDER_ALIASES`.
    let driver = if provider == "claude" { "claudeagent" } else { provider };
    let usable: Vec<_> = providers
        .iter()
        .filter(|p| p.driver.to_lowercase() == driver && p.enabled && p.availability != Some(ServerProviderAvailability::Unavailable))
        .collect();
    let instance = usable
        .iter()
        .find(|p| p.status == ServerProviderState::Ready)
        .or_else(|| usable.iter().find(|p| p.status == ServerProviderState::Warning))?;
    let requested = model.and_then(|model| resolve_model(instance, model));
    let chosen = requested.clone().or_else(|| {
        instance.models.iter().find(|m| m.is_default == Some(true)).or_else(|| instance.models.first()).map(|m| m.slug.clone())
    })?;
    Some(PromptLinkModelSelection {
        selection: ModelSelection { instance_id: instance.instance_id.clone(), model: chosen, options: None },
        exact: model.is_none() || requested.is_some(),
    })
}
