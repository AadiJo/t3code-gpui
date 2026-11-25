//! The command palette. Ports `CommandPalette.tsx`, `CommandPaletteContent.tsx`
//! and `ui/command.tsx`: a `dialog-glass` popup over the window with a search
//! input, grouped results in a virtual list, and a footer of key hints.
//!
//! Rows run in three ways: a command row dispatches the command's action
//! from the focus the palette opened from; a navigation row emits a
//! [`PaletteEvent`] for the app; a few rows (theme, appearance) act directly.
//! Command rows whose action has no handler there show as unavailable.

use crate::components::ui::GlassExt as _;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{rems, 
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement,
    KeyBinding, ListAlignment, ListOffset, ListState, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, WeakEntity, Window, actions, anchored, deferred, div, list, point, prelude::FluentBuilder as _, px,
};
use t3code_shared::keybindings::{KeybindingCommand, StaticCommand, THREAD_JUMP_COMMANDS};

use super::command_palette_add_project::{add_project_item, sources_view};
use super::command_palette_logic::{AddProjectStep, PaletteTrailing};
use super::command_palette_project_flow::{
    BrowseEntry, BrowseListing, CloneSource, FlowEnvironment, FlowOpened, ProjectFlow, RemoteSource, RepositoryInfo, append_segment,
    can_browse_up, clone_destination, clone_destination_browse, clone_directory_name, clone_directory_path, directory_path,
    incomplete_owner_message, new_project_repository, filter_entries, has_trailing_separator, initial_query,
    is_browse_query, leaf_segment, new_project_folder_name, normalize_clone_url, parent_path, tab_completion,
};
use super::sidebar::model::EnvironmentId;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use super::command_palette_logic::{
    FilterInput, PaletteEvent, PaletteGroup, PaletteItem, PaletteItemKind, PaletteMode, PaletteRow, PaletteRows,
    PaletteRun, PaletteSearchState, PaletteTitle, PaletteView, ShownGroup, build_nested_search_items, build_root_groups,
    build_rows, empty_state_message, filter_command_palette_groups, find_item, input_placeholder,
};
use super::command_palette_lookups::{CommandPaletteSource, PaletteFeature};
use super::command_palette_port_forward_items::port_forwards_item;
use super::command_palette_project_items::{new_thread_in_items, project_search_items, ProjectContext};
use super::command_palette_results::{RowCallbacks, render_empty, render_item, render_label, render_summary_row, row_height};
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
        /// Tab in a folder path: enters the highlighted or only matching folder, or completes the
        /// common prefix.
        CompletePath,
        /// Mod+Enter: submits an Add project page even with a row highlighted.
        SubmitFlow,
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
            KeyBinding::new("tab", CompletePath, Some(context)),
            KeyBinding::new(if cfg!(target_os = "macos") { "cmd-enter" } else { "ctrl-enter" }, SubmitFlow, Some(context)),
        ]);
    }
}

/// One open palette: its rows, query, page stack and highlight.
struct Session {
    input: Entity<InputState>,
    /// Where focus was when the palette opened; commands run from there.
    return_focus: Option<FocusHandle>,
    root_groups: Vec<PaletteGroup>,
    /// The root actions; their submenu options join a query's search (`buildNestedSearchItems`).
    actions: Vec<Rc<PaletteItem>>,
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
    /// The Add project page shown, when one is.
    flow: Option<ProjectFlow>,
    /// The folder listing the path query reads, keyed by the directory asked for.
    listing: Option<(SharedString, Result<BrowseListing, SharedString>)>,
    /// The directory being listed now, and the request.
    listing_request: Option<(SharedString, Task<()>)>,
    /// A create, add or clone is running.
    busy: bool,
    /// New project's "Create private repository" is checked (`newProjectPublishes`).
    publishes: bool,
    /// What the Add project pages know about each connected environment, read at open.
    flow_envs: Vec<FlowEnvironment>,
    /// Where a path typed on the root page browses.
    default_flow_env: Option<EnvironmentId>,
    _input_events: Subscription,
}

/// Which "Copy prompt link" rows the palette shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptLinkRows {
    /// The composer holds no prompt a link can carry.
    None,
    Plain,
    /// The project is on GitHub, so the link can name its repository and model too.
    WithContext,
}

