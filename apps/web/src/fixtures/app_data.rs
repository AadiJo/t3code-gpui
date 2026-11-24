//! [`AppData`] over the [`FixtureStore`]: the data `app-check`, the scenes and the parity tool
//! run on. Terminals run a local shell, the git controls a scripted source, the Files tab a
//! real directory per project.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, SharedString, Task, WeakEntity};
use t3code_contracts::{EditorId, ProjectId};

use super::palette_source::FixturePaletteSource;
use super::{FixtureEvent, FixtureStore, ThreadView};
use crate::app_root::{local_script_backend, local_shell_backend};
use crate::components::branch_toolbar::BranchToolbarProps;
use crate::components::chat::composer::{ChatComposer, ComposerEvent, ComposerState};
use crate::components::chat::composer_command_menu::ComposerMenuSource;
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::command_palette_lookups::CommandPaletteSource;
use crate::components::diff_panel::DiffPanelInput;
use crate::components::git_actions_control::GitActionsProps;
use crate::components::git_actions_control_fixture::{FixtureGitSource, fixture_status};
use crate::components::right_panel_tabs::{TerminalBackends, Workspace};
use crate::components::sidebar::context_menu::MenuCommand;
use crate::components::sidebar::model::{ProjectRef, SidebarDraft, SidebarSnapshot, ThreadKey};
use crate::composer_draft_store::{ComposerDraftKey, ComposerDraftStore, InMemoryDraftStore};
use crate::diff_panel_store::DiffPanelSelection;
use crate::file_source::{DirectoryFiles, SharedWorkspaceFiles};
use crate::remote_open::RemoteOpenState;
use crate::router::{DraftId, ThreadTarget};
use crate::scenes::composer::FixtureMenuSource;
use crate::scenes::git_actions::{fixture_git_props, fixture_scripts, fixture_toolbar_props};
use crate::state::app_data::{ActionOutcome, AppData, CommandOutcome, DataEvent, DataEvents, GitControlsSetup};
use crate::components::issue::issues_page::EnvironmentIssueSource;
use crate::components::preview::preview_source::PreviewSource;
use crate::components::pull_request::pull_request_fixture::FixtureSource as PrFixture;
use crate::components::pull_request::pull_request_source::{IssueSource, PullRequestDetailSource};
use crate::components::pull_request::pull_requests_page::EnvironmentListSource;
use crate::components::settings::server_settings_data::ServerPanelsModel;
use crate::components::usage::usage_source::{FixtureUsageSource, UsageSource};

/// The fixture store as the window's data.
pub struct FixtureData {
    store: Entity<FixtureStore>,
    events: Entity<DataEvents>,
    palette: Rc<FixturePaletteSource>,
    drafts: Rc<RefCell<dyn ComposerDraftStore>>,
    /// Each thread's scripted git source, for checks that read its calls.
    git: RefCell<HashMap<SharedString, FixtureGitSource>>,
    /// One directory-backed workspace per project.
    workspaces: RefCell<HashMap<SharedString, Workspace>>,
}

impl FixtureData {
    pub fn new(store: FixtureStore, cx: &mut App) -> Rc<Self> {
        let palette = Rc::new(FixturePaletteSource::new(store.snapshot(None)));
        let store = cx.new(|_| store);
        let events = cx.new(|_| DataEvents::default());
        let subscription = {
            let (palette, store) = (palette.clone(), store.clone());
            events.update(cx, |_, cx| {
                cx.subscribe(&store, move |_, store, event: &FixtureEvent, cx| {
                    let event = match event {
                        FixtureEvent::SnapshotChanged => {
                            palette.set_snapshot(store.read(cx).snapshot(None));
                            DataEvent::SnapshotChanged
                        }
                        FixtureEvent::ThreadChanged(target) => DataEvent::ThreadChanged(target.clone()),
                        FixtureEvent::Delta { thread, message, text } => {
                            DataEvent::Text { thread: thread.clone(), changes: Arc::from([(message.clone(), text.clone(), true)]) }
                        }
                        FixtureEvent::DraftPromoted { draft, thread } => {
                            DataEvent::DraftPromoted { draft: draft.clone(), thread: thread.clone() }
                        }
                    };
                    cx.emit(event);
                })
            })
        };
        events.update(cx, |events, _| events._subscriptions.push(subscription));
        Rc::new(Self {
            store,
            events,
            palette,
            drafts: Rc::new(RefCell::new(InMemoryDraftStore::default())),
            git: RefCell::default(),
            workspaces: RefCell::default(),
        })
    }

