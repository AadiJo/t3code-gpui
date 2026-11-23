//! Palette items, groups and filtering. Ports `CommandPalette.logic.ts`:
//! the root groups, the search across commands, threads, projects and
//! settings, submenu fallbacks, row building and the view stack.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Hsla, SharedString, Window};
use t3code_shared::keybindings::{KeybindingCommand, THREAD_JUMP_COMMANDS};
use t3code_shared::palette_search::{
    PALETTE_SECONDARY_LIMIT, PALETTE_THREAD_LIMIT, RankKey, SearchMatchHint, SearchQuery, SearchText,
    ThreadSearchMatch, fold_search_query, rank_search_groups, score_search_terms, score_thread_search,
};

use super::command_palette_lookups::PaletteThread;
use super::icons::IconName;
use super::sidebar::model::{EnvironmentId, ProjectRef, ThreadKey};
use crate::i18n::t;

/// Recent threads shown on the root page.
pub const RECENT_THREAD_LIMIT: usize = 12;

/// Group ids the filter treats specially.
pub mod group_ids {
    pub const ACTIONS: &str = "actions";
    pub const RECENT_THREADS: &str = "recent-threads";
    pub const NESTED_SEARCH: &str = "nested-search";
    pub const THREADS_SEARCH: &str = "threads-search";
    pub const PROJECTS_SEARCH: &str = "projects-search";
    pub const SETTINGS_SEARCH: &str = "settings-search";
}

/// A row's title. "New thread in **project**" sets the project in bold.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteTitle {
    Text(SharedString),
    NewThreadIn { project: SharedString },
}

/// What the app should do; the palette emits these as events.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteEvent {
    OpenThread { thread: ThreadKey, query: SharedString },
    NewThreadIn(ProjectRef),
    NewThreadWithoutProject(EnvironmentId),
    Triage,
    CopyPromptLink { with_context: bool },
    LinkPullRequest,
    ShowLinkedItems,
    ShareWithGuest,
    RestartAgentSession,
    NewProject,
    AddProject(AddProjectStep),
    OpenPullRequests,
    OpenIssues,
    UseInAnotherApp,
    OpenSettings,
    OpenSetting(SharedString),
    ToggleProjectPin(ProjectRef),
    OpenProjectSettings(ProjectRef),
    PortForward(PortForwardRequest),
}

/// Where Add project goes after the palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddProjectStep {
    ConnectEnvironment,
    BrowseFolders(EnvironmentId),
    CloneRepository(EnvironmentId),
}

/// A port forwarding request from the Forwarded ports page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortForwardRequest {
    Start { environment_id: EnvironmentId, port: u16 },
    Open { environment_id: EnvironmentId, port: u16 },
    CopyAddress { environment_id: EnvironmentId, port: u16 },
    Stop { environment_id: EnvironmentId, port: u16 },
}

/// What running an action row does.
#[derive(Clone)]
pub enum PaletteRun {
    /// Dispatch the command's action from the focus the palette opened from.
    Command(KeybindingCommand),
    /// Tell the app.
    Event(PaletteEvent),
    /// Something the palette does itself, such as switching the appearance.
    Local(Rc<dyn Fn(&mut Window, &mut App)>),
}

/// What a row draws after its title.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteTrailing {
    /// "Current", for the theme or mode in use.
    Current,
    /// Theme preview circles, optionally after "Current".
    Swatches { current: bool, colors: Vec<[Hsla; 2]> },
}

/// A row action or a page.
#[derive(Clone)]
pub enum PaletteItemKind {
    Action { run: PaletteRun, keep_open: bool },
    Submenu(Rc<PaletteView>),
}

/// One palette row (`CommandPaletteItem`).
#[derive(Clone)]
pub struct PaletteItem {
    pub value: SharedString,
    pub title: PaletteTitle,
    /// The name first, then keywords. Scored when `thread` is `None`.
    pub terms: Rc<[SearchText]>,
    /// Whether `terms[0]` is the title as shown, so its ranges highlight it.
    pub title_is_first_term: bool,
    pub description: Option<SharedString>,
    pub icon: IconName,
    pub disabled: bool,
    /// The command has no handler where the palette opened.
    pub unavailable: bool,
    pub shortcut: Option<KeybindingCommand>,
    /// Listed last in its group, and never decides where the group sits.
    pub secondary: bool,
    pub timestamp: Option<SharedString>,
    pub thread: Option<Arc<PaletteThread>>,
    pub trailing: Option<PaletteTrailing>,
    pub kind: PaletteItemKind,
}