/// The command palette view. Render it once at the window root, above the
/// app; it draws nothing while closed.
pub struct CommandPalette {
    source: Rc<dyn CommandPaletteSource>,
    /// Which prompt link rows the composer's prompt offers. `None` offers the plain one.
    prompt_link_probe: Option<Rc<dyn Fn(&App) -> PromptLinkRows>>,
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
        Self {
            source,
            prompt_link_probe: None,
            ui: CommandPaletteUiState::default(),
            session: None,
            focus_handle: cx.focus_handle(),
            this: cx.weak_entity(),
        }
    }

    /// Sets what decides which "Copy prompt link" rows show (`composerPromptLink` being
    /// non-null, `promptLinkRepositoryOf` naming a GitHub repository).
    pub fn set_prompt_link_probe(&mut self, probe: Rc<dyn Fn(&App) -> PromptLinkRows>) {
        self.prompt_link_probe = Some(probe);
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

    /// The visible rows' titles that show as unavailable, for checks.
    pub fn unavailable_titles(&self) -> Vec<SharedString> {
        self.session
            .iter()
            .flat_map(|s| s.shown.iter())
            .flat_map(|g| g.items.iter())
            .filter(|shown| shown.item.unavailable)
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
        let root_groups = build_root_groups(actions.clone(), recent);
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
        let connected: Vec<EnvironmentId> = source.environments().into_iter().filter(|e| e.connected).map(|e| e.id).collect();
        let flow_envs: Vec<FlowEnvironment> =
            source.project_flows().map(|flows| connected.iter().filter_map(|id| flows.environment(id, cx)).collect()).unwrap_or_default();
        let default_flow_env = connected.first().cloned();
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
            actions,
            thread_items,
            project_items: project_search_items(&projects, &source.environments()),
            settings_items,
            search,
            shown: Vec::new(),
            rows: Rc::new(build_rows(&[])),
            highlighted: None,
            highlight_cleared,
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            now_ms,
            flow: None,
            listing: None,
            listing_request: None,
            busy: false,
            publishes: false,
            flow_envs,
            default_flow_env,
            _input_events: events,
        });
        self.refilter();
        // The root page opens with its first row highlighted.
        if let Some(session) = &mut self.session
            && !session.highlight_cleared
        {
            session.highlighted = session.rows.item_values.first().cloned();
        }
        self.discover_sources(window, cx);
    }

    /// Reads each connected environment's source-control discovery, so its Sources page and new
    /// projects offer its hosts; an open session takes the result when it lands.
    fn discover_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.source.project_flows() else { return };
        let ids: Vec<EnvironmentId> = self.source.environments().into_iter().filter(|e| e.connected).map(|e| e.id).collect();
        for id in ids {
            let task = backend.discover(&id, cx);
            let backend = backend.clone();
            cx.spawn_in(window, async move |this, cx| {
                task.await;
                this.update(cx, |this, cx| {
                    let Some(info) = backend.environment(&id, cx) else { return };
                    let Some(session) = &mut this.session else { return };
                    if let Some(slot) = session.flow_envs.iter_mut().find(|e| e.id == id) {
                        *slot = info;
                    }
                    this.refilter();
                    cx.notify();
                })
                .ok();
            })
            .detach();
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
                .map(|p| p.group_label.clone());
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
                    items: new_thread_in_items(
                        &projects,
                        scratch_environment.as_ref(),
                        &ProjectContext {
                            environments: &environments,
                            current: current_project.clone(),
                            current_group: current_project
                                .as_ref()
                                .and_then(|r| projects.iter().find(|p| &p.project_ref == r))
                                .map(|p| p.group_key.clone()),
                        },
                    ),
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
        // A thread or a draft (`promptLinkTarget`); the probe is `None` without a composer prompt.
        let rows = self.prompt_link_probe.as_ref().map_or(PromptLinkRows::Plain, |probe| probe(cx));
        if source.supports(PaletteFeature::PromptLinks) && rows != PromptLinkRows::None {
            items.push(event("action:copy-prompt-link", t!("Copy prompt link"), IconName::Link, PaletteEvent::CopyPromptLink { with_context: false }, &[
                "copy", "share", "prompt", "link",
            ]));
            if rows == PromptLinkRows::WithContext {
                items.push(event("action:copy-prompt-link-with-context", t!("Copy prompt link with repo and model"), IconName::Link, PaletteEvent::CopyPromptLink { with_context: true }, &[
                    "copy", "share", "prompt", "link", "repository", "model",
                ]));
            }
        }
        if active.is_some() {
            // The subtitle is the thread's title (`resolveThreadReferenceCopyTarget`).
            let title = active
                .as_ref()
                .and_then(|key| source.threads().iter().find(|t| &t.key == key).map(|t| t.title.clone()))
                .filter(|t| !t.is_empty());
            let mut copy = command("action:copy-thread-reference", t!("Copy thread"), IconName::Link, StaticCommand::ThreadCopyReference, &[
                "copy", "thread", "thread id", "reference",
            ])
            .shortcut(StaticCommand::ThreadCopyReference);
            if let Some(title) = title {
                copy = copy.description(title);
            }
            items.push(copy);
            if source.supports(PaletteFeature::LinkPullRequest) {
                items.push(event("action:link-pull-request", t!("Link pull request to thread"), IconName::Link2, PaletteEvent::LinkPullRequest, &[
                    "link", "pull request", "pr", "attach", "stack",
                ]));
                let mut linked = event("action:open-thread-pull-requests", t!("Show linked items"), IconName::Link2, PaletteEvent::ShowLinkedItems, &[
                    "pull requests", "issues", "linked", "stack", "prs",
                ]);
                linked.disabled = !source.active_thread_has_linked_items();
                items.push(linked);
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
        let flows = source.project_flows();
        let can_create_new = |id: &super::sidebar::model::EnvironmentId| {
            flows.as_ref().and_then(|f| f.environment(id, cx)).is_some_and(|e| e.new_projects_root.is_some())
        };
        // Shown while some connected environment can make one (`newProjectEnvironmentOptions`).
        if source.supports(PaletteFeature::NewProject) && (flows.is_none() || environments.iter().any(|e| e.connected && can_create_new(&e.id))) {
            items.push(
                event("action:new-project", t!("New project"), IconName::FolderGit2, PaletteEvent::NewProject, &[
                    "new project", "create project", "empty", "repository", "repo", "git init",
                ])
                .keep_open(),
            );
        }
        let sources = |id: &super::sidebar::model::EnvironmentId| match flows.as_ref().and_then(|f| f.environment(id, cx)) {
            Some(e) => (e.new_projects_root.is_some(), e.remote_sources),
            None => (false, Vec::new()),
        };
        items.push(add_project_item(&environments, &sources).without_chevron());
        items.push(change_theme_item(cx));
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
        self.request_listing(cx);
        cx.notify();
    }

    /// Filters for the current page and query, and resets the list.
    fn refilter(&mut self) {
        if let Some(groups) = self.flow_groups() {
            let Some(session) = &mut self.session else { return };
            let shown: Vec<ShownGroup> = groups.iter().map(ShownGroup::from).collect();
            session.rows = Rc::new(build_rows(&shown));
            session.shown = shown;
            session.list.reset(session.rows.rows.len());
            session.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) });
            return;
        }
        let Some(session) = &mut self.session else { return };
        let (active_groups, in_submenu): (&[PaletteGroup], bool) = match session.search.current_view() {
            Some(view) => (&view.groups, true),
            None => (&session.root_groups, false),
        };
        // Built per query: a query that only names a submenu keeps the submenu row itself.
        let nested_items = build_nested_search_items(&session.actions, &session.search.query);
        let shown = filter_command_palette_groups(&FilterInput {
            active_groups,
            query: &session.search.query,
            is_in_submenu: in_submenu,
            root_groups: in_submenu.then_some(session.root_groups.as_slice()),
            nested_items: &nested_items,
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
        if self.confirm_flow(false, window, cx) {
            return;
        }
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
            PaletteItemKind::Action { run: PaletteRun::Event(event), .. } if self.start_flow_for(event, window, cx) => {}
            PaletteItemKind::Action { run, keep_open } => {
                let Some(session) = &mut self.session else { return };
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
        session.flow = None;
        session.busy = false;
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
        self.request_listing(cx);
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
        let chrome = theme.sp(3.) + theme.sp(8.5) + px(2.) + footer_height(&theme) + px(2.);
        let list_padding = theme.sp(4.);
        let content: gpui_kit::Pixels = rows.rows.iter().map(|row| row_height(row, &theme)).sum::<gpui_kit::Pixels>();
        let flow = self.flow_chrome(&theme);
        let chrome = chrome + flow.header_height;
        let list_height = (content + list_padding).min(popup_max - chrome);
        let actions_only = session.search.query.starts_with('>');
        let weak = self.this.clone();
        let theme_for_rows = theme.clone();
        let slots: Rc<Vec<gpui_kit::Pixels>> = Rc::new(rows.rows.iter().map(|row| row_height(row, &theme)).collect());
        let list_element = list(session.list.clone(), move |ix, _window, cx| {
            let theme = &theme_for_rows;
            let slot = slots[ix];
            let row = match &rows.rows[ix] {
                PaletteRow::Label { label, first } => render_label(label.clone(), *first, theme),
                PaletteRow::Item { shown, .. } => {
                    let value = shown.item.value.clone();
                    let active = highlighted.as_ref() == Some(&value);
                    let callbacks = {
                        let (hover, click, drag, pin) = (weak.clone(), weak.clone(), weak.clone(), weak.clone());
                        let (hover_value, item) = (value.clone(), shown.item.clone());
                        RowCallbacks {
                            on_hover: Box::new(move |_, cx| {
                                hover.update(cx, |palette, cx| palette.highlight(hover_value.clone(), cx)).ok();
                            }),
                            on_click: Box::new(move |window, cx| {
                                click.update(cx, |palette, cx| palette.run_item(item.clone(), window, cx)).ok();
                            }),
                            on_pin: {
                                let pin = pin.clone();
                                let project = shown.item.project.as_ref().map(|p| p.project_ref.clone());
                                Box::new(move |_, cx| {
                                    if let Some(project) = project.clone() {
                                        pin.update(cx, |_, cx| cx.emit(PaletteEvent::ToggleProjectPin(project))).ok();
                                    }
                                })
                            },
                            on_drag_start: Box::new(move |window, cx| {
                                drag.update(cx, |palette, cx| palette.close(window, cx)).ok();
                            }),
                        }
                    };
                    let _ = cx;
                    render_item(ix, shown, active, labels[ix].clone(), theme, Some(callbacks), cx)
                }
            };
            div().h(slot).flex_none().child(row).into_any_element()
        })
        .px(theme.sp(2.))
        .py(theme.sp(2.))
        .h(list_height)
        .w_full();
        let body = if session.rows.rows.is_empty() {
            match flow.empty.clone() {
                Some(message) => render_empty(message, &theme),
                None if flow.status.is_some() => div().into_any_element(),
                None => render_empty(empty_state_message(actions_only), &theme),
            }
        } else {
            list_element.into_any_element()
        };
        let input_row = self.render_input(in_submenu, &theme, cx);
        let footer = render_footer(in_submenu, flow.footer_action.clone(), &theme);
        let header = flow.header;
        let status = flow.status.map(|text| {
            // `px-4 py-3 text-sm text-muted-foreground`.
            div().px(theme.sp(4.)).py(theme.sp(3.)).ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.muted_foreground).child(text)
        });
        let c = &theme.colors;
        div()
            .id("command-palette")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_highlight(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_highlight(1, cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.confirm(window, cx)))
            .on_action(cx.listener(|this, _: &Cancel, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &CompletePath, window, cx| this.complete_path(window, cx)))
            .on_action(cx.listener(|this, _: &SubmitFlow, window, cx| {
                this.confirm_flow(true, window, cx);
            }))
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
            .glass(theme.derived.glass_surface)
            .shadow(theme.shadows.dialog.clone())
            .text_color(c.foreground())
            .occlude()
            .child(input_row)
            .children(header)
            .children(status)
            .child(body)
            .child(footer)
    }

    fn render_input(&self, in_submenu: bool, theme: &AppTheme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let session = self.session.as_ref().expect("open palette has a session");
        let c = &theme.colors;
        let browsing_root = !in_submenu && self.browse_target().is_some();
        let addon = if browsing_root {
            Icon::new(IconName::FolderPlus).size(IconSize::Custom(theme.rem(1.))).color(c.foreground()).into_any_element()
        } else if in_submenu {
            div()
                .id("command-palette-back")
                .cursor_pointer()
                // The back arrow takes the text colour; the addon's 80% opacity dims it.
                .child(Icon::new(IconName::ArrowLeft).size(IconSize::Custom(theme.rem(1.))).color(c.foreground()))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.pop_view(window, cx);
                }))
                .into_any_element()
        } else {
            Icon::new(IconName::Search).size(IconSize::Custom(theme.rem(1.))).color(c.icon_muted).into_any_element()
        };
        let field = gpui_kit::component::input::Input::new(&session.input).appearance(false);
        let button = self.submit_button(theme, cx);
        let field = gpui_kit::Styled::h(field, theme.sp(8.5))
            .w_full()
            .pl(theme.sp(2.) + theme.sp(6.))
            .pr(self.input_end_padding(theme))
            .ui_text(theme, TextSize::Sm, weight::NORMAL);
        let accessory = button.map(|(label, shortcut, enabled)| {
            // `variant="outline" size="xs"` at `inset-e-2.5`, centred on the input row.
            let weak = self.this.clone();
            div().absolute().top_0().bottom_0().right(theme.sp(2.5)).flex().items_center().child(
                crate::components::ui::button::Button::new("command-palette-submit")
                    .variant(crate::components::ui::button::ButtonVariant::Outline)
                    .size(crate::components::ui::button::ButtonSize::Xs)
                    .label(label)
                    .accessory(div().mr(theme.sp(-0.5)).child(crate::components::ui::kbd::Kbd::new(shortcut)))
                    .disabled(!enabled)
                    .on_click(move |_, window, cx| {
                        weak.update(cx, |palette, cx| {
                            palette.confirm_flow(true, window, cx);
                        })
                        .ok();
                    }),
            )
        });
        // `CommandInput`: `px-[--command-shell-inset] py-1.5`, a `lg` input whose
        // control keeps its 1px transparent border; the addon sits at
        // `ps-[calc(--spacing(3)-1px)]` inside it.
        div().relative().px(theme.spacing.command_shell_inset).py(theme.sp(1.5)).child(
            div()
                .relative()
                .h(theme.sp(8.5) + px(2.))
                .p(px(1.))
                .flex()
                .items_center()
                .child(field)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        // `[&_svg]:-mx-0.5` pulls the icon back by 0.125rem.
                        .left(px(1.) + theme.sp(3.) - px(1.) - theme.rem(0.125))
                        .flex()
                        .items_center()
                        .opacity(0.8)
                        .child(addon),
                ),
        )
        .children(accessory)
    }
}

