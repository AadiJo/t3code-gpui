//! The command palette. Ports `CommandPalette.tsx`, `CommandPaletteContent.tsx`
//! and `ui/command.tsx`: a `dialog-glass` popup over the window with a search
//! input, grouped results in a virtual list, and a footer of key hints.
//!
//! Rows run in three ways: a command row dispatches the command's action
//! from the focus the palette opened from; a navigation row emits a
//! [`PaletteEvent`] for the app; a few rows (theme, appearance) act directly.
//! Command rows whose action has no handler there show as unavailable.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyBinding, ListAlignment, ListOffset, ListState, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, actions, anchored, deferred, div, list, point, prelude::FluentBuilder as _, px,
};
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand, THREAD_JUMP_COMMANDS};

use super::command_palette_add_project::add_project_item;
use super::command_palette_logic::{
    FilterInput, PaletteEvent, PaletteGroup, PaletteItem, PaletteItemKind, PaletteMode, PaletteRow, PaletteRows,
    PaletteRun, PaletteSearchState, PaletteTitle, PaletteView, ShownGroup, build_nested_search_items, build_root_groups,
    build_rows, empty_state_message, filter_command_palette_groups, find_item, input_placeholder,
};
use super::command_palette_lookups::{CommandPaletteSource, PaletteFeature};
use super::command_palette_port_forward_items::port_forwards_item;
use super::command_palette_project_items::{new_thread_in_items, project_search_items};
use super::command_palette_results::{RowCallbacks, render_empty, render_item, render_label, row_height};
use super::command_palette_theme_items::{change_appearance_item, change_theme_item, theme_editor_item};
use super::command_palette_thread_items::{build_thread_items, recent_thread_items};
use super::icons::{Icon, IconName, IconSize};
use super::ui::TextStyleExt as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::command_palette_store::{CommandPaletteUiAction, CommandPaletteUiState, OpenIntent, SearchOverlayMode};
use crate::i18n::t;
use crate::shortcuts::commands::{self, command_action};
use crate::shortcuts::keymap::ShortcutState;
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

actions!(
    command_palette,
    [
        /// Highlights the previous row.
        SelectPrevious,
        /// Highlights the next row.
        SelectNext,
        /// Runs the highlighted row.
        Confirm,
        /// Closes the palette.
        Cancel,
    ]
);

/// Key context of the palette popup.
pub const KEY_CONTEXT: &str = "CommandPalette";

/// Binds the palette's own keys. They also apply inside its input, where
/// they win over the input's bindings for the same keys.
pub fn init(cx: &mut App) {
    for context in [KEY_CONTEXT, "CommandPalette > Input"] {
        cx.bind_keys([
            KeyBinding::new("up", SelectPrevious, Some(context)),
            KeyBinding::new("down", SelectNext, Some(context)),
            KeyBinding::new("enter", Confirm, Some(context)),
            KeyBinding::new("escape", Cancel, Some(context)),
        ]);
    }
}

/// One open palette: its rows, query, page stack and highlight.
struct Session {
    input: Entity<InputState>,
    /// Where focus was when the palette opened; commands run from there.
    return_focus: Option<FocusHandle>,
    root_groups: Vec<PaletteGroup>,
    nested_items: Vec<Rc<PaletteItem>>,
    thread_items: Vec<Rc<PaletteItem>>,
    project_items: Vec<Rc<PaletteItem>>,
    settings_items: Vec<Rc<PaletteItem>>,
    search: PaletteSearchState,
    shown: Vec<ShownGroup>,
    rows: Rc<PaletteRows>,
    /// The highlighted row's value.
    highlighted: Option<SharedString>,
    /// Typing or a new page drops the highlight until the arrows move it
    /// again; Enter still runs the first row (`highlightClearedRef`).
    highlight_cleared: bool,
    list: ListState,
    now_ms: i64,
    _input_events: Subscription,
}

