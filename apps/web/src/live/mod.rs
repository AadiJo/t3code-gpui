//! [`AppData`] over the live state bridge: the saved environments' shells, thread streams and
//! commands, and every panel's data over the environment's RPC session. The real app runs on
//! this whenever an environment is saved.

pub mod diff;
pub mod draft_workspace;
pub mod git;
pub mod rpc;
pub mod sources;
pub mod terminal;
pub mod thread_details;
pub mod thread_relationships;
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
use crate::components::sidebar::model::{ProjectRef, SidebarDraft, SidebarSnapshot, ThreadKey, TimestampMs};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::composer_draft_store::{ComposerDraftKey, ComposerDraftStore};
use crate::diff_panel_store::DiffPanelSelection;
use crate::i18n::t;
use crate::provider_models_live::{model_catalog, to_wire};
use crate::router::{DraftId, ThreadTarget};
use crate::components::chat::thread_details_panel::{ThreadDetailsEvent, ThreadDetailsInput};
use crate::state::thread_errors::ThreadErrors;
use crate::state::app_data::{ActionOutcome, AppData, CommandOutcome, DataEvent, DataEvents, DetailsOutcome, GitControlsSetup, RevertedDraft, ThreadView};
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
    created_at: TimestampMs,
    workspace: draft_workspace::DraftWorkspace,
}