impl PaletteItem {
    /// An action row. `search_terms` are its keywords, first the main one.
    pub fn action(value: impl Into<SharedString>, title: PaletteTitle, icon: IconName, run: PaletteRun, search_terms: &[&str]) -> Self {
        let mut item = Self {
            value: value.into(),
            title,
            terms: Rc::from([]),
            title_is_first_term: false,
            description: None,
            icon,
            disabled: false,
            unavailable: false,
            shortcut: None,
            secondary: false,
            timestamp: None,
            thread: None,
            trailing: None,
            kind: PaletteItemKind::Action { run, keep_open: false },
        };
        item.set_terms(search_terms.iter().map(|t| SharedString::from(t.to_string())));
        item
    }

    /// A row that opens a page.
    pub fn submenu(value: impl Into<SharedString>, title: SharedString, icon: IconName, view: PaletteView, search_terms: &[&str]) -> Self {
        let mut item = Self::action(value, PaletteTitle::Text(title), icon, PaletteRun::Event(PaletteEvent::OpenSettings), &[]);
        item.kind = PaletteItemKind::Submenu(Rc::new(view));
        item.set_terms(search_terms.iter().map(|t| SharedString::from(t.to_string())));
        item
    }

    /// Sets the keywords; the title joins them first when it differs
    /// (`scorePaletteItem`).
    pub fn set_terms(&mut self, keywords: impl IntoIterator<Item = SharedString>) {
        let name = self.search_name();
        let keywords: Vec<SharedString> = keywords.into_iter().collect();
        let mut terms = Vec::with_capacity(keywords.len() + 1);
        if keywords.first() != Some(&name) {
            terms.push(SearchText::new(name.as_ref()));
        }
        terms.extend(keywords.iter().map(|k| SearchText::new(k.as_ref())));
        self.title_is_first_term = matches!(self.title, PaletteTitle::Text(_));
        self.terms = terms.into();
    }