/// What an Add project page adds around the list.
#[derive(Default)]
struct FlowChrome {
    header: Option<gpui_kit::AnyElement>,
    /// The header and status line's height, for sizing the list.
    header_height: gpui_kit::Pixels,
    status: Option<SharedString>,
    empty: Option<SharedString>,
    /// The footer's Enter label; `None` hides it.
    footer_action: Option<SharedString>,
}

/// What the path query reads on the current page.
struct BrowseTarget {
    env: FlowEnvironment,
    directory: SharedString,
    filter: SharedString,
}

/// The Add project pages (`CommandPalette.tsx` browse, clone and new-project modes).
impl CommandPalette {
    /// The folder the query lists, when it is a path on this page: a Local folder or clone
    /// destination page, or a path typed on the root page.
    fn browse_target(&self) -> Option<BrowseTarget> {
        let session = self.session.as_ref()?;
        let env = match &session.flow {
            Some(flow) if flow.browses() => flow.environment().clone(),
            Some(_) => return None,
            None if session.search.views.is_empty() => session.default_flow_env.clone()?,
            None => return None,
        };
        let env = session.flow_envs.iter().find(|e| e.id == env)?.clone();
        let query = session.search.query.as_ref();
        if !is_browse_query(query, env.windows) {
            return None;
        }
        let filter = if has_trailing_separator(query) { "" } else { leaf_segment(query) };
        Some(BrowseTarget { directory: directory_path(query).to_string().into(), filter: filter.to_string().into(), env })
    }