/// The window's data on live state. See the module docs.
pub struct LiveData {
    events: Entity<DataEvents>,
    palette: Rc<LivePaletteSource>,
    drafts: Rc<RefCell<HashMap<DraftId, Draft>>>,
    next_draft: std::cell::Cell<usize>,
    draft_store: Rc<RefCell<dyn ComposerDraftStore>>,
    catalogs: RefCell<HashMap<t3code_contracts::EnvironmentId, CatalogCache>>,
    /// The open thread's event subscription.
    thread: RefCell<Option<(ThreadKey, Subscription)>>,
    /// The right panel's side chat's event subscription.
    side_thread: RefCell<Option<(ThreadKey, Subscription)>>,
    /// One menu source per environment and workspace.
    /// By environment, workspace and composer: a source holds its composer's shown thread, so the
    /// main pane and a side chat pane in the same workspace each keep their own.
    menus: RefCell<HashMap<(t3code_contracts::EnvironmentId, Option<String>, gpui_kit::EntityId), Rc<LiveMenuSource>>>,
    /// The last `/` facts per environment and workspace, for a menu source made after them.
    menu_facts: RefCell<HashMap<(t3code_contracts::EnvironmentId, Option<String>), live_composer::SlashFacts>>,
    workspaces: RefCell<HashMap<SharedString, Workspace>>,
    usage: RefCell<Option<Rc<dyn UsageSource>>>,
    /// The server settings model and the environment it follows.
    settings: RefCell<Option<(t3code_contracts::EnvironmentId, Entity<ServerPanelsModel>, Subscription)>>,
    /// The environment of the thread the main pane shows.
    shown_environment: RefCell<Option<t3code_contracts::EnvironmentId>>,
    /// The details card's pull request details, servers and scheduled tasks.
    details: Entity<thread_details::DetailsFeed>,
    /// The latest sidebar search; older answers are dropped.
    search_generation: Rc<std::cell::Cell<u64>>,
    /// Feeds the notification coordinator from the shells.
    _notifications: Entity<crate::state::notifications::LiveNotifications>,
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
        if !cx.has_global::<crate::port_discovery_state::PortForwards>() {
            crate::port_discovery_state::PortForwards::init(cx);
        }
        let details = cx.new(|_| thread_details::DetailsFeed::default());
        let followed = thread_details::follow_changes(&details, &events, cx);
        events.update(cx, |events, _| {
            events._subscriptions.push(subscription);
            events._subscriptions.extend(followed);
        });
        cx.set_global(crate::state::LiveEvents(events.clone()));
        let notifications = cx.new(crate::state::notifications::LiveNotifications::new);
        Rc::new(Self {
            events,
            palette,
            drafts: Rc::default(),
            next_draft: std::cell::Cell::new(0),
            draft_store: Rc::new(RefCell::new(LiveDraftStore::new(StateBridge::drafts(cx)))),
            catalogs: RefCell::default(),
            thread: RefCell::default(),
            side_thread: RefCell::default(),
            menus: RefCell::default(),
            menu_facts: RefCell::default(),
            workspaces: RefCell::default(),
            usage: RefCell::default(),
            settings: RefCell::default(),
            shown_environment: RefCell::default(),
            details,
            search_generation: Rc::default(),
            _notifications: notifications,
        })
    }

    /// The workspace choice of the draft `view` shows.
    pub(crate) fn draft_workspace(&self, view: &ThreadView) -> Option<draft_workspace::DraftWorkspace> {
        let id = view.key.strip_prefix("draft:")?;
        self.drafts.borrow().get(&DraftId(id.to_owned().into())).map(|d| d.workspace.clone())
    }

    fn emit(&self, event: DataEvent, cx: &mut App) {
        self.events.update(cx, |_, cx| cx.emit(event));
    }

    /// The open thread for `key`, opening it (and following its events) when another is open.
    fn open(&self, key: &ThreadKey, cx: &mut App) -> Option<Entity<LiveThread>> {
        let threads = StateBridge::threads(cx);
        // The main pane takes over a side chat it navigates to; the panel's pane reopens its own.
        if self.side_thread.borrow().as_ref().is_some_and(|(k, _)| k == key) {
            *self.side_thread.borrow_mut() = None;
        }
        if let Some(active) = threads.read(cx).active()
            && active.read(cx).key() == key
        {
            return Some(active.clone());
        }
        let thread = threads.update(cx, |threads, cx| threads.open(key.clone(), cx))?;
        *self.thread.borrow_mut() = Some((key.clone(), self.forward_events(&thread, key, cx)));
        Some(thread)
    }

    /// Re-emits a live thread's changes as [`DataEvent`]s, and a change of its environment's
    /// config (provider commands, usage limits) as a thread change.
    fn forward_events(&self, thread: &Entity<LiveThread>, key: &ThreadKey, cx: &mut App) -> Subscription {
        let events = self.events.clone();
        let target = ThreadTarget::Server(key.clone());
        let thread_events = cx.subscribe(thread, {
            let (events, target) = (events.clone(), target.clone());
            move |thread, event: &LiveThreadEvent, cx| {
                let event = match event {
                    LiveThreadEvent::Text(changes) => DataEvent::Text { thread: thread.read(cx).key().clone(), changes: changes.clone() },
                    LiveThreadEvent::Structure | LiveThreadEvent::Status => DataEvent::ThreadChanged(target.clone()),
                };
                events.update(cx, |_, cx| cx.emit(event));
            }
        });
        let (environment_id, _) = runtime_ids(key);
        let Some(environment) = self.environment(&environment_id, cx) else { return thread_events };
        let mut last = environment.read(cx).config.clone();
        let config_changes = cx.observe(&environment, move |environment, cx| {
            let config = environment.read(cx).config.clone();
            if config.as_ref().map(Arc::as_ptr) != last.as_ref().map(Arc::as_ptr) {
                last = config;
                events.update(cx, |_, cx| cx.emit(DataEvent::ThreadChanged(target.clone())));
            }
        });
        Subscription::join(thread_events, config_changes)
    }

    /// The live thread when `key` is the open one or the right panel's side chat.
    fn active(&self, key: &ThreadKey, cx: &App) -> Option<Entity<LiveThread>> {
        let threads = StateBridge::threads(cx).read(cx);
        threads.active().into_iter().chain(threads.side()).find(|t| t.read(cx).key() == key).cloned()
    }

    /// The chat view's facts for a server thread already open in either slot.
    fn server_view(&self, key: &ThreadKey, thread: &Entity<LiveThread>, cx: &mut App) -> ThreadView {
        let shell = StateBridge::shell(cx);
        let row = shell.read(cx).snapshot().threads.iter().find(|t| t.key == *key).cloned();
        let source = shell.read(cx).row(key).cloned();
        // Unlisted threads (side chats, subagents) and threads whose row has not arrived name
        // their project themselves, as the web's header reads it from the thread.
        let projection = thread.read(cx).projection().cloned();
        let project_ref = row.as_ref().map(|row| row.project_ref()).or_else(|| {
            let id = source.as_ref().map(|r| r.source.project_id.to_string()).or_else(|| projection.as_ref().map(|p| p.thread.project_id.to_string()))?;
            Some(ProjectRef { environment_id: key.environment_id.clone(), project_id: crate::components::sidebar::model::ProjectId(id.into()) })
        });
        let project = project_ref.as_ref().and_then(|r| self.project(r, cx));
        let project_icon = project_ref.as_ref().and_then(|r| self.project_icon_of(r, cx));
        let supports_rollback = self.supports_rollback(key, cx);
        let thread = thread.read(cx);
        let mut timeline: TimelineInput = thread.input().clone();
        timeline.supports_rollback = supports_rollback && !timeline.revert_turn_counts.is_empty();
        let questions = thread.projection().map(|p| crate::session_logic::pending_questions(p)).unwrap_or_default();
        let cwd = source
            .as_ref()
            .and_then(|row| row.source.worktree_path.clone())
            .map(SharedString::from)
            .or_else(|| project.as_ref().map(|(_, root)| root.clone()))
            .unwrap_or_default();
        ThreadView {
            key: crate::fixtures::scoped_key(key),
            project: project.map(|(title, _)| title).unwrap_or_default(),
            title: row
                .as_ref()
                .map(|r| r.title.clone())
                .or_else(|| source.as_ref().map(|r| r.source.title.clone().into()))
                .or_else(|| projection.as_ref().map(|p| p.thread.title.clone().into()))
                .unwrap_or_default(),
            cwd,
            running: timeline.is_working,
            timeline,
            is_draft: false,
            questions: questions.into_iter().map(|(question, _)| question).collect(),
            branch: row.as_ref().and_then(|r| r.branch.clone()),
            project_id: project_ref.map(|r| r.project_id),
            project_icon,
        }
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

    /// A project's icon override and fetched favicon, as its sidebar rows draw them.
    fn project_icon_of(&self, project: &ProjectRef, cx: &App) -> Option<crate::components::project_favicon::ProjectIconSource> {
        let shell = StateBridge::shell(cx);
        let shell = shell.read(cx);
        let p = shell.snapshot().projects.iter().find(|p| p.project_ref == *project)?;
        Some(crate::components::project_favicon::ProjectIconSource { icon: p.icon.clone(), favicon: p.favicon.clone() })
    }

    fn catalog(&self, environment: &LiveEnvironment, cx: &App) -> Arc<crate::model_selection::ModelCatalog> {
        let legacy = &crate::hooks::use_settings::ClientSettingsStore::get(cx).favorites;
        self.catalogs.borrow_mut().entry(environment.environment_id().clone()).or_default().get(environment.config.as_ref(), legacy)
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
        SidebarSnapshot { active_thread: active.cloned(), drafts: self.drafts(cx), ..shell.read(cx).snapshot().clone() }
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
                Some(self.server_view(key, &thread, cx))
            }
            ThreadTarget::Draft(id) => {
                let (project, branch) = {
                    let drafts = self.drafts.borrow();
                    let draft = drafts.get(id)?;
                    (draft.project.clone(), draft.workspace.branch.clone())
                };
                let info = project.as_ref().and_then(|p| self.project(p, cx));
                let project_icon = project.as_ref().and_then(|p| self.project_icon_of(p, cx));
                Some(ThreadView {
                    key: format!("draft:{}", id.0).into(),
                    project: info.as_ref().map(|(title, _)| title.clone()).unwrap_or_default(),
                    title: t!("New thread"),
                    cwd: info.map(|(_, root)| root).unwrap_or_default(),
                    timeline: TimelineInput::default(),
                    is_draft: true,
                    running: false,
                    questions: Arc::from([]),
                    branch: branch.map(SharedString::from),
                    project_id: project.map(|p| p.project_id),
                    project_icon,
                })
            }
        }
    }

    fn create_draft(&self, project: Option<ProjectRef>, cx: &mut App) -> DraftId {
        let next = self.next_draft.get() + 1;
        self.next_draft.set(next);
        let id = DraftId(format!("draft-{}-{next}", uuid::Uuid::new_v4().simple()).into());
        let created_at = TimestampMs(crate::timestamp_format::now_ms());
        let workspace = project
            .as_ref()
            .map(|p| {
                let (environment_id, project_id) = draft_ids(p);
                draft_workspace::DraftWorkspace::new(&environment_id, &project_id, cx)
            })
            .unwrap_or_default();
        self.drafts.borrow_mut().insert(id.clone(), Draft { project: project.clone(), created_at, workspace: workspace.clone() });
        if let Some(project) = project {
            self.read_project_file(&id, &project, workspace, cx);
        }
        id
    }

    fn project_icon(&self, target: &ThreadTarget, cx: &mut App) -> Option<crate::components::project_favicon::ProjectIconSource> {
        let project = match target {
            ThreadTarget::Server(key) => StateBridge::shell(cx).read(cx).snapshot().threads.iter().find(|t| t.key == *key).map(|row| row.project_ref())?,
            ThreadTarget::Draft(id) => self.drafts.borrow().get(id)?.project.clone()?,
        };
        self.project_icon_of(&project, cx)
    }

    fn draft_workspace_event(&self, draft: &DraftId, event: &crate::components::branch_toolbar::BranchToolbarEvent, cx: &mut App) {
        let changed = {
            let mut drafts = self.drafts.borrow_mut();
            let Some(entry) = drafts.get_mut(draft) else { return };
            let Some(project) = entry.project.clone() else { return };
            let (environment_id, project_id) = draft_ids(&project);
            let before = entry.workspace.clone();
            let seed = git::previous_worktree(&environment_id, project_id.as_str(), entry.workspace.worktree_path.as_deref(), cx);
            entry.workspace.apply(event, &environment_id, &project_id, seed, cx);
            entry.workspace != before
        };
        if changed {
            self.emit(DataEvent::ThreadChanged(ThreadTarget::Draft(draft.clone())), cx);
        }
    }

    fn discard_draft(&self, key: &ThreadKey, cx: &mut App) {
        sidebar_actions::handle(&SidebarEvent::DiscardDraft(key.clone()), cx);
    }

    fn drafts(&self, cx: &App) -> Vec<SidebarDraft> {
        let composers = StateBridge::drafts(cx).current();
        self.drafts
            .borrow()
            .iter()
            .filter_map(|(id, draft)| {
                let prompt = &composers.drafts_by_thread_key.get(&format!("draft:{}", id.0))?.prompt;
                let preview = prompt.trim().split('\n').next().filter(|line| !line.is_empty())?;
                Some(SidebarDraft {
                    id: id.0.clone(),
                    project: draft.project.clone()?,
                    preview: SharedString::from(preview.to_owned()),
                    created_at: draft.created_at,
                })
            })
            .collect()
    }

    fn discard_draft_session(&self, id: &DraftId, cx: &mut App) {
        self.drafts.borrow_mut().remove(id);
        let key = format!("draft:{}", id.0);
        StateBridge::drafts(cx).update(|drafts| {
            drafts.drafts_by_thread_key.remove(&key);
        });
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
                let catalog = self.catalog(environment.read(cx), cx);
                let drafts = StateBridge::drafts(cx).current();
                let draft = drafts.drafts_by_thread_key.get(live_composer::draft_key(&key).0.as_ref());
                let armed = live_composer::FreshSessionArms::armed(&key, cx);
                let mut state = composer_state(&key, row.as_deref(), thread.read(cx), environment.read(cx), catalog, draft, armed);
                let show_skills = crate::hooks::use_settings::ClientSettingsStore::get(cx).show_skills_in_slash_menu;
                let mut facts = live_composer::slash_facts(row.as_deref(), thread.read(cx), environment.read(cx), &state.selection.instance_id.0, show_skills);
                // `/usage-limits` (`useUsageLimitsPanel`): offered while the provider reports limits.
                let instance = state.selection.instance_id.0.to_string();
                let panel_key = live_composer::usage_panel_key(&key, row.as_deref(), thread.read(cx).input(), &instance);
                let config = environment.read(cx).config.clone();
                state.usage_limits = panel_key.is_some() && live_composer::usage_limits_offered(config.as_deref(), &instance);
                facts.usage_limits = state.usage_limits;
                if let Some(banner) = live_composer::usage_banner(&key, panel_key.as_deref(), config.as_deref(), &instance, cx) {
                    state.banners.insert(0, banner);
                }
                let cwd = (!view.cwd.is_empty()).then(|| view.cwd.to_string());
                self.menu_facts.borrow_mut().insert((environment_id.clone(), cwd.clone()), facts.clone());
                for ((environment, workspace, _), source) in self.menus.borrow().iter() {
                    if *environment == environment_id && *workspace == cwd {
                        source.set_facts(facts.clone());
                    }
                }
                state
            }
            None => {
                let environment = view
                    .project_id
                    .as_ref()
                    .and_then(|_| self.draft_environment(view, cx))
                    .and_then(|id| self.environment(&id, cx));
                let catalog = environment.as_ref().map(|e| self.catalog(e.read(cx), cx)).unwrap_or_default();
                // A new thread starts on the last pick (`stickyModelSelectionByProvider`).
                let drafts = StateBridge::drafts(cx).current();
                let selection = drafts
                    .sticky_active_provider
                    .as_ref()
                    .and_then(|provider| drafts.sticky_model_selection_by_provider.get(provider))
                    .map(crate::provider_models_live::from_wire)
                    .filter(|s| catalog.instances.iter().any(|i| i.instance_id == s.instance_id))
                    .unwrap_or_else(|| crate::state::composer::default_selection(&catalog));
                let file_attachment_limit = crate::state::attachments::file_limit(environment.as_ref().and_then(|e| e.read(cx).config.clone()).as_deref());
                ComposerState {
                    draft_key: ComposerDraftKey(view.key.clone()),
                    is_draft_thread: true,
                    catalog,
                    selection,
                    send_disabled_reason: environment
                        .as_ref()
                        .filter(|e| e.read(cx).store.session().is_none())
                        .map(|_| t!("Not connected")),
                    file_attachment_limit,
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
        let key = (environment.read(cx).environment_id().clone(), cwd.clone(), composer.entity_id());
        let facts = self.menu_facts.borrow().get(&(key.0.clone(), cwd.clone())).cloned();
        let source = self
            .menus
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let source = LiveMenuSource::new(&environment, cwd, self.palette.clone(), cx);
                if let Some(facts) = facts {
                    source.set_facts(facts);
                }
                source
            })
            .clone();
        source.set_config(environment.read(cx).config.as_ref());
        source.set_shown(view_target(view));
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
                match live_composer::handle_event(event, &thread, composer, cx) {
                    live_composer::EventOutcome::OpenThread(key) => crate::state::open_thread(key, cx),
                    live_composer::EventOutcome::Done => {}
                }
                None
            }
            ThreadTarget::Draft(id) => {
                let draft_environment = self.drafts.borrow().get(id)?.project.as_ref().map(|p| t3code_contracts::EnvironmentId::new(p.environment_id.0.as_ref()));
                match event {
                    ComposerEvent::AttachmentsAdded(attachments) => {
                        if let Some(environment_id) = draft_environment {
                            crate::state::attachments::start_uploads(&environment_id, attachments, composer, cx);
                        }
                        return None;
                    }
                    ComposerEvent::AttachmentsRestored(attachments) => {
                        let fresh: Vec<_> = attachments.iter().filter(|a| !crate::state::attachments::AttachmentUploads::known(&a.context_id)).cloned().collect();
                        if let Some(environment_id) = draft_environment {
                            crate::state::attachments::start_uploads(&environment_id, &fresh, composer, cx);
                        }
                        return None;
                    }
                    ComposerEvent::AttachmentRemoved(context_id) => {
                        crate::state::attachments::AttachmentUploads::release(context_id, cx);
                        return None;
                    }
                    ComposerEvent::AttachRequested => {
                        live_composer::pick_files(composer, cx);
                        return None;
                    }
                    ComposerEvent::FavoriteToggled { instance_id, model } => {
                        if let Some(environment_id) = draft_environment {
                            crate::state::favorites::toggle(&environment_id, instance_id, model, cx);
                        }
                        return None;
                    }
                    ComposerEvent::ModelChanged(selection) => {
                        let config = draft_environment.and_then(|id| self.environment(&id, cx)).and_then(|e| e.read(cx).config.clone());
                        let catalog = config.as_deref().map(model_catalog).unwrap_or_default();
                        let wire = to_wire(selection, &catalog, config.as_deref());
                        let instance = wire.instance_id.clone();
                        StateBridge::drafts(cx).update(|d| {
                            d.sticky_model_selection_by_provider.insert(instance.clone(), wire);
                            d.sticky_active_provider = Some(instance);
                        });
                        return None;
                    }
                    _ => {}
                }
                let ComposerEvent::Submit { prompt, model, attachments, .. } = event else { return None };
                let (project, workspace) = {
                    let drafts = self.drafts.borrow();
                    let draft = drafts.get(id)?;
                    (draft.project.clone()?, draft.workspace.clone())
                };
                let draft_key: SharedString = format!("draft:{}", id.0).into();
                let Ok(workspace_strategy) = workspace.strategy() else {
                    ThreadErrors::set(&draft_key, Some(t!("Select a base branch before sending in New worktree mode.")), cx);
                    return None;
                };
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
                let runtime_mode = composer
                    .upgrade()
                    .map(|c| live_composer::wire_runtime_mode(c.read(cx).state().runtime_mode))
                    .or_else(|| config.as_ref().and_then(|c| c.settings.default_runtime_mode))
                    .unwrap_or(WireRuntimeMode::FullAccess);
                let Some(turn) = crate::state::attachments::turn_attachments(&environment_id, attachments) else {
                    ThreadErrors::set(&draft_key, Some(t!("Retry or remove failed uploads before sending.")), cx);
                    return None;
                };
                ThreadErrors::set(&draft_key, None, cx);
                let restore = SharedString::from(prompt.to_owned());
                let title = live_composer::title_seed(prompt.trim(), attachments);
                let text = match prompt.trim() {
                    "" if !attachments.is_empty() => crate::state::attachments::ATTACHMENT_ONLY_PROMPT.to_owned(),
                    text => text.to_owned(),
                };
                let message = SendMessage::text(thread_id.clone(), text.clone());
                let sent: Vec<SharedString> = attachments.iter().map(|a| a.context_id.clone()).collect();
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
                    workspace_strategy,
                    initial_message: Some(OrchestrationV2ThreadLaunchInputInitialMessage {
                        message_id: Some(message.message_id.clone()),
                        text,
                        context: live_composer::with_thread_records(turn.context, prompt, &environment_id),
                        attachments: turn.attachments,
                    }),
                };
                let task = StateBridge::spawn(async move { commands.launch_thread(input).await }, cx);
                // A launch the server refused created no thread: as the web does, the draft comes
                // back with the error and its prompt, and the route returns to it.
                let removed = self.drafts.borrow_mut().remove(id);
                let key = thread_key(&environment_id, &thread_id);
                let (drafts, events, draft_id, failed_thread) = (Rc::clone(&self.drafts), self.events.clone(), id.clone(), key.clone());
                cx.spawn(async move |cx| match task.await {
                    Some(Err(error)) => cx.update(|cx| {
                        if let Some(draft) = removed {
                            drafts.borrow_mut().insert(draft_id.clone(), draft);
                        }
                        let error = crate::state::composer::send_error(&error);
                        ThreadErrors::send_failed(&draft_key, error.clone(), restore, cx);
                        events.update(cx, |_, cx| cx.emit(DataEvent::DraftLaunchFailed { draft: draft_id, thread: failed_thread, error }));
                    }),
                    Some(Ok(_)) => crate::state::attachments::AttachmentUploads::forget_sent(sent),
                    None => {}
                })
                .detach();
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
            TimelineAction::Fork(message) => self.fork_from(key, message, cx),
            TimelineAction::EditFrom(message) => return self.confirm_revert(key, message, cx),
            TimelineAction::CancelWorktreeSetup => {
                // `worktreeSetup.cancel`; the setup stream shows the result.
                let (environment_id, thread_id) = runtime_ids(key);
                let Some(link) = rpc::RpcLink::new(&environment_id, cx) else { return ActionOutcome::Done };
                let call = link.call::<t3code_contracts::rpc::methods::WorktreeSetupCancel>(t3code_contracts::WorktreeSetupCancelInput { thread_id });
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = call.await {
                            tracing::warn!(%error, "cancelling worktree setup failed");
                        }
                    })
                    .detach();
            }
            // The route view handles these itself.
            TimelineAction::OpenDiff { .. }
            | TimelineAction::OpenImage { .. }
            | TimelineAction::OpenLink(_)
            | TimelineAction::RunShellCommand(_)
            | TimelineAction::Reply { .. }
            | TimelineAction::OpenTerminal(_) => {}
        }
        ActionOutcome::Done
    }

    fn revert(&self, key: &ThreadKey, message: &crate::components::chat::timeline_entries::MessageId, turn_count: usize, restore_files: bool, cx: &mut App) -> Task<Option<RevertedDraft>> {
        self.revert_to(key, message, turn_count, restore_files, cx)
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

    fn show_older_settled(&self, request: crate::components::sidebar::model::SettledShelfRequest, cx: &mut App) {
        StateBridge::shell(cx).update(cx, |shell, cx| shell.set_settled_request(request, cx));
    }

    fn search_threads(&self, query: SharedString, cx: &mut App) {
        // `useThreadSearch`: a pasted wall of text searches by its start; messages need a word
        // of three or more letters; answers land 120 ms after the typing stops. A server whose
        // shell leaves older settled threads out is also asked for their titles
        // (`olderSettledBefore`), and the rows it returns are merged into its shell while the
        // search stands (`createThreadSearchResultsAtomFamily`, `mountRows`).
        const SEARCH_ROWS_KEY: &str = "thread-search";
        let generation = self.search_generation.get() + 1;
        self.search_generation.set(generation);
        let trimmed: String = query.trim().chars().take(200).collect::<String>().trim().to_owned();
        let shell = StateBridge::shell(cx);
        let search_messages = trimmed.split_whitespace().any(|token| token.chars().count() >= 3);
        let environments = StateBridge::environments(cx).read(cx).list.clone();
        let targets: Vec<_> = environments
            .iter()
            .filter_map(|env| {
                let env = env.read(cx);
                // Nothing older is asked for before this session's first snapshot (`searchGenerationAtom`).
                let before = env.older.older.as_ref().filter(|_| env.shell.generation > 0).map(|older| older.before.clone());
                Some((env.store.clone(), before, rpc::RpcLink::new(env.environment_id(), cx)?))
            })
            .collect();
        if trimmed.is_empty() || !targets.iter().any(|(_, before, _)| search_messages || before.is_some()) {
            for (store, _, _) in &targets {
                store.shell().unmount_rows(SEARCH_ROWS_KEY);
            }
            shell.update(cx, |shell, cx| shell.set_search_matches(None, cx));
            return;
        }
        let weak = Rc::downgrade(&self.search_generation);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(120)).await;
            if weak.upgrade().is_none_or(|token| token.get() != generation) {
                return;
            }
            let mut found = Vec::new();
            let mut rows = Vec::new();
            for (store, before, link) in targets {
                if !search_messages && before.is_none() {
                    rows.push((store, None));
                    continue;
                }
                let environment_id = link.environment_id().clone();
                let input = t3code_contracts::OrchestrationV2SearchThreadsInput {
                    query: trimmed.clone(),
                    titles_only: (before.is_some() && !search_messages).then_some(true),
                    older_settled_before: before,
                    limit: None,
                    project_ids: None,
                    environment_label: None,
                };
                let mut older = None;
                if let Ok(result) = link.call::<t3code_contracts::rpc::methods::OrchestrationSearchThreads>(input).await {
                    found.extend(result.matches.into_iter().map(|m| (thread_key(&environment_id, &m.thread_id), m)));
                    older = result
                        .threads
                        .filter(|threads| !threads.is_empty())
                        .map(|threads| t3code_client_runtime::state::thread_families::FamilyRows::new(result.snapshot_sequence.unwrap_or(0), threads));
                }
                rows.push((store, older));
            }
            if weak.upgrade().is_none_or(|token| token.get() != generation) {
                return;
            }
            for (store, older) in rows {
                match older {
                    Some(read) => store.shell().mount_rows(SEARCH_ROWS_KEY, vec![read]),
                    None => store.shell().unmount_rows(SEARCH_ROWS_KEY),
                }
            }
            cx.update(|cx| {
                // Ranked across environments as the palette ranks them (`rankMessageMatches`).
                let subjects: HashMap<ThreadKey, t3code_shared::palette_search::ThreadSearchSubject> = found
                    .iter()
                    .filter_map(|(key, _)| {
                        let row = StateBridge::shell(cx).read(cx).row(key)?.clone();
                        let mut subject = t3code_shared::palette_search::ThreadSearchSubject::new(row.title()).with_branch(row.source.branch.as_deref());
                        subject.id = Some(Arc::clone(&row.source.id.0));
                        subject.activity_at = row.latest_user_message_at.or(row.updated_at).or(row.created_at);
                        Some((key.clone(), subject))
                    })
                    .collect();
                let ranked = t3code_client_runtime::state::thread_search::rank_message_matches(
                    found.iter().map(|(key, m)| (key.clone(), m)),
                    |key| subjects.get(key),
                    t3code_client_runtime::state::models::now_ms(),
                );
                let mut seen = std::collections::HashSet::new();
                let keys: Vec<ThreadKey> = ranked.into_iter().filter(|key| seen.insert(key.clone())).collect();
                let matches = crate::components::sidebar::model::SearchContentMatches { query: query.clone(), threads: Arc::new(keys) };
                shell.update(cx, |shell, cx| shell.set_search_matches(Some(matches), cx))
            });
        })
        .detach();
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

    fn set_active_thread(&self, active: Option<&ThreadKey>, cx: &mut App) {
        *self.shown_environment.borrow_mut() = active.map(|key| runtime_ids(key).0);
        // The route owns the open thread's family mount (`useOpenThreadFamily`).
        let active = active.cloned();
        StateBridge::shell(cx).update(cx, |shell, cx| shell.set_active_thread(active.clone(), cx));
        self.details.update(cx, |details, cx| details.follow(active, cx));
    }

    fn thread_details(&self, view: &ThreadView, base: ThreadDetailsInput, cx: &mut App) -> ThreadDetailsInput {
        thread_details::input(&self.details, view, base, cx)
    }

    fn thread_details_event(&self, key: &ThreadKey, event: &ThreadDetailsEvent, cx: &mut App) -> Task<DetailsOutcome> {
        thread_details::event(&self.details, key, event, cx)
    }

    fn side_chat(&self, key: &ThreadKey, cx: &App) -> crate::state::app_data::SideChatLookup {
        use crate::state::app_data::{SideChatFacts, SideChatLookup, SideChatState};
        let (environment_id, thread_id) = runtime_ids(key);
        let Some(environment) = StateBridge::environment(&environment_id, cx) else { return SideChatLookup::Unknown };
        let Some(snapshot) = environment.read(cx).shell.snapshot.clone() else { return SideChatLookup::Unknown };
        let Some(shell) = snapshot.threads.iter().find(|t| t.id == thread_id) else { return SideChatLookup::Deleted };
        let row = StateBridge::shell(cx).read(cx).row(key).cloned();
        let state = row.as_ref().and_then(|row| {
            use t3code_client_runtime::state::models::{PendingSubagentRequest, is_active_status};
            if row.has_pending_approvals || row.pending_subagent_request == Some(PendingSubagentRequest::Approval) {
                Some(SideChatState::NeedsApproval)
            } else if row.has_pending_user_input || row.pending_subagent_request == Some(PendingSubagentRequest::UserInput) {
                Some(SideChatState::NeedsInput)
            } else {
                row.runtime.as_ref().filter(|r| is_active_status(r.status)).map(|_| SideChatState::Running)
            }
        });
        SideChatLookup::Found(SideChatFacts { title: shell.title.clone().into(), has_user_message: shell.latest_user_message_at.is_some(), state })
    }

    fn side_chat_view(&self, key: &ThreadKey, cx: &mut App) -> Option<ThreadView> {
        let threads = StateBridge::threads(cx);
        let held = self.side_thread.borrow().as_ref().is_some_and(|(k, _)| k == key);
        let thread = threads.update(cx, |threads, cx| threads.open_side(key.clone(), cx))?;
        if !held {
            *self.side_thread.borrow_mut() = Some((key.clone(), self.forward_events(&thread, key, cx)));
        }
        let mut view = self.server_view(key, &thread, cx);
        // The side chat's timeline opens on where its context came from (`parentThreadLink`).
        let (environment_id, thread_id) = runtime_ids(key);
        let parent = StateBridge::environment(&environment_id, cx).and_then(|env| {
            let snapshot = env.read(cx).shell.snapshot.clone()?;
            let shell = snapshot.threads.iter().find(|t| t.id == thread_id)?.clone();
            let parent_id = shell.lineage.parent_thread_id.clone()?;
            Some(snapshot.threads.iter().find(|t| t.id == parent_id).map(|p| p.title.clone()))
        });
        if let Some(parent) = parent {
            use crate::components::chat::timeline_entries::{Divider, DividerKind, EntryKind, TimelineEntry, TimelineEvent};
            let divider = TimelineEntry {
                id: crate::components::chat::timeline_entries::EntryId::new("side-chat-parent-link"),
                created_at: crate::components::chat::timeline_entries::Timestamp(0),
                kind: EntryKind::Event(TimelineEvent::Divider(Divider {
                    kind: DividerKind::SideChatParent,
                    run_id: None,
                    label: t!("Context from", context = "divider label; the parent thread title follows as a link"),
                    detail: Some(parent.map(SharedString::from).unwrap_or_else(|| t!("Parent thread"))),
                    action: false,
                })),
            };
            let mut entries = Vec::with_capacity(view.timeline.entries.len() + 1);
            entries.push(divider);
            entries.extend(view.timeline.entries.iter().cloned());
            view.timeline.entries = Arc::new(entries);
        }
        Some(view)
    }

    fn close_side_chat(&self, key: &ThreadKey, cx: &mut App) {
        let held = self.side_thread.borrow().as_ref().is_some_and(|(k, _)| k == key);
        if held {
            *self.side_thread.borrow_mut() = None;
        }
        StateBridge::threads(cx).update(cx, |threads, cx| threads.close_side(key, cx));
    }

    fn delete_thread(&self, key: &ThreadKey, only_if_unused: bool, cx: &mut App) -> Task<Result<(), SharedString>> {
        let (environment_id, thread_id) = runtime_ids(key);
        let Some(environment) = StateBridge::environment(&environment_id, cx) else { return Task::ready(Ok(())) };
        let commands = environment.read(cx).commands.clone();
        let task = StateBridge::spawn(async move { commands.delete(&thread_id, only_if_unused).await }, cx);
        cx.spawn(async move |_| match task.await {
            Some(Err(error)) => Err(SharedString::from(error.to_string())),
            _ => Ok(()),
        })
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

impl LiveData {
    /// Whether the thread's provider can roll its conversation back (`supportsConversationRollback`).
    fn supports_rollback(&self, key: &ThreadKey, cx: &App) -> bool {
        let (environment_id, _) = runtime_ids(key);
        let Some(environment) = self.environment(&environment_id, cx) else { return false };
        let instance = StateBridge::shell(cx).read(cx).row(key).map(|row| row.source.model_selection.instance_id.clone());
        let env = environment.read(cx);
        let Some(config) = env.config.as_ref() else { return false };
        config
            .providers
            .iter()
            .find(|p| Some(p.instance_id.as_str()) == instance.as_deref())
            .is_some_and(|p| p.supports_conversation_rollback != Some(false))
    }

    /// Forks the thread after the run a reply ends (`onForkFromRun`) and opens the fork.
    fn fork_from(&self, key: &ThreadKey, reply: &crate::components::chat::timeline_entries::MessageId, cx: &mut App) {
        let (environment_id, source) = runtime_ids(key);
        let Some(environment) = self.environment(&environment_id, cx) else { return };
        let Some(thread) = self.active(key, cx) else { return };
        let run = thread
            .read(cx)
            .projection()
            .and_then(|p| p.messages.iter().find(|m| m.id.as_ref() == reply.0.as_ref()).and_then(|m| m.run_id.clone()));
        let Some(run_id) = run else {
            toast_error(t!("Failed to fork this response."), "", cx);
            return;
        };
        let title = StateBridge::shell(cx).read(cx).row(key).map(|row| row.title().to_owned()).unwrap_or_default();
        let (commands, store) = {
            let env = environment.read(cx);
            (env.commands.clone(), env.store.clone())
        };
        let target = WireThreadId::new(uuid::Uuid::new_v4().to_string());
        let task = StateBridge::spawn(
            {
                let target = target.clone();
                async move {
                    let point = t3code_contracts::OrchestrationV2ThreadForkSourcePoint::Run(t3code_contracts::OrchestrationV2ThreadForkSourcePointRun { run_id });
                    commands.fork(&source, target.clone(), point, Some(format!("{title} fork")), false).await?;
                    live_composer::wait_for_thread(&store, &target).await
                }
            },
            cx,
        );
        let opened = thread_key(&environment_id, &target);
        cx.spawn(async move |cx| match task.await {
            Some(Ok(())) => cx.update(|cx| crate::state::open_thread(opened, cx)),
            Some(Err(error)) => cx.update(|cx| toast_error(t!("Failed to fork this response."), error, cx)),
            None => {}
        })
        .detach();
    }

    /// The checkpoint "Edit from here" rewinds to (`findRevertCheckpoint`).
    /// Reads the project's `t3.json` when it decides a new draft's mode, and applies the mode
    /// unless the draft's workspace was changed meanwhile. The web waits for the file before it
    /// opens the draft; here the draft opens at once and its mode follows the file.
    fn read_project_file(&self, draft: &DraftId, project: &ProjectRef, initial: draft_workspace::DraftWorkspace, cx: &mut App) {
        let (environment_id, project_id) = draft_ids(project);
        let Some(root) = draft_workspace::project_file_root(&environment_id, &project_id, cx) else { return };
        let Some(link) = crate::live::rpc::RpcLink::new(&environment_id, cx) else { return };
        let input = t3code_contracts::ProjectReadFileInput { cwd: root, relative_path: draft_workspace::PROJECT_FILE.into(), allow_missing: Some(true) };
        let read = link.call::<t3code_contracts::rpc::methods::ProjectsReadFile>(input);
        let (drafts, draft, events) = (Rc::downgrade(&self.drafts), draft.clone(), self.events.downgrade());
        cx.spawn(async move |cx| {
            let Ok(file) = read.await else { return };
            if file.exists == Some(false) || file.truncated {
                return;
            }
            let Some(mode) = draft_workspace::project_file_env_mode(&file.contents) else { return };
            cx.update(|cx| {
                let Some(drafts) = drafts.upgrade() else { return };
                let workspace = draft_workspace::DraftWorkspace::from_project_file(mode, &environment_id, &project_id, cx);
                {
                    let mut drafts = drafts.borrow_mut();
                    let Some(entry) = drafts.get_mut(&draft) else { return };
                    if entry.workspace != initial || entry.workspace == workspace {
                        return;
                    }
                    entry.workspace = workspace;
                }
                _ = events.update(cx, |_, cx| cx.emit(DataEvent::ThreadChanged(ThreadTarget::Draft(draft))));
            });
        })
        .detach();
    }

    fn revert_checkpoint(projection: &t3code_shared::thread_projection::ThreadProjection, turn_count: usize) -> Option<Arc<t3code_contracts::OrchestrationV2Checkpoint>> {
        use t3code_contracts::OrchestrationV2CheckpointStatus as Status;
        projection
            .checkpoints
            .iter()
            .rev()
            .find(|c| {
                matches!(c.status, Status::Ready | Status::Missing)
                    && if turn_count == 0 { c.ordinal_within_scope == 0 && c.app_run_ordinal.is_none() } else { c.app_run_ordinal == Some(turn_count as u64) }
            })
            .cloned()
    }

    /// Checks an "Edit from here" can run (`onRevertToTurnCount`) and asks for confirmation.
    fn confirm_revert(&self, key: &ThreadKey, message: &crate::components::chat::timeline_entries::MessageId, cx: &mut App) -> ActionOutcome {
        let Some(thread) = self.active(key, cx) else { return ActionOutcome::Done };
        let thread = thread.read(cx);
        let input = thread.input();
        if input.is_working {
            toast_error(t!("Interrupt the current turn before reverting checkpoints."), "", cx);
            return ActionOutcome::Done;
        }
        let Some(&turn_count) = input.revert_turn_counts.get(&message.0) else { return ActionOutcome::Done };
        let ready = thread
            .projection()
            .and_then(|p| Self::revert_checkpoint(p, turn_count))
            .is_some_and(|c| c.status == t3code_contracts::OrchestrationV2CheckpointStatus::Ready);
        // The server refuses file restore outside an isolated worktree.
        let worktree = StateBridge::shell(cx).read(cx).row(key).is_some_and(|row| row.source.worktree_path.is_some());
        ActionOutcome::ConfirmRevert { message: message.clone(), turn_count, restore_files: ready && worktree }
    }

    /// Rolls back to the checkpoint and resolves with the message's text for the composer.
    fn revert_to(&self, key: &ThreadKey, message: &crate::components::chat::timeline_entries::MessageId, turn_count: usize, restore_files: bool, cx: &mut App) -> Task<Option<RevertedDraft>> {
        use crate::components::chat::timeline_entries::EntryKind;
        let (environment_id, thread_id) = runtime_ids(key);
        let (Some(environment), Some(thread), Some(link)) = (self.environment(&environment_id, cx), self.active(key, cx), crate::live::rpc::RpcLink::new(&environment_id, cx)) else {
            return Task::ready(None);
        };
        let thread = thread.read(cx);
        let text = thread.input().entries.iter().find_map(|e| match &e.kind {
            EntryKind::Message(m) if m.id == *message => Some(m.text.clone()),
            _ => None,
        });
        // The sent message's attachments and the records binding its chips to them.
        let (attachments, context) = match thread.input().item_sources.get(&message.0).map(|item| &**item) {
            Some(t3code_contracts::OrchestrationV2TurnItem::UserMessage(item)) => (item.attachments.clone(), item.context.clone()),
            _ => (Vec::new(), None),
        };
        let checkpoint = thread.projection().and_then(|p| Self::revert_checkpoint(p, turn_count));
        let commands = environment.read(cx).commands.clone();
        let runner = link.clone();
        let task = runner.run(async move {
            let restored = crate::state::attachments::fetch_message_attachments(link, attachments, context).await?;
            let rolled = match checkpoint {
                Some(c) => commands.rollback_to_checkpoint(&thread_id, c.scope_id.clone(), c.id.clone(), Some(restore_files)).await,
                None => commands.rollback_to_run(&thread_id, turn_count as u64, restore_files).await,
            };
            rolled.map_err(|error| SharedString::from(error.to_string()))?;
            Ok(restored)
        });
        cx.spawn(async move |cx| match task.await {
            Ok(attachments) => Some(RevertedDraft { prompt: text.unwrap_or_default(), attachments }),
            Err(error) => {
                cx.update(|cx| toast_error(t!("Failed to revert thread state."), error, cx));
                None
            }
        })
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

/// The wire ids of a draft's project.
fn draft_ids(project: &ProjectRef) -> (t3code_contracts::EnvironmentId, t3code_contracts::ProjectId) {
    (t3code_contracts::EnvironmentId::new(project.environment_id.0.as_ref()), t3code_contracts::ProjectId::new(project.project_id.0.as_ref()))
}
