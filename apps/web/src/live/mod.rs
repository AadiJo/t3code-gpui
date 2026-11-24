//! [`AppData`] over the live state bridge: the saved environments' shells, thread streams and
//! commands, and every panel's data over the environment's RPC session. The real app runs on
//! this whenever an environment is saved.

pub mod diff;
pub mod git;
pub mod rpc;
pub mod sources;
pub mod terminal;
pub mod workspace;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, SharedString, Subscription, Task, WeakEntity};
use t3code_client_runtime::operations::SendMessage;
use t3code_contracts::{
    CommandId, OrchestrationV2ThreadLaunchInput, OrchestrationV2ThreadLaunchInputInitialMessage,
    OrchestrationV2ThreadLaunchWorkspaceStrategy, OrchestrationV2ThreadLaunchWorkspaceStrategyRoot, ProviderApprovalDecision,
    ProviderInteractionMode, ProviderUserInputAnswers, RuntimeMode as WireRuntimeMode, ThreadId as WireThreadId,
};

use crate::components::branch_toolbar::BranchToolbarProps;
use crate::components::chat::composer::{ChatComposer, ComposerEvent, ComposerState};
use crate::components::chat::composer_command_menu::ComposerMenuSource;
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::chat::timeline_entries::TimelineInput;
use crate::components::command_palette_lookups::CommandPaletteSource;
use crate::components::diff_panel::DiffPanelInput;
use crate::components::right_panel_tabs::{TerminalBackends, Workspace};
use crate::components::sidebar::SidebarEvent;
use crate::components::sidebar::context_menu::MenuCommand;
use crate::components::sidebar::model::{ProjectRef, SidebarSnapshot, ThreadKey};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::composer_draft_store::{ComposerDraftKey, ComposerDraftStore};
use crate::diff_panel_store::DiffPanelSelection;
use crate::i18n::t;
use crate::provider_models_live::{model_catalog, to_wire};
use crate::router::{DraftId, ThreadTarget};
use crate::state::app_data::{ActionOutcome, AppData, CommandOutcome, DataEvent, DataEvents, GitControlsSetup, ThreadView};
use crate::state::composer::{self as live_composer, CatalogCache, LiveDraftStore, LiveMenuSource, composer_state};
use crate::state::palette::LivePaletteSource;
use crate::state::shell::{runtime_ids, thread_key};
use crate::state::sidebar_actions::{self, SidebarOutcome};
use crate::state::{LiveEnvironment, StateBridge};
use crate::thread_sync::{LiveThread, LiveThreadEvent};
use crate::components::issue::issues_page::EnvironmentIssueSource;
use crate::components::preview::preview_source::{LivePreviewSource, PreviewSource};
use crate::components::pull_request::pull_request_source::PullRequestDetailSource;
use crate::components::pull_request::pull_requests_page::EnvironmentListSource;
use crate::components::settings::server_settings_data::ServerPanelsModel;
use crate::components::usage::usage_source::{LiveUsageSource, UsageSource};

/// A new thread that has no messages yet (`/draft/$draftId`).
struct Draft {
    project: Option<ProjectRef>,
}

/// The window's data on live state. See the module docs.
pub struct LiveData {
    events: Entity<DataEvents>,
    palette: Rc<LivePaletteSource>,
    drafts: RefCell<HashMap<DraftId, Draft>>,
    next_draft: std::cell::Cell<usize>,
    draft_store: Rc<RefCell<dyn ComposerDraftStore>>,
    catalogs: RefCell<HashMap<t3code_contracts::EnvironmentId, CatalogCache>>,
    /// The open thread's event subscription.
    thread: RefCell<Option<(ThreadKey, Subscription)>>,
    /// One menu source per environment and workspace.
    menus: RefCell<HashMap<(t3code_contracts::EnvironmentId, Option<String>), Rc<LiveMenuSource>>>,
    workspaces: RefCell<HashMap<SharedString, Workspace>>,
    usage: RefCell<Option<Rc<dyn UsageSource>>>,
    /// The server settings model and the environment it follows.
    settings: RefCell<Option<(t3code_contracts::EnvironmentId, Entity<ServerPanelsModel>, Subscription)>>,
    /// The environment of the thread the main pane shows.
    shown_environment: RefCell<Option<t3code_contracts::EnvironmentId>>,
}

