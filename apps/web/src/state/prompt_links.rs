//! What a `/new` prompt link opens on live data (`NewThreadLinkView.tsx`'s decision): the
//! thread already on its pull request, its repository's most recent project, or a projectless
//! draft; and the clone it offers when the repository is missing.

use std::sync::Arc;

use gpui_kit::{App, SharedString, Task};
use t3code_client_runtime::connection::ConnectionPhase;
use t3code_client_runtime::prompt_links::{find_prompt_link_projects, resolve_prompt_link_model_selection};
use t3code_client_runtime::state::SyncStatus;
use t3code_client_runtime::state::thread_pull_requests::{thread_pull_request_key_of, visible_thread_pull_requests};
use t3code_contracts::rpc::methods::PullRequestsLinkedThreads;
use t3code_contracts::{ModelSelection, OrchestrationProjectShell, PullRequestRef};
use t3code_shared::prompt_links::{NewThreadLink, PromptLinkPullRequest, PromptLinkRepository};

use super::StateBridge;
use crate::components::sidebar::model::{ProjectRef, ThreadKey};
use crate::i18n::t;

/// A project a link can open in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkProject {
    pub project: ProjectRef,
    pub workspace_root: String,
}

/// Where the link lands once every reachable shell is live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkDecision {
    /// The newest thread on the link's pull request.
    Thread(ThreadKey),
    /// No repository named: a No project draft.
    Scratch,
    /// The repository's most recent project.
    Draft(LinkProject),
    /// The repository is not here: offer to clone it.
    MissingRepository(PromptLinkRepository),
    /// A pull request whose repository is here but has no thread: ask before checking out.
    PullRequestCheckout(LinkProject),
}

/// `allReachableShellsLiveAtom`: resolves once every environment's shell is live, or its
/// connection has settled offline (offline, blocked, or past its second backoff).
pub fn wait_until_ready(cx: &App) -> Task<()> {
    let stores: Vec<_> = StateBridge::environments(cx).read(cx).list.iter().map(|env| env.read(cx).store.clone()).collect();
    let task = StateBridge::spawn(
        async move {
            for store in stores {
                let mut shell = store.shell().subscribe();
                let mut status = store.context().supervisor.status.clone();
                loop {
                    let live = {
                        let shell = shell.borrow_and_update();
                        shell.status == SyncStatus::Live && shell.snapshot.is_some()
                    };
                    let offline = {
                        let status = status.borrow_and_update();
                        matches!(status.phase, ConnectionPhase::Offline | ConnectionPhase::Blocked)
                            || (status.phase == ConnectionPhase::Backoff && status.attempt > 2)
                    };
                    if live || offline {
                        break;
                    }
                    tokio::select! {
                        changed = shell.changed() => if changed.is_err() { break },
                        changed = status.changed() => if changed.is_err() { break },
                    }
                }
            }
        },
        cx,
    );
    cx.background_executor().spawn(async move {
        task.await;
    })
}

/// A non-scratch, non-archived project with its environment.
struct Candidate {
    project: LinkProject,
    shell: OrchestrationProjectShell,
    timestamp: String,
}

/// `sortScopedProjectsForSidebar(…, "updated_at")` over projects that are not the No project
/// workspace: newest thread activity first, else the project's own update time.
fn recent_projects(cx: &App) -> Vec<Candidate> {
    let mut out = Vec::new();
    for env in StateBridge::environments(cx).read(cx).list.iter() {
        let env = env.read(cx);
        let Some(snapshot) = env.shell.snapshot.as_ref() else { continue };
        let scratch = env.config.as_ref().and_then(|c| c.scratch_workspace_root.clone());
        let environment_id = super::shell::environment_key(env.environment_id());
        for project in snapshot.projects.iter() {
            if project.archived_at.is_some() || scratch.as_deref() == Some(project.workspace_root.as_str()) {
                continue;
            }
            let latest = snapshot
                .threads
                .iter()
                .filter(|t| t.project_id == project.id && t.archived_at.is_none())
                .map(|t| t.updated_at.clone())
                .max()
                .unwrap_or_else(|| project.updated_at.clone());
            out.push(Candidate {
                project: LinkProject {
                    project: ProjectRef { environment_id: environment_id.clone(), project_id: super::palette::project_key(&project.id) },
                    workspace_root: project.workspace_root.clone(),
                },
                shell: project.as_ref().clone(),
                timestamp: latest,
            });
        }
    }
    out.sort_by(|a, b| {
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| a.shell.title.cmp(&b.shell.title))
            .then_with(|| a.project.project.environment_id.0.cmp(&b.project.project.environment_id.0))
            .then_with(|| a.project.project.project_id.0.cmp(&b.project.project.project_id.0))
    });
    out
}

