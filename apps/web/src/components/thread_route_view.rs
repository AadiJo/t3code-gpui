//! The chat for a thread route. Ports `ThreadRouteView.tsx`: one chat view
//! and one composer stay mounted across thread switches (the composer keeps a
//! draft per thread key), and switching only swaps the data they show. The
//! composer's events, the timeline's buttons and the source control controls
//! go to the fixture store until the sync layer replaces it.
//!
//! Each thread gets its own source control model and branch strip (under the
//! composer), built when the thread is first shown and kept for the most
//! recent threads. The thread details card (git actions, scripts, open in)
//! is built the first time it opens for that thread.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::*;
use t3code_contracts::{EditorId, ProjectId, ProjectScript};

use crate::components::branch_toolbar::{BranchToolbar, BranchToolbarEvent, BranchToolbarLayout, BranchToolbarProps};
use crate::components::chat::composer::{ChatComposer, ComposerEvent, ComposerState};
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::chat::open_in_picker::OpenInPicker;
use crate::components::chat::thread_details_items::ThreadDetailsItemSettings;
use crate::components::chat::thread_details_panel::{ThreadDetailsInput, ThreadDetailsPanel};
use crate::components::chat::timeline_entries::{Attachment, MessageId};
use crate::components::chat_view::{ChatView, ChatViewEvent, HeaderThreadMenu};
use crate::components::git_actions_control::{GitActionsControl, GitActionsEvent, GitActionsProps};
use crate::components::git_actions_control_fixture::{FixtureGitSource, fixture_status};
use crate::components::git_actions_control_source::{GitActionsSource, SourceControlModel};
use crate::components::project_scripts_control::{ProjectScriptsControl, ProjectScriptsEvent};
use crate::components::sidebar::model::ThreadKey;
use crate::composer_draft_store::{ComposerDraftKey, ComposerDraftStore, InMemoryDraftStore};
use crate::composer_editor_mentions::format_context_reference;
use crate::fixtures::{FixtureEvent, FixtureStore, ThreadView};
use crate::i18n::t;
use crate::remote_open::RemoteOpenState;
use crate::router::ThreadTarget;
use crate::scenes::composer::{FixtureMenuSource, fixture_state};
use crate::scenes::git_actions::{fixture_git_props, fixture_scripts, fixture_toolbar_props};

/// Threads whose source control controls stay built.
const GIT_CONTROLS_KEPT: usize = 16;

/// What the route view asks the app to do.
#[derive(Clone, Debug)]
pub enum ThreadRouteEvent {
    /// A draft got its first message; the route should become the thread's.
    Promoted(ThreadTarget),
    /// Show another thread (a fork was created).
    OpenThread(ThreadKey),
    /// Open the diff tab on a turn (its reply's id) or the branch, revealing `file`.
    OpenDiff { turn: Option<MessageId>, file: Option<SharedString> },
    /// Open the media viewer on these attachments.
    OpenImage { attachments: Arc<[Attachment]>, index: usize },
    /// A file chip or relative link: open it in the Files tab or the viewer.
    OpenLink(SharedString),
    /// Run a project script in a terminal tab.
    RunScript(ProjectScript),
    /// A click outside the details popover closed it.
    DetailsClosed,
}

/// One thread's source control model and the controls drawn on it.
struct GitControls {
    key: SharedString,
    model: Entity<SourceControlModel>,
    source: FixtureGitSource,
    strip: Entity<BranchToolbar>,
    props: BranchToolbarProps,
    project_id: ProjectId,
    /// The details card, built the first time it opens.
    panel: Option<Entity<ThreadDetailsPanel>>,
    open_in: Option<Entity<OpenInPicker>>,
    _subscriptions: Vec<Subscription>,
}