impl LiveData {
    /// Starts the state bridge (the connection runtime must run) and follows its shell.
    pub fn new(cx: &mut App) -> Rc<Self> {
        if !StateBridge::is_running(cx) {
            StateBridge::init(cx);
        }
        let shell = StateBridge::shell(cx);
        let palette = LivePaletteSource::new();
        palette.refresh(shell.read(cx), cx);
        let events = cx.new(|_| DataEvents::default());
        let subscription = {
            let palette = palette.clone();
            events.update(cx, |_, cx| {
                cx.observe(&shell, move |_, shell, cx| {
                    palette.refresh(shell.read(cx), cx);
                    cx.emit(DataEvent::SnapshotChanged);
                })
            })
        };
        events.update(cx, |events, _| events._subscriptions.push(subscription));
        Rc::new(Self {
            events,
            palette,
            drafts: RefCell::default(),
            next_draft: std::cell::Cell::new(0),
            draft_store: Rc::new(RefCell::new(LiveDraftStore::new(StateBridge::drafts(cx)))),
            catalogs: RefCell::default(),
            thread: RefCell::default(),
            menus: RefCell::default(),
            workspaces: RefCell::default(),
            usage: RefCell::default(),
            settings: RefCell::default(),
            shown_environment: RefCell::default(),
        })
    }

    fn emit(&self, event: DataEvent, cx: &mut App) {
        self.events.update(cx, |_, cx| cx.emit(event));
    }

    /// The open thread for `key`, opening it (and following its events) when another is open.
    fn open(&self, key: &ThreadKey, cx: &mut App) -> Option<Entity<LiveThread>> {
        let threads = StateBridge::threads(cx);
        if let Some(active) = threads.read(cx).active()
            && active.read(cx).key() == key
        {
            return Some(active.clone());
        }
        let thread = threads.update(cx, |threads, cx| threads.open(key.clone(), cx))?;
        let events = self.events.clone();
        let target = ThreadTarget::Server(key.clone());
        let subscription = cx.subscribe(&thread, move |thread, event: &LiveThreadEvent, cx| {
            let event = match event {
                LiveThreadEvent::Text(changes) => DataEvent::Text { thread: thread.read(cx).key().clone(), changes: changes.clone() },
                LiveThreadEvent::Structure | LiveThreadEvent::Status => DataEvent::ThreadChanged(target.clone()),
            };
            events.update(cx, |_, cx| cx.emit(event));
        });
        *self.thread.borrow_mut() = Some((key.clone(), subscription));
        Some(thread)
    }

    /// The live thread when `key` is the open one.
    fn active(&self, key: &ThreadKey, cx: &App) -> Option<Entity<LiveThread>> {
        StateBridge::threads(cx).read(cx).active().filter(|t| t.read(cx).key() == key).cloned()
    }

    fn environment(&self, environment_id: &t3code_contracts::EnvironmentId, cx: &App) -> Option<Entity<LiveEnvironment>> {
        StateBridge::environment(environment_id, cx)
    }

    /// The project a target works in, with its title and workspace root.
    fn project(&self, project: &ProjectRef, cx: &App) -> Option<(SharedString, SharedString)> {
        let shell = StateBridge::shell(cx);
        let shell = shell.read(cx);
        shell
            .snapshot()
            .projects
            .iter()
            .find(|p| p.project_ref == *project)
            .map(|p| (p.title.clone(), p.workspace_root.clone()))
    }

