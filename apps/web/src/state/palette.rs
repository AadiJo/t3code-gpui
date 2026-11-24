//! The command palette over live threads and projects (`CommandPalette.tsx` data,
//! `commandPaletteStore.ts` handlers). [`LivePaletteSource`] is refreshed from
//! [`LiveShell`] by [`LivePaletteSource::refresh`]; a palette thread keeps its `Arc` while its
//! sidebar row's `Arc` is unchanged, so a refresh after one thread changed builds one search
//! subject.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, SharedString, Task};
use t3code_client_runtime::change_request_links::SourceControlHostKind;
use t3code_client_runtime::operations::commands::new_command_id;
use t3code_contracts::rpc::methods::{ProjectsEnsureScratch, ServerPrepareTriage, ServerRefreshProviders};
use t3code_contracts::{
    AuthClientMetadataDeviceType, AuthClientPresentationMetadata, ClientSurface, OrchestrationV2Command as Command,
    OrchestrationV2CommandThreadPullRequestLink, ServerPrepareTriageInput, ServerRefreshProvidersPayload, ThreadPullRequestLinkSource,
};

use super::shell::LiveShell;
use super::sidebar_actions::{SidebarOutcome, handle as handle_sidebar};
use crate::components::command_palette_logic::PaletteEvent;
use crate::components::command_palette_lookups::{
    CommandPaletteSource, PaletteEnvironment, PaletteFeature, PalettePort, PaletteProject, PaletteSetting, PaletteThread,
};
use crate::components::pull_request::link_pull_request_dialog::{LinkProject, ResolvedLink};
use crate::components::sidebar::SidebarEvent;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::components::sidebar::model::{EnvironmentId, ProjectRef, SidebarThread, ThreadKey};
use t3code_client_runtime::state::models::ThreadRow;

#[derive(Default)]
struct PaletteData {
    revision: u64,
    threads: Arc<[Arc<PaletteThread>]>,
    projects: Arc<[PaletteProject]>,
    active_thread: Option<ThreadKey>,
    /// The active thread links pull requests or issues.
    active_has_links: bool,
    /// The active thread's environment has its server config, so links can change
    /// (`threadPullRequestLinkMode`).
    active_can_link: bool,
    /// Some environment has its server config (`Open pull requests`).
    pull_requests: bool,
    /// Some environment's server offers issues.
    issues: bool,
    /// Where `Triage an issue` runs (`pickTriageEnvironment`), when that server offers it.
    triage: Option<t3code_contracts::EnvironmentId>,
    environments: Vec<PaletteEnvironment>,
    /// The Forwarded ports page (`PortForwards::palette_ports`).
    ports: Vec<PalettePort>,
    /// Every settings page and control the palette can open (`SETTINGS_SEARCH_ITEMS`), and
    /// whether an environment was connected when they were listed.
    settings: Option<(bool, Arc<[PaletteSetting]>)>,
    now_ms: i64,
    memo: HashMap<ThreadKey, (Arc<SidebarThread>, Arc<PaletteThread>)>,
    mentions: Arc<[Arc<MentionCandidate>]>,
    mention_memo: HashMap<ThreadKey, (Arc<ThreadRow>, Arc<MentionCandidate>)>,
}

/// A thread the composer's `@` menu may reference (`ComposerThreadCandidate`): every listed
/// thread and every side chat, not archived. Side chats are offered only from their own parent.
pub struct MentionCandidate {
    pub key: ThreadKey,
    pub title: SharedString,
    /// The thread it is a side chat of, if it is one.
    pub side_chat_of: Option<ThreadKey>,
    /// The user wrote in it; an unused side chat is deleted on close, so it is not offered.
    pub has_user_message: bool,
    /// Searched without the project title, as the composer searches.
    pub subject: t3code_shared::palette_search::ThreadSearchSubject,
}