/// The command palette view. Render it once at the window root, above the
/// app; it draws nothing while closed.
pub struct CommandPalette {
    source: Rc<dyn CommandPaletteSource>,
    ui: CommandPaletteUiState,
    session: Option<Session>,
    focus_handle: FocusHandle,
    this: WeakEntity<Self>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CommandPalette {
    pub fn new(source: Rc<dyn CommandPaletteSource>, cx: &mut Context<Self>) -> Self {
        Self { source, ui: CommandPaletteUiState::default(), session: None, focus_handle: cx.focus_handle(), this: cx.weak_entity() }
    }

    /// Replaces the data source; an open palette keeps its rows until reopened.
    pub fn set_source(&mut self, source: Rc<dyn CommandPaletteSource>) {
        self.source = source;
    }

    pub fn is_open(&self) -> bool {
        self.ui.open
    }

    /// The current query, for checks.
    pub fn query(&self) -> Option<SharedString> {
        self.session.as_ref().map(|s| s.search.query.clone())
    }

    /// The highlighted row, or the first one Enter would run.
    pub fn target_value(&self) -> Option<SharedString> {
        let session = self.session.as_ref()?;
        session.highlighted.clone().or_else(|| session.rows.item_values.first().cloned())
    }

    /// How many rows the list shows, labels included.
    pub fn row_count(&self) -> usize {
        self.session.as_ref().map_or(0, |s| s.rows.rows.len())
    }

    /// The visible rows' titles, for checks.
    pub fn shown_titles(&self) -> Vec<SharedString> {
        self.session
            .iter()
            .flat_map(|s| s.shown.iter())
            .flat_map(|g| g.items.iter())
            .map(|shown| shown.item.search_name())
            .collect()
    }

    /// The `commandPalette.toggle` shortcut: opens on the root page, or closes.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(CommandPaletteUiAction::ToggleMode(SearchOverlayMode::Command), window, cx);
    }