    fn catalog(&self, environment: &LiveEnvironment) -> Arc<crate::model_selection::ModelCatalog> {
        self.catalogs.borrow_mut().entry(environment.environment_id().clone()).or_default().get(environment.config.as_ref())
    }
}

/// Toasts a failed command.
fn toast_error(title: SharedString, error: impl ToString, cx: &mut App) {
    Toaster::push(ToastData::new(ToastKind::Error, title).description(SharedString::from(error.to_string())), cx);
}

impl AppData for LiveData {
    fn events(&self) -> Entity<DataEvents> {
        self.events.clone()
    }

    fn has_data(&self, cx: &App) -> bool {
        !StateBridge::environments(cx).read(cx).list.is_empty()
    }

    fn snapshot(&self, active: Option<&ThreadKey>, cx: &App) -> SidebarSnapshot {
        let shell = StateBridge::shell(cx);
        SidebarSnapshot { active_thread: active.cloned(), ..shell.read(cx).snapshot().clone() }
    }

    fn most_recent_project(&self, cx: &App) -> Option<ProjectRef> {
        let shell = StateBridge::shell(cx);
        let snapshot = shell.read(cx).snapshot();
        snapshot
            .projects
            .iter()
            .filter(|p| !p.archived)
            .max_by_key(|p| {
                snapshot
                    .threads
                    .iter()
                    .filter(|t| t.project_id == p.project_ref.project_id && t.key.environment_id == p.project_ref.environment_id)
                    .map(|t| t.updated_at)
                    .max()
                    .unwrap_or(p.updated_at)
            })
            .map(|p| p.project_ref.clone())
    }

    fn thread_exists(&self, target: &ThreadTarget, cx: &App) -> bool {
        match target {
            ThreadTarget::Server(key) => StateBridge::shell(cx).read(cx).snapshot().threads.iter().any(|t| t.key == *key),
            ThreadTarget::Draft(id) => self.drafts.borrow().contains_key(id),
        }
    }

    fn thread_view(&self, target: &ThreadTarget, cx: &mut App) -> Option<ThreadView> {
        match target {
            ThreadTarget::Server(key) => {
                let thread = self.open(key, cx)?;
                let shell = StateBridge::shell(cx);
                let row = shell.read(cx).snapshot().threads.iter().find(|t| t.key == *key).cloned();
                let source = shell.read(cx).row(key).cloned();
                let project = row.as_ref().and_then(|row| self.project(&row.project_ref(), cx));
                let thread = thread.read(cx);
                let timeline: TimelineInput = thread.input().clone();
                let questions = thread.projection().map(|p| crate::session_logic::pending_questions(p)).unwrap_or_default();
                let cwd = source
                    .as_ref()
                    .and_then(|row| row.source.worktree_path.clone())
                    .map(SharedString::from)
                    .or_else(|| project.as_ref().map(|(_, root)| root.clone()))
                    .unwrap_or_default();
                Some(ThreadView {
                    key: crate::fixtures::scoped_key(key),
                    project: project.map(|(title, _)| title).unwrap_or_default(),
                    title: row.as_ref().map(|r| r.title.clone()).unwrap_or_default(),
                    cwd,
                    running: timeline.is_working,
                    timeline,
                    is_draft: false,
                    questions: questions.into_iter().map(|(question, _)| question).collect(),
                    branch: row.as_ref().and_then(|r| r.branch.clone()),
                    project_id: row.as_ref().map(|r| r.project_id.clone()),
                })
            }
            ThreadTarget::Draft(id) => {
                let project = self.drafts.borrow().get(id)?.project.clone();
                let info = project.as_ref().and_then(|p| self.project(p, cx));
                Some(ThreadView {
                    key: format!("draft:{}", id.0).into(),
                    project: info.as_ref().map(|(title, _)| title.clone()).unwrap_or_default(),
                    title: t!("New thread"),
                    cwd: info.map(|(_, root)| root).unwrap_or_default(),
                    timeline: TimelineInput::default(),
                    is_draft: true,
                    running: false,
                    questions: Arc::from([]),
                    branch: None,
                    project_id: project.map(|p| p.project_id),
                })
            }
        }
    }