impl MentionCandidate {
    fn new(row: &ThreadRow) -> Self {
        let source = &row.source;
        let key = super::shell::thread_key(&row.environment_id, row.id());
        let mut subject = t3code_shared::palette_search::ThreadSearchSubject::new(row.title()).with_branch(source.branch.as_deref());
        subject.id = Some(Arc::clone(&source.id.0));
        subject.activity_at = row.latest_user_message_at.or(row.updated_at).or(row.created_at);
        let side_chat_of = (source.lineage.side_chat == Some(true))
            .then(|| source.lineage.parent_thread_id.as_ref().map(|parent| super::shell::thread_key(&row.environment_id, parent)))
            .flatten();
        Self { key, title: SharedString::from(row.title().to_owned()), side_chat_of, has_user_message: row.latest_user_message_at.is_some(), subject }
    }
}

/// Palette data from the live shell.
#[derive(Default)]
pub struct LivePaletteSource {
    data: RefCell<PaletteData>,
}

impl LivePaletteSource {
    pub fn new() -> Rc<Self> {
        Rc::default()
    }

    /// The threads `@` may reference, as of the last refresh.
    pub fn mention_candidates(&self) -> Arc<[Arc<MentionCandidate>]> {
        self.data.borrow().mentions.clone()
    }

    /// Re-reads the forwarded and discovered ports.
    pub fn refresh_ports(&self, cx: &App) {
        self.data.borrow_mut().ports = crate::port_discovery_state::PortForwards::palette_ports(cx);
    }

    /// Takes the shell's current rows if they changed since the last refresh.
    pub fn refresh(&self, shell: &LiveShell, cx: &App) {
        self.refresh_ports(cx);
        let mut data = self.data.borrow_mut();
        if data.revision == shell.revision() && data.revision != 0 {
            return;
        }
        let snapshot = shell.snapshot();
        let projects = crate::components::command_palette_lookups::palette_projects(snapshot);
        let mut memo = std::mem::take(&mut data.memo);
        let mut next = HashMap::with_capacity(snapshot.threads.len());
        let threads: Arc<[Arc<PaletteThread>]> = snapshot
            .threads
            .iter()
            .filter(|thread| !thread.archived)
            .map(|thread| {
                let palette = match memo.remove(&thread.key) {
                    Some((source, palette)) if Arc::ptr_eq(&source, thread) => palette,
                    _ => {
                        let project = projects.iter().find(|p| p.project_ref == thread.project_ref());
                        Arc::new(PaletteThread::from_sidebar(thread, project.map(|p| p.title.as_ref()), None, None))
                    }
                };
                next.insert(thread.key.clone(), (Arc::clone(thread), Arc::clone(&palette)));
                palette
            })
            .collect();
        data.memo = next;
        data.threads = threads;
        let mut memo = std::mem::take(&mut data.mention_memo);
        let mut next = HashMap::with_capacity(memo.len());
        let mentions: Arc<[Arc<MentionCandidate>]> = shell
            .presented()
            .iter()
            .filter(|row| row.source.archived_at.is_none() && (row.source.lineage.is_listed() || row.source.lineage.side_chat == Some(true)))
            .map(|row| {
                let key = super::shell::thread_key(&row.environment_id, row.id());
                let candidate = match memo.remove(&key) {
                    Some((source, candidate)) if Arc::ptr_eq(&source, row) => candidate,
                    _ => Arc::new(MentionCandidate::new(row)),
                };
                next.insert(key, (Arc::clone(row), Arc::clone(&candidate)));
                candidate
            })
            .collect();
        data.mention_memo = next;
        data.mentions = mentions;
        data.projects = projects;
        data.active_thread = snapshot.active_thread.clone();
        data.active_has_links = snapshot.active_thread.as_ref().and_then(|key| shell.row(key)).is_some_and(|row| {
            row.source.pull_requests.as_ref().is_some_and(|links| !links.is_empty()) || row.source.issues.as_ref().is_some_and(|issues| !issues.is_empty())
        });
        data.now_ms = snapshot.now.0;
        let environments = shell.environments(cx);
        let config = |env: &gpui_kit::Entity<super::LiveEnvironment>| env.read(cx).config.clone();
        let active_environment = snapshot.active_thread.as_ref().map(|key| super::shell::runtime_ids(key).0);
        data.active_can_link = active_environment
            .as_ref()
            .and_then(|id| environments.iter().find(|env| env.read(cx).environment_id() == id))
            .is_some_and(|env| config(env).is_some());
        data.pull_requests = environments.iter().any(|env| config(env).is_some());
        data.issues = environments.iter().any(|env| config(env).is_some_and(|c| c.environment.capabilities.issues == Some(true)));
        data.triage = triage_environment(snapshot.active_thread.as_ref(), cx);
        data.environments = shell
            .environments(cx)
            .iter()
            .map(|env| {
                let env = env.read(cx);
                PaletteEnvironment {
                    id: EnvironmentId(SharedString::from(Arc::clone(&env.environment_id().0))),
                    label: env.config.as_ref().map(|c| SharedString::from(c.environment.label.clone())).unwrap_or_default(),
                    connected: env.store.session().is_some(),
                }
            })
            .collect();
        let has_environment = !data.environments.is_empty();
        if data.settings.as_ref().is_none_or(|(had, _)| *had != has_environment) {
            let availability = crate::components::settings::settings_search::SearchAvailability { has_environment, has_cloud: true };
            data.settings = Some((has_environment, crate::components::settings::settings_search::palette_settings(availability)));
        }
        data.revision = shell.revision();
    }
}