    /// The listing for the target's directory, once it has arrived.
    fn current_listing(&self, target: &BrowseTarget) -> Option<&Result<BrowseListing, SharedString>> {
        let (directory, listing) = self.session.as_ref()?.listing.as_ref()?;
        (directory == &target.directory).then_some(listing)
    }

    /// Lists the query's directory when it is not listed or being listed already.
    fn request_listing(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.browse_target() else { return };
        let Some(backend) = self.source.project_flows() else { return };
        let Some(session) = &self.session else { return };
        if session.listing.as_ref().is_some_and(|(d, _)| d == &target.directory)
            || session.listing_request.as_ref().is_some_and(|(d, _)| d == &target.directory)
        {
            return;
        }
        let directory = target.directory.clone();
        let task = backend.browse(&target.env.id, directory.to_string(), cx);
        let request = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                let Some(session) = &mut this.session else { return };
                if session.listing_request.as_ref().is_some_and(|(d, _)| d == &directory) {
                    session.listing_request = None;
                }
                session.listing = Some((directory, result));
                this.refilter();
                cx.notify();
            })
            .ok();
        });
        if let Some(session) = &mut self.session {
            session.listing_request = Some((target.directory, request));
        }
    }

    /// A row action that calls back into the palette.
    fn local_run(&self, run: impl Fn(&mut CommandPalette, &mut Window, &mut Context<CommandPalette>) + 'static) -> PaletteRun {
        let weak = self.this.clone();
        let run = Rc::new(run);
        PaletteRun::Local(Rc::new(move |window, cx| {
            // Enter runs rows while the palette is updating, so this waits for that to end.
            let (weak, run) = (weak.clone(), run.clone());
            window.defer(cx, move |window, cx| {
                weak.update(cx, |palette, cx| run(palette, window, cx)).ok();
            });
        }))
    }

    /// The rows an Add project page or a path query shows in place of filtered results.
    fn flow_groups(&self) -> Option<Vec<PaletteGroup>> {
        let session = self.session.as_ref()?;
        match &session.flow {
            Some(ProjectFlow::CloneRepository { .. }) => Some(Vec::new()),
            Some(ProjectFlow::NewProject { env, sources }) => {
                let target = sources.clone().unwrap_or_else(|| env.clone());
                let mut item = PaletteItem::action(
                    "new-project:existing",
                    PaletteTitle::Text(t!("Add existing project")),
                    IconName::FolderPlus,
                    self.local_run(move |palette, window, cx| palette.show_existing_project_sources(target.clone(), window, cx)),
                    &[],
                )
                .description(t!("Open a folder or clone a repository"))
                .keep_open();
                item.trailing = Some(PaletteTrailing::Chevron);
                let mut groups: Vec<PaletteGroup> = self.new_project_machine_group(env, session.busy).into_iter().collect();
                groups.extend(self.new_project_option_group(env));
                groups.push(PaletteGroup { value: "new-project-existing".into(), label: SharedString::default(), items: vec![Rc::new(item)] });
                Some(groups)
            }
            _ => {
                let target = self.browse_target()?;
                Some(self.browse_groups(&target))
            }
        }
    }

    /// The folder rows (`buildBrowseGroups`).
    fn browse_groups(&self, target: &BrowseTarget) -> Vec<PaletteGroup> {
        let Some(session) = self.session.as_ref() else { return Vec::new() };
        let Some(Ok(listing)) = self.current_listing(target) else { return Vec::new() };
        let clone_destination = matches!(session.flow, Some(ProjectFlow::CloneDestination { .. }));
        let (visible, _) = self.visible_entries(target, listing);
        let mut items = Vec::new();
        if can_browse_up(&target.directory) {
            let mut item = PaletteItem::action(
                "browse:up",
                PaletteTitle::Text("..".into()),
                IconName::CornerLeftUp,
                self.local_run(|palette, window, cx| palette.browse_up(window, cx)),
                &[],
            )
            .keep_open();
            item.set_terms([SharedString::from("..")]);
            items.push(Rc::new(item));
        }
        for entry in visible {
            let name = entry.name.clone();
            let mut item = PaletteItem::action(
                format!("browse:{}", entry.full_path),
                PaletteTitle::Text(entry.name.clone()),
                IconName::Folder,
                self.local_run(move |palette, window, cx| palette.browse_to(name.clone(), window, cx)),
                &[],
            )
            .keep_open();
            item.set_terms([entry.name.clone()]);
            items.push(Rc::new(item));
        }
        let label = if clone_destination { t!("Select where to clone") } else { t!("Directories") };
        vec![PaletteGroup { value: "directories".into(), label, items }]
    }

    /// The listing's folders the filter keeps, and the one it names exactly. On a clone
    /// destination the clone's own folder name filters nothing (`filterPinnedBrowseEntries`).
    fn visible_entries(&self, target: &BrowseTarget, listing: &BrowseListing) -> (Vec<BrowseEntry>, Option<BrowseEntry>) {
        match self.session.as_ref().and_then(|s| s.flow.as_ref()) {
            Some(ProjectFlow::CloneDestination { pinned, .. }) => {
                let same = |a: &str, b: &str| if target.env.windows { a.eq_ignore_ascii_case(b) } else { a == b };
                let filter = if same(&target.filter, pinned) { "" } else { target.filter.as_ref() };
                let (visible, _) = filter_entries(&listing.entries, filter);
                let exact = listing.entries.iter().find(|e| same(&e.name, &target.filter)).cloned();
                (visible, exact)
            }
            _ => filter_entries(&listing.entries, &target.filter),
        }
    }

    /// New project's Options group: "Create private repository on <host>", when a host can take
    /// one (`newProjectOptionGroups`).
    fn new_project_option_group(&self, env: &EnvironmentId) -> Option<PaletteGroup> {
        let session = self.session.as_ref()?;
        let info = session.flow_envs.iter().find(|e| &e.id == env)?;
        let root = info.new_projects_root.clone()?;
        let target = info.publish_targets.first()?.clone();
        let name = session.search.query.trim();
        let title = if info.publish_targets.len() > 1 {
            t!("Create private repository")
        } else {
            t!("Create private repository on {host}", host = target.label.clone())
        };
        let description: SharedString = if name.is_empty() {
            target.owner.clone().unwrap_or_else(|| t!("Your {host} account", host = target.label.clone()))
        } else {
            new_project_repository(&target, &clone_destination(&root, &new_project_folder_name(name))).into()
        };
        let mut item = PaletteItem::action(
            "new-project:publish",
            PaletteTitle::Text(title),
            IconName::Link,
            self.local_run(|palette, _, cx| {
                if let Some(session) = &mut palette.session {
                    session.publishes = !session.publishes;
                }
                palette.refilter();
                cx.notify();
            }),
            &[],
        )
        .description(description)
        .keep_open();
        item.brand = Some(crate::source_control_presentation::source_control_icon(crate::source_control_presentation::source_control_definition(Some(&target.kind))));
        item.trailing = Some(PaletteTrailing::Checkbox(session.publishes));
        item.disabled = session.busy;
        Some(PaletteGroup { value: "new-project-options".into(), label: t!("Options"), items: vec![Rc::new(item)] })
    }

    /// Connected environments whose server can make a new project (`newProjectEnvironmentOptions`).
    fn new_project_environments(&self) -> Vec<FlowEnvironment> {
        let Some(session) = &self.session else { return Vec::new() };
        session.flow_envs.iter().filter(|e| e.new_projects_root.is_some()).cloned().collect()
    }

    /// The Environments group of a new project, when more than one machine can make one.
    fn new_project_machine_group(&self, selected: &EnvironmentId, busy: bool) -> Option<PaletteGroup> {
        let options = self.new_project_environments();
        if options.len() <= 1 {
            return None;
        }
        let items = options
            .into_iter()
            .map(|option| {
                let id = option.id.clone();
                let mut item = PaletteItem::action(
                    format!("new-project:environment:{}", option.id.0),
                    PaletteTitle::Text(option.label.clone()),
                    IconName::Server,
                    self.local_run(move |palette, _, cx| palette.switch_new_project_environment(id.clone(), cx)),
                    &[],
                )
                .description(SharedString::from(option.id.0.to_string()))
                .keep_open();
                item.disabled = busy;
                if &option.id == selected {
                    item.trailing = Some(PaletteTrailing::Check);
                }
                Rc::new(item)
            })
            .collect();
        Some(PaletteGroup { value: "new-project-machines".into(), label: t!("Environments"), items })
    }

    /// Runs an Add project row in the palette when it can (`keepOpen` rows); `false` leaves the
    /// event to the app.
    fn start_flow_for(&mut self, event: &PaletteEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.source.project_flows().is_none() || self.session.is_none() {
            return false;
        }
        let flow = match event {
            PaletteEvent::AddProject(AddProjectStep::BrowseFolders(env)) => ProjectFlow::Browse { env: env.clone() },
            PaletteEvent::AddProject(AddProjectStep::CloneRepository(env)) => ProjectFlow::CloneRepository { env: env.clone(), source: CloneSource::Url },
            PaletteEvent::AddProject(AddProjectStep::CloneFromHost(env, host)) => {
                ProjectFlow::CloneRepository { env: env.clone(), source: CloneSource::Host(host.clone()) }
            }
            PaletteEvent::AddProject(AddProjectStep::NewProject(env)) => ProjectFlow::NewProject { env: env.clone(), sources: Some(env.clone()) },
            PaletteEvent::NewProject => match self.new_project_environments().first() {
                Some(env) => ProjectFlow::NewProject { env: env.id.clone(), sources: None },
                None => return false,
            },
            _ => return false,
        };
        self.start_flow(flow, window, cx);
        true
    }

    /// Opens the palette straight on an Add project page, for an event raised outside it.
    /// Returns `false` when the source has no project flows.
    pub fn open_project_flow(&mut self, event: &PaletteEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.source.project_flows().is_none() {
            return false;
        }
        if !self.ui.open {
            self.dispatch(CommandPaletteUiAction::ToggleMode(SearchOverlayMode::Command), window, cx);
        }
        self.start_flow_for(event, window, cx)
    }

    /// Opens an Add project page (`startAddProjectBrowse`, `startAddProjectClone`, `startNewProject`).
    pub fn start_flow(&mut self, flow: ProjectFlow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        let initial_query = match &flow {
            ProjectFlow::Browse { env } => {
                let base = session.flow_envs.iter().find(|e| &e.id == env).map(|e| e.base_directory.clone()).unwrap_or_default();
                Some(SharedString::from(initial_query(&base)))
            }
            ProjectFlow::CloneRepository { .. } => Some(SharedString::default()),
            _ => None,
        };
        session.search.push(Rc::new(PaletteView { addon_icon: flow.addon_icon(), groups: Vec::new(), initial_query }));
        session.flow = Some(flow);
        session.busy = false;
        session.publishes = false;
        session.highlighted = None;
        session.highlight_cleared = true;
        self.sync_input(window, cx);
        self.refilter();
        self.request_listing(cx);
        cx.notify();
    }

    /// Puts the session's query and placeholder into the input.
    fn sync_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let placeholder = match &session.flow {
            Some(flow) => flow.placeholder(),
            None => input_placeholder(if session.search.views.is_empty() { PaletteMode::Root } else { PaletteMode::Submenu }),
        };
        let query = session.search.query.clone();
        session.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
    }

    /// Replaces the query from code (folder navigation, the clone destination).
    fn replace_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        session.search.query = query.into();
        session.highlighted = None;
        session.highlight_cleared = true;
        self.sync_input(window, cx);
        self.refilter();
        self.request_listing(cx);
        cx.notify();
    }

    /// `browseTo`: enters a folder of the listing.
    fn browse_to(&mut self, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else { return };
        let query = session.search.query.to_string();
        let next = match (&session.flow, self.browse_target()) {
            (Some(ProjectFlow::CloneDestination { pinned, .. }), Some(target)) => clone_destination_browse(&target.directory, &name, pinned, !target.env.windows),
            _ => append_segment(&query, &name),
        };
        self.replace_query(next, window, cx);
    }

    /// `browseUp`: the parent folder, keeping a clone's own folder at the end.
    fn browse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.browse_target() else { return };
        let Some(parent) = parent_path(&target.directory) else { return };
        let next = match self.session.as_ref().and_then(|s| s.flow.as_ref()) {
            Some(ProjectFlow::CloneDestination { pinned, .. }) => clone_destination(&parent, pinned),
            _ => parent,
        };
        self.replace_query(next, window, cx);
    }

    /// Tab on a path (`handleKeyDown`'s Tab branch).
    fn complete_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.browse_target() else { return };
        let highlighted = self.session.as_ref().and_then(|s| s.highlighted.clone());
        if highlighted.as_deref() == Some("browse:up") {
            return self.browse_up(window, cx);
        }
        let Some(Ok(listing)) = self.current_listing(&target) else { return };
        let (visible, _) = self.visible_entries(&target, listing);
        let highlighted_entry = highlighted.as_ref().and_then(|value| visible.iter().find(|e| value.strip_prefix("browse:") == Some(e.full_path.as_ref())));
        let completion = match highlighted_entry {
            Some(entry) => Some((entry.name.clone(), true)),
            None => tab_completion(&visible, &target.filter),
        };
        let pinned = matches!(self.session.as_ref().and_then(|s| s.flow.as_ref()), Some(ProjectFlow::CloneDestination { .. }));
        match completion {
            Some((name, true)) => self.browse_to(name, window, cx),
            Some((name, false)) if !pinned => self.replace_query(format!("{}{}", target.directory, name), window, cx),
            _ => {}
        }
    }

    /// Enter (or Mod+Enter with `force`) on an Add project page. Returns whether it handled it.
    fn confirm_flow(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(session) = &self.session else { return false };
        let highlighted = session.highlighted.clone();
        match &session.flow {
            Some(ProjectFlow::CloneRepository { .. }) => {
                self.submit_clone_repository(window, cx);
                return true;
            }
            Some(ProjectFlow::NewProject { .. }) => {
                if highlighted.is_none() || force {
                    self.submit_new_project(window, cx);
                    return true;
                }
                return false;
            }
            _ => {}
        }
        if self.browse_target().is_none() {
            return false;
        }
        let on_row = highlighted.as_ref().is_some_and(|h| h.starts_with("browse:"));
        if on_row && !force {
            return false;
        }
        self.submit_path(window, cx);
        true
    }

    /// The path a submit uses (`resolvedAddProjectPath`) and whether it names a folder that
    /// does not exist yet (`willCreateProjectPath`).
    fn resolved_path(&self) -> Option<(FlowEnvironment, String, bool)> {
        let target = self.browse_target()?;
        let session = self.session.as_ref()?;
        let query = session.search.query.trim().to_string();
        let on_row = session.highlighted.as_ref().is_some_and(|h| h.starts_with("browse:"));
        let listing = self.current_listing(&target);
        let ok_listing = listing.and_then(|l| l.as_ref().ok());
        let (path, missing) = if has_trailing_separator(&session.search.query) {
            match ok_listing {
                Some(listing) => (listing.parent_path.to_string(), false),
                None => (query.clone(), listing.is_some()),
            }
        } else {
            let exact = ok_listing.and_then(|l| self.visible_entries(&target, l).1);
            match exact {
                Some(entry) => (entry.full_path.to_string(), false),
                None => (query.clone(), ok_listing.is_some()),
            }
        };
        Some((target.env, path, missing && !on_row && !query.is_empty()))
    }

    fn submit_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((env, path, _)) = self.resolved_path() else { return };
        let Some(backend) = self.source.project_flows() else { return };
        let Some(session) = &mut self.session else { return };
        if path.is_empty() || session.busy {
            return;
        }
        let clone = match &session.flow {
            Some(ProjectFlow::CloneDestination { remote_url, .. }) => Some(remote_url.to_string()),
            _ => None,
        };
        session.busy = true;
        cx.notify();
        match clone {
            Some(remote_url) => {
                // The palette closes once the server takes the clone, then a draft opens when the
                // project arrives (`submitAddProjectCloneFlow`).
                let task = backend.start_clone(&env.id, remote_url, path, cx);
                cx.spawn_in(window, async move |this, cx| {
                    let started = task.await;
                    let wait = this
                        .update_in(cx, |this, window, cx| {
                            if let Some(session) = &mut this.session {
                                session.busy = false;
                            }
                            let (project, wait) = started?;
                            this.close(window, cx);
                            Some((project, wait))
                        })
                        .ok()
                        .flatten();
                    let Some((project, wait)) = wait else { return };
                    wait.await;
                    this.update(cx, |_, cx| cx.emit(PaletteEvent::NewThreadIn(project))).ok();
                })
                .detach();
            }
            None => {
                let task = backend.add_project(&env.id, path, cx);
                let threads = self.source.threads();
                self.finish(
                    async move {
                        task.await.map(|opened| match opened {
                            // `getLatestThreadForProject`: the project's most recently active thread.
                            FlowOpened::Existing(project) => match threads.iter().filter(|t| t.project.as_ref() == Some(&project)).max_by_key(|t| t.activity_at) {
                                Some(thread) => PaletteEvent::OpenThread { thread: thread.key.clone(), query: SharedString::default() },
                                None => PaletteEvent::NewThreadIn(project),
                            },
                            FlowOpened::Created(project) => PaletteEvent::NewThreadIn(project),
                        })
                    },
                    window,
                    cx,
                );
            }
        }
    }

    /// Waits for a submit: closes the palette and tells the app where to go, or stays open
    /// (the failure has toasted).
    fn finish(&mut self, task: impl std::future::Future<Output = Option<PaletteEvent>> + 'static, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let event = task.await;
            this.update_in(cx, |this, window, cx| {
                if let Some(session) = &mut this.session {
                    session.busy = false;
                }
                if let Some(event) = event {
                    this.close(window, cx);
                    cx.emit(event);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `submitAddProjectCloneFlow`'s repository step: a URL leads to the destination page; a
    /// host's `owner/name` is looked up first (`sourceControl.lookupRepository`).
    fn submit_clone_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        let Some(ProjectFlow::CloneRepository { env, source }) = &session.flow else { return };
        let input = session.search.query.trim().to_string();
        if input.is_empty() || session.busy {
            return;
        }
        let (env, source) = (env.clone(), source.clone());
        let Some(info) = session.flow_envs.iter().find(|e| e.id == env).cloned() else { return };
        let host = match source {
            CloneSource::Url => {
                let remote_url = normalize_clone_url(&input);
                self.show_clone_destination(&info, remote_url, input, None, window, cx);
                return;
            }
            CloneSource::Host(host) => host,
        };
        if let Some(message) = incomplete_owner_message(&host.kind, &input) {
            Toaster::push(ToastData::new(ToastKind::Error, t!("Repository name incomplete")).description(message).stacked(), cx);
            return;
        }
        let Some(backend) = self.source.project_flows() else { return };
        session.busy = true;
        cx.notify();
        let lookup = backend.lookup_repository(&env, &host, input.clone(), cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = lookup.await;
            this.update_in(cx, |this, window, cx| {
                let Some(session) = &mut this.session else { return };
                // A page left while the lookup ran keeps what it shows now.
                if !matches!(&session.flow, Some(ProjectFlow::CloneRepository { env: e, .. }) if *e == env) {
                    return;
                }
                session.busy = false;
                match result {
                    Ok(repository) => {
                        let remote_url = repository.clone_url.to_string();
                        this.show_clone_destination(&info, remote_url, input, Some((host, repository)), window, cx);
                    }
                    Err(error) => {
                        let error = if error.is_empty() { t!("An error occurred.") } else { error };
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Repository lookup failed")).description(error).stacked(), cx);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// The destination step, its query the default clone destination (`getDefaultCloneDestination`).
    fn show_clone_destination(&mut self, info: &FlowEnvironment, remote_url: String, input: String, repository: Option<(RemoteSource, RepositoryInfo)>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        let folder = match &repository {
            Some((_, repo)) if !info.ghq_clone_layout => clone_directory_name(&repo.name_with_owner),
            _ => clone_directory_path(&remote_url, info.ghq_clone_layout),
        };
        let pinned = folder.rsplit('/').next().unwrap_or_default().to_string();
        let destination = clone_destination(&initial_query(&info.base_directory), &folder);
        session.flow = Some(ProjectFlow::CloneDestination { env: info.id.clone(), remote_url: remote_url.into(), input: input.into(), pinned: pinned.into(), repository });
        self.replace_query(destination, window, cx);
    }

    fn submit_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(backend) = self.source.project_flows() else { return };
        let Some(session) = &mut self.session else { return };
        let Some(ProjectFlow::NewProject { env, .. }) = &session.flow else { return };
        let name = session.search.query.trim().to_string();
        if name.is_empty() || session.busy {
            return;
        }
        let env = env.clone();
        let publish = session.publishes.then(|| session.flow_envs.iter().find(|e| e.id == env).and_then(|e| e.publish_targets.first().cloned())).flatten();
        session.busy = true;
        self.refilter();
        cx.notify();
        let task = backend.create_new_project(&env, name, publish, cx);
        self.finish(async move { task.await.map(PaletteEvent::NewThreadIn) }, window, cx);
    }

    /// `switchNewProjectEnvironment`.
    fn switch_new_project_environment(&mut self, id: EnvironmentId, cx: &mut Context<Self>) {
        let Some(session) = &mut self.session else { return };
        if let Some(ProjectFlow::NewProject { env, .. }) = &mut session.flow {
            *env = id;
        }
        session.highlighted = None;
        self.refilter();
        cx.notify();
    }

    /// `showExistingProjectSources`: back to the Sources page of the new project's machine.
    fn show_existing_project_sources(&mut self, env: EnvironmentId, window: &mut Window, cx: &mut Context<Self>) {
        let came_from = match self.session.as_ref().and_then(|s| s.flow.as_ref()) {
            Some(ProjectFlow::NewProject { sources, .. }) => sources.clone(),
            _ => return,
        };
        self.pop_view(window, cx);
        if came_from.as_ref() == Some(&env) {
            return;
        }
        if came_from.is_some() {
            self.pop_view(window, cx);
        }
        let info = self
            .session
            .as_ref()
            .and_then(|s| s.flow_envs.iter().find(|e| e.id == env))
            .map(|e| (e.new_projects_root.is_some(), e.remote_sources.clone()))
            .unwrap_or_default();
        let Some(session) = &mut self.session else { return };
        session.search.push(Rc::new(sources_view(&env, info)));
        session.highlighted = None;
        session.highlight_cleared = true;
        self.sync_input(window, cx);
        self.refilter();
        cx.notify();
    }

    /// The input's end padding: room for the submit button (`browseInputEndPaddingClass` and the
    /// `pe-32`/`pe-38` classes), else the input's own.
    fn input_end_padding(&self, theme: &AppTheme) -> gpui_kit::Pixels {
        let Some(session) = &self.session else { return theme.sp(3.) - px(1.) };
        let highlighted = session.highlighted.is_some();
        match &session.flow {
            Some(ProjectFlow::NewProject { .. }) if highlighted => theme.sp(38.),
            Some(ProjectFlow::NewProject { .. } | ProjectFlow::CloneRepository { .. }) => theme.sp(32.),
            _ => match self.resolved_path() {
                Some((_, _, true)) => theme.sp(38.),
                Some(_) if session.highlighted.as_ref().is_some_and(|h| h.starts_with("browse:")) => theme.sp(30.),
                Some(_) => theme.sp(24.),
                None => theme.sp(3.) - px(1.),
            },
        }
    }

    /// The summaries, status line, empty message and footer label an Add project page shows.
    fn flow_chrome(&self, theme: &AppTheme) -> FlowChrome {
        let mut chrome = FlowChrome { footer_action: Some(t!("Select")), ..FlowChrome::default() };
        let Some(session) = &self.session else { return chrome };
        let rem = theme.fonts.interface;
        let row = theme.sp(3.) + px(TextSize::Sm.line_height(rem)) + px(TextSize::Xs.line_height(rem));
        let status_height = theme.sp(6.) + px(TextSize::Sm.line_height(rem));
        match &session.flow {
            Some(ProjectFlow::NewProject { env, .. }) => {
                chrome.footer_action = Some(match session.highlighted.as_deref() {
                    None => t!("Create"),
                    Some("new-project:publish") => t!("Toggle"),
                    Some(_) => t!("Select"),
                });
                let info = session.flow_envs.iter().find(|e| &e.id == env);
                if let Some(root) = info.and_then(|e| e.new_projects_root.clone()) {
                    let name = session.search.query.trim();
                    let machine = (self.new_project_environments().len() > 1).then(|| info.map(|e| e.label.clone())).flatten();
                    let preview = clone_destination(&root, &new_project_folder_name(name));
                    let description = match (name.is_empty(), machine) {
                        (false, None) => t!("Creates {path}", path = preview),
                        (false, Some(environment)) => t!("Creates {path} on {environment}", path = preview, environment = environment),
                        (true, None) => t!("Goes in {root}", root = root),
                        (true, Some(environment)) => t!("Goes in {root} on {environment}", root = root, environment = environment),
                    };
                    let title: SharedString = if name.is_empty() { t!("New project") } else { name.to_string().into() };
                    chrome.header = Some(
                        div()
                            .p(theme.sp(2.))
                            .pb_0()
                            .child(render_summary_row(IconName::FolderGit2, title, description, theme))
                            .into_any_element(),
                    );
                    chrome.header_height = theme.sp(2.) + row;
                }
                return chrome;
            }
            Some(ProjectFlow::CloneRepository { source: CloneSource::Url, .. }) => {
                chrome.footer_action = Some(t!("Continue"));
                chrome.empty = Some(t!("Enter a Git clone URL and press Enter to continue."));
                return chrome;
            }
            Some(ProjectFlow::CloneRepository { source: CloneSource::Host(_), .. }) => {
                chrome.footer_action = Some(t!("Lookup"));
                chrome.empty = Some(t!("Enter a repository path and press Enter to look it up."));
                return chrome;
            }
            Some(ProjectFlow::CloneDestination { remote_url, input, repository, .. }) => {
                let summary = match repository {
                    Some((host, repo)) => render_summary_row(
                        crate::source_control_presentation::source_control_icon(crate::source_control_presentation::source_control_definition(Some(&host.kind))),
                        repo.name_with_owner.clone(),
                        repo.url.clone(),
                        theme,
                    ),
                    None => render_summary_row(IconName::Link, input.clone(), remote_url.clone(), theme),
                };
                chrome.header = Some(
                    div()
                        .flex_none()
                        .p(theme.sp(2.))
                        .pb_0()
                        .child(render_label(t!("Repository"), true, theme))
                        .child(summary)
                        .into_any_element(),
                );
                let label = row_height(&PaletteRow::Label { label: SharedString::default(), first: true }, theme);
                chrome.header_height = theme.sp(2.) + label + row;
                chrome.empty = Some(t!("Choose a destination path and press Enter to clone."));
            }
            _ => {}
        }
        let Some(target) = self.browse_target() else { return chrome };
        let on_row = session.highlighted.as_ref().is_some_and(|h| h.starts_with("browse:"));
        chrome.footer_action = on_row.then(|| t!("Select"));
        match self.current_listing(&target) {
            None if session.listing.is_none() => {
                chrome.status = Some(t!("Loading folders..."));
                chrome.header_height += status_height;
            }
            _ => {}
        }
        if chrome.empty.is_none() && self.resolved_path().is_some_and(|(_, _, creates)| creates) {
            chrome.empty = Some(t!("Press Enter to create this folder and add it as a project."));
        }
        chrome
    }

    /// The input's submit button (`CommandPaletteSubmitButton`), when the page has one.
    fn submit_button(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Option<(SharedString, SharedString, bool)> {
        let session = self.session.as_ref()?;
        let highlighted = session.highlighted.is_some();
        let mod_enter: SharedString = if cfg!(target_os = "macos") { "\u{2318} Enter".into() } else { "Ctrl Enter".into() };
        let _ = (theme, cx);
        match &session.flow {
            Some(ProjectFlow::NewProject { .. }) => {
                let label = if session.busy { t!("Creating") } else { t!("Create") };
                let enabled = !session.busy && !session.search.query.trim().is_empty();
                Some((label, if highlighted { mod_enter } else { "Enter".into() }, enabled))
            }
            Some(ProjectFlow::CloneRepository { source, .. }) => {
                let label = match (source, session.busy) {
                    (_, true) => t!("Working"),
                    (CloneSource::Url, false) => t!("Continue"),
                    (CloneSource::Host(_), false) => t!("Lookup"),
                };
                Some((label, "Enter".into(), !session.busy && !session.search.query.trim().is_empty()))
            }
            _ => {
                let (_, _, creates) = self.resolved_path()?;
                let clone = matches!(session.flow, Some(ProjectFlow::CloneDestination { .. }));
                let on_row = session.highlighted.as_ref().is_some_and(|h| h.starts_with("browse:"));
                let label = match (clone, creates) {
                    (true, _) if session.busy => t!("Cloning"),
                    (true, true) => t!("Create & Clone"),
                    (true, false) => t!("Clone"),
                    (false, true) => t!("Create & Add"),
                    (false, false) => t!("Add"),
                };
                Some((label, if on_row { mod_enter } else { "Enter".into() }, !(clone && session.busy)))
            }
        }
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
        .rounded(rems(0.25))
        .bg(with_alpha(theme.colors.foreground(), 0.08))
        .text_color(theme.colors.foreground())
        .ui_text(theme, TextSize::Xs, weight::MEDIUM)
        .child(content)
}

/// `CommandFooter`: arrows to navigate, Enter, Backspace on pages, Esc.
fn render_footer(in_submenu: bool, action: Option<SharedString>, theme: &AppTheme) -> impl IntoElement + use<> {
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
                .when_some(action, |d, action| d.child(group(vec![text_kbd("Enter")], action)))
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
                            .glass(theme.derived.glass_dialog_backdrop)
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close(window, cx))),
                    )
                    .child(div().absolute().size_full().flex().flex_col().items_center().px(theme.sp(4.)).pt(top).child(popup)),
            ),
        )
        .with_priority(4)
        .into_any_element()
    }
}