    fn create_draft(&self, project: Option<ProjectRef>, _: &mut App) -> DraftId {
        let next = self.next_draft.get() + 1;
        self.next_draft.set(next);
        let id = DraftId(format!("draft-{}-{next}", uuid::Uuid::new_v4().simple()).into());
        self.drafts.borrow_mut().insert(id.clone(), Draft { project });
        id
    }

    fn discard_draft(&self, key: &ThreadKey, cx: &mut App) {
        sidebar_actions::handle(&SidebarEvent::DiscardDraft(key.clone()), cx);
    }

    fn composer_state(&self, view: &ThreadView, base: &ComposerState, cx: &mut App) -> ComposerState {
        let settings = ComposerSettings::read(cx);
        let state = match view_target(view) {
            Some(key) => {
                let (environment_id, _) = runtime_ids(&key);
                let (Some(thread), Some(environment)) = (self.active(&key, cx), self.environment(&environment_id, cx)) else {
                    return base.clone();
                };
                let row = StateBridge::shell(cx).read(cx).row(&key).cloned();
                let catalog = self.catalog(environment.read(cx));
                composer_state(&key, row.as_deref(), thread.read(cx), environment.read(cx), catalog)
            }
            None => {
                let environment = view
                    .project_id
                    .as_ref()
                    .and_then(|_| self.draft_environment(view, cx))
                    .and_then(|id| self.environment(&id, cx));
                let catalog = environment.as_ref().map(|e| self.catalog(e.read(cx))).unwrap_or_default();
                let selection = crate::state::composer::default_selection(&catalog);
                ComposerState {
                    draft_key: ComposerDraftKey(view.key.clone()),
                    is_draft_thread: true,
                    catalog,
                    selection,
                    send_disabled_reason: environment
                        .as_ref()
                        .filter(|e| e.read(cx).store.session().is_none())
                        .map(|_| t!("Not connected")),
                    ..base.clone()
                }
            }
        };
        settings.apply(state)
    }