impl CommandPaletteSource for LivePaletteSource {
    fn threads(&self) -> Arc<[Arc<PaletteThread>]> {
        self.data.borrow().threads.clone()
    }

    fn projects(&self) -> Arc<[PaletteProject]> {
        self.data.borrow().projects.clone()
    }

    fn active_thread(&self) -> Option<ThreadKey> {
        self.data.borrow().active_thread.clone()
    }

    fn current_project(&self) -> Option<ProjectRef> {
        let data = self.data.borrow();
        let active = data.active_thread.as_ref()?;
        data.threads.iter().find(|t| &t.key == active).and_then(|t| t.project.clone())
    }

    fn environments(&self) -> Vec<PaletteEnvironment> {
        self.data.borrow().environments.clone()
    }

    fn ports(&self) -> Vec<PalettePort> {
        self.data.borrow().ports.clone()
    }

    fn settings(&self) -> Arc<[PaletteSetting]> {
        self.data.borrow().settings.as_ref().map_or_else(|| Arc::from([]), |(_, settings)| settings.clone())
    }

    fn supports(&self, feature: PaletteFeature) -> bool {
        let data = self.data.borrow();
        match feature {
            PaletteFeature::NewProject
            | PaletteFeature::ProjectSettings
            | PaletteFeature::RestartAgentSession
            | PaletteFeature::PortForwarding
            | PaletteFeature::UseInAnotherApp => true,
            PaletteFeature::PromptLinks => data.active_thread.is_some(),
            PaletteFeature::LinkPullRequest => data.active_can_link,
            PaletteFeature::PullRequests => data.pull_requests,
            PaletteFeature::Issues => data.issues,
            PaletteFeature::Triage => data.triage.is_some(),
            _ => false,
        }
    }

    fn active_thread_has_linked_items(&self) -> bool {
        self.data.borrow().active_has_links
    }

    fn now_ms(&self) -> i64 {
        self.data.borrow().now_ms
    }
}

/// The machine `Triage an issue` runs on (`pickTriageEnvironment`): the one on screen, else the
/// first connected one that offers triage. This client has no primary (local) environment.
pub fn triage_environment(active: Option<&ThreadKey>, cx: &App) -> Option<t3code_contracts::EnvironmentId> {
    let environments = super::StateBridge::environments(cx).read(cx).list.clone();
    let supports = |env: &gpui_kit::Entity<super::LiveEnvironment>| {
        env.read(cx).config.as_ref().is_some_and(|c| c.environment.capabilities.triage == Some(true))
    };
    match active.map(|key| super::shell::runtime_ids(key).0) {
        Some(id) => environments.iter().find(|env| env.read(cx).environment_id() == &id).filter(|env| supports(env)),
        None => environments.iter().find(|env| env.read(cx).store.session().is_some() && supports(env)),
    }
    .map(|env| env.read(cx).environment_id().clone())
}

