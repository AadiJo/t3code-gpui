//! The chat for a thread route. Ports `ThreadRouteView.tsx`: one chat view
//! and one composer stay mounted across thread switches (the composer keeps a
//! draft per thread key), and switching only swaps the data they show. The
//! composer's events, the timeline's buttons and the source control controls
//! go to the window's [`AppData`].
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
use t3code_contracts::{ProjectId, ProjectScript};

use crate::components::branch_toolbar::{BranchToolbar, BranchToolbarEvent, BranchToolbarLayout, BranchToolbarProps};
use crate::components::chat::composer::{ChatComposer, ComposerEvent, ComposerState};
use crate::components::chat::messages_timeline::TimelineAction;
use crate::components::chat::open_in_picker::OpenInPicker;
use crate::components::chat::thread_details_items::ThreadDetailsItemSettings;
use crate::components::chat::thread_details_panel::{ThreadDetailsEvent, ThreadDetailsInput, ThreadDetailsPanel};
use crate::components::chat::thread_details_sections::ThreadDetailsSectionEvent;
use crate::components::chat::timeline_entries::{Attachment, MessageId};
use crate::components::chat_view::{ChatView, ChatViewEvent, HeaderThreadMenu};
use crate::components::git_actions_control::{GitActionsControl, GitActionsEvent};
use crate::components::chat::composer_command_menu::{ComposerCommandItem, ComposerMenuSource};
use crate::components::git_actions_control_source::SourceControlModel;
use crate::components::project_scripts_control::{ProjectScriptsControl, ProjectScriptsEvent};
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::composer_editor_mentions::format_context_reference;
use crate::composer_logic::ComposerTrigger;
use crate::i18n::t;
use crate::router::ThreadTarget;
use crate::scenes::composer::fixture_state;
use crate::state::app_data::{ActionOutcome, DataEvent, DetailsOutcome, GitControlsSetup, SharedAppData, ThreadView};

/// The card's facts that do not come from the server.
fn details_base(is_git_repo: bool) -> ThreadDetailsInput {
    ThreadDetailsInput { offers_git: true, is_git_repo, show_open_in: true, ..Default::default() }
}

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
    /// Open an app route (a settings page the controls link to).
    Navigate(&'static str),
    /// Open the Linked items tab (the strip's badge with several pull requests).
    OpenLinkedItems,
    /// Show this browser tab (a thread server opened in the preview).
    OpenBrowser(SharedString),
}

/// The menu of the shown thread's workspace; the composer keeps one source.
#[derive(Clone)]
struct ShownMenu(Rc<RefCell<Option<Rc<dyn ComposerMenuSource>>>>);

impl ComposerMenuSource for ShownMenu {
    fn items(&self, trigger: &ComposerTrigger) -> (Vec<ComposerCommandItem>, bool) {
        let source = self.0.borrow().clone();
        source.map(|source| source.items(trigger)).unwrap_or_default()
    }
}

/// One thread's source control model and the controls drawn on it.
struct GitControls {
    key: SharedString,
    model: Entity<SourceControlModel>,
    setup: GitControlsSetup,
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
    data: SharedAppData,
    menu: ShownMenu,
    chat: Entity<ChatView>,
    composer: Entity<ChatComposer>,
    target: Option<ThreadTarget>,
    /// The composer settings that do not depend on the thread.
    base: ComposerState,
    /// Most recently shown first; the front is the shown thread's.
    git: VecDeque<GitControls>,
    details_open: bool,
    /// The open "Edit from here?" confirmation.
    pending_revert: Option<PendingRevert>,
    _subscriptions: Vec<Subscription>,
}

/// An "Edit from here" waiting for its confirmation (`RevertConfirmHost`).
#[derive(Clone)]
struct PendingRevert {
    key: ThreadKey,
    message: MessageId,
    turn_count: usize,
    restore_files: bool,
}

impl EventEmitter<ThreadRouteEvent> for ThreadRouteView {}