    fn menu_source(&self, view: &ThreadView, composer: &Entity<ChatComposer>, cx: &mut App) -> Rc<dyn ComposerMenuSource> {
        let environment_id = match view_target(view) {
            Some(key) => Some(runtime_ids(&key).0),
            None => self.draft_environment(view, cx),
        };
        let Some(environment) = environment_id.as_ref().and_then(|id| self.environment(id, cx)) else {
            return Rc::new(crate::scenes::composer::FixtureMenuSource);
        };
        let cwd = (!view.cwd.is_empty()).then(|| view.cwd.to_string());
        let key = (environment.read(cx).environment_id().clone(), cwd.clone());
        let source = self
            .menus
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| LiveMenuSource::new(&environment, cwd, cx))
            .clone();
        source.set_config(environment.read(cx).config.as_deref());
        source.attach(composer);
        source
    }

    fn draft_store(&self, _: &mut App) -> Rc<RefCell<dyn ComposerDraftStore>> {
        Rc::clone(&self.draft_store)
    }

    fn composer_event(&self, target: &ThreadTarget, event: &ComposerEvent, composer: &WeakEntity<ChatComposer>, cx: &mut App) -> Option<ThreadKey> {
        match target {
            ThreadTarget::Server(key) => {
                let thread = self.active(key, cx)?;
                live_composer::handle_event(event, &thread, composer, cx);
                None
            }
            ThreadTarget::Draft(id) => {
                let ComposerEvent::Submit { prompt, model, .. } = event else { return None };
                let project = self.drafts.borrow().get(id)?.project.clone()?;
                let (environment_id, project_id) = (
                    t3code_contracts::EnvironmentId::new(project.environment_id.0.as_ref()),
                    t3code_contracts::ProjectId::new(project.project_id.0.as_ref()),
                );
                let environment = self.environment(&environment_id, cx)?;
                let (commands, config) = {
                    let env = environment.read(cx);
                    (env.commands.clone(), env.config.clone())
                };
                let catalog = config.as_deref().map(model_catalog).unwrap_or_default();
                let thread_id = WireThreadId::new(uuid::Uuid::new_v4().to_string());
                let runtime_mode = config.as_ref().and_then(|c| c.settings.default_runtime_mode).unwrap_or(WireRuntimeMode::FullAccess);
                let title: String = prompt.lines().next().unwrap_or_default().chars().take(48).collect();
                let message = SendMessage::text(thread_id.clone(), prompt.clone());
                let input = OrchestrationV2ThreadLaunchInput {
                    command_id: CommandId::new(uuid::Uuid::new_v4().to_string()),
                    creation_source: None,
                    thread_id: Some(thread_id.clone()),
                    reuse_existing_thread: None,
                    project_id,
                    title,
                    generate_title: Some(true),
                    model_selection: to_wire(model, &catalog, config.as_deref()),
                    runtime_mode,
                    interaction_mode: ProviderInteractionMode::Default,
                    workspace_strategy: OrchestrationV2ThreadLaunchWorkspaceStrategy::Root(
                        OrchestrationV2ThreadLaunchWorkspaceStrategyRoot { branch: None },
                    ),
                    initial_message: Some(OrchestrationV2ThreadLaunchInputInitialMessage {
                        message_id: Some(message.message_id.clone()),
                        text: prompt.clone(),
                        context: None,
                        attachments: Vec::new(),
                    }),
                };
                let task = StateBridge::spawn(async move { commands.launch_thread(input).await }, cx);
                cx.spawn(async move |cx| {
                    if let Some(Err(error)) = task.await {
                        cx.update(|cx| toast_error(t!("Failed to send message."), error, cx));
                    }
                })
                .detach();
                self.drafts.borrow_mut().remove(id);
                let key = thread_key(&environment_id, &thread_id);
                self.emit(DataEvent::DraftPromoted { draft: id.clone(), thread: key.clone() }, cx);
                Some(key)
            }
        }
    }

    fn timeline_action(&self, key: &ThreadKey, action: &TimelineAction, cx: &mut App) -> ActionOutcome {
        match action {
            TimelineAction::RespondToApproval { request, approve } => {
                let decision = if *approve { ProviderApprovalDecision::Accept } else { ProviderApprovalDecision::Decline };
                live_composer::respond_to_approval(key, request.0.as_ref(), decision, cx);
            }
            TimelineAction::RemoveQueued(id) | TimelineAction::SendQueuedNow(id) | TimelineAction::EditQueued(id) => {
                return self.queued_action(key, action, id, cx);
            }
            TimelineAction::Fork(_) | TimelineAction::EditFrom(_) => {
                Toaster::push(ToastData::new(ToastKind::Info, t!("Not available in this build yet.")), cx);
            }
            TimelineAction::OpenDiff { .. } | TimelineAction::OpenImage { .. } | TimelineAction::OpenLink(_) | TimelineAction::Reply { .. } => {}
        }
        ActionOutcome::Done
    }

    fn answer_questions(&self, key: &ThreadKey, answers: &[usize], cx: &mut App) {
        let Some(thread) = self.active(key, cx) else { return };
        let questions = thread.read(cx).projection().map(|p| crate::session_logic::pending_questions(p)).unwrap_or_default();
        let Some(request) = questions.first().map(|(q, _)| q.request_id.0.clone()) else { return };
        let mut map = ProviderUserInputAnswers::new();
        for ((question, id), &choice) in questions.iter().zip(answers) {
            if let Some((label, _)) = question.options.get(choice) {
                map.insert(id.clone(), serde_json::Value::String(label.to_string()));
            }
        }
        live_composer::answer_question(key, request.as_ref(), map, cx);
    }

    fn set_thread_branch(&self, key: &ThreadKey, branch: SharedString, cx: &mut App) {
        let (environment_id, thread_id) = runtime_ids(key);
        let Some(environment) = self.environment(&environment_id, cx) else { return };
        let commands = environment.read(cx).commands.clone();
        let worktree = StateBridge::shell(cx).read(cx).row(key).and_then(|row| row.source.worktree_path.clone());
        let branch = branch.to_string();
        StateBridge::spawn(async move { commands.set_workspace(&thread_id, Some(branch), worktree).await }, cx).detach();
    }

    fn thread_command(&self, keys: &[ThreadKey], command: MenuCommand, cx: &mut App) -> CommandOutcome {
        match sidebar_actions::handle(&SidebarEvent::ThreadCommand(keys.to_vec(), command), cx) {
            SidebarOutcome::Rename(key) => CommandOutcome::Rename(key),
            SidebarOutcome::NewThread(project) => CommandOutcome::NewThread(project),
            _ => CommandOutcome::Done,
        }
    }

    fn rename_thread(&self, key: &ThreadKey, title: String, cx: &mut App) {
        sidebar_actions::rename(key, title, cx);
    }

    fn diff_input(&self, target: &ThreadTarget, selection: &DiffPanelSelection, cx: &mut App) -> Task<Option<DiffPanelInput>> {
        diff::load(self, target, selection, cx)
    }

    fn workspace(&self, target: &ThreadTarget, cx: &mut App) -> Option<Workspace> {
        let view = self.thread_view(target, cx)?;
        let project = view.project_id.as_ref()?;
        let environment_id = match target {
            ThreadTarget::Server(key) => runtime_ids(key).0,
            ThreadTarget::Draft(_) => self.draft_environment(&view, cx)?,
        };
        if view.cwd.is_empty() {
            return None;
        }
        let project_key: SharedString = format!("{}:{}:{}", environment_id, project.0, view.cwd).into();
        let mut workspaces = self.workspaces.borrow_mut();
        if let Some(workspace) = workspaces.get(&project_key) {
            return Some(workspace.clone());
        }
        let link = rpc::RpcLink::new(&environment_id, cx)?;
        let workspace = Workspace {
            key: project_key.clone(),
            name: view.project.clone(),
            files: Arc::new(workspace::LiveWorkspaceFiles::new(link, view.cwd.to_string(), view_target(&view).map(|key| runtime_ids(&key).1))),
        };
        workspaces.insert(project_key, workspace.clone());
        Some(workspace)
    }

    fn git_controls(&self, view: &ThreadView, cx: &mut App) -> GitControlsSetup {
        git::controls(self, view, cx)
    }

    fn toolbar_props(&self, view: &ThreadView, cx: &mut App) -> BranchToolbarProps {
        git::toolbar_props(self, view, cx)
    }

    fn terminal_backends(&self, cx: &mut App) -> TerminalBackends {
        terminal::backends(cx)
    }

    fn palette_source(&self) -> Rc<dyn CommandPaletteSource> {
        self.palette.clone()
    }

    fn set_active_thread(&self, active: Option<&ThreadKey>, _: &mut App) {
        *self.shown_environment.borrow_mut() = active.map(|key| runtime_ids(key).0);
    }

    fn pull_request_sources(&self, cx: &mut App) -> Vec<EnvironmentListSource> {
        self.environment_links(cx)
            .into_iter()
            .map(|(label, link)| EnvironmentListSource {
                environment_id: link.environment_id().clone(),
                label,
                source: Rc::new(sources::LivePullRequests { link }),
            })
            .collect()
    }

    fn pull_request_detail(&self, environment: &t3code_contracts::EnvironmentId, cx: &mut App) -> Option<Rc<dyn PullRequestDetailSource>> {
        Some(Rc::new(sources::LivePullRequests { link: rpc::RpcLink::new(environment, cx)? }))
    }

    fn issue_sources(&self, cx: &mut App) -> Vec<EnvironmentIssueSource> {
        self.environment_links(cx)
            .into_iter()
            .map(|(label, link)| EnvironmentIssueSource {
                environment_id: link.environment_id().clone(),
                label,
                source: Rc::new(sources::LiveIssues { link }),
            })
            .collect()
    }

    fn usage_source(&self, cx: &mut App) -> Rc<dyn UsageSource> {
        if let Some(source) = self.usage.borrow().as_ref() {
            return source.clone();
        }
        let source: Rc<dyn UsageSource> = LiveUsageSource::new(cx);
        *self.usage.borrow_mut() = Some(source.clone());
        source
    }

    fn server_settings(&self, cx: &mut App) -> Option<Entity<ServerPanelsModel>> {
        let environments = StateBridge::environments(cx).read(cx).list.clone();
        let shown = self.shown_environment.borrow().clone();
        let environment = shown
            .and_then(|id| environments.iter().find(|e| e.read(cx).environment_id() == &id).cloned())
            .or_else(|| environments.first().cloned())?;
        let id = environment.read(cx).environment_id().clone();
        if let Some((existing, model, _)) = self.settings.borrow().as_ref()
            && *existing == id
        {
            return Some(model.clone());
        }
        let source = Arc::new(sources::LiveServerSettings { link: rpc::RpcLink::new(&id, cx)?, snapshot: Default::default() });
        let sync = {
            let source = source.clone();
            move |environment: &LiveEnvironment| {
                if let Some(config) = &environment.config {
                    let label = SharedString::from(config.environment.label.clone());
                    sources::apply_config(&source, label, config);
                }
                if let Some(shell) = &environment.shell.snapshot {
                    source.snapshot.lock().expect("snapshot").projects = shell.projects.iter().map(|p| project_of(p)).collect();
                }
            }
        };
        sync(environment.read(cx));
        let model = cx.new(|_| ServerPanelsModel::new(source.clone()));
        let subscription = cx.observe(&environment, {
            let model = model.downgrade();
            move |environment, cx| {
                sync(environment.read(cx));
                _ = model.update(cx, |model, cx| model.refresh(cx));
            }
        });
        *self.settings.borrow_mut() = Some((id, model.clone(), subscription));
        Some(model)
    }

    fn preview_source(&self, environment: &t3code_contracts::EnvironmentId, cx: &mut App) -> Option<Rc<dyn PreviewSource>> {
        let link = rpc::RpcLink::new(environment, cx)?;
        Some(LivePreviewSource::new(link.link, link.tokio))
    }
}