/// What the app must do after a palette event the bridge cannot finish alone.
pub type PaletteOutcome = SidebarOutcome;

/// Handles a palette event: opening a thread goes through the same path as a sidebar click.
pub fn handle(event: &PaletteEvent, cx: &mut App) -> PaletteOutcome {
    match event {
        PaletteEvent::OpenThread { thread, .. } => handle_sidebar(&SidebarEvent::ActivateThread(thread.clone()), cx),
        PaletteEvent::NewThreadIn(project) => SidebarOutcome::NewThread(Some(project.clone())),
        _ => SidebarOutcome::Done,
    }
}

fn command_error(error: impl std::fmt::Display) -> SharedString {
    SharedString::from(error.to_string())
}

/// The thread project's repository, for resolving a bare pull request number
/// (`pullRequestLinkProject`).
pub fn link_project(key: &ThreadKey, cx: &App) -> Option<LinkProject> {
    let shell = super::StateBridge::shell(cx);
    let shell = shell.read(cx);
    let row = shell.row(key)?;
    let environment = super::StateBridge::environment(&row.environment_id, cx)?;
    let snapshot = environment.read(cx).shell.snapshot.clone()?;
    let project = snapshot.projects.iter().find(|p| p.id == row.source.project_id)?;
    let identity = project.repository_identity.as_ref()?;
    let repository = identity.display_name.clone().or_else(|| Some(format!("{}/{}", identity.owner.as_ref()?, identity.name.as_ref()?)))?;
    let kind = identity.provider.as_ref().and_then(|p| SourceControlHostKind::from_wire(p.0.as_ref()))?;
    let remote = identity.locator.remote_url.clone();
    let host = parse_remote_host(&remote).unwrap_or_default();
    Some(LinkProject { kind, host, repository, remote_url: Some(remote), web_url: identity.web_url.clone() })
}

/// The lower-case host of a git remote, HTTP or SSH (`git@host:owner/name`).
fn parse_remote_host(remote: &str) -> Option<String> {
    let rest = remote.split_once("://").map_or(remote, |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
    let host = rest.split(['/', ':']).next()?;
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// Links a pull request to the thread (`thread.pull-request.link`, source manual).
pub fn link_pull_request(key: &ThreadKey, link: ResolvedLink, cx: &App) -> Task<Result<(), SharedString>> {
    let (environment_id, thread_id) = super::shell::runtime_ids(key);
    let Some(environment) = super::StateBridge::environment(&environment_id, cx) else {
        return Task::ready(Err(t!("The pull request is not available in this environment.")));
    };
    let commands = environment.read(cx).commands.clone();
    let task = super::StateBridge::spawn(
        async move {
            // `normalizeThreadPullRequestKey`: links are keyed by the lower-case host and path.
            let command = Command::ThreadPullRequestLink(OrchestrationV2CommandThreadPullRequestLink {
                command_id: new_command_id(),
                thread_id,
                host: link.host.to_lowercase(),
                repository: link.repository.to_lowercase(),
                number: link.number,
                url: link.url,
                source: ThreadPullRequestLinkSource::Manual,
                repository_web_url: None,
            });
            commands.dispatch(command).await.map(|_| ()).map_err(command_error)
        },
        cx,
    );
    cx.background_executor().spawn(async move { task.await.unwrap_or_else(|| Err(t!("Link update interrupted."))) })
}

/// `Restart agent session`: detaches the thread's provider sessions, says the next message
/// starts fresh, then asks the provider for a fresh probe in the thread's checkout.
pub fn restart_agent_session(key: &ThreadKey, cx: &mut App) {
    let (environment_id, thread_id) = super::shell::runtime_ids(key);
    let Some(environment) = super::StateBridge::environment(&environment_id, cx) else { return };
    let shell = super::StateBridge::shell(cx);
    let Some(row) = shell.read(cx).row(key).cloned() else { return };
    let environment = environment.read(cx);
    let commands = environment.commands.clone();
    let workspace_root = environment
        .shell
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.projects.iter().find(|p| p.id == row.source.project_id).map(|p| p.workspace_root.clone()));
    let link = crate::live::rpc::RpcLink::new(&environment_id, cx);
    let running = row.runtime.is_some();
    let input = workspace_root.map(|root| ServerRefreshProvidersPayload {
        instance_id: Some(row.source.provider_instance_id.clone()),
        cwd: Some(row.source.worktree_path.clone().unwrap_or(root)),
        fresh: Some(true),
        ..Default::default()
    });
    let stop = super::StateBridge::spawn(
        async move { if running { commands.stop_sessions(&thread_id).await.map(|_| ()).map_err(command_error) } else { Ok(()) } },
        cx,
    );
    cx.spawn(async move |cx| {
        let failed = |error: SharedString, cx: &mut gpui_kit::AsyncApp| {
            cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, t!("Something went wrong.")).description(error), cx));
        };
        if let Some(Err(error)) = stop.await {
            return failed(error, cx);
        }
        cx.update(|cx| {
            Toaster::push(
                ToastData::new(ToastKind::Success, t!("Agent session will restart")).description(t!("Your next message starts a fresh session.")),
                cx,
            )
        });
        let (Some(link), Some(input)) = (link, input) else { return };
        if let Err(error) = link.call::<ServerRefreshProviders>(input).await {
            failed(error, cx);
        }
    })
    .detach();
}