    /// Opens the palette to a page or query (`openCommandPalette`).
    pub fn open(&mut self, intent: OpenIntent, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(CommandPaletteUiAction::Open(intent), window, cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dispatch(CommandPaletteUiAction::SetOpen(false), window, cx);
    }

    fn dispatch(&mut self, action: CommandPaletteUiAction, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.ui.reduce(action);
        let (was_open, session) = (self.ui.open, self.ui.session);
        self.ui = next;
        if self.ui.open && self.ui.mode == SearchOverlayMode::Command {
            if !was_open || self.ui.session != session || self.session.is_none() {
                self.start_session(window, cx);
            }
        } else if let Some(session) = self.session.take() {
            // The destination owns focus after a run; otherwise go back to the opener.
            if let Some(focus) = session.return_focus.filter(|f| f.is_rendered(window)) {
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    fn start_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let return_focus = self
            .session
            .take()
            .and_then(|s| s.return_focus)
            .or_else(|| window.focused(cx).filter(|f| !self.focus_handle.contains(f, window)));
        let source = self.source.clone();
        let now_ms = source.now_ms();
        let threads = source.threads();
        let projects = source.projects();
        let active = source.active_thread();
        let thread_items = build_thread_items(&threads, &projects, active.as_ref(), now_ms);
        let recent = recent_thread_items(&thread_items, active.as_ref());
        let mut actions = self.root_actions(window, cx);
        // Availability is judged from the opener; before any frame there is nothing to judge.
        if let Some(focus) = return_focus.as_ref().filter(|f| f.is_rendered(window)) {
            for item in &mut actions {
                if let PaletteItemKind::Action { run: PaletteRun::Command(command), .. } = &item.kind {
                    item.unavailable = !commands::is_command_available(command, focus, window);
                }
            }
        }
        let actions: Vec<Rc<PaletteItem>> = actions.into_iter().map(Rc::new).collect();
        let nested_items = build_nested_search_items(&actions, "");
        let root_groups = build_root_groups(actions, recent);
        let settings_items = source
            .settings()
            .iter()
            .map(|setting| {
                let mut item = PaletteItem::action(
                    format!("setting:{}", setting.id),
                    PaletteTitle::Text(setting.title.clone()),
                    IconName::Settings,
                    PaletteRun::Event(PaletteEvent::OpenSetting(setting.id.clone())),
                    &[],
                );
                item.set_terms(std::iter::once(setting.title.clone()).chain(std::iter::once(setting.section.clone())).chain(setting.terms.iter().cloned()));
                item.description = Some(t!("Settings · {section}", section = setting.section.clone()));
                item.secondary = setting.secondary;
                Rc::new(item)
            })
            .collect();
        let intent = self.ui.open_intent.clone();
        let initial_query = match &intent {
            Some(OpenIntent::Search(query)) => query.clone(),
            _ => SharedString::default(),
        };
        let mut search = PaletteSearchState::new(initial_query.clone());
        let intent_value = match intent {
            Some(OpenIntent::NewThreadIn) => Some("action:new-thread-in"),
            Some(OpenIntent::ChangeTheme) => Some("action:change-theme"),
            Some(OpenIntent::AddProject) => Some("action:add-project"),
            Some(OpenIntent::PortForwards) => Some("action:port-forwards"),
            Some(OpenIntent::Search(_)) | None => None,
        };
        if let Some(view) = intent_value.and_then(|value| submenu_view(&root_groups, value)) {
            search.push(view);
        }
        let placeholder = input_placeholder(if search.views.is_empty() { PaletteMode::Root } else { PaletteMode::Submenu });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        if !search.query.is_empty() {
            let query = search.query.clone();
            input.update(cx, |input, cx| input.set_value(query.to_string(), window, cx));
        }
        let events = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value();
                this.set_query(query, window, cx);
            }
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let highlight_cleared = !search.views.is_empty();
        self.session = Some(Session {
            input,
            return_focus,
            root_groups,
            nested_items,
            thread_items,
            project_items: project_search_items(&projects),
            settings_items,
            search,
            shown: Vec::new(),
            rows: Rc::new(build_rows(&[])),
            highlighted: None,
            highlight_cleared,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            now_ms,
            _input_events: events,
        });
        self.refilter();
        // The root page opens with its first row highlighted.
        if let Some(session) = &mut self.session
            && !session.highlight_cleared
        {
            session.highlighted = session.rows.item_values.first().cloned();
        }
    }

    /// The root page's action rows, in the order of `CommandPalette.tsx`.
    fn root_actions(&self, _window: &mut Window, cx: &mut Context<Self>) -> Vec<PaletteItem> {
        let source = &self.source;
        let projects = source.projects();
        let active = source.active_thread();
        let current_project = source.current_project();
        let environments = source.environments();
        let scratch_environment = environments.iter().find(|e| e.connected).map(|e| e.id.clone());
        let command = |value: &str, title: SharedString, icon, command: StaticCommand, terms: &[&str]| {
            PaletteItem::action(value, PaletteTitle::Text(title), icon, PaletteRun::Command(command.into()), terms)
        };
        let event = |value: &str, title: SharedString, icon, event: PaletteEvent, terms: &[&str]| {
            PaletteItem::action(value, PaletteTitle::Text(title), icon, PaletteRun::Event(event), terms)
        };
        let mut items = Vec::new();
        if !projects.is_empty() {
            let active_title = current_project
                .as_ref()
                .and_then(|r| projects.iter().find(|p| &p.project_ref == r))
                .or_else(|| projects.first())
                .map(|p| p.title.clone());
            if let Some(project) = active_title {
                items.push(
                    PaletteItem::action(
                        "action:new-thread",
                        PaletteTitle::NewThreadIn { project },
                        IconName::SquarePen,
                        PaletteRun::Command(StaticCommand::ChatNewLocal.into()),
                        &["new thread", "chat", "create", "draft"],
                    )
                    .shortcut(StaticCommand::ChatNewLocal),
                );
            }
            let view = PaletteView {
                addon_icon: IconName::SquarePen,
                groups: vec![PaletteGroup {
                    value: "projects".into(),
                    label: t!("Projects"),
                    items: new_thread_in_items(&projects, scratch_environment.as_ref()),
                }],
                initial_query: None,
            };
            items.push(PaletteItem::submenu(
                "action:new-thread-in",
                t!("New thread in..."),
                IconName::SquarePen,
                view,
                &["new thread", "project", "pick", "choose", "select"],
            ));
        }
        if scratch_environment.is_some() {
            items.push(
                command(
                    "action:new-thread-without-project",
                    t!("New thread without a project"),
                    IconName::MessageSquareDashed,
                    StaticCommand::ChatNewWithoutProject,
                    &["new thread", "no project", "without project", "none", "chat"],
                )
                .shortcut(StaticCommand::ChatNewWithoutProject),
            );
        }
        if source.supports(PaletteFeature::Triage) {
            items.push(
                event("action:triage", t!("Triage an issue"), IconName::Bug, PaletteEvent::Triage, &[
                    "triage", "bug", "issue", "report", "broken", "debug", "help", "support",
                ])
                .description(t!("An agent investigates a T3 Code problem and helps file an issue")),
            );
        }
        if active.is_some() {
            items.push(
                command("find-current-thread", t!("Find in current thread"), IconName::TextSearch, StaticCommand::ChatFind, &[
                    "find", "search", "messages", "plans",
                ])
                .shortcut(StaticCommand::ChatFind),
            );
        }
        if source.supports(PaletteFeature::PreviewFind) {
            items.push(
                command("find-in-browser-page", t!("Find on page in browser"), IconName::TextSearch, StaticCommand::PreviewFind, &[
                    "find", "search", "browser", "preview", "page",
                ])
                .shortcut(StaticCommand::PreviewFind),
            );
        }
        if source.supports(PaletteFeature::PortForwarding) {
            items.push(port_forwards_item(&source.ports()));
        }
        if active.is_some() {
            if source.supports(PaletteFeature::PromptLinks) {
                items.push(event("action:copy-prompt-link", t!("Copy prompt link"), IconName::Link, PaletteEvent::CopyPromptLink { with_context: false }, &[
                    "copy", "share", "prompt", "link",
                ]));
            }
            items.push(
                command("action:copy-thread-reference", t!("Copy thread"), IconName::Link, StaticCommand::ThreadCopyReference, &[
                    "copy", "thread", "thread id", "reference",
                ])
                .shortcut(StaticCommand::ThreadCopyReference),
            );
            if source.supports(PaletteFeature::LinkPullRequest) {
                items.push(event("action:link-pull-request", t!("Link pull request to thread"), IconName::GitPullRequestArrow, PaletteEvent::LinkPullRequest, &[
                    "link", "pull request", "pr", "attach", "stack",
                ]));
                items.push(event("action:open-thread-pull-requests", t!("Show linked items"), IconName::GitPullRequestArrow, PaletteEvent::ShowLinkedItems, &[
                    "pull requests", "issues", "linked", "stack", "prs",
                ]));
            }
            if source.supports(PaletteFeature::GuestShare) {
                items.push(event("action:share-thread-with-guest", t!("Share with guest"), IconName::UserPlus, PaletteEvent::ShareWithGuest, &[
                    "share", "guest", "invite", "link", "teammate",
                ]));
            }
            if source.supports(PaletteFeature::RestartAgentSession) {
                items.push(event("action:restart-agent-session", t!("Restart agent session"), IconName::RotateCcw, PaletteEvent::RestartAgentSession, &[
                    "restart", "reset", "reload", "agent", "session", "skills", "plugins", "mcp",
                ]));
            }
        }
        items.push(
            command("action:open-file-picker", t!("Go to file"), IconName::FileSearch, StaticCommand::FilePickerToggle, &[
                "go to file", "open file", "file picker", "find file", "quick open",
            ])
            .shortcut(StaticCommand::FilePickerToggle)
            .keep_open(),
        );
        items.push(
            command("action:search-project-contents", t!("Search project contents"), IconName::TextSearch, StaticCommand::ProjectSearchToggle, &[
                "search project", "find in files", "grep", "content search", "text search",
            ])
            .shortcut(StaticCommand::ProjectSearchToggle)
            .keep_open(),
        );
        if source.supports(PaletteFeature::NewProject) {
            items.push(event("action:new-project", t!("New project"), IconName::FolderGit2, PaletteEvent::NewProject, &[
                "new project", "create project", "empty", "repository", "repo", "git init",
            ]));
        }
        items.push(add_project_item(&environments));
        items.push(change_theme_item(&source.themes(), cx));
        items.push(change_appearance_item(cx));
        items.push(theme_editor_item());
        if source.supports(PaletteFeature::PullRequests) {
            items.push(event("action:pull-requests", t!("Open pull requests"), IconName::GitPullRequestArrow, PaletteEvent::OpenPullRequests, &[
                "pull requests", "prs", "pr", "review", "merge", "branch",
            ]));
        }
        if source.supports(PaletteFeature::Issues) {
            items.push(event("action:issues", t!("Open issues"), IconName::CircleDot, PaletteEvent::OpenIssues, &[
                "issues", "issue", "bugs", "tracker", "github", "triage",
            ]));
        }
        if source.supports(PaletteFeature::UseInAnotherApp) {
            items.push(event("action:use-in-another-app", t!("Use T3 Code in another app (MCP)"), IconName::Plug, PaletteEvent::UseInAnotherApp, &[
                "mcp", "connector", "another app", "poke", "claude", "chatgpt", "grok", "muse", "instinct", "openclaw", "hermes",
            ]));
        }
        items.push(
            command("action:usage", t!("Open usage"), IconName::ChartNoAxesColumn, StaticCommand::UsageOpen, &[
                "usage", "use", "tokens", "cost", "spend", "limits", "stats", "analytics",
            ])
            .shortcut(StaticCommand::UsageOpen),
        );
        for (value, title, command_id) in [
            ("action:workspace-focus:1", t!("Focus Next Workspace Area"), StaticCommand::WorkspaceFocusNext),
            ("action:workspace-focus:-1", t!("Focus Previous Workspace Area"), StaticCommand::WorkspaceFocusPrevious),
        ] {
            let mut item = command(value, title.clone(), IconName::Monitor, command_id, &[]);
            item.set_terms([title, "accessibility".into(), "navigation".into(), "focus".into(), "F6".into()]);
            items.push(item.shortcut(command_id));
        }
        items.push(event("action:settings", t!("Open settings"), IconName::Settings, PaletteEvent::OpenSettings, &[
            "settings", "preferences", "configuration", "keybindings",
        ]));
        let contextual = current_project.as_ref().and_then(|r| projects.iter().find(|p| &p.project_ref == r));
        if let Some(project) = contextual {
            let (title, icon) = if project.pinned { (t!("Unpin project"), IconName::PinOff) } else { (t!("Pin project"), IconName::Pin) };
            items.push(
                event("action:project-pin", title, icon, PaletteEvent::ToggleProjectPin(project.project_ref.clone()), &["pin", "unpin", "project", "favorite"])
                    .description(project.title.clone()),
            );
        }
        if let Some(project) = contextual.or_else(|| projects.first())
            && source.supports(PaletteFeature::ProjectSettings)
        {
            items.push(
                event("action:project-settings", t!("Project settings"), IconName::Folder, PaletteEvent::OpenProjectSettings(project.project_ref.clone()), &[
                    "project", "settings", "name", "icon", "scripts", "model", "workspace", "grouping", "checkout", "remove", "t3.json",
                ])
                .description(project.title.clone()),
            );
        }
        items
    }

    fn set_query(&mut self, query: SharedString, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        if session.search.query == query {
            return;
        }
        session.search.query = query;
        session.highlighted = None;
        session.highlight_cleared = true;
        self.refilter();
        cx.notify();
    }

    /// Filters for the current page and query, and resets the list.
    fn refilter(&mut self) {
        let Some(session) = &mut self.session else { return };
        let (active_groups, in_submenu): (&[PaletteGroup], bool) = match session.search.current_view() {
            Some(view) => (&view.groups, true),
            None => (&session.root_groups, false),
        };
        let shown = filter_command_palette_groups(&FilterInput {
            active_groups,
            query: &session.search.query,
            is_in_submenu: in_submenu,
            root_groups: in_submenu.then_some(session.root_groups.as_slice()),
            nested_items: &session.nested_items,
            project_items: &session.project_items,
            settings_items: &session.settings_items,
            thread_items: &session.thread_items,
            now_ms: session.now_ms,
        });
        session.rows = Rc::new(build_rows(&shown));
        session.shown = shown;
        session.list.reset(session.rows.rows.len());
        session.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        let values = &session.rows.item_values;
        if values.is_empty() {
            return;
        }
        let current = session.highlighted.as_ref().and_then(|v| values.iter().position(|x| x == v));
        let next = match current {
            // After typing, Down lands on the first row, which Enter already targets.
            _ if session.highlight_cleared && delta > 0 => 0,
            Some(index) => (index as isize + delta).rem_euclid(values.len() as isize) as usize,
            None if delta > 0 => 0,
            None => values.len() - 1,
        };
        session.highlight_cleared = false;
        session.highlighted = Some(values[next].clone());
        if next == 0 {
            session.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
        } else {
            session.list.scroll_to_reveal_item(session.rows.row_index_by_item[next]);
        }
        cx.notify();
    }

    fn highlight(&mut self, value: SharedString, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        if session.highlighted.as_ref() == Some(&value) {
            return;
        }
        session.highlight_cleared = false;
        session.highlighted = Some(value);
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let target = session.highlighted.clone().or_else(|| session.rows.item_values.first().cloned());
        if let Some(item) = target.and_then(|value| find_item(&session.shown, &value).cloned()) {
            self.run_item(item, window, cx);
        }
    }

    /// Runs a row: opens its page, or closes the palette and acts (`executeItem`).
    pub fn run_item(&mut self, item: Rc<PaletteItem>, window: &mut Window, cx: &mut Context<Self>) {
        if !item.is_enabled() {
            return;
        }
        let Some(session) = &mut self.session else { return };
        match &item.kind {
            PaletteItemKind::Submenu(view) => {
                session.search.push(view.clone());
                session.highlighted = None;
                session.highlight_cleared = true;
                let (query, placeholder) = (session.search.query.clone(), input_placeholder(PaletteMode::Submenu));
                session.input.update(cx, |input, cx| {
                    input.set_value(query.to_string(), window, cx);
                    input.set_placeholder(placeholder, window, cx);
                });
                self.refilter();
                cx.notify();
            }
            PaletteItemKind::Action { run, keep_open } => {
                let return_focus = session.return_focus.clone();
                let query = session.search.query.clone();
                if !keep_open {
                    self.close(window, cx);
                }
                match run {
                    PaletteRun::Command(command) => dispatch_command(command, return_focus.as_ref(), window, cx),
                    PaletteRun::Event(PaletteEvent::OpenThread { thread, .. }) => {
                        cx.emit(PaletteEvent::OpenThread { thread: thread.clone(), query })
                    }
                    PaletteRun::Event(event) => cx.emit(event.clone()),
                    PaletteRun::Local(run) => run(window, cx),
                }
            }
        }
    }

    fn pop_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = &mut self.session else { return false };
        if !session.search.back() {
            return false;
        }
        session.highlighted = None;
        let (query, placeholder) = (
            session.search.query.clone(),
            input_placeholder(if session.search.views.is_empty() { PaletteMode::Root } else { PaletteMode::Submenu }),
        );
        session.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        self.refilter();
        cx.notify();
        true
    }

    /// A command shortcut pressed inside the palette: a row with that shortcut
    /// runs (jumps, new-thread commands), others act as in `handleKeyDown`.
    fn run_shortcut(&mut self, command: StaticCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let wanted = KeybindingCommand::Static(command);
        let item = session.shown.iter().flat_map(|g| g.items.iter()).find(|s| s.item.shortcut.as_ref() == Some(&wanted)).map(|s| s.item.clone());
        match item {
            Some(item) => self.run_item(item, window, cx),
            None if matches!(command, StaticCommand::ChatFind | StaticCommand::ThreadCopyReference) => {
                let return_focus = session.return_focus.clone();
                self.close(window, cx);
                dispatch_command(&wanted, return_focus.as_ref(), window, cx);
            }
            None => {}
        }
    }

    fn render_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.app_theme().clone();
        let session = self.session.as_ref().expect("open palette has a session");
        let viewport = window.viewport_size();
        let in_submenu = !session.search.views.is_empty();
        let rows = session.rows.clone();
        let highlighted = session.highlighted.clone();
        let shortcuts = cx.global::<ShortcutState>();
        let labels: Rc<Vec<Option<SharedString>>> = Rc::new(
            rows.rows
                .iter()
                .map(|row| match row {
                    PaletteRow::Item { shown, .. } => shown.item.shortcut.as_ref().and_then(|c| shortcuts.label(c)),
                    PaletteRow::Label { .. } => None,
                })
                .collect(),
        );
        // `max-h-105` on the popup; the list takes what the input and footer leave.
        let popup_max = theme.rem(26.25).min(viewport.height - theme.sp(8.));
        let chrome = theme.sp(3.) + theme.sp(8.5) + footer_height(&theme) + px(2.);
        let list_padding = theme.sp(4.);
        let content: gpui_kit::Pixels = rows.rows.iter().map(|row| row_height(row, &theme)).sum::<gpui_kit::Pixels>();
        let list_height = (content + list_padding).min(popup_max - chrome);
        let actions_only = session.search.query.starts_with('>');
        let weak = self.this.clone();
        let theme_for_rows = theme.clone();
        let list_element = list(session.list.clone(), move |ix, _window, cx| {
            let theme = &theme_for_rows;
            match &rows.rows[ix] {
                PaletteRow::Label { label, first } => render_label(label.clone(), *first, theme),
                PaletteRow::Item { shown, .. } => {
                    let value = shown.item.value.clone();
                    let active = highlighted.as_ref() == Some(&value);
                    let callbacks = {
                        let (hover, click, drag) = (weak.clone(), weak.clone(), weak.clone());
                        let (hover_value, item) = (value.clone(), shown.item.clone());
                        RowCallbacks {
                            on_hover: Box::new(move |_, cx| {
                                hover.update(cx, |palette, cx| palette.highlight(hover_value.clone(), cx)).ok();
                            }),
                            on_click: Box::new(move |window, cx| {
                                click.update(cx, |palette, cx| palette.run_item(item.clone(), window, cx)).ok();
                            }),
                            on_drag_start: Box::new(move |window, cx| {
                                drag.update(cx, |palette, cx| palette.close(window, cx)).ok();
                            }),
                        }
                    };
                    let _ = cx;
                    render_item(ix, shown, active, labels[ix].clone(), theme, Some(callbacks))
                }
            }
        })
        .px(theme.sp(2.))
        .py(theme.sp(2.))
        .h(list_height)
        .w_full();
        let body = if session.rows.rows.is_empty() {
            render_empty(empty_state_message(actions_only), &theme)
        } else {
            list_element.into_any_element()
        };
        let input_row = self.render_input(in_submenu, &theme, cx);
        let footer = render_footer(in_submenu, &theme);
        let c = &theme.colors;
        div()
            .id("command-palette")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_highlight(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_highlight(1, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &commands::ToggleCommandPalette, window, cx| this.close(window, cx)))
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, window, cx| {
                let query_empty = this.session.as_ref().is_some_and(|s| s.search.query.is_empty());
                if event.keystroke.key == "backspace" && query_empty && this.pop_view(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .map(|popup| register_shortcut_overrides(popup, cx))
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .max_w(theme.rem(36.))
            .max_h(popup_max)
            .overflow_hidden()
            .rounded(theme.radii.xxl)
            .border_1()
            .border_color(theme.derived.dialog_glass_border)
            .bg(theme.derived.surface_glass_opaque)
            .shadow(theme.shadows.dialog.clone())
            .text_color(c.foreground())
            .occlude()
            .child(input_row)
            .child(body)
            .child(footer)
    }

    fn render_input(&self, in_submenu: bool, theme: &AppTheme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.as_ref().expect("open palette has a session");
        let c = &theme.colors;
        let addon = if in_submenu {
            div()
                .id("command-palette-back")
                .cursor_pointer()
                .child(Icon::new(IconName::ArrowLeft).size(IconSize::Md).color(c.icon_muted))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.pop_view(window, cx);
                }))
                .into_any_element()
        } else {
            Icon::new(IconName::Search).size(IconSize::Md).color(c.icon_muted).into_any_element()
        };
        let field = gpui_kit::component::input::Input::new(&session.input).appearance(false);
        let field = gpui_kit::Styled::h(field, theme.sp(8.5))
            .w_full()
            .pl(theme.sp(2.) + theme.sp(6.))
            .pr(theme.sp(3.) - px(1.))
            .ui_text(theme, TextSize::Sm, weight::NORMAL);
        // `CommandInput`: `px-[--command-shell-inset] py-1.5`, a `lg` input without chrome.
        div().px(theme.spacing.command_shell_inset).py(theme.sp(1.5)).child(
            div()
                .relative()
                .h(theme.sp(8.5))
                .flex()
                .items_center()
                .child(field)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(theme.spacing.command_shell_inset + px(1.) + px(2.))
                        .flex()
                        .items_center()
                        .opacity(0.8)
                        .child(addon),
                ),
        )
    }
}