    /// The store, for checks that script it directly.
    pub fn store(&self) -> &Entity<FixtureStore> {
        &self.store
    }

    /// The RPC calls a thread's scripted git source received (`view.key`), for checks.
    pub fn git_calls(&self, key: &str) -> Vec<String> {
        self.git.borrow().get(key).map(|g| g.calls.lock().map(|c| c.clone()).unwrap_or_default()).unwrap_or_default()
    }
}

impl AppData for FixtureData {
    fn events(&self) -> Entity<DataEvents> {
        self.events.clone()
    }

    fn has_data(&self, cx: &App) -> bool {
        !self.store.read(cx).is_empty()
    }

    fn snapshot(&self, active: Option<&ThreadKey>, cx: &App) -> SidebarSnapshot {
        self.store.read(cx).snapshot(active)
    }

    fn most_recent_project(&self, cx: &App) -> Option<ProjectRef> {
        self.store.read(cx).most_recent_project()
    }

    fn thread_exists(&self, target: &ThreadTarget, cx: &App) -> bool {
        self.store.read(cx).thread_exists(target)
    }

    fn thread_view(&self, target: &ThreadTarget, cx: &mut App) -> Option<ThreadView> {
        self.store.update(cx, |store, _| store.thread_view(target))
    }

    fn create_draft(&self, project: Option<ProjectRef>, cx: &mut App) -> DraftId {
        self.store.update(cx, |store, _| store.create_draft(project))
    }

    fn drafts(&self, cx: &App) -> Vec<SidebarDraft> {
        self.store.read(cx).sidebar_drafts().to_vec()
    }

    fn discard_draft_session(&self, id: &DraftId, cx: &mut App) {
        self.store.update(cx, |store, _| store.discard_draft(id));
    }

    fn composer_state(&self, view: &ThreadView, base: &ComposerState, _: &mut App) -> ComposerState {
        ComposerState {
            draft_key: ComposerDraftKey(view.key.clone()),
            is_draft_thread: view.is_draft,
            running: view.running,
            can_interrupt: view.running,
            ..base.clone()
        }
    }

    fn menu_source(&self, _: &ThreadView, _: &Entity<ChatComposer>, _: &mut App) -> Rc<dyn ComposerMenuSource> {
        Rc::new(FixtureMenuSource)
    }

    fn draft_store(&self, _: &mut App) -> Rc<RefCell<dyn ComposerDraftStore>> {
        Rc::clone(&self.drafts)
    }

    fn composer_event(&self, target: &ThreadTarget, event: &ComposerEvent, _: &WeakEntity<ChatComposer>, cx: &mut App) -> Option<ThreadKey> {
        match event {
            ComposerEvent::Submit { prompt, dispatch, .. } => {
                let sent_to = self.store.update(cx, |store, cx| store.submit(target, prompt.clone(), *dispatch, cx));
                matches!(target, ThreadTarget::Draft(_)).then_some(sent_to).flatten()
            }
            ComposerEvent::Stop => {
                if let ThreadTarget::Server(key) = target {
                    self.store.update(cx, |store, cx| store.stop(key, cx));
                }
                None
            }
            _ => None,
        }
    }

    fn timeline_action(&self, key: &ThreadKey, action: &TimelineAction, cx: &mut App) -> ActionOutcome {
        self.store.update(cx, |store, cx| match action {
            TimelineAction::RespondToApproval { request, approve } => {
                store.respond_to_approval(key, request, *approve, cx);
                ActionOutcome::Done
            }
            TimelineAction::Fork(message) => {
                store.fork_from_message(key, message, cx).map_or(ActionOutcome::Done, ActionOutcome::OpenThread)
            }
            TimelineAction::EditFrom(message) => {
                store.revert_to_message(key, message, cx).map_or(ActionOutcome::Done, ActionOutcome::LoadComposer)
            }
            TimelineAction::SendQueuedNow(id) => {
                store.send_queued_now(key, id, cx);
                ActionOutcome::Done
            }
            TimelineAction::EditQueued(id) => {
                store.remove_queued(key, id, cx).map_or(ActionOutcome::Done, |text| ActionOutcome::LoadComposer(text.into()))
            }
            TimelineAction::RemoveQueued(id) => {
                store.remove_queued(key, id, cx);
                ActionOutcome::Done
            }
            _ => ActionOutcome::Done,
        })
    }