impl ThreadRouteView {
    pub fn new(data: SharedAppData, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.default_global::<ThreadDetailsItemSettings>();
        let base = fixture_state("none");
        let drafts = data.draft_store(cx);
        let menu = ShownMenu(Rc::default());
        let composer = {
            let (state, menu) = (base.clone(), menu.clone());
            cx.new(|cx| ChatComposer::new(state, Rc::new(menu), drafts, cx))
        };
        let chat = cx.new(ChatView::new);
        let focus = composer.focus_handle(cx);
        chat.update(cx, |chat, cx| chat.set_composer(composer.clone().into(), focus, cx));
        let subscriptions = vec![
            cx.subscribe(&composer, Self::on_composer_event),
            cx.subscribe_in(&chat, window, Self::on_chat_event),
            cx.subscribe(&data.events(), Self::on_data_event),
        ];
        Self { data, menu, chat, composer, target: None, base, git: VecDeque::new(), details_open: false, pending_revert: None, _subscriptions: subscriptions }
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
        let Some(view) = self.data.thread_view(&target, cx) else { return false };
        let thread_menu = self.thread_menu(&target, cx);
        self.target = Some(target);
        let source = self.data.menu_source(&view, &self.composer, cx);
        *self.menu.0.borrow_mut() = Some(source);
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
        let snapshot = self.data.snapshot(None, cx);
        let row = snapshot.threads.iter().find(|row| &row.key == key)?;
        Some(HeaderThreadMenu::from_row(row, snapshot.now, !snapshot.auto_settle_disabled.contains(key)))
    }