fn submenu_view(groups: &[PaletteGroup], value: &str) -> Option<Rc<PaletteView>> {
    groups.iter().flat_map(|g| g.items.iter()).find_map(|item| match &item.kind {
        PaletteItemKind::Submenu(view) if item.value == value => Some(view.clone()),
        _ => None,
    })
}

/// Runs a command from `focus`, or from the window's focus when there is none.
fn dispatch_command(command: &KeybindingCommand, focus: Option<&FocusHandle>, window: &mut Window, cx: &mut App) {
    let action = command_action(command);
    match focus.filter(|f| f.is_rendered(window)) {
        Some(focus) => {
            window.focus(focus, cx);
            focus.dispatch_action(action.as_ref(), window, cx);
        }
        None => window.dispatch_action(action, cx),
    }
}

/// Shortcuts the palette answers itself while open (`handleKeyDown`).
fn register_shortcut_overrides(
    popup: gpui_kit::Stateful<gpui_kit::Div>,
    cx: &mut Context<CommandPalette>,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let mut overrides: Vec<StaticCommand> = THREAD_JUMP_COMMANDS.to_vec();
    overrides.extend([
        StaticCommand::ChatNew,
        StaticCommand::ChatNewLocal,
        StaticCommand::ChatNewWithoutProject,
        StaticCommand::ChatFind,
        StaticCommand::ThreadCopyReference,
    ]);
    let weak = cx.weak_entity();
    let listener: Rc<dyn Fn(StaticCommand, &mut Window, &mut App)> = Rc::new(move |command, window, cx| {
        weak.update(cx, |palette, cx| palette.run_shortcut(command, window, cx)).ok();
    });
    commands::on_commands(popup, move |command| overrides.contains(&command), listener)
}