/// `TRIAGE_CLI_COMMAND`, the terminal fallback the failure toast offers to copy.
const TRIAGE_CLI_COMMAND: &str = "npx t3 triage";

fn triage_failed(description: SharedString, cx: &mut App) {
    Toaster::push(
        ToastData::new(ToastKind::Error, t!("Could not start triage"))
            .description(t!("{description} Run {command} in a terminal on that machine instead.", description = description.as_ref(), command = TRIAGE_CLI_COMMAND))
            .persistent()
            .action(t!("Copy command"), |_, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(TRIAGE_CLI_COMMAND.to_owned()))),
        cx,
    );
}

/// In-app `t3 triage` (`useStartTriage`): the server writes the session files and returns the
/// playbook prompt, then the environment's No project (scratch) project is found or created and
/// shown in the shell. Resolves to the project and prompt for a new draft; failures toast.
pub fn start_triage(environment_id: t3code_contracts::EnvironmentId, cx: &mut App) -> Task<Option<(ProjectRef, String)>> {
    let Some(environment) = super::StateBridge::environment(&environment_id, cx) else { return Task::ready(None) };
    if environment.read(cx).config.as_ref().is_none_or(|c| c.scratch_workspace_root.is_none()) {
        triage_failed(t!("This machine cannot start threads without a project."), cx);
        return Task::ready(None);
    }
    let Some(link) = crate::live::rpc::RpcLink::new(&environment_id, cx) else { return Task::ready(None) };
    let store = environment.read(cx).store.clone();
    let presentation = t3code_client_runtime::platform::ClientPresentation::default();
    let client = AuthClientPresentationMetadata {
        label: Some(presentation.label),
        device_type: Some(AuthClientMetadataDeviceType::Desktop),
        os: Some(presentation.os.to_owned()),
        surface: Some(ClientSurface::Desktop),
        app_version: Some(presentation.app_version),
        ..AuthClientPresentationMetadata::default()
    };
    let prepare = link.call::<ServerPrepareTriage>(ServerPrepareTriageInput { client });
    let scratch = link.call::<ProjectsEnsureScratch>(Default::default());
    let pending = std::rc::Rc::new(std::cell::Cell::new(None::<u64>));
    let shown = pending.clone();
    // A slow source lookup holds the draft back, so say something after 400 ms.
    let timer = cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
        cx.update(|cx| shown.set(Some(Toaster::push(ToastData::new(ToastKind::Loading, t!("Starting triage…")).persistent(), cx))));
    });
    cx.spawn(async move |cx| {
        let result: Result<(ProjectRef, String), SharedString> = async {
            let prompt = prepare.await?.prompt;
            let project_id = scratch.await?.project_id;
            // Drafts key off the stored project, so wait until the shell has it.
            let wait = project_id.clone();
            let loaded = cx
                .update(|cx| super::StateBridge::spawn(async move { wait_for_project(store, wait).await }, cx))
                .await
                .flatten();
            if loaded.is_none() {
                return Err(t!("T3 Code could not open a new thread."));
            }
            Ok((ProjectRef { environment_id: super::shell::environment_key(&environment_id), project_id: project_key(&project_id) }, prompt))
        }
        .await;
        drop(timer);
        cx.update(|cx| {
            if let Some(id) = pending.get() {
                Toaster::dismiss(id, cx);
            }
            match result {
                Ok(found) => Some(found),
                Err(error) => {
                    triage_failed(if error.is_empty() { t!("An error occurred.") } else { error }, cx);
                    None
                }
            }
        })
    })
}