/// A shell project as the server settings pages list it.
fn project_of(project: &t3code_contracts::OrchestrationProjectShell) -> t3code_contracts::Project {
    serde_json::to_value(project).ok().and_then(|value| serde_json::from_value(value).ok()).unwrap_or_else(|| {
        serde_json::from_value(serde_json::json!({
            "id": project.id, "title": project.title, "workspaceRoot": project.workspace_root,
            "scripts": [], "createdAt": project.created_at, "updatedAt": project.updated_at
        }))
        .expect("a minimal project decodes")
    })
}

impl LiveData {
    /// Every enabled environment's label and RPC link.
    fn environment_links(&self, cx: &mut App) -> Vec<(SharedString, rpc::RpcLink)> {
        let environments = StateBridge::environments(cx).read(cx).list.clone();
        environments
            .iter()
            .filter_map(|env| {
                let env = env.read(cx);
                let label = env
                    .config
                    .as_ref()
                    .map(|c| SharedString::from(c.environment.label.clone()))
                    .unwrap_or_else(|| SharedString::from(env.environment_id().to_string()));
                Some((label, rpc::RpcLink::new(env.environment_id(), cx)?))
            })
            .collect()
    }

    /// The environment of the project a draft starts in.
    fn draft_environment(&self, view: &ThreadView, _: &App) -> Option<t3code_contracts::EnvironmentId> {
        let id = view.key.strip_prefix("draft:")?;
        let drafts = self.drafts.borrow();
        let project = drafts.get(&DraftId(id.to_owned().into()))?.project.as_ref()?;
        Some(t3code_contracts::EnvironmentId::new(project.environment_id.0.as_ref()))
    }