/// The chat view for whichever thread the route names.
pub struct ThreadRouteView {
    store: Entity<FixtureStore>,
    chat: Entity<ChatView>,
    composer: Entity<ChatComposer>,
    target: Option<ThreadTarget>,
    /// The composer settings that do not depend on the thread.
    base: ComposerState,
    /// Most recently shown first; the front is the shown thread's.
    git: VecDeque<GitControls>,
    details_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadRouteEvent> for ThreadRouteView {}

impl ThreadRouteView {
    pub fn new(store: Entity<FixtureStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.default_global::<ThreadDetailsItemSettings>();
        let base = fixture_state("none");
        let drafts: Rc<RefCell<dyn ComposerDraftStore>> = Rc::new(RefCell::new(InMemoryDraftStore::default()));
        let composer = {
            let state = base.clone();
            cx.new(|cx| ChatComposer::new(state, Rc::new(FixtureMenuSource), drafts, cx))
        };
        let chat = cx.new(ChatView::new);
        let focus = composer.focus_handle(cx);
        chat.update(cx, |chat, cx| chat.set_composer(composer.clone().into(), focus, cx));
        let subscriptions = vec![
            cx.subscribe(&composer, Self::on_composer_event),
            cx.subscribe_in(&chat, window, Self::on_chat_event),
            cx.subscribe(&store, Self::on_store_event),
        ];
        Self { store, chat, composer, target: None, base, git: VecDeque::new(), details_open: false, _subscriptions: subscriptions }
    }

    pub fn chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    pub fn composer(&self) -> &Entity<ChatComposer> {
        &self.composer
    }

    pub fn target(&self) -> Option<&ThreadTarget> {
        self.target.as_ref()
    }

    /// Shows `target`. Returns false when the store has no such thread.
    pub fn show(&mut self, target: ThreadTarget, cx: &mut Context<Self>) -> bool {
        if self.target.as_ref() == Some(&target) {
            return true;
        }
        let Some(view) = self.store.update(cx, |store, _| store.thread_view(&target)) else { return false };
        let thread_menu = self.thread_menu(&target, cx);
        self.target = Some(target);
        self.chat.update(cx, |chat, cx| {
            chat.set_thread_menu(thread_menu, cx);
            let hero = view.is_draft && view.timeline.entries.is_empty();
            chat.set_thread(view.key.clone(), view.project.clone(), view.title.clone(), view.timeline.clone(), cx);
            chat.set_hero(hero, cx);
            chat.set_questions(view.questions.clone(), cx);
        });
        self.sync_composer(&view, cx);
        self.mount_git(&view, cx);
        cx.notify();
        true
    }

    /// The header "Thread actions" facts for a server thread's sidebar row.
    fn thread_menu(&self, target: &ThreadTarget, cx: &App) -> Option<HeaderThreadMenu> {
        let ThreadTarget::Server(key) = target else { return None };
        let snapshot = self.store.read(cx).snapshot(None);
        let row = snapshot.threads.iter().find(|row| &row.key == key)?;
        Some(HeaderThreadMenu::from_row(row, snapshot.now, !snapshot.auto_settle_disabled.contains(key)))
    }

    /// Re-reads the shown thread after the store changed it.
    fn refresh(&mut self, anchor: bool, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        let Some(view) = self.store.update(cx, |store, _| store.thread_view(&target)) else { return };
        let timeline = self.chat.read(cx).timeline().clone();
        timeline.update(cx, |timeline, cx| {
            timeline.update_input(view.timeline.clone(), cx);
            if anchor {
                timeline.anchor_latest_user_message(cx);
            }
        });
        let hero = view.is_draft && view.timeline.entries.is_empty();
        let thread_menu = self.thread_menu(&target, cx);
        self.chat.update(cx, |chat, cx| {
            chat.set_hero(hero, cx);
            chat.set_questions(view.questions.clone(), cx);
            chat.set_thread_menu(thread_menu, cx);
        });
        self.sync_composer(&view, cx);
        self.sync_git_props(&view, cx);
    }