    fn answer_questions(&self, key: &ThreadKey, answers: &[usize], cx: &mut App) {
        self.store.update(cx, |store, cx| store.answer_questions(key, answers, cx));
    }

    fn set_thread_branch(&self, key: &ThreadKey, branch: SharedString, cx: &mut App) {
        self.store.update(cx, |store, cx| store.set_thread_branch(key, branch, cx));
    }

    fn thread_command(&self, _: &[ThreadKey], _: MenuCommand, _: &mut App) -> CommandOutcome {
        CommandOutcome::Done
    }

    fn rename_thread(&self, _: &ThreadKey, _: String, _: &mut App) {}

    fn diff_input(&self, target: &ThreadTarget, selection: &DiffPanelSelection, cx: &mut App) -> Task<Option<DiffPanelInput>> {
        Task::ready(self.store.update(cx, |store, _| store.diff_input(target, selection)))
    }

    fn workspace(&self, target: &ThreadTarget, cx: &mut App) -> Option<Workspace> {
        let fixture = self.store.read(cx).workspace(target)?;
        let mut workspaces = self.workspaces.borrow_mut();
        let workspace = workspaces.entry(fixture.project_key.clone()).or_insert_with(|| Workspace {
            key: fixture.project_key.clone(),
            name: fixture.name.clone(),
            files: DirectoryFiles::new(PathBuf::from(&fixture.root)) as SharedWorkspaceFiles,
        });
        Some(workspace.clone())
    }

    fn git_controls(&self, view: &ThreadView, _: &mut App) -> GitControlsSetup {
        let branch = view.branch.clone().unwrap_or_else(|| "main".into());
        let source = FixtureGitSource::new(fixture_status(Some(&branch), true, true, 1, 0));
        self.git.borrow_mut().insert(view.key.clone(), source.clone());
        let project_id = ProjectId::new(view.project_id.as_ref().map(|p| p.0.to_string()).unwrap_or_default());
        GitControlsSetup {
            source: Arc::new(source),
            git: GitActionsProps { project_id: Some(project_id), ..fixture_git_props() },
            scripts: fixture_scripts(),
            editors: vec![EditorId::Vscode, EditorId::Cursor, EditorId::Zed],
            open_state: RemoteOpenState::LocalExec,
            is_git_repo: true,
        }
    }

    fn toolbar_props(&self, view: &ThreadView, _: &mut App) -> BranchToolbarProps {
        let branch = view.branch.as_deref().unwrap_or("main");
        BranchToolbarProps {
            project_cwd: view.cwd.to_string(),
            is_server_thread: !view.is_draft,
            env_locked: !view.is_draft,
            turn_running: view.running,
            thread_branch: Some(branch.to_owned()),
            ..fixture_toolbar_props(Some(branch))
        }
    }

    fn terminal_backends(&self, _: &mut App) -> TerminalBackends {
        TerminalBackends { shell: local_shell_backend(), script: local_script_backend() }
    }

    fn palette_source(&self) -> Rc<dyn CommandPaletteSource> {
        self.palette.clone()
    }

    fn set_active_thread(&self, active: Option<&ThreadKey>, _: &mut App) {
        self.palette.set_active_thread(active.cloned());
    }

    fn pull_request_sources(&self, _: &mut App) -> Vec<EnvironmentListSource> {
        crate::scenes::pull_requests::fixture_sources(&PrFixture::standard())
    }

    fn pull_request_detail(&self, _: &t3code_contracts::EnvironmentId, _: &mut App) -> Option<Rc<dyn PullRequestDetailSource>> {
        Some(Rc::new(PrFixture::standard()))
    }

    fn issue_sources(&self, _: &mut App) -> Vec<EnvironmentIssueSource> {
        let source: Rc<dyn IssueSource> = Rc::new(PrFixture::standard());
        vec![EnvironmentIssueSource { environment_id: t3code_contracts::EnvironmentId::new("env-studio"), label: "Studio".into(), source }]
    }

    fn usage_source(&self, _: &mut App) -> Rc<dyn UsageSource> {
        FixtureUsageSource::new()
    }

    fn server_settings(&self, _: &mut App) -> Option<Entity<ServerPanelsModel>> {
        None
    }

    fn preview_source(&self, _: &t3code_contracts::EnvironmentId, _: &mut App) -> Option<Rc<dyn PreviewSource>> {
        None
    }
}