/// The repository's projects, most recent first (`findPromptLinkProjects`).
fn repository_projects(repository: &PromptLinkRepository, cx: &App) -> Vec<Candidate> {
    let mut recent = recent_projects(cx);
    let found = find_prompt_link_projects(recent.iter().map(|c| c.shell.repository_identity.as_ref()), repository);
    let mut out = Vec::with_capacity(found.len());
    for index in found.into_iter().rev() {
        out.push(recent.swap_remove(index));
    }
    out.reverse();
    out
}

/// `findPromptLinkPullRequestThread`: the newest visible thread linked to the pull request.
fn local_pull_request_thread(pull_request: &PromptLinkPullRequest, cx: &App) -> Option<ThreadKey> {
    let key = format!("github.com/{}#{}", pull_request.repository.path(), pull_request.number).to_lowercase();
    let mut latest: Option<(String, ThreadKey)> = None;
    for env in StateBridge::environments(cx).read(cx).list.iter() {
        let env = env.read(cx);
        let Some(snapshot) = env.shell.snapshot.as_ref() else { continue };
        for thread in snapshot.threads.iter() {
            if latest.as_ref().is_some_and(|(at, _)| thread.updated_at <= *at) || thread.lineage.side_chat == Some(true) {
                continue;
            }
            let links = thread.pull_requests.as_deref().unwrap_or_default();
            if visible_thread_pull_requests(links).iter().any(|link| thread_pull_request_key_of(link) == key) {
                latest = Some((thread.updated_at.clone(), super::shell::thread_key(env.environment_id(), &thread.id)));
            }
        }
    }
    latest.map(|(_, key)| key)
}

/// `pickPromptLinkPullRequestThread`: asks each environment with the repository, in project
/// order, for an open thread on the pull request (it can be an older thread no shell holds).
fn server_pull_request_thread(pull_request: &PromptLinkPullRequest, projects: &[Candidate], cx: &App) -> Task<Option<ThreadKey>> {
    let lookups: Vec<_> = projects
        .iter()
        .filter_map(|candidate| {
            let environment_id = runtime_environment(&candidate.project.project.environment_id);
            let link = crate::live::rpc::RpcLink::new(&environment_id, cx)?;
            let input = PullRequestRef {
                project_id: candidate.shell.id.clone(),
                repository: pull_request.repository.path(),
                number: pull_request.number,
                host: None,
                expected_account_id: None,
                allow_stale: None,
            };
            Some((environment_id, link.call::<PullRequestsLinkedThreads>(input)))
        })
        .collect();
    cx.background_executor().spawn(async move {
        for (environment_id, lookup) in lookups {
            if let Ok(result) = lookup.await
                && let Some(thread) = result.threads.iter().find(|t| t.archived_at.is_none())
            {
                return Some(super::shell::thread_key(&environment_id, &thread.id));
            }
        }
        None
    })
}

/// Decides where `link` lands. Call once [`wait_until_ready`] resolved.
pub fn decide(link: &NewThreadLink, cx: &App) -> Task<LinkDecision> {
    let repository = link.repository().cloned();
    let projects = repository.as_ref().map(|r| repository_projects(r, cx)).unwrap_or_default();
    let server = match link {
        NewThreadLink::PullRequest { pull_request, .. } => match local_pull_request_thread(pull_request, cx) {
            Some(thread) => return Task::ready(LinkDecision::Thread(thread)),
            None => Some(server_pull_request_thread(pull_request, &projects, cx)),
        },
        NewThreadLink::New { .. } => None,
    };
    let first = projects.into_iter().next().map(|c| c.project);
    let pull_request = matches!(link, NewThreadLink::PullRequest { .. });
    let settle = move || match (repository, first) {
        (None, _) => LinkDecision::Scratch,
        (Some(repository), None) => LinkDecision::MissingRepository(repository),
        (Some(_), Some(project)) if pull_request => LinkDecision::PullRequestCheckout(project),
        (Some(_), Some(project)) => LinkDecision::Draft(project),
    };
    match server {
        Some(lookup) => gpui_kit::App::background_executor(cx).spawn(async move { lookup.await.map(LinkDecision::Thread).unwrap_or_else(settle) }),
        None => Task::ready(settle()),
    }
}