    /// A queued message's row buttons (`QueuedMessagesPanel`): send now steers the running turn,
    /// edit takes the text back into the composer, remove cancels the queued run.
    fn queued_action(&self, key: &ThreadKey, action: &TimelineAction, id: &SharedString, cx: &mut App) -> ActionOutcome {
        let (environment_id, thread_id) = runtime_ids(key);
        let Some(environment) = self.environment(&environment_id, cx) else { return ActionOutcome::Done };
        let Some(thread) = self.active(key, cx) else { return ActionOutcome::Done };
        let commands = environment.read(cx).commands.clone();
        let (queued, text) = {
            let input = thread.read(cx).input();
            let Some(item) = input.queued.iter().find(|q| q.id == *id) else { return ActionOutcome::Done };
            (t3code_contracts::RunId::new(item.id.as_ref()), item.text.clone())
        };
        let running = thread.read(cx).input().latest_run.as_ref().filter(|r| !r.turn_ended).map(|r| t3code_contracts::RunId::new(r.run_id.0.as_ref()));
        match action {
            TimelineAction::SendQueuedNow(_) => {
                let task = match running {
                    Some(target) => StateBridge::spawn(async move { commands.promote_queued_to_steer(&thread_id, queued, target).await }, cx),
                    None => StateBridge::spawn(async move { commands.start_queued_run(&thread_id, queued).await }, cx),
                };
                task.detach();
                ActionOutcome::Done
            }
            TimelineAction::EditQueued(_) => {
                StateBridge::spawn(async move { commands.cancel_queued_run(&thread_id, queued).await }, cx).detach();
                ActionOutcome::LoadComposer(text)
            }
            _ => {
                StateBridge::spawn(async move { commands.cancel_queued_run(&thread_id, queued).await }, cx).detach();
                ActionOutcome::Done
            }
        }
    }
}

