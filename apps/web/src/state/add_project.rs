//! The Add project flows on live servers: `filesystem.browse`, `project.create` through
//! `projects.mutate`, `projects.createNew` (`useNewProject.ts`) and `projectClone.start`
//! (`CommandPalette.tsx` `submitAddProjectCloneFlow`), with the fork's toasts.

use std::sync::Arc;

use gpui_kit::{App, SharedString, Task};
use t3code_client_runtime::operations::commands::new_command_id;
use t3code_contracts::rpc::methods::{FilesystemBrowse, ProjectCloneStart, ProjectsCreateNew, ProjectsMutate};
use t3code_contracts::{
    ExecutionEnvironmentPlatformOs, FilesystemBrowseInput, ProjectCloneStartInput, ProjectCreateNewInput, ProjectId, ProjectMutation,
    ProjectMutationProjectCreate, ProjectMutationProjectUpdate,
};

use crate::components::command_palette_project_flow::{
    BrowseEntry, BrowseListing, FlowEnvironment, FlowOpened, ProjectFlowBackend, PublishTarget, RemoteSource, RepositoryInfo, default_clone_url, new_project_repository,
    publish_targets, remote_sources, title_from_path,
};
use crate::components::sidebar::model::{self, EnvironmentId, ProjectRef};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::live::rpc::RpcLink;

/// [`ProjectFlowBackend`] over the connected environments.
pub struct LiveProjectFlows;

/// Each environment's last source-control discovery (`sourceControlEnvironment.discovery`).
#[derive(Default)]
struct Discoveries(std::collections::HashMap<EnvironmentId, t3code_contracts::SourceControlDiscoveryResult>);

impl gpui_kit::Global for Discoveries {}

/// `SourceControlRepositoryError.message`.
fn repository_error_message(error: &t3code_contracts::SourceControlRepositoryError) -> String {
    format!("Source control repository operation {} failed for {}: {}", error.operation, error.provider.as_str(), error.detail)
}

fn discovered<'a>(id: &EnvironmentId, cx: &'a App) -> Option<&'a t3code_contracts::SourceControlDiscoveryResult> {
    cx.try_global::<Discoveries>()?.0.get(id)
}

/// Opens a draft in a project: the window's handler for a clone toast's "Open project".
type ProjectOpener = std::rc::Rc<dyn Fn(ProjectRef, &mut gpui_kit::Window, &mut App)>;

struct GlobalProjectOpener(ProjectOpener);

impl gpui_kit::Global for GlobalProjectOpener {}

/// Sets what "Open project" on a clone toast does; the window root calls it once.
pub fn set_project_opener(opener: ProjectOpener, cx: &mut App) {
    cx.set_global(GlobalProjectOpener(opener));
}

fn runtime_id(id: &EnvironmentId) -> t3code_contracts::EnvironmentId {
    t3code_contracts::EnvironmentId::new(id.0.as_ref())
}

fn project_ref(env: &EnvironmentId, id: &ProjectId) -> ProjectRef {
    ProjectRef { environment_id: env.clone(), project_id: model::ProjectId(SharedString::from(id.0.to_string())) }
}

fn error_toast(title: SharedString, description: SharedString, cx: &mut App) {
    let description = if description.is_empty() { t!("An error occurred.") } else { description };
    Toaster::push(ToastData::new(ToastKind::Error, title).description(description), cx);
}

/// `normalizeProjectPathForComparison`: trailing separators off, case kept.
fn comparable(path: &str) -> &str {
    let trimmed = path.trim();
    let stripped = trimmed.trim_end_matches(['/', '\\']);
    if stripped.is_empty() { trimmed } else { stripped }
}