fn footer_height(theme: &AppTheme) -> gpui_kit::Pixels {
    theme.sp(5.) + theme.sp(5.) + px(1.)
}

/// A footer key: `Kbd` with `bg-foreground/8%` and `text-foreground`.
fn footer_kbd(theme: &AppTheme, content: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(theme.sp(1.))
        .h(theme.sp(5.))
        .min_w(theme.sp(5.))
        .px(theme.sp(1.))
        .rounded(px(4.))
        .bg(with_alpha(theme.colors.foreground(), 0.08))
        .text_color(theme.colors.foreground())
        .ui_text(theme, TextSize::Xs, weight::MEDIUM)
        .child(content)
}

/// `CommandFooter`: arrows to navigate, Enter, Backspace on pages, Esc.
fn render_footer(in_submenu: bool, theme: &AppTheme) -> impl IntoElement + use<> {
    let c = &theme.colors;
    let group = |keys: Vec<gpui_kit::AnyElement>, label: SharedString| {
        div().flex().items_center().gap(theme.sp(1.)).children(keys).child(div().child(label))
    };
    let arrow = |icon| footer_kbd(theme, Icon::new(icon).size(IconSize::Xs)).into_any_element();
    let text_kbd = |label: &'static str| footer_kbd(theme, label).into_any_element();
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(theme.sp(2.))
        .px(theme.spacing.command_content_inset)
        .py(theme.sp(2.5))
        .border_t_1()
        .border_color(with_alpha(c.foreground(), 0.07))
        .bg(with_alpha(c.foreground(), 0.025))
        .rounded_b(theme.radii.xxl - px(1.))
        .ui_text(theme, TextSize::Sm, weight::MEDIUM)
        .text_color(c.muted_foreground)
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(theme.sp(3.))
                .gap_y(theme.sp(1.5))
                .child(group(vec![arrow(IconName::ArrowUp), arrow(IconName::ArrowDown)], t!("Navigate")))
                .child(group(vec![text_kbd("Enter")], t!("Select")))
                .when(in_submenu, |d| d.child(group(vec![text_kbd("Backspace")], t!("Back"))))
                .child(group(vec![text_kbd("Esc")], t!("Close"))),
        )
}

impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.ui.open || self.session.is_none() {
            return div().into_any_element();
        }
        let theme = cx.app_theme().clone();
        let viewport = window.viewport_size();
        let popup = self.render_popup(window, cx);
        // `CommandDialogViewport`: centred column, `pt-[max(10vh, titlebar+8px)]` from 640px up.
        let top = if viewport.width >= px(640.) {
            (viewport.height * 0.1).max(theme.spacing.workspace_topbar_height + theme.sp(2.))
        } else {
            (viewport.height * 0.04).max(theme.sp(4.))
        };
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("command-palette-layer")
                    .occlude()
                    .w(viewport.width)
                    .h(viewport.height)
                    .relative()
                    .child(
                        div()
                            .id("command-palette-backdrop")
                            .absolute()
                            .size_full()
                            .bg(theme.derived.dialog_backdrop)
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx))),
                    )
                    .child(div().absolute().size_full().flex().flex_col().items_center().px(theme.sp(4.)).pt(top).child(popup)),
            ),
        )
        .with_priority(4)
        .into_any_element()
    }
}
