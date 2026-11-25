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

use crate::components::command_palette_project_flow::{BrowseEntry, BrowseListing, FlowEnvironment, FlowOpened, ProjectFlowBackend, title_from_path};
use crate::components::sidebar::model::{self, EnvironmentId, ProjectRef};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::live::rpc::RpcLink;

/// [`ProjectFlowBackend`] over the connected environments.
pub struct LiveProjectFlows;

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
        })
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

    fn create_new_project(&self, id: &EnvironmentId, name: String, cx: &mut App) -> Task<Option<ProjectRef>> {
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
                watch_clone(env.clone(), result.project_id.clone(), cx);
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

/// `ProjectCloneToastCoordinator` for one clone this client started: a loading toast with
/// Cancel while it runs, then "Cloned" with Open project, or the failure with Retry.
fn watch_clone(env: EnvironmentId, project_id: ProjectId, cx: &mut App) {
    let Some(open) = cx.try_global::<GlobalProjectOpener>().map(|g| g.0.clone()) else { return };
    use futures::StreamExt as _;
    use t3code_contracts::ProjectClonePhase as Phase;
    use t3code_contracts::rpc::methods::{ProjectCloneCancel, ProjectCloneRetry, SubscribeProjectClones};
    let Some(link) = RpcLink::new(&runtime_id(&env), cx) else { return };
    let mut stream = link.stream::<SubscribeProjectClones>(Default::default());
    cx.spawn(async move |cx| {
        let mut toast: Option<u64> = None;
        let mut rendered: Option<String> = None;
        while let Some(Ok(clones)) = stream.next().await {
            let Some(clone) = clones.into_iter().find(|c| c.project_id == project_id) else { continue };
            let key = format!("{:?}:{:?}:{:?}:{:?}:{:?}:{:?}", clone.phase, clone.stage, clone.percent, clone.detail, clone.stalled, clone.error);
            if rendered.as_deref() == Some(&key) {
                continue;
            }
            rendered = Some(key);
            let name = clone_name(&clone);
            let action_link = link.clone();
            let action_id = project_id.clone();
            let data = match clone.phase {
                Phase::Running => ToastData::new(ToastKind::Loading, t!("Cloning {name}", name = name.clone()))
                    .description(clone_progress(&clone))
                    .persistent()
                    .action(t!("Cancel"), move |_, cx| {
                        let call = action_link.call::<ProjectCloneCancel>(t3code_contracts::ProjectCloneActionInput { project_id: action_id.clone() });
                        cx.spawn(async move |cx| {
                            if let Err(error) = call.await {
                                cx.update(|cx| error_toast(t!("Failed to cancel clone"), error, cx));
                            }
                        })
                        .detach();
                    }),
                Phase::Done => {
                    let (open, project) = (open.clone(), project_ref(&env, &project_id));
                    ToastData::new(ToastKind::Success, t!("Cloned {name}", name = name.clone()))
                        .description(clone.destination_path.clone())
                        .timeout(std::time::Duration::from_secs(8))
                        .action(t!("Open project"), move |window, cx| open(project.clone(), window, cx))
                }
                phase => {
                    let cancelled = phase == Phase::Cancelled;
                    let title = if cancelled { t!("Cancelled cloning {name}", name = name.clone()) } else { t!("Failed to clone {name}", name = name.clone()) };
                    let description = if cancelled { clone.destination_path.clone() } else { clone.error.clone().unwrap_or_else(|| t!("The clone failed.").to_string()) };
                    ToastData::new(if cancelled { ToastKind::Info } else { ToastKind::Error }, title).description(description).persistent().action(
                        t!("Retry"),
                        move |_, cx| {
                            let call = action_link.call::<ProjectCloneRetry>(t3code_contracts::ProjectCloneActionInput { project_id: action_id.clone() });
                            cx.spawn(async move |cx| {
                                if let Err(error) = call.await {
                                    cx.update(|cx| error_toast(t!("Failed to retry clone"), error, cx));
                                }
                            })
                            .detach();
                        },
                    )
                }
            };
            let done = clone.phase == Phase::Done;
            cx.update(|cx| {
                // A finished clone's toast expires on its own timer, so it is shown anew.
                match toast {
                    Some(id) if !done && Toaster::update(id, data.clone(), cx) => {}
                    Some(id) => {
                        Toaster::dismiss(id, cx);
                        toast = Some(Toaster::push(data, cx));
                    }
                    None => toast = Some(Toaster::push(data, cx)),
                }
            });
            if done {
                break;
            }
        }
    })
    .detach();
}