    /// The title as plain text, for search and drag labels.
    pub fn search_name(&self) -> SharedString {
        match &self.title {
            PaletteTitle::Text(text) => text.clone(),
            PaletteTitle::NewThreadIn { project } => t!("New thread in {project}", project = project.clone()),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn shortcut(mut self, command: impl Into<KeybindingCommand>) -> Self {
        self.shortcut = Some(command.into());
        self
    }

    pub fn keep_open(mut self) -> Self {
        if let PaletteItemKind::Action { keep_open, .. } = &mut self.kind {
            *keep_open = true;
        }
        self
    }

    pub fn trailing(mut self, trailing: PaletteTrailing) -> Self {
        self.trailing = Some(trailing);
        self
    }

    pub fn is_submenu(&self) -> bool {
        matches!(self.kind, PaletteItemKind::Submenu(_))
    }

    /// Whether the row can be highlighted and run.
    pub fn is_enabled(&self) -> bool {
        !self.disabled && !self.unavailable
    }
}

/// A labelled list of rows.
#[derive(Clone)]
pub struct PaletteGroup {
    pub value: SharedString,
    pub label: SharedString,
    pub items: Vec<Rc<PaletteItem>>,
}

/// A page: its groups and the icon by its back button.
#[derive(Clone)]
pub struct PaletteView {
    pub addon_icon: IconName,
    pub groups: Vec<PaletteGroup>,
    pub initial_query: Option<SharedString>,
}

/// The query and the page stack (`CommandPaletteSearchState`).
#[derive(Clone, Default)]
pub struct PaletteSearchState {
    pub query: SharedString,
    /// Each pushed page with the query it was opened from.
    pub views: Vec<(Rc<PaletteView>, SharedString)>,
}

impl PaletteSearchState {
    pub fn new(query: impl Into<SharedString>) -> Self {
        Self { query: query.into(), views: Vec::new() }
    }

    pub fn current_view(&self) -> Option<&Rc<PaletteView>> {
        self.views.last().map(|(view, _)| view)
    }

    /// Opens a page, starting from its own query.
    pub fn push(&mut self, view: Rc<PaletteView>) {
        let parent_query = std::mem::replace(&mut self.query, view.initial_query.clone().unwrap_or_default());
        self.views.push((view, parent_query));
    }

    /// Returns to the parent page and its query.
    pub fn back(&mut self) -> bool {
        match self.views.pop() {
            Some((_, parent_query)) => {
                self.query = parent_query;
                true
            }
            None => false,
        }
    }
}

/// A row after filtering, with the hint explaining a title-less match.
#[derive(Clone)]
pub struct ShownItem {
    pub item: Rc<PaletteItem>,
    pub hint: Option<SearchMatchHint>,
}

/// A group after filtering.
#[derive(Clone)]
pub struct ShownGroup {
    pub value: SharedString,
    pub label: SharedString,
    pub items: Vec<ShownItem>,
}

impl From<&PaletteGroup> for ShownGroup {
    fn from(group: &PaletteGroup) -> Self {
        Self {
            value: group.value.clone(),
            label: group.label.clone(),
            items: group.items.iter().map(|item| ShownItem { item: item.clone(), hint: None }).collect(),
        }
    }
}

/// Everything the filter searches besides the page's own groups.
pub struct FilterInput<'a> {
    pub active_groups: &'a [PaletteGroup],
    pub query: &'a str,
    pub is_in_submenu: bool,
    /// Root groups and every submenu option, for a submenu query with no match.
    pub root_groups: Option<&'a [PaletteGroup]>,
    pub nested_items: &'a [Rc<PaletteItem>],
    pub project_items: &'a [Rc<PaletteItem>],
    pub settings_items: &'a [Rc<PaletteItem>],
    pub thread_items: &'a [Rc<PaletteItem>],
    pub now_ms: i64,
}

/// A row's match: threads keep theirs to explain a match the title does not show.
fn score_item(item: &PaletteItem, query: &SearchQuery, now: i64, typos: bool) -> Option<(Option<ThreadSearchMatch>, RankKey)> {
    let (thread_match, mut key) = match &item.thread {
        Some(thread) => {
            let found = score_thread_search(query, &thread.subject, now, typos, true)?;
            let key = RankKey::from(&found);
            (Some(found), key)
        }
        None => (None, score_search_terms(query, &item.terms, typos)?.key),
    };
    if item.secondary {
        key.group_quality = Some(0.0);
    }
    Some((thread_match, key))
}

/// Filters and ranks the palette's groups for a query (`filterCommandPaletteGroups`).
/// Equal match classes put commands before threads, so "new" runs "New thread".
pub fn filter_command_palette_groups(input: &FilterInput<'_>) -> Vec<ShownGroup> {
    if let (Some(root_groups), true) = (input.root_groups, input.is_in_submenu) {
        let scoped = filter_command_palette_groups(&FilterInput { root_groups: None, ..*input });
        if !scoped.is_empty() || input.query.trim().is_empty() {
            return scoped;
        }
        return filter_command_palette_groups(&FilterInput {
            active_groups: root_groups,
            is_in_submenu: false,
            root_groups: None,
            ..*input
        });
    }
    let actions_only = input.query.starts_with('>');
    let folded = fold_search_query(if actions_only { &input.query[1..] } else { input.query });
    if folded.is_empty() {
        let groups = input.active_groups.iter().filter(|g| !actions_only || g.value == group_ids::ACTIONS);
        return groups.map(ShownGroup::from).collect();
    }
    let mut searchable: Vec<PaletteGroup> = input
        .active_groups
        .iter()
        .filter(|g| {
            if actions_only {
                g.value == group_ids::ACTIONS
            } else {
                input.is_in_submenu || g.value != group_ids::RECENT_THREADS
            }
        })
        .cloned()
        .collect();
    let searches_everything = !input.is_in_submenu && !actions_only;
    if searches_everything {
        let extra = [
            (group_ids::NESTED_SEARCH, t!("Suggestions"), input.nested_items),
            (group_ids::THREADS_SEARCH, t!("Threads"), input.thread_items),
            (group_ids::PROJECTS_SEARCH, t!("Projects"), input.project_items),
            (group_ids::SETTINGS_SEARCH, t!("Settings"), input.settings_items),
        ];
        for (value, label, items) in extra {
            if !items.is_empty() {
                searchable.push(PaletteGroup { value: value.into(), label, items: items.to_vec() });
            }
        }
    }
    let query = SearchQuery::new(&folded);
    let sizes: Vec<usize> = searchable.iter().map(|g| g.items.len()).collect();
    let ranked = rank_search_groups(&sizes, |group, item, typos| score_item(&searchable[group].items[item], &query, input.now_ms, typos));
    let threads_match = ranked.iter().any(|(group, _)| searchable[*group].value == group_ids::THREADS_SEARCH);
    ranked
        .into_iter()
        .map(|(group_index, mut items)| {
            let group = &searchable[group_index];
            let limit = if group.value == group_ids::THREADS_SEARCH {
                PALETTE_THREAD_LIMIT
            } else if group.value == group_ids::NESTED_SEARCH || (searches_everything && threads_match) {
                PALETTE_SECONDARY_LIMIT
            } else {
                items.len()
            };
            // Secondary rows go last; the sort is stable, so score order holds.
            items.sort_by_key(|(item, _, _)| group.items[*item].secondary);
            items.truncate(limit);
            let items = items
                .into_iter()
                .map(|(index, thread_match, _)| {
                    let item = group.items[index].clone();
                    // Hints are built only for the rows shown.
                    let hint = thread_match
                        .zip(item.thread.as_ref())
                        .and_then(|(found, thread)| found.explain(&thread.subject, query.as_str()));
                    ShownItem { item, hint }
                })
                .collect();
            ShownGroup { value: group.value.clone(), label: group.label.clone(), items }
        })
        .collect()
}

/// Every option inside a submenu as one row, such as "New thread in t3code"
/// or "Change theme: Graphite" (`buildNestedSearchItems`).
pub fn build_nested_search_items(items: &[Rc<PaletteItem>], query: &str) -> Vec<Rc<PaletteItem>> {
    let top_level: Vec<SharedString> = items.iter().map(|i| i.search_name()).collect();
    let folded_query = fold_search_query(query);
    let mut out = Vec::new();
    for parent in items {
        let PaletteItemKind::Submenu(view) = &parent.kind else { continue };
        let PaletteTitle::Text(title) = &parent.title else { continue };
        let stem = title.trim_end_matches("...").trim_end_matches('…');
        // A query that only names the submenu wants the submenu row itself.
        if fold_search_query(stem).starts_with(&folded_query) {
            continue;
        }
        let prefix = if stem.len() == title.len() { format!("{stem}: ") } else { format!("{} ", stem.trim_end()) };
        for group in &view.groups {
            for child in &group.items {
                if child.is_submenu() {
                    continue;
                }
                let combined: SharedString = format!("{prefix}{}", child.search_name()).into();
                if top_level.contains(&combined) {
                    continue;
                }
                let mut row = (**child).clone();
                row.title = PaletteTitle::Text(combined.clone());
                row.set_terms([combined]);
                out.push(Rc::new(row));
            }
        }
    }
    out
}

/// Gives the first nine rows the thread jump shortcuts (`enumerateCommandPaletteItems`).
pub fn enumerate_items(items: Vec<PaletteItem>) -> Vec<PaletteItem> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, mut item)| {
            item.shortcut = THREAD_JUMP_COMMANDS.get(index).map(|c| KeybindingCommand::Static(*c));
            item
        })
        .collect()
}