    fn sync_composer(&mut self, view: &ThreadView, cx: &mut Context<Self>) {
        let input = &view.timeline;
        let placeholder = if input.pending_question.is_some() {
            Some(t!("Type your own answer, or leave this blank to use the selected option"))
        } else if !input.pending_approval_ids.is_empty() {
            Some(t!("Send a message to decline and redirect the agent"))
        } else {
            None
        };
        let state = ComposerState {
            draft_key: ComposerDraftKey(view.key.clone()),
            is_draft_thread: view.is_draft,
            running: view.running,
            can_interrupt: view.running,
            // The branch toolbar under the composer replaces the static strip.
            context_strip: None,
            placeholder,
            ..self.base.clone()
        };
        self.composer.update(cx, |composer, cx| composer.set_state(state, cx));
    }

    /// The toolbar props for a thread on the fixture machine.
    fn toolbar_props(view: &ThreadView) -> BranchToolbarProps {
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

    /// Mounts the shown thread's controls, building them on first show.
    fn mount_git(&mut self, view: &ThreadView, cx: &mut Context<Self>) {
        let controls = match self.git.iter().position(|g| g.key == view.key) {
            Some(index) => self.git.remove(index).expect("index in range"),
            None => self.build_git(view, cx),
        };
        let strip: AnyView = controls.strip.clone().into();
        self.git.push_front(controls);
        self.git.truncate(GIT_CONTROLS_KEPT);
        let panel = self.details_open.then(|| self.ensure_panel(cx)).flatten();
        self.chat.update(cx, |chat, cx| {
            chat.set_toolbar(Some(strip), cx);
            chat.set_details(panel.map(Into::into), cx);
        });
    }

    fn build_git(&mut self, view: &ThreadView, cx: &mut Context<Self>) -> GitControls {
        let branch = view.branch.clone().unwrap_or_else(|| "main".into());
        let source = FixtureGitSource::new(fixture_status(Some(&branch), true, true, 1, 0));
        let arc: Arc<dyn GitActionsSource> = Arc::new(source.clone());
        let cwd = view.cwd.to_string();
        let model = cx.new(|cx| SourceControlModel::new(arc, cwd, cx));
        let props = Self::toolbar_props(view);
        let strip = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::Composer, model.clone(), props.clone(), cx));
        let key = view.key.clone();
        let subscription = cx.subscribe(&strip, {
            let key = key.clone();
            move |this, _, event: &BranchToolbarEvent, cx| this.on_branch_event(&key, event, cx)
        });
        let project_id = ProjectId::new(view.project_id.as_ref().map(|p| p.0.to_string()).unwrap_or_default());
        GitControls { key, model, source, strip, props, project_id, panel: None, open_in: None, _subscriptions: vec![subscription] }
    }