/// Resolves once the environment's shell lists `project_id`, or after `seconds` with `None`.
fn wait_for_project(env: &EnvironmentId, project_id: ProjectId, seconds: u64, cx: &mut App) -> Task<Option<()>> {
    let Some(environment) = super::StateBridge::environment(&runtime_id(env), cx) else { return Task::ready(None) };
    let store = environment.read(cx).store.clone();
    let task = super::StateBridge::spawn(
        async move {
            let mut shell = store.shell().subscribe();
            let found = async {
                loop {
                    if shell.borrow().snapshot.as_ref().is_some_and(|s| s.projects.iter().any(|p| p.id == project_id)) {
                        return;
                    }
                    if shell.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(seconds), found).await.ok()
        },
        cx,
    );
    cx.background_executor().spawn(async move { task.await.flatten() })
}

impl ProjectFlowBackend for LiveProjectFlows {
    fn environment(&self, id: &EnvironmentId, cx: &App) -> Option<FlowEnvironment> {
        let environment = super::StateBridge::environment(&runtime_id(id), cx)?;
        let config = environment.read(cx).config.clone()?;
        Some(FlowEnvironment {
            id: id.clone(),
            label: config.environment.label.clone().into(),
            windows: config.environment.platform.os == ExecutionEnvironmentPlatformOs::Windows,
            base_directory: config.settings.add_project_base_directory.clone().unwrap_or_default().into(),
            new_projects_root: config.new_projects_root.clone().map(Into::into),
            ghq_clone_layout: config.settings.ghq_clone_layout == Some(true),
            remote_sources: remote_sources(discovered(id, cx), config.settings.offered_source_control_providers.as_ref()),
            publish_targets: publish_targets(discovered(id, cx), config.settings.offered_source_control_providers.as_ref()),
        })
    }

    fn discover(&self, id: &EnvironmentId, cx: &mut App) -> Task<()> {
        use t3code_contracts::rpc::methods::ServerDiscoverSourceControl;
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else { return Task::ready(()) };
        let call = link.call::<ServerDiscoverSourceControl>(Default::default());
        let env = id.clone();
        cx.spawn(async move |cx| {
            if let Ok(result) = call.await {
                cx.update(|cx| {
                    cx.default_global::<Discoveries>().0.insert(env, result);
                });
            }
        })
    }

    fn lookup_repository(&self, id: &EnvironmentId, source: &RemoteSource, repository: String, cx: &mut App) -> Task<Result<RepositoryInfo, SharedString>> {
        use t3code_contracts::rpc::methods::SourceControlLookupRepository;
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else { return Task::ready(Err(t!("The selected environment is not connected."))) };
        let kind = source.kind.clone();
        let call = link.call_with::<SourceControlLookupRepository>(
            t3code_contracts::SourceControlRepositoryLookupInput { provider: t3code_contracts::SourceControlProviderKind::new(kind.as_ref()), repository, cwd: None },
            |error| match error {
                t3code_contracts::SourceControlLookupRepositoryError::Repository(error) => Some(repository_error_message(error)),
                _ => None,
            },
        );
        cx.background_executor().spawn(async move {
            let info = call.await?;
            Ok(RepositoryInfo {
                name_with_owner: info.name_with_owner.clone().into(),
                url: info.url.clone().into(),
                clone_url: default_clone_url(&kind, &info.url, &info.ssh_url).into(),
            })
        })
    }

    fn publish_new_project(&self, id: &EnvironmentId, workspace_root: String, target: PublishTarget, cx: &mut App) {
        use t3code_contracts::rpc::methods::SourceControlPublishRepository;
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else { return };
        let label = target.label.clone();
        let call = link.call_with::<SourceControlPublishRepository>(
            t3code_contracts::SourceControlPublishRepositoryInput {
                cwd: workspace_root.clone(),
                provider: t3code_contracts::SourceControlProviderKind::new(target.kind.as_ref()),
                repository: new_project_repository(&target, &workspace_root),
                visibility: t3code_contracts::SourceControlRepositoryVisibility::Private,
                remote_name: None,
                protocol: None,
            },
            |error| match error {
                t3code_contracts::SourceControlPublishRepositoryError::Repository(error) => Some(repository_error_message(error)),
                _ => None,
            },
        );
        cx.spawn(async move |cx| {
            let result = call.await;
            cx.update(|cx| match result {
                Ok(published) => {
                    Toaster::push(
                        ToastData::new(ToastKind::Success, t!("Published to {label}", label = label.clone())).description(published.repository.name_with_owner.clone()).stacked(),
                        cx,
                    );
                }
                Err(error) => {
                    let error = if error.is_empty() { t!("An error occurred.") } else { error };
                    Toaster::push(
                        ToastData::new(ToastKind::Error, t!("Could not create the {label} repository", label = label.clone()))
                            .description(t!("{error} Use Publish Repository in the Git menu to try again.", error = error))
                            .stacked(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn browse(&self, id: &EnvironmentId, partial_path: String, cx: &mut App) -> Task<Result<BrowseListing, SharedString>> {
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else { return Task::ready(Err(t!("This environment is not connected."))) };
        let call = link.call::<FilesystemBrowse>(FilesystemBrowseInput { partial_path, cwd: None });
        cx.background_executor().spawn(async move {
            let result = call.await?;
            Ok(BrowseListing {
                parent_path: result.parent_path.into(),
                entries: result.entries.into_iter().map(|e| BrowseEntry { name: e.name.into(), full_path: e.full_path.into() }).collect::<Arc<[_]>>(),
            })
        })
    }

    fn add_project(&self, id: &EnvironmentId, path: String, cx: &mut App) -> Task<Option<FlowOpened>> {
        let Some(environment) = super::StateBridge::environment(&runtime_id(id), cx) else {
            error_toast(t!("Environment unavailable"), t!("The selected environment is not connected."), cx);
            return Task::ready(None);
        };
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else {
            error_toast(t!("Environment unavailable"), t!("The selected environment is not connected."), cx);
            return Task::ready(None);
        };
        let existing = environment.read(cx).shell.snapshot.as_ref().and_then(|snapshot| {
            snapshot.projects.iter().find(|p| comparable(&p.workspace_root) == comparable(&path)).map(|p| (p.id.clone(), p.archived_at.is_some()))
        });
        if let Some((project_id, archived)) = existing {
            // Adding an archived project's folder again brings the project back.
            if archived {
                let update = link.call::<ProjectsMutate>(ProjectMutation::ProjectUpdate(ProjectMutationProjectUpdate {
                    archived: Some(false),
                    ..unarchive_defaults(project_id.clone())
                }));
                cx.spawn(async move |cx| {
                    if let Err(error) = update.await {
                        cx.update(|cx| error_toast(t!("Failed to unarchive project"), error, cx));
                    }
                })
                .detach();
            }
            return Task::ready(Some(FlowOpened::Existing(project_ref(id, &project_id))));
        }
        let project_id = ProjectId::new(uuid::Uuid::new_v4().to_string());
        let create = link.call::<ProjectsMutate>(ProjectMutation::ProjectCreate(ProjectMutationProjectCreate {
            command_id: new_command_id(),
            project_id: project_id.clone(),
            title: title_from_path(&path),
            workspace_root: path,
            create_workspace_root_if_missing: Some(true),
            default_model_selection: None,
            scripts: None,
        }));
        let env = id.clone();
        cx.spawn(async move |cx| {
            let created = match create.await {
                Ok(project) => project,
                Err(error) => {
                    cx.update(|cx| error_toast(t!("Failed to add project"), error, cx));
                    return None;
                }
            };
            // Drafts key off the stored project, so wait until the shell has it.
            let wait = cx.update(|cx| wait_for_project(&env, created.id.clone(), 10, cx));
            wait.await;
            Some(FlowOpened::Created(project_ref(&env, &created.id)))
        })
    }

    fn create_new_project(&self, id: &EnvironmentId, name: String, publish: Option<PublishTarget>, cx: &mut App) -> Task<Option<ProjectRef>> {
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else {
            error_toast(t!("Could not create the project"), t!("The selected environment is not connected."), cx);
            return Task::ready(None);
        };
        let call = link.call::<ProjectsCreateNew>(ProjectCreateNewInput { name: name.clone() });
        let env = id.clone();
        cx.spawn(async move |cx| {
            let result = match call.await {
                Ok(result) => result,
                Err(error) => {
                    cx.update(|cx| error_toast(t!("Could not create the project"), error, cx));
                    return None;
                }
            };
            cx.update(|cx| {
                let toast = match &result.commit_error {
                    None => ToastData::new(ToastKind::Success, t!("Created {name}", name = name.clone())).description(result.workspace_root.clone()),
                    Some(error) => ToastData::new(ToastKind::Warning, t!("Created {name} without a first commit", name = name.clone()))
                        .description(t!("{error} The project is in {path}.", error = error.clone(), path = result.workspace_root.clone())),
                };
                Toaster::push(toast, cx);
                if let Some(target) = publish {
                    LiveProjectFlows.publish_new_project(&env, result.workspace_root.clone(), target, cx);
                }
            });
            let wait = cx.update(|cx| wait_for_project(&env, result.project_id.clone(), 10, cx));
            if wait.await.is_none() {
                cx.update(|cx| {
                    error_toast(
                        t!("Failed to open project"),
                        t!("{error} It will appear in the sidebar once this client catches up.", error = t!("An error occurred.")),
                        cx,
                    )
                });
                return None;
            }
            Some(project_ref(&env, &result.project_id))
        })
    }

    fn start_clone(&self, id: &EnvironmentId, remote_url: String, destination: String, cx: &mut App) -> Task<Option<(ProjectRef, Task<()>)>> {
        let Some(link) = RpcLink::new(&runtime_id(id), cx) else {
            error_toast(t!("Environment unavailable"), t!("The selected environment is not connected."), cx);
            return Task::ready(None);
        };
        let call = link.call::<ProjectCloneStart>(ProjectCloneStartInput {
            project_id: ProjectId::new(uuid::Uuid::new_v4().to_string()),
            title: title_from_path(&destination),
            created_at: super::sidebar_actions::now_iso(),
            provider: None,
            repository: None,
            remote_url: Some(remote_url),
            destination_path: destination,
            protocol: None,
        });
        let env = id.clone();
        cx.spawn(async move |cx| {
            let result = match call.await {
                Ok(result) => result,
                Err(error) => {
                    cx.update(|cx| error_toast(t!("Clone failed"), error, cx));
                    return None;
                }
            };
            let wait = cx.update(|cx| {
                let wait = wait_for_project(&env, result.project_id.clone(), 3, cx);
                cx.background_executor().spawn(async move {
                    wait.await;
                })
            });
            Some((project_ref(&env, &result.project_id), wait))
        })
    }
}

/// A `project.update` that changes nothing until fields are set.
fn unarchive_defaults(project_id: ProjectId) -> ProjectMutationProjectUpdate {
    serde_json::from_value(serde_json::json!({ "type": "project.update", "commandId": new_command_id(), "projectId": project_id }))
        .expect("a bare project.update decodes")
}

/// `projectCloneProgressSummary`: the stage, the percentage, then git's detail or "Stalled".
fn clone_progress(snapshot: &t3code_contracts::ProjectCloneSnapshot) -> SharedString {
    use t3code_contracts::ProjectCloneStage as Stage;
    let mut parts: Vec<SharedString> = vec![match snapshot.stage {
        Stage::Connecting => t!("Connecting", context = "git clone stage"),
        Stage::Counting => t!("Counting objects"),
        Stage::Receiving => t!("Receiving objects"),
        Stage::Resolving => t!("Resolving deltas"),
        _ => t!("Checking out files"),
    }];
    if let Some(percent) = snapshot.percent {
        parts.push(format!("{percent}%").into());
    }
    if snapshot.stalled == Some(true) {
        parts.push(t!("Stalled", context = "git clone progress"));
    } else if let Some(detail) = &snapshot.detail {
        parts.push(detail.clone().into());
    }
    parts.join(" · ").into()
}

/// `projectCloneDisplayName`.
fn clone_name(snapshot: &t3code_contracts::ProjectCloneSnapshot) -> SharedString {
    match &snapshot.repository {
        Some(repository) => repository.name_with_owner.clone().into(),
        None => title_from_path(&snapshot.destination_path).into(),
    }
}

/// What a clone toast is showing (`TrackedToast`).
struct Tracked {
    toast: u64,
    key: String,
    phase: t3code_contracts::ProjectClonePhase,
}

/// Per environment: the latest clones the server reported, their toasts, and the clones whose
/// toasts stay quiet because their draft was on screen (`silenceProjectCloneToasts`).
#[derive(Default)]
struct EnvironmentClones {
    clones: Vec<t3code_contracts::ProjectCloneSnapshot>,
    toasts: std::collections::HashMap<ProjectId, Tracked>,
    silenced: std::collections::HashSet<ProjectId>,
}

/// `ProjectCloneToastCoordinator`: toasts for every clone each environment reports, started
/// here or anywhere else.
#[derive(Default)]
struct CloneToasts {
    environments: std::collections::HashMap<EnvironmentId, EnvironmentClones>,
    watching: std::collections::HashSet<EnvironmentId>,
    /// The project whose draft is on screen, set by the window root.
    viewing: Option<std::rc::Rc<dyn Fn(&App) -> Option<ProjectRef>>>,
    _observer: Option<gpui_kit::Subscription>,
}

impl gpui_kit::Global for CloneToasts {}

/// Starts the clone toasts and says how to read the project whose draft is on screen; the window
/// root calls it once. Call [`refresh_clone_toasts`] after each navigation.
pub fn install_clone_toasts(viewing: std::rc::Rc<dyn Fn(&App) -> Option<ProjectRef>>, cx: &mut App) {
    if !super::StateBridge::is_running(cx) {
        return;
    }
    let environments = super::StateBridge::environments(cx);
    let observer = cx.observe(&environments, |_, cx| watch_environments(cx));
    cx.set_global(CloneToasts { viewing: Some(viewing), _observer: Some(observer), ..Default::default() });
    watch_environments(cx);
}

/// Re-reads which draft is on screen (`isViewingProjectDraft` after a route change).
pub fn refresh_clone_toasts(cx: &mut App) {
    let Some(state) = cx.try_global::<CloneToasts>() else { return };
    let ids: Vec<EnvironmentId> = state.environments.keys().cloned().collect();
    for id in ids {
        reconcile(&id, cx);
    }
}

/// Subscribes to `subscribeProjectClones` on each environment not yet watched; a stream that ends
/// (a disconnect) subscribes again while the environment stays.
fn watch_environments(cx: &mut App) {
    use futures::StreamExt as _;
    let ids: Vec<EnvironmentId> =
        super::StateBridge::environments(cx).read(cx).list.iter().map(|env| model::EnvironmentId(SharedString::from(env.read(cx).environment_id().0.to_string()))).collect();
    for id in ids {
        if !cx.global_mut::<CloneToasts>().watching.insert(id.clone()) {
            continue;
        }
        cx.spawn(async move |cx| {
            loop {
                let Some(link) = cx.update(|cx| RpcLink::new(&runtime_id(&id), cx)) else { break };
                let mut stream = link.stream::<t3code_contracts::rpc::methods::SubscribeProjectClones>(Default::default());
                while let Some(Ok(clones)) = stream.next().await {
                    let id = id.clone();
                    cx.update(|cx| {
                        cx.global_mut::<CloneToasts>().environments.entry(id.clone()).or_default().clones = clones;
                        reconcile(&id, cx);
                    });
                }
                cx.background_executor().timer(std::time::Duration::from_secs(2)).await;
                let present = cx.update(|cx| super::StateBridge::environment(&runtime_id(&id), cx).is_some());
                if !present {
                    cx.update(|cx| cx.global_mut::<CloneToasts>().watching.remove(&id));
                    break;
                }
            }
        })
        .detach();
    }
}

fn clones_of<'a>(env: &EnvironmentId, cx: &'a mut App) -> &'a mut EnvironmentClones {
    cx.global_mut::<CloneToasts>().environments.entry(env.clone()).or_default()
}

/// Opens, updates and closes the environment's clone toasts to match its clones.
fn reconcile(env: &EnvironmentId, cx: &mut App) {
    use t3code_contracts::ProjectClonePhase as Phase;
    let Some(state) = cx.try_global::<CloneToasts>() else { return };
    let viewing = state.viewing.as_ref().and_then(|viewing| viewing(cx));
    let clones = state.environments.get(env).map(|e| e.clones.clone()).unwrap_or_default();
    let open = cx.try_global::<GlobalProjectOpener>().map(|g| g.0.clone());
    let mut seen = std::collections::HashSet::new();
    for clone in clones {
        seen.insert(clone.project_id.clone());
        let id = clone.project_id.clone();
        let project = project_ref(env, &id);
        let key = format!("{:?}:{:?}:{:?}:{:?}:{:?}:{:?}", clone.phase, clone.stage, clone.percent, clone.detail, clone.stalled, clone.error);
        let close = |cx: &mut App| {
            if let Some(tracked) = clones_of(env, cx).toasts.remove(&id) {
                Toaster::dismiss(tracked.toast, cx);
            }
        };
        if viewing.as_ref() == Some(&project) {
            clones_of(env, cx).silenced.insert(id.clone());
            close(cx);
            continue;
        }
        if clones_of(env, cx).toasts.get(&id).is_some_and(|t| t.key == key) {
            continue;
        }
        if clones_of(env, cx).silenced.contains(&id) && matches!(clone.phase, Phase::Running | Phase::Done) {
            close(cx);
            continue;
        }
        let Some(link) = RpcLink::new(&runtime_id(env), cx) else { continue };
        let data = clone_toast(&clone, env, link, open.clone());
        let replace = clone.phase == Phase::Done;
        let tracked = clones_of(env, cx).toasts.remove(&id);
        let toast = match tracked {
            // A finished clone's toast expires on its own timer, so it is shown anew.
            Some(tracked) if !replace && Toaster::update(tracked.toast, data.clone(), cx) => tracked.toast,
            Some(tracked) => {
                Toaster::dismiss(tracked.toast, cx);
                Toaster::push(data, cx)
            }
            None => Toaster::push(data, cx),
        };
        clones_of(env, cx).toasts.insert(id, Tracked { toast, key, phase: clone.phase });
    }
    let gone: Vec<(ProjectId, u64, bool)> = cx
        .global::<CloneToasts>()
        .environments
        .get(env)
        .map(|e| e.toasts.iter().filter(|(id, _)| !seen.contains(*id)).map(|(id, t)| (id.clone(), t.toast, t.phase != Phase::Done)).collect())
        .unwrap_or_default();
    for (id, toast, dismiss) in gone {
        if dismiss {
            Toaster::dismiss(toast, cx);
        }
        if let Some(e) = cx.global_mut::<CloneToasts>().environments.get_mut(env) {
            e.toasts.remove(&id);
        }
    }
}

/// One clone's toast: loading with Cancel while it runs, "Cloned" with Open project, or the
/// failure (or cancellation) with Retry and Remove project.
fn clone_toast(clone: &t3code_contracts::ProjectCloneSnapshot, env: &EnvironmentId, link: RpcLink, open: Option<ProjectOpener>) -> ToastData {
    use t3code_contracts::ProjectClonePhase as Phase;
    use t3code_contracts::rpc::methods::{ProjectCloneCancel, ProjectCloneRetry};
    let name = clone_name(clone);
    let id = clone.project_id.clone();
    let run = |link: RpcLink, id: ProjectId, cancel: bool| {
        move |_: &mut gpui_kit::Window, cx: &mut App| {
            let input = t3code_contracts::ProjectCloneActionInput { project_id: id.clone() };
            let (call, title) = if cancel {
                (link.call::<ProjectCloneCancel>(input), t!("Failed to cancel clone"))
            } else {
                (link.call::<ProjectCloneRetry>(input), t!("Failed to retry clone"))
            };
            cx.spawn(async move |cx| {
                if let Err(error) = call.await {
                    cx.update(|cx| error_toast(title, error, cx));
                }
            })
            .detach();
        }
    };
    match clone.phase {
        Phase::Running => ToastData::new(ToastKind::Loading, t!("Cloning {name}", name = name.clone()))
            .description(clone_progress(clone))
            .persistent()
            .stacked()
            .hide_copy()
            .action(t!("Cancel"), run(link, id, true)),
        Phase::Done => {
            let project = project_ref(env, &id);
            let data = ToastData::new(ToastKind::Success, t!("Cloned {name}", name = name.clone()))
                .description(clone.destination_path.clone())
                .timeout(std::time::Duration::from_secs(8))
                .stacked()
                .hide_copy();
            match open {
                Some(open) => data.action(t!("Open project"), move |window, cx| open(project.clone(), window, cx)),
                None => data,
            }
        }
        phase => {
            let cancelled = phase == Phase::Cancelled;
            let title = if cancelled { t!("Cancelled cloning {name}", name = name.clone()) } else { t!("Failed to clone {name}", name = name.clone()) };
            let description = if cancelled { clone.destination_path.clone() } else { clone.error.clone().unwrap_or_else(|| t!("The clone failed.").to_string()) };
            let data = ToastData::new(if cancelled { ToastKind::Info } else { ToastKind::Error }, title).description(description).persistent().stacked();
            let data = if cancelled { data.hide_copy() } else { data };
            let remove_env = env.clone();
            let remove_id = id.clone();
            data.secondary_action(t!("Remove project"), move |_, cx| remove_cloned_project(&remove_env, remove_id.clone(), cx))
                .action(t!("Retry"), run(link, id, false))
        }
    }
}

/// `useRemoveClonedProject`: deletes the project (unforced, so a thread made since the failed
/// clone is never removed with it); failures toast.
fn remove_cloned_project(env: &EnvironmentId, project_id: ProjectId, cx: &mut App) {
    let Some(link) = RpcLink::new(&runtime_id(env), cx) else { return };
    let call = link.call::<ProjectsMutate>(ProjectMutation::ProjectDelete(t3code_contracts::ProjectMutationProjectDelete {
        command_id: new_command_id(),
        project_id,
        force: None,
    }));
    cx.spawn(async move |cx| {
        if let Err(error) = call.await {
            cx.update(|cx| error_toast(t!("Failed to remove project"), error, cx));
        }
    })
    .detach();
}