/// The root page: actions, then recent threads (`buildRootGroups`).
pub fn build_root_groups(actions: Vec<Rc<PaletteItem>>, recent_threads: Vec<Rc<PaletteItem>>) -> Vec<PaletteGroup> {
    let mut groups = Vec::new();
    if !actions.is_empty() {
        groups.push(PaletteGroup { value: group_ids::ACTIONS.into(), label: t!("Actions"), items: actions });
    }
    if !recent_threads.is_empty() {
        groups.push(PaletteGroup { value: group_ids::RECENT_THREADS.into(), label: t!("Recent Threads"), items: recent_threads });
    }
    groups
}

/// One list row: a group label or an item.
#[derive(Clone)]
pub enum PaletteRow {
    Label { label: SharedString, first: bool },
    /// `item_index` counts enabled rows only; disabled rows cannot be highlighted.
    Item { shown: ShownItem, item_index: Option<usize> },
}

/// Rows for the virtual list, the enabled rows' values, and each enabled
/// row's position in `rows` (`buildCommandPaletteRows`).
pub struct PaletteRows {
    pub rows: Vec<PaletteRow>,
    pub item_values: Vec<SharedString>,
    pub row_index_by_item: Vec<usize>,
}

pub fn build_rows(groups: &[ShownGroup]) -> PaletteRows {
    let mut rows = Vec::new();
    let mut item_values = Vec::new();
    let mut row_index_by_item = Vec::new();
    for group in groups {
        if !group.label.is_empty() {
            rows.push(PaletteRow::Label { label: group.label.clone(), first: rows.is_empty() });
        }
        for shown in &group.items {
            let item_index = shown.item.is_enabled().then(|| {
                item_values.push(shown.item.value.clone());
                row_index_by_item.push(rows.len());
                item_values.len() - 1
            });
            rows.push(PaletteRow::Item { shown: shown.clone(), item_index });
        }
    }
    PaletteRows { rows, item_values, row_index_by_item }
}

/// The enabled row with `value`, if shown.
pub fn find_item<'a>(groups: &'a [ShownGroup], value: &str) -> Option<&'a Rc<PaletteItem>> {
    groups.iter().flat_map(|g| g.items.iter()).map(|s| &s.item).find(|item| item.value == value && item.is_enabled())
}

/// Which input mode the palette is in (`CommandPaletteMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Root,
    Submenu,
}

/// The input placeholder (`getCommandPaletteInputPlaceholder`).
pub fn input_placeholder(mode: PaletteMode) -> SharedString {
    match mode {
        PaletteMode::Root => t!("Search commands, projects, and threads..."),
        PaletteMode::Submenu => t!("Search..."),
    }
}

/// What the palette shows when nothing matches.
pub fn empty_state_message(actions_only: bool) -> SharedString {
    if actions_only { t!("No matching actions.") } else { t!("No matching commands, projects, or threads.") }
}