fn project_key(id: &t3code_contracts::ProjectId) -> crate::components::sidebar::model::ProjectId {
    crate::components::sidebar::model::ProjectId(SharedString::from(id.0.to_string()))
}

/// Resolves once the environment's shell lists `project_id`, or after 10 s with `None`.
async fn wait_for_project(store: t3code_client_runtime::state::EnvironmentStore, project_id: t3code_contracts::ProjectId) -> Option<()> {
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
    tokio::time::timeout(std::time::Duration::from_secs(10), found).await.ok()
}

/// `scratchEnvironmentId`: the machine a projectless thread starts on: the one on screen when it
/// offers a scratch workspace, else the only one that does.
pub fn scratch_environment(active: Option<&ThreadKey>, cx: &App) -> Option<t3code_contracts::EnvironmentId> {
    let environments = super::StateBridge::environments(cx).read(cx).list.clone();
    let offers = |env: &gpui_kit::Entity<super::LiveEnvironment>| env.read(cx).config.as_ref().is_some_and(|c| c.scratch_workspace_root.is_some());
    if let Some(id) = active.map(|key| super::shell::runtime_ids(key).0)
        && environments.iter().any(|env| env.read(cx).environment_id() == &id && offers(env))
    {
        return Some(id);
    }
    let mut offering = environments.iter().filter(|env| offers(env));
    match (offering.next(), offering.next()) {
        (Some(env), None) => Some(env.read(cx).environment_id().clone()),
        _ => None,
    }
}

/// `startScratchThread`: finds or creates the environment's No project (scratch) project and
/// resolves once the shell lists it, for a new draft there. Failures toast.
pub fn start_scratch_thread(environment_id: t3code_contracts::EnvironmentId, cx: &mut App) -> Task<Option<ProjectRef>> {
    let failed = |description: SharedString, cx: &mut App| {
        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not start without a project")).description(description), cx);
    };
    let Some(environment) = super::StateBridge::environment(&environment_id, cx) else { return Task::ready(None) };
    let Some(link) = crate::live::rpc::RpcLink::new(&environment_id, cx) else {
        failed(t!("An error occurred."), cx);
        return Task::ready(None);
    };
    let store = environment.read(cx).store.clone();
    let scratch = link.call::<ProjectsEnsureScratch>(Default::default());
    cx.spawn(async move |cx| {
        let result: Result<ProjectRef, SharedString> = async {
            let project_id = scratch.await?.project_id;
            let wait = project_id.clone();
            let loaded = cx.update(|cx| super::StateBridge::spawn(async move { wait_for_project(store, wait).await }, cx)).await.flatten();
            if loaded.is_none() {
                return Err(t!("T3 Code could not open a new thread."));
            }
            Ok(ProjectRef { environment_id: super::shell::environment_key(&environment_id), project_id: project_key(&project_id) })
        }
        .await;
        cx.update(|cx| match result {
            Ok(project) => Some(project),
            Err(error) => {
                failed(error, cx);
                None
            }
        })
    })
}