    /// The shown thread's details card, built on first use: two branch
    /// toolbars, the git rows, the project's scripts and open in.
    fn ensure_panel(&mut self, cx: &mut Context<Self>) -> Option<Entity<ThreadDetailsPanel>> {
        let front = self.git.front()?;
        if let Some(panel) = &front.panel {
            return Some(panel.clone());
        }
        let (key, model, props, source, project_id) =
            (front.key.clone(), front.model.clone(), front.props.clone(), front.source.clone(), front.project_id.clone());
        let arc: Arc<dyn GitActionsSource> = Arc::new(source);
        let workspace = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelWorkspace, model.clone(), props.clone(), cx));
        let branch = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelBranch, model.clone(), props.clone(), cx));
        let git_props = GitActionsProps { project_id: Some(project_id.clone()), ..fixture_git_props() };
        let git = cx.new(|cx| GitActionsControl::new(model.clone(), git_props, cx));
        let scripts = cx.new(|_| ProjectScriptsControl::new(arc.clone(), project_id, fixture_scripts(), true));
        let cwd = Some(props.project_cwd.clone());
        let open_in = cx.new(|_| {
            OpenInPicker::new(arc, vec![EditorId::Vscode, EditorId::Cursor, EditorId::Zed], RemoteOpenState::LocalExec, cwd)
        });
        let input = ThreadDetailsInput { offers_git: true, is_git_repo: true, show_open_in: true, ..Default::default() };
        let panel = cx.new(|cx| {
            ThreadDetailsPanel::new(input, workspace.clone(), branch.clone(), git.clone(), Some(scripts.clone()), Some(open_in.clone()), cx)
        });
        let mut subscriptions = Vec::new();
        for toolbar in [&workspace, &branch] {
            let key = key.clone();
            subscriptions.push(cx.subscribe(toolbar, move |this, _, event: &BranchToolbarEvent, cx| this.on_branch_event(&key, event, cx)));
        }
        subscriptions.push(cx.subscribe(&git, {
            let key = key.clone();
            move |this, _, event: &GitActionsEvent, cx| match event {
                GitActionsEvent::OpenChanges => cx.emit(ThreadRouteEvent::OpenDiff { turn: None, file: None }),
                GitActionsEvent::ThreadBranchChanged(branch) => this.set_branch(&key, branch.clone().into(), cx),
                GitActionsEvent::OpenPublish => {}
            }
        }));
        subscriptions.push(cx.subscribe(&scripts, |_, _, event: &ProjectScriptsEvent, cx| {
            let ProjectScriptsEvent::Run(script) = event;
            cx.emit(ThreadRouteEvent::RunScript(script.clone()));
        }));
        let front = self.git.front_mut()?;
        front.panel = Some(panel.clone());
        front.open_in = Some(open_in);
        front._subscriptions.extend(subscriptions);
        Some(panel)
    }

    fn on_branch_event(&mut self, key: &SharedString, event: &BranchToolbarEvent, cx: &mut Context<Self>) {
        if let BranchToolbarEvent::ThreadBranchChanged { branch, .. } = event {
            self.set_branch(key, branch.clone().into(), cx);
        }
    }

    /// The thread moved to `branch`: the fixture records it and every toolbar follows.
    fn set_branch(&mut self, key: &SharedString, branch: SharedString, cx: &mut Context<Self>) {
        let Some(ThreadTarget::Server(thread)) = self.target.clone() else { return };
        if crate::fixtures::scoped_key(&thread) != *key {
            return;
        }
        self.store.update(cx, |store, cx| store.set_thread_branch(&thread, branch, cx));
    }

    fn sync_git_props(&mut self, view: &ThreadView, cx: &mut Context<Self>) {
        let Some(controls) = self.git.iter_mut().find(|g| g.key == view.key) else { return };
        let props = Self::toolbar_props(view);
        if props == controls.props {
            return;
        }
        controls.props = props.clone();
        controls.strip.update(cx, |strip, cx| strip.set_props(props.clone(), cx));
        if let Some(panel) = &controls.panel {
            let (workspace, branch) = {
                let panel = panel.read(cx);
                (panel.workspace().clone(), panel.branch().clone())
            };
            workspace.update(cx, |t, cx| t.set_props(props.clone(), cx));
            branch.update(cx, |t, cx| t.set_props(props, cx));
        }
    }

    /// Opens or closes the thread details card (the titlebar toggle, mod+shift+b).
    pub fn set_details_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.details_open = open;
        let panel = if open { self.ensure_panel(cx) } else { None };
        self.chat.update(cx, |chat, cx| chat.set_details(panel.map(Into::into), cx));
    }

    pub fn details_open(&self) -> bool {
        self.details_open
    }

    /// The shown thread's details card, once built.
    pub fn details(&self) -> Option<&Entity<ThreadDetailsPanel>> {
        self.git.front().and_then(|g| g.panel.as_ref())
    }

    /// The shown thread's branch strip.
    pub fn branch_strip(&self) -> Option<&Entity<BranchToolbar>> {
        self.git.front().map(|g| &g.strip)
    }

    /// Opens the shown thread's checkout in the preferred editor (`editor.openFavorite`).
    pub fn open_favorite_editor(&mut self, cx: &mut Context<Self>) {
        self.ensure_panel(cx);
        if let Some(open_in) = self.git.front().and_then(|g| g.open_in.clone()) {
            open_in.update(cx, |open_in, cx| open_in.open(None, cx));
        }
    }

    /// The RPC calls the shown thread's fixture git source received, for checks.
    pub fn git_calls(&self) -> Vec<String> {
        self.git.front().map(|g| g.source.calls.lock().map(|c| c.clone()).unwrap_or_default()).unwrap_or_default()
    }

    /// Inserts a reference chip at the composer's caret and focuses it.
    pub fn insert_reference(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.composer.read(cx).editor().clone();
        editor.update(cx, |editor, cx| editor.insert(&format!("{text} "), cx));
        window.focus(&self.composer.focus_handle(cx), cx);
    }

    /// Puts `text` in the composer (edit from here, edit a queued message).
    fn load_composer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.composer.read(cx).editor().clone();
        editor.update(cx, |editor, cx| {
            editor.set_text(text.to_string(), cx);
            editor.set_cursor(text.len(), cx);
        });
        window.focus(&self.composer.focus_handle(cx), cx);
    }

    fn on_composer_event(&mut self, _: Entity<ChatComposer>, event: &ComposerEvent, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        match event {
            ComposerEvent::Submit { prompt, dispatch, .. } => {
                let (prompt, dispatch) = (prompt.clone(), *dispatch);
                let sent_to = self.store.update(cx, |store, cx| store.submit(&target, prompt, dispatch, cx));
                if let (ThreadTarget::Draft(_), Some(key)) = (&target, sent_to) {
                    // The draft is now a server thread: show it under its own key.
                    let promoted = ThreadTarget::Server(key);
                    self.target = None;
                    self.show(promoted.clone(), cx);
                    cx.emit(ThreadRouteEvent::Promoted(promoted));
                }
                self.refresh(true, cx);
            }
            ComposerEvent::Stop => {
                if let ThreadTarget::Server(key) = &target {
                    self.store.update(cx, |store, cx| store.stop(key, cx));
                }
            }
            ComposerEvent::PageScroll { .. }
            | ComposerEvent::ClearSession
            | ComposerEvent::SideChat { .. }
            | ComposerEvent::AttachRequested
            | ComposerEvent::AttachmentsAdded(_)
            | ComposerEvent::AttachmentRemoved(_)
            | ComposerEvent::ModelChanged(_)
            | ComposerEvent::RuntimeModeChanged(_)
            | ComposerEvent::FavoriteToggled { .. }
            | ComposerEvent::VoiceRequested
            | ComposerEvent::ContextStripClicked(_) => {}
        }
    }

    fn on_chat_event(&mut self, _: &Entity<ChatView>, event: &ChatViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ChatViewEvent::AnswerQuestions(answers) => {
                if let Some(ThreadTarget::Server(key)) = self.target.clone() {
                    self.store.update(cx, |store, cx| store.answer_questions(&key, answers, cx));
                }
            }
            ChatViewEvent::InsertReference(text) => self.insert_reference(text, window, cx),
            ChatViewEvent::CloseDetails => {
                self.set_details_open(false, cx);
                cx.emit(ThreadRouteEvent::DetailsClosed);
            }
            ChatViewEvent::Timeline(action) => self.on_timeline_action(action.clone(), window, cx),
            // The header menu runs the sidebar's thread commands; they are wired with the live store.
            ChatViewEvent::ThreadCommand(..) => {}
        }
    }

    /// Runs a timeline button against the store, as the web app's handlers do.
    fn on_timeline_action(&mut self, action: TimelineAction, window: &mut Window, cx: &mut Context<Self>) {
        let key = match (&action, self.target.clone()) {
            (TimelineAction::OpenDiff { turn, file }, _) => {
                return cx.emit(ThreadRouteEvent::OpenDiff { turn: turn.clone(), file: file.clone() });
            }
            (TimelineAction::OpenImage { attachments, index }, _) => {
                return cx.emit(ThreadRouteEvent::OpenImage { attachments: attachments.clone(), index: *index });
            }
            (TimelineAction::OpenLink(target), _) => return cx.emit(ThreadRouteEvent::OpenLink(target.clone())),
            // `runShellCommand`: an ad-hoc "Chat code block" script in a terminal tab.
            (TimelineAction::RunShellCommand(command), _) => {
                return cx.emit(ThreadRouteEvent::RunScript(t3code_contracts::ProjectScript {
                    id: "chat-code-block".into(),
                    name: crate::i18n::t!("Chat code block").to_string(),
                    command: command.to_string(),
                    icon: t3code_contracts::ProjectScriptIcon::Play,
                    run_on_worktree_create: false,
                    run_on_settle: None,
                    stop_on_settle: None,
                    r#async: None,
                    preview_url: None,
                    auto_open_preview: None,
                }));
            }
            (TimelineAction::Reply { message, text }, _) => {
                // `responseContextRecord`: a chip naming the reply by its opening words.
                let opening: String = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().chars().take(48).collect();
                let reference = format_context_reference("response", message.as_str(), opening.trim());
                return self.insert_reference(&reference, window, cx);
            }
            (_, Some(ThreadTarget::Server(key))) => key,
            _ => return,
        };
        match action {
            TimelineAction::RespondToApproval { request, approve } => {
                self.store.update(cx, |store, cx| store.respond_to_approval(&key, &request, approve, cx));
            }
            TimelineAction::Fork(message) => {
                if let Some(fork) = self.store.update(cx, |store, cx| store.fork_from_message(&key, &message, cx)) {
                    cx.emit(ThreadRouteEvent::OpenThread(fork));
                }
            }
            TimelineAction::EditFrom(message) => {
                if let Some(text) = self.store.update(cx, |store, cx| store.revert_to_message(&key, &message, cx)) {
                    self.load_composer(&text, window, cx);
                }
            }
            TimelineAction::SendQueuedNow(id) => self.store.update(cx, |store, cx| store.send_queued_now(&key, &id, cx)),
            TimelineAction::EditQueued(id) => {
                if let Some(text) = self.store.update(cx, |store, cx| store.remove_queued(&key, &id, cx)) {
                    self.load_composer(&text, window, cx);
                }
            }
            TimelineAction::RemoveQueued(id) => {
                self.store.update(cx, |store, cx| store.remove_queued(&key, &id, cx));
            }
            TimelineAction::OpenDiff { .. }
            | TimelineAction::OpenImage { .. }
            | TimelineAction::OpenLink(_)
            | TimelineAction::RunShellCommand(_)
            | TimelineAction::Reply { .. } => {}
        }
    }

    fn on_store_event(&mut self, _: Entity<FixtureStore>, event: &FixtureEvent, cx: &mut Context<Self>) {
        match event {
            FixtureEvent::ThreadChanged(target) if self.target.as_ref() == Some(target) => self.refresh(false, cx),
            FixtureEvent::Delta { thread, message, text } => {
                if self.target.as_ref() == Some(&ThreadTarget::Server(thread.clone())) {
                    let timeline = self.chat.read(cx).timeline().clone();
                    timeline.update(cx, |timeline, cx| timeline.update_message_text(message, text.clone(), true, cx));
                }
            }
            FixtureEvent::DraftPromoted { draft, thread } => {
                if self.target.as_ref() == Some(&ThreadTarget::Draft(draft.clone())) {
                    let target = ThreadTarget::Server(thread.clone());
                    self.target = Some(target.clone());
                    cx.emit(ThreadRouteEvent::Promoted(target));
                }
            }
            FixtureEvent::ThreadChanged(_) | FixtureEvent::SnapshotChanged => {}
        }
    }
}

impl Render for ThreadRouteView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.chat.clone()
    }
}