/// The server thread a view shows, from its `environmentId:threadId` key.
fn view_target(view: &ThreadView) -> Option<ThreadKey> {
    if view.is_draft {
        return None;
    }
    let (environment, thread) = view.key.split_once(':')?;
    Some(thread_key(&t3code_contracts::EnvironmentId::new(environment), &WireThreadId::new(thread)))
}

/// The composer behaviour the client settings choose (send key, follow-up, rich text).
struct ComposerSettings {
    follow_up: crate::components::chat::composer::FollowUpBehavior,
    send_shortcut: crate::composer_logic::SendShortcut,
    rich_text: bool,
}

impl ComposerSettings {
    fn read(cx: &App) -> Self {
        use crate::components::chat::composer::FollowUpBehavior;
        use crate::hooks::settings_bridge::{composer_rich_text, composer_send_shortcut};
        let follow_up = match crate::hooks::use_settings::ClientSettingsStore::get(cx).follow_up_behavior {
            t3code_contracts::ClientSettingsSchemaFollowUpBehavior::Queue => FollowUpBehavior::Queue,
            _ => FollowUpBehavior::Steer,
        };
        Self { follow_up, send_shortcut: composer_send_shortcut(cx), rich_text: composer_rich_text(cx) }
    }

    fn apply(self, state: ComposerState) -> ComposerState {
        ComposerState { follow_up: self.follow_up, send_shortcut: self.send_shortcut, rich_text: self.rich_text, ..state }
    }
}