/// `lastActiveEnvironmentId`: the environment of the most recently updated thread.
pub fn last_active_environment(cx: &App) -> Option<t3code_contracts::EnvironmentId> {
    let mut latest: Option<(String, t3code_contracts::EnvironmentId)> = None;
    for env in StateBridge::environments(cx).read(cx).list.iter() {
        let env = env.read(cx);
        let Some(snapshot) = env.shell.snapshot.as_ref() else { continue };
        if let Some(at) = snapshot.threads.iter().map(|t| &t.updated_at).max()
            && latest.as_ref().is_none_or(|(best, _)| at > best)
        {
            latest = Some((at.clone(), env.environment_id().clone()));
        }
    }
    latest.map(|(_, id)| id)
}

/// The link's model for `project`'s environment, with the note the fork adds when it falls
/// back (`resolvePromptLinkModelSelection` in `openDraft`).
pub fn link_model(link: &NewThreadLink, project: &ProjectRef, cx: &App) -> (Option<ModelSelection>, Option<SharedString>) {
    let NewThreadLink::New { provider: Some(provider), model, .. } = link else { return (None, None) };
    let environment_id = runtime_environment(&project.environment_id);
    let config = StateBridge::environment(&environment_id, cx).and_then(|env| env.read(cx).config.clone());
    let providers = config.as_ref().map(|c| c.providers.as_slice()).unwrap_or_default();
    let resolved = resolve_prompt_link_model_selection(providers, provider, model.as_deref(), |instance, requested| {
        crate::provider_models::resolve_selectable_model(&instance.driver, Some(requested), &instance.models).map(str::to_owned)
    });
    let note = match &resolved {
        None => Some(t!(
            "The link asked for {provider}, which is not set up here, so your default model is used.",
            provider = provider.as_str()
        )),
        Some(found) if !found.exact => Some(t!(
            "The link asked for {model}, which {provider} does not offer here, so its default model is used.",
            model = model.as_deref().unwrap_or_default(),
            provider = provider.as_str()
        )),
        Some(_) => None,
    };
    (resolved.map(|r| r.selection), note)
}

/// What a clone from a link needs: the environment, the URL and where it goes
/// (`cloneRepository`). `None` when that machine cannot clone from a link.
pub fn clone_target(repository: &PromptLinkRepository, cx: &App) -> Option<(crate::components::sidebar::model::EnvironmentId, String, String)> {
    let environment_id = last_active_environment(cx)?;
    let env = StateBridge::environment(&environment_id, cx)?;
    let config: Arc<_> = env.read(cx).config.clone()?;
    let new_projects_root = config.new_projects_root.clone()?;
    let remote_url = format!("https://github.com/{}/{}.git", repository.owner, repository.name);
    let ghq = config.settings.ghq_clone_layout == Some(true);
    use crate::components::command_palette_project_flow::{clone_destination, clone_directory_path, initial_query};
    let base = if ghq { initial_query(config.settings.add_project_base_directory.as_deref().unwrap_or_default()) } else { new_projects_root };
    let destination = clone_destination(&base, &clone_directory_path(&remote_url, ghq));
    Some((super::shell::environment_key(&environment_id), remote_url, destination))
}

fn runtime_environment(id: &crate::components::sidebar::model::EnvironmentId) -> t3code_contracts::EnvironmentId {
    t3code_contracts::EnvironmentId::new(id.0.as_ref())
}

/// `promptLinkRepositoryOf`: the GitHub repository of `project`, from its grouping key.
pub fn project_repository(project: &ProjectRef, cx: &App) -> Option<PromptLinkRepository> {
    let env = StateBridge::environment(&runtime_environment(&project.environment_id), cx)?;
    let env = env.read(cx);
    let snapshot = env.shell.snapshot.as_ref()?;
    let shell = snapshot.projects.iter().find(|p| p.id.0.as_ref() == project.project_id.0.as_ref())?;
    let key = shell.repository_identity.as_ref()?.grouping_key();
    t3code_shared::prompt_links::parse_prompt_link_repository(key.strip_prefix("github.com/")?)
}