    /// Re-reads the shown thread after the store changed it.
    fn refresh(&mut self, anchor: bool, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        let Some(view) = self.data.thread_view(&target, cx) else { return };
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
        self.sync_details(&view, cx);
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
        let state = self.data.composer_state(view, &self.base, cx);
        // The branch toolbar under the composer replaces the static strip.
        let state = ComposerState { context_strip: None, placeholder, ..state };
        self.composer.update(cx, |composer, cx| composer.set_state(state, cx));
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
        let setup = self.data.git_controls(view, cx);
        let cwd = view.cwd.to_string();
        let source = setup.source.clone();
        let model = cx.new(|cx| SourceControlModel::new(source, cwd, cx));
        let props = self.data.toolbar_props(view, cx);
        let strip = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::Composer, model.clone(), props.clone(), cx));
        let key = view.key.clone();
        let subscription = cx.subscribe(&strip, {
            let key = key.clone();
            move |this, _, event: &BranchToolbarEvent, cx| this.on_branch_event(&key, event, cx)
        });
        let project_id = ProjectId::new(view.project_id.as_ref().map(|p| p.0.to_string()).unwrap_or_default());
        GitControls { key, model, setup, strip, props, project_id, panel: None, open_in: None, _subscriptions: vec![subscription] }
    }

    /// The shown thread's details card, built on first use: two branch
    /// toolbars, the git rows, the project's scripts and open in.
    fn ensure_panel(&mut self, cx: &mut Context<Self>) -> Option<Entity<ThreadDetailsPanel>> {
        let front = self.git.front()?;
        if let Some(panel) = &front.panel {
            return Some(panel.clone());
        }
        let (key, model, props, project_id) = (front.key.clone(), front.model.clone(), front.props.clone(), front.project_id.clone());
        let setup = &front.setup;
        let (arc, git_props, scripts, editors, open_state, is_git_repo) = (
            setup.source.clone(),
            setup.git.clone(),
            setup.scripts.clone(),
            setup.editors.clone(),
            setup.open_state.clone(),
            setup.is_git_repo,
        );
        let workspace = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelWorkspace, model.clone(), props.clone(), cx));
        let branch = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelBranch, model.clone(), props.clone(), cx));
        let git = cx.new(|cx| GitActionsControl::new(model.clone(), git_props, cx));
        let scripts = cx.new(|_| ProjectScriptsControl::new(arc.clone(), project_id, scripts, true));
        let cwd = Some(props.project_cwd.clone());
        let open_in = cx.new(|_| OpenInPicker::new(arc, editors, open_state, cwd));
        let panel = cx.new(|cx| {
            ThreadDetailsPanel::new(details_base(is_git_repo), workspace.clone(), branch.clone(), git.clone(), Some(scripts.clone()), Some(open_in.clone()), cx)
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
                GitActionsEvent::OpenUrl(url) => cx.open_url(url),
                GitActionsEvent::OpenSourceControlSettings => cx.emit(ThreadRouteEvent::Navigate("/settings/source-control")),
            }
        }));
        subscriptions.push(cx.subscribe(&panel, Self::on_details_event));
        subscriptions.push(cx.subscribe(&scripts, |_, _, event: &ProjectScriptsEvent, cx| {
            let ProjectScriptsEvent::Run(script) = event;
            cx.emit(ThreadRouteEvent::RunScript(script.clone()));
        }));
        let front = self.git.front_mut()?;
        front.panel = Some(panel.clone());
        front.open_in = Some(open_in);
        front._subscriptions.extend(subscriptions);
        if let Some(view) = self.target.clone().and_then(|target| self.data.thread_view(&target, cx)) {
            self.sync_details(&view, cx);
        }
        Some(panel)
    }

    /// Re-reads the shown thread's details card from the data, once built.
    fn sync_details(&mut self, view: &ThreadView, cx: &mut Context<Self>) {
        let Some(front) = self.git.front().filter(|g| g.key == view.key) else { return };
        let Some(panel) = front.panel.clone() else { return };
        let input = self.data.thread_details(view, details_base(front.setup.is_git_repo), cx);
        if panel.read(cx).input() != &input {
            panel.update(cx, |panel, cx| panel.set_input(input, cx));
        }
    }

    /// A details card row: links and settings pages open here, the rest runs on the data.
    fn on_details_event(&mut self, _: Entity<ThreadDetailsPanel>, event: &ThreadDetailsEvent, cx: &mut Context<Self>) {
        match event {
            ThreadDetailsEvent::OpenUrl(url) => return cx.open_url(url),
            ThreadDetailsEvent::Section(ThreadDetailsSectionEvent::ManageAutomations(_)) => {
                return cx.emit(ThreadRouteEvent::Navigate("/settings/scheduled-tasks"));
            }
            _ => {}
        }
        let Some(ThreadTarget::Server(key)) = self.target.clone() else { return };
        let task = self.data.thread_details_event(&key, event, cx);
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            this.update(cx, |this, cx| match outcome {
                DetailsOutcome::Done => {}
                DetailsOutcome::OpenBrowser(tab) => cx.emit(ThreadRouteEvent::OpenBrowser(tab)),
                DetailsOutcome::LoadComposer(text) => {
                    this.append_handoff(&text, cx);
                    Toaster::push(
                        ToastData::new(ToastKind::Success, t!("Added to the composer"))
                            .description(t!("The task is in the composer. Read it over, then send.")),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_branch_event(&mut self, key: &SharedString, event: &BranchToolbarEvent, cx: &mut Context<Self>) {
        match event {
            BranchToolbarEvent::ThreadBranchChanged { branch, .. } => self.set_branch(key, branch.clone().into(), cx),
            BranchToolbarEvent::OpenPullRequest(url) => cx.open_url(url),
            BranchToolbarEvent::OpenPullRequestList => cx.emit(ThreadRouteEvent::OpenLinkedItems),
            _ => {}
        }
    }

    /// The thread moved to `branch`: the data records it and every toolbar follows.
    fn set_branch(&mut self, key: &SharedString, branch: SharedString, cx: &mut Context<Self>) {
        let Some(ThreadTarget::Server(thread)) = self.target.clone() else { return };
        if crate::fixtures::scoped_key(&thread) != *key {
            return;
        }
        self.data.set_thread_branch(&thread, branch, cx);
    }

    fn sync_git_props(&mut self, view: &ThreadView, cx: &mut Context<Self>) {
        if !self.git.iter().any(|g| g.key == view.key) {
            return;
        }
        let props = self.data.toolbar_props(view, cx);
        let Some(controls) = self.git.iter_mut().find(|g| g.key == view.key) else { return };
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

    /// The shown thread's key (`ThreadView::key`), once shown.
    pub fn shown_key(&self) -> Option<SharedString> {
        self.git.front().map(|g| g.key.clone())
    }

    /// Inserts a reference chip at the composer's caret and focuses it.
    pub fn insert_reference(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.composer.read(cx).editor().clone();
        editor.update(cx, |editor, cx| editor.insert(&format!("{text} "), cx));
        window.focus(&self.composer.focus_handle(cx), cx);
    }

    /// Puts `text` in the composer (edit from here, edit a queued message).
    /// `appendHandoff`: adds a task below the composer's text unless it is already there.
    fn append_handoff(&mut self, task: &str, cx: &mut Context<Self>) {
        let editor = self.composer.read(cx).editor().clone();
        editor.update(cx, |editor, cx| {
            let kept = editor.text().trim_end().to_owned();
            let added = task.trim();
            if added.is_empty() || format!("\n\n{kept}\n\n").contains(&format!("\n\n{added}\n\n")) {
                return;
            }
            let text = if kept.is_empty() { added.to_owned() } else { format!("{kept}\n\n{added}") };
            let end = text.len();
            editor.set_text(text, cx);
            editor.set_cursor(end, cx);
        });
    }

    fn load_composer(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.composer.read(cx).editor().clone();
        editor.update(cx, |editor, cx| {
            editor.set_text(text.to_string(), cx);
            editor.set_cursor(text.len(), cx);
        });
        window.focus(&self.composer.focus_handle(cx), cx);
    }

    fn on_composer_event(&mut self, composer: Entity<ChatComposer>, event: &ComposerEvent, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else { return };
        let promoted = self.data.composer_event(&target, event, &composer.downgrade(), cx);
        if let (ThreadTarget::Draft(_), Some(key)) = (&target, promoted) {
            // The draft is now a server thread: show it under its own key.
            let promoted = ThreadTarget::Server(key);
            self.target = None;
            self.show(promoted.clone(), cx);
            cx.emit(ThreadRouteEvent::Promoted(promoted));
        }
        match event {
            ComposerEvent::Submit { .. } => self.refresh(true, cx),
            // These change what the composer shows from the draft (model, mode, the `/clear` banner).
            ComposerEvent::ModelChanged(_) | ComposerEvent::RuntimeModeChanged(_) | ComposerEvent::ClearSession | ComposerEvent::BannerAction(_) => {
                self.refresh(false, cx)
            }
            _ => {}
        }
    }

    fn on_chat_event(&mut self, _: &Entity<ChatView>, event: &ChatViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ChatViewEvent::AnswerQuestions(answers) => {
                if let Some(ThreadTarget::Server(key)) = self.target.clone() {
                    self.data.answer_questions(&key, answers, cx);
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
            (TimelineAction::Reply { message, text }, _) => {
                // `responseContextRecord`: a chip naming the reply by its opening words.
                let opening: String = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().chars().take(48).collect();
                let reference = format_context_reference("response", message.as_str(), opening.trim());
                return self.insert_reference(&reference, window, cx);
            }
            (_, Some(ThreadTarget::Server(key))) => key,
            _ => return,
        };
        match self.data.timeline_action(&key, &action, cx) {
            ActionOutcome::Done => {}
            ActionOutcome::OpenThread(thread) => cx.emit(ThreadRouteEvent::OpenThread(thread)),
            ActionOutcome::LoadComposer(text) => self.load_composer(&text, window, cx),
            ActionOutcome::ConfirmRevert { message, turn_count, restore_files } => {
                self.pending_revert = Some(PendingRevert { key, message, turn_count, restore_files });
                cx.notify();
            }
        }
    }

    /// Closes the confirmation; with `restore_files` set, rewinds (`onConfirmRevert`) and puts
    /// the message back in the composer.
    pub fn resolve_revert(&mut self, choice: Option<bool>, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_revert.take() else { return };
        cx.notify();
        let Some(restore_files) = choice else { return };
        if restore_files && !pending.restore_files {
            return;
        }
        let task = self.data.revert(&pending.key, &pending.message, pending.turn_count, restore_files, cx);
        cx.spawn(async move |this, cx| {
            let Some(prompt) = task.await else { return };
            _ = this.update(cx, |this, cx| {
                if this.target.as_ref() != Some(&ThreadTarget::Server(pending.key.clone())) {
                    return;
                }
                let editor = this.composer.read(cx).editor().clone();
                editor.update(cx, |editor, cx| {
                    let current = editor.text().to_owned();
                    let next = match (current.is_empty(), prompt.is_empty()) {
                        (_, true) => current,
                        (true, false) => prompt.to_string(),
                        (false, false) => format!("{current}\n\n{prompt}"),
                    };
                    let end = next.len();
                    editor.set_text(next, cx);
                    editor.set_cursor(end, cx);
                });
            });
        })
        .detach();
    }

    /// "Edit from here?" with Cancel, "Revert files too" (only where files can be restored)
    /// and "Revert and keep changes".
    fn render_revert_confirm(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
        use crate::components::ui::dialog::{DialogSurface, Modal};
        let pending = self.pending_revert.as_ref()?;
        let dialog = DialogSurface::new(t!("Edit from here?"))
            .description(t!("Rewind chat to before this message. Your prompt and attachments return to the composer. “Revert files too” also undoes your own edits."))
            .footer(Button::new("revert-cancel").variant(ButtonVariant::Outline).size(ButtonSize::Default).label(t!("Cancel")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.resolve_revert(None, cx))))
            .footer(
                Button::new("revert-files")
                    .variant(ButtonVariant::DestructiveOutline)
                    .size(ButtonSize::Default)
                    .label(t!("Revert files too"))
                    .disabled(!pending.restore_files)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.resolve_revert(Some(true), cx))),
            )
            .footer(Button::new("revert-keep").variant(ButtonVariant::Default).size(ButtonSize::Default).label(t!("Revert and keep changes")).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.resolve_revert(Some(false), cx))));
        let this = cx.weak_entity();
        Some(Modal::dialog("revert-confirm-dialog", dialog, move |_, cx| {
            this.update(cx, |this, cx| this.resolve_revert(None, cx)).ok();
        }))
    }

    fn on_data_event(&mut self, _: Entity<crate::state::app_data::DataEvents>, event: &DataEvent, cx: &mut Context<Self>) {
        match event {
            DataEvent::ThreadChanged(target) if self.target.as_ref() == Some(target) => self.refresh(false, cx),
            DataEvent::Text { thread, changes } => {
                if self.target.as_ref() == Some(&ThreadTarget::Server(thread.clone())) {
                    let timeline = self.chat.read(cx).timeline().clone();
                    timeline.update(cx, |timeline, cx| {
                        for (message, text, streaming) in changes.iter() {
                            timeline.update_message_text(message, text.clone(), *streaming, cx);
                        }
                    });
                }
            }
            DataEvent::DraftPromoted { draft, thread } => {
                if self.target.as_ref() == Some(&ThreadTarget::Draft(draft.clone())) {
                    let target = ThreadTarget::Server(thread.clone());
                    self.target = Some(target.clone());
                    cx.emit(ThreadRouteEvent::Promoted(target));
                }
            }
            DataEvent::ThreadChanged(_) | DataEvent::SnapshotChanged | DataEvent::OpenThread(_) => {}
        }
    }
}

impl Render for ThreadRouteView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.chat.clone()).children(self.render_revert_confirm(cx))
    }
}
