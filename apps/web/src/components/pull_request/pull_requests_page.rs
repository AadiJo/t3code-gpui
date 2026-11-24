//! The pull requests page: every environment's open pull requests in one
//! list, narrowed by state, involvement, typed filters and search, grouped
//! into authored, review requested and others, with saved views, the stack
//! column, quick actions and keyboard navigation. Ports `PullRequestsPage.tsx`,
//! `PullRequestsColumn.tsx`, `PullRequestRow.tsx` and `PullRequestListFilters.tsx`.
//!
//! Each environment loads on its own: one that fails shows a banner whose
//! Retry asks only the failed environments again. Filtering runs on rows that
//! carry their search text, and the list is virtualized.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, KeyBinding, ListAlignment, ListState, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div, list, prelude::FluentBuilder as _, px,
};
use serde::{Deserialize, Serialize};
use t3code_client_runtime::pull_request_query::{has_pull_request_filters, parse_pull_request_query};
use futures::StreamExt as _;
use t3code_contracts::{
    EnvironmentId, ProjectId, PullRequestAction, PullRequestActionInput, PullRequestInvolvement, PullRequestListFilters,
    PullRequestListFiltersChecks, PullRequestListFiltersDraft, PullRequestListFiltersReview, PullRequestListInput,
    PullRequestListResult, PullRequestListState, PullRequestRef,
};

use super::pull_request_list_logic::*;
use super::pull_request_presentation::*;
use super::pull_request_source::{PullRequestListSource, pull_request_ref};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::client_persistence_storage::LocalStorage;
use crate::components::saved_views_menu::{ApplySavedView, SavedViewsKind, SavedViewsList, SavedViewsMenu};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::popover::{Popover, PopupHandle};
use crate::components::ui::spinner::{Spinner, SpinnerSize};
use crate::components::ui::empty::Empty;
use crate::i18n::{t, t_rich};
use crate::{ActiveAppTheme as _, AppTheme};

/// Key context of the page.
pub const KEY_CONTEXT: &str = "PullRequestsPage";

actions!(pull_requests_page, [SelectNext, SelectPrevious, OpenSelected, FocusSearch]);

/// Registers the page's key bindings.
pub fn init(cx: &mut App) {
    let find = if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-f" };
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", OpenSelected, Some(KEY_CONTEXT)),
        KeyBinding::new(find, FocusSearch, Some(KEY_CONTEXT)),
    ]);
}

/// The list controls the page remembers and a saved view keeps, in the web's
/// stored shape (`t3.pullRequests.preferences`). Ports `PullRequestListPreferences`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListPreferences {
    pub involvement: PullRequestInvolvement,
    pub state: PullRequestListState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<EnvironmentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<ProjectId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<PullRequestListFiltersDraft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<PullRequestListFiltersReview>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<PullRequestListFiltersChecks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<PullRequestListSort>,
}

impl Default for PullRequestListPreferences {
    fn default() -> Self {
        Self {
            involvement: PullRequestInvolvement::All,
            state: PullRequestListState::Open,
            environment_id: None,
            project_id: None,
            host: None,
            q: None,
            draft: None,
            review: None,
            checks: None,
            author: None,
            labels: None,
            sort: None,
        }
    }
}

impl PullRequestListPreferences {
    /// `pullRequestListPreferences`: drops empty fields and the default sort.
    pub fn normalized(&self) -> Self {
        let text = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        Self {
            host: text(&self.host),
            q: text(&self.q),
            author: text(&self.author),
            labels: self.labels.clone().filter(|l| !l.is_empty()),
            sort: self.sort.filter(|s| *s != PullRequestListSort::Ready),
            ..self.clone()
        }
    }
}

/// The pull request list's saved views (`PULL_REQUEST_SAVED_VIEWS`).
pub struct PullRequestSavedViews;

impl SavedViewsKind for PullRequestSavedViews {
    type Preferences = PullRequestListPreferences;
    const STORAGE_KEY: &'static str = "t3.pullRequests.savedViews";
    const LIST: SavedViewsList = SavedViewsList::PullRequest;

    fn placeholder() -> SharedString {
        t!("PRs Requesting My Review")
    }

    fn suggested_name(p: &PullRequestListPreferences) -> SharedString {
        if p.involvement == PullRequestInvolvement::Reviewing { t!("PRs Requesting My Review") } else { SharedString::default() }
    }

    fn matches(view: &PullRequestListPreferences, current: &PullRequestListPreferences) -> bool {
        let key = |p: &PullRequestListPreferences| {
            let mut p = p.normalized();
            if let Some(labels) = &mut p.labels {
                labels.sort();
            }
            p
        };
        key(view) == key(current)
    }
}

/// Where the page remembers its controls between visits.
const PREFERENCES_KEY: &str = "t3.pullRequests.preferences";

/// The rows the host returns per page (`PAGE_SIZE`) and the most one listing grows to.
const PAGE_SIZE: u64 = 99;
const MAX_PAGE_SIZE: u64 = 500;

/// A project the Project filter offers.
#[derive(Clone, Debug)]
pub struct PullRequestProjectChoice {
    pub environment_id: EnvironmentId,
    pub project_id: ProjectId,
    pub title: SharedString,
}

/// Which filter submenu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterSubmenu {
    State,
    Involvement,
    Author,
    Labels,
    Draft,
    Review,
    Checks,
    Host,
    Server,
    Project,
}

/// One environment the page reads.
#[derive(Clone)]
pub struct EnvironmentListSource {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub source: Rc<dyn PullRequestListSource>,
}

/// Where one environment's listing is.
#[derive(Clone, Debug)]
pub enum EnvironmentLoad {
    Loading,
    Loaded(Arc<PullRequestListResult>),
    Failed(SharedString),
}

/// What the page asks its owner to do.
#[derive(Clone, Debug)]
pub enum PullRequestsPageEvent {
    /// Open the detail of a pull request, on the page's right panel.
    Open { environment_id: EnvironmentId, reference: PullRequestRef },
}

#[derive(Clone)]
enum ListItem {
    Header(PullRequestGroupKey, usize),
    Row(Row),
    /// The banner for an environment whose read failed while others answered.
    Failed(SharedString),
    /// The Load more row under a truncated listing.
    LoadMore,
}

/// How long the last refilter took, for the performance check.
#[derive(Clone, Copy, Debug, Default)]
pub struct FilterTiming {
    pub rows_in: usize,
    pub rows_out: usize,
    pub micros: u128,
}

/// The pull requests page.
pub struct PullRequestsPage {
    sources: Vec<EnvironmentListSource>,
    loads: BTreeMap<EnvironmentId, EnvironmentLoad>,
    /// Each environment's last answer, kept while a newer read travels.
    answers: BTreeMap<EnvironmentId, EnvironmentLoad>,
    merged: Option<MergedPullRequestList>,
    search: Entity<InputState>,
    query: String,
    state: PullRequestListState,
    involvement: PullRequestInvolvement,
    sort: PullRequestListSort,
    menu_filters: PullRequestListFilters,
    overrides: HashMap<String, PullRequestListOverride>,
    next_token: u64,
    items: Vec<ListItem>,
    rows: usize,
    selected: Option<SharedString>,
    list: ListState,
    host: Option<String>,
    environment: Option<EnvironmentId>,
    project: Option<(EnvironmentId, ProjectId)>,
    projects: Vec<PullRequestProjectChoice>,
    /// Whether the owner has said which projects exist; until then the page assumes some do.
    projects_known: bool,
    filters_submenu: Option<FilterSubmenu>,
    /// Rows asked of each host (`PAGE_SIZE` growing to `MAX_PAGE_SIZE`).
    page_size: u64,
    loading_more: bool,
    views_menu: Entity<SavedViewsMenu<PullRequestSavedViews>>,
    /// Fixed clock for scenes; `None` reads the system clock.
    pub now_ms: Option<i64>,
    pub last_filter: FilterTiming,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PullRequestsPageEvent> for PullRequestsPage {}

impl Focusable for PullRequestsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PullRequestsPage {
    /// A page over `sources`; it starts loading every environment at once.
    pub fn new(sources: Vec<EnvironmentListSource>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search pull requests, or label:bug")));
        let events = cx.subscribe_in(&search, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.refilter(cx);
            }
        });
        let remembered: PullRequestListPreferences = LocalStorage::get(cx, PREFERENCES_KEY).unwrap_or_default();
        let views_menu = cx.new(|cx| SavedViewsMenu::new(remembered.normalized(), cx));
        let apply = cx.subscribe_in(&views_menu, window, |this, _, event: &ApplySavedView<PullRequestListPreferences>, window, cx| {
            this.apply_preferences(&event.0, window, cx);
        });
        let subscriptions = vec![events, apply];
        // `useLiveRefresh`: an open list reads again whenever a host says it changed.
        for env in &sources {
            let mut ticks = env.source.subscribe_refreshes();
            cx.spawn(async move |this, cx| {
                let mut first = true;
                while let Some(tick) = ticks.next().await {
                    if tick.is_err() {
                        continue;
                    }
                    // The stream's first value is the current count, not a change.
                    if std::mem::take(&mut first) {
                        continue;
                    }
                    if this.update(cx, |this, cx| this.load(false, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        let mut page = Self {
            sources,
            loads: BTreeMap::new(),
            answers: BTreeMap::new(),
            merged: None,
            search,
            query: String::new(),
            state: PullRequestListState::Open,
            involvement: PullRequestInvolvement::All,
            sort: PullRequestListSort::Ready,
            menu_filters: PullRequestListFilters::default(),
            overrides: HashMap::new(),
            next_token: 0,
            items: Vec::new(),
            rows: 0,
            selected: None,
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            host: None,
            environment: None,
            project: None,
            projects: Vec::new(),
            projects_known: false,
            filters_submenu: None,
            page_size: PAGE_SIZE,
            loading_more: false,
            views_menu,
            now_ms: None,
            last_filter: FilterTiming::default(),
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        page.restore(&remembered, window, cx);
        page.load(false, cx);
        page
    }

    /// The projects the Project filter offers, from the owner's shell.
    pub fn set_projects(&mut self, projects: Vec<PullRequestProjectChoice>, cx: &mut Context<Self>) {
        self.projects = projects;
        self.projects_known = true;
        cx.notify();
    }

    /// Moves keyboard focus to the search box.
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Moves keyboard focus to the list, for arrow-key navigation.
    pub fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    /// The selected row's key.
    pub fn selected_key(&self) -> Option<SharedString> {
        self.selected.clone()
    }

    /// The saved views menu.
    pub fn views_menu(&self) -> &Entity<SavedViewsMenu<PullRequestSavedViews>> {
        &self.views_menu
    }

    fn list_input(&self, cursors: Option<BTreeMap<String, String>>) -> PullRequestListInput {
        PullRequestListInput {
            state: self.state,
            involvement: None,
            filters: None,
            project_id: None,
            project_ids: None,
            host: None,
            limit: Some(self.page_size),
            cursors,
            query: None,
        }
    }

    /// Loads every environment, or with `only_failed` only those whose last load failed.
    pub fn load(&mut self, only_failed: bool, cx: &mut Context<Self>) {
        let input = self.list_input(None);
        for env in self.sources.clone() {
            let failed = matches!(self.loads.get(&env.environment_id), Some(EnvironmentLoad::Failed(_)));
            if only_failed && !failed {
                continue;
            }
            self.loads.insert(env.environment_id.clone(), EnvironmentLoad::Loading);
            self.spawn_list(env, input.clone(), false, cx);
        }
        cx.notify();
    }

    /// Asks one environment for a page; `append` adds its rows to the ones already held.
    fn spawn_list(&mut self, env: EnvironmentListSource, input: PullRequestListInput, append: bool, cx: &mut Context<Self>) {
        let request = env.source.list(input);
        let id = env.environment_id;
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                let load = match result {
                    Ok(mut answer) => {
                        if append && let Some(EnvironmentLoad::Loaded(held)) = this.answers.get(&id) {
                            let mut entries = held.entries.clone();
                            entries.extend(answer.entries);
                            answer.entries = entries;
                        }
                        let answer = Arc::new(answer);
                        this.answers.insert(id.clone(), EnvironmentLoad::Loaded(answer.clone()));
                        EnvironmentLoad::Loaded(answer)
                    }
                    Err(error) => EnvironmentLoad::Failed(error.0),
                };
                this.loads.insert(id, load);
                this.loading_more = this.loading_more && this.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
                this.remerge(cx);
            })
            .ok();
        })
        .detach();
    }

    /// `loadMore`: continues from each host's cursor, or asks for a longer first page.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(merged) = &self.merged else { return };
        let cursors = merged.next_cursors.clone();
        self.loading_more = true;
        if cursors.is_empty() {
            self.page_size = (self.page_size + PAGE_SIZE).min(MAX_PAGE_SIZE);
            self.load(false, cx);
            return;
        }
        for env in self.sources.clone() {
            let Some(cursor) = cursors.get(env.environment_id.as_str()) else { continue };
            let input = self.list_input(Some(cursor.clone()));
            self.loads.insert(env.environment_id.clone(), EnvironmentLoad::Loading);
            self.spawn_list(env, input, true, cx);
        }
        cx.notify();
    }

    /// Environments still loading and those that failed, with their messages.
    pub fn load_states(&self) -> &BTreeMap<EnvironmentId, EnvironmentLoad> {
        &self.loads
    }

    fn remerge(&mut self, cx: &mut Context<Self>) {
        let answers: Vec<(EnvironmentId, Arc<PullRequestListResult>)> = self
            .sources
            .iter()
            .filter_map(|s| match self.answers.get(&s.environment_id) {
                Some(EnvironmentLoad::Loaded(answer)) => Some((s.environment_id.clone(), answer.clone())),
                _ => None,
            })
            .collect();
        self.merged = merge_pull_request_lists(&answers);
        if let Some(merged) = &self.merged {
            let now = self.now() as f64;
            self.overrides = settle_pull_request_overrides(&self.overrides, &merged.entries, now);
        }
        self.refilter(cx);
    }

    fn now(&self) -> i64 {
        self.now_ms.unwrap_or_else(crate::timestamp_format::now_ms)
    }

    /// The query typed into the search box.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.set_value(query.to_owned(), window, cx));
        self.query = query.to_owned();
        self.refilter(cx);
    }

    /// The filters the menus hold, laid under what the query types.
    pub fn set_menu_filters(&mut self, filters: PullRequestListFilters, cx: &mut Context<Self>) {
        self.menu_filters = filters;
        self.refilter(cx);
    }

    /// The list's state filter.
    pub fn set_state(&mut self, state: PullRequestListState, cx: &mut Context<Self>) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.page_size = PAGE_SIZE;
        self.load(false, cx);
        self.refilter(cx);
    }

    /// The involvement filter.
    pub fn set_involvement(&mut self, involvement: PullRequestInvolvement, cx: &mut Context<Self>) {
        self.involvement = involvement;
        self.refilter(cx);
    }

    /// The order within groups.
    pub fn set_sort(&mut self, sort: PullRequestListSort, cx: &mut Context<Self>) {
        self.sort = sort;
        self.refilter(cx);
    }

    /// The controls as the page remembers them and a saved view keeps them.
    pub fn preferences(&self) -> PullRequestListPreferences {
        let labels = self.menu_filters.labels.as_ref().map(|g| g.iter().flatten().cloned().collect());
        PullRequestListPreferences {
            involvement: self.involvement,
            state: self.state,
            environment_id: self.project.as_ref().map(|(e, _)| e.clone()).or_else(|| self.environment.clone()),
            project_id: self.project.as_ref().map(|(_, p)| p.clone()),
            host: self.host.clone(),
            q: (!self.query.is_empty()).then(|| self.query.clone()),
            draft: self.menu_filters.draft,
            review: self.menu_filters.review,
            checks: self.menu_filters.checks,
            author: self.menu_filters.author.clone(),
            labels,
            sort: Some(self.sort),
        }
        .normalized()
    }

    fn restore(&mut self, p: &PullRequestListPreferences, window: &mut Window, cx: &mut Context<Self>) {
        self.involvement = p.involvement;
        self.state = p.state;
        self.sort = p.sort.unwrap_or(PullRequestListSort::Ready);
        self.host = p.host.clone();
        self.project = p.environment_id.clone().zip(p.project_id.clone());
        self.environment = if self.project.is_some() { None } else { p.environment_id.clone() };
        self.menu_filters = PullRequestListFilters {
            draft: p.draft,
            review: p.review,
            checks: p.checks,
            labels: p.labels.as_ref().map(|l| l.iter().map(|n| vec![n.clone()]).collect()),
            excluded_labels: None,
            author: p.author.clone(),
        };
        let q = p.q.clone().unwrap_or_default();
        self.search.update(cx, |input, cx| input.set_value(q.clone(), window, cx));
        self.query = q;
    }

    /// Restores the controls a saved view kept.
    pub fn apply_preferences(&mut self, p: &PullRequestListPreferences, window: &mut Window, cx: &mut Context<Self>) {
        let state_changed = p.state != self.state;
        self.restore(p, window, cx);
        if state_changed {
            self.page_size = PAGE_SIZE;
            self.load(false, cx);
        }
        self.refilter(cx);
    }

    /// The host filter (`host` search param); `None` is every host.
    pub fn set_host(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        self.host = host;
        self.refilter(cx);
    }

    fn set_environment(&mut self, environment: Option<EnvironmentId>, cx: &mut Context<Self>) {
        self.environment = environment;
        self.project = None;
        self.refilter(cx);
    }

    fn set_project(&mut self, project: Option<(EnvironmentId, ProjectId)>, cx: &mut Context<Self>) {
        self.project = project;
        self.refilter(cx);
    }

    /// The typed qualifiers over the menu filters, the typed ones winning.
    fn effective_filters(&self) -> (String, PullRequestListFilters) {
        let parsed = parse_pull_request_query(&self.query);
        let m = &self.menu_filters;
        let t = parsed.filters;
        let labels = match (t.labels, m.labels.clone()) {
            (Some(mut a), Some(b)) => {
                a.extend(b);
                Some(a)
            }
            (a, b) => a.or(b),
        };
        let filters = PullRequestListFilters {
            draft: t.draft.or(m.draft),
            review: t.review.or(m.review),
            checks: t.checks.or(m.checks),
            labels,
            excluded_labels: t.excluded_labels.or(m.excluded_labels.clone()),
            author: t.author.or(m.author.clone()),
        };
        (parsed.text, filters)
    }

    /// Stores the controls for the next visit and shows them to the Views menu.
    fn remember(&mut self, cx: &mut Context<Self>) {
        let preferences = self.preferences();
        LocalStorage::set(cx, PREFERENCES_KEY, Some(&preferences));
        self.views_menu.update(cx, |menu, cx| menu.set_current(preferences, cx));
    }

    /// Recomputes the visible rows from the held answers. Runs per keystroke.
    pub fn refilter(&mut self, cx: &mut Context<Self>) {
        let started = Instant::now();
        let Some(merged) = self.merged.as_ref() else {
            self.items.clear();
            self.rows = 0;
            self.list.reset(0);
            self.remember(cx);
            cx.notify();
            return;
        };
        let (text, filters) = self.effective_filters();
        let known = apply_pull_request_overrides(&merged.entries, &self.overrides, self.state);
        let checks = filters.checks;
        let (_, groups) = pull_request_groups(PullRequestGroupsInput {
            known: &known,
            baseline: None,
            providers: &merged.providers,
            viewers: &merged.viewers,
            involvement: self.involvement,
            local_filters: &filters,
            has_local_filters: has_pull_request_filters(&filters),
            text: &text,
            answered_locally: false,
            baseline_empty: true,
            authored: None,
            reviewing: None,
            partitions_wanted: false,
        });
        let groups = sort_pull_request_groups(groups, self.sort, &text, self.involvement);
        // The contract's checks filter narrows on the host; rows carry the rollup, so narrow here too.
        let host = self.host.as_deref();
        let environment = self.environment.as_ref();
        let project = self.project.as_ref();
        let in_scope = |row: &Row| {
            host.is_none_or(|h| row.entry.host.eq_ignore_ascii_case(h))
                && environment.is_none_or(|e| &row.environment_id == e)
                && project.is_none_or(|(e, p)| &row.environment_id == e && &row.entry.project_id == p)
        };
        let keep = |row: &Row| in_scope(row) && match checks {
            None => true,
            Some(PullRequestListFiltersChecks::Passing) => row.entry.checks_state == Some(t3code_contracts::PullRequestChecksState::Passing),
            Some(PullRequestListFiltersChecks::Failing) => row.entry.checks_state == Some(t3code_contracts::PullRequestChecksState::Failing),
        };
        self.items.clear();
        self.rows = 0;
        let show_headers = groups.len() > 1 || groups.first().is_some_and(|g| g.key != PullRequestGroupKey::Others);
        for group in groups {
            let rows: Vec<Row> = group.entries.into_iter().filter(keep).collect();
            if rows.is_empty() {
                continue;
            }
            if show_headers {
                self.items.push(ListItem::Header(group.key, rows.len()));
            }
            self.rows += rows.len();
            self.items.extend(rows.into_iter().map(ListItem::Row));
        }
        if self.rows > 0
            && let Some(error) = self.list_error()
        {
            self.items.push(ListItem::Failed(error));
        }
        if merged.truncated && self.rows > 0 {
            self.items.push(ListItem::LoadMore);
        }
        self.list.reset(self.items.len());
        if self.selected.as_ref().is_some_and(|key| !self.items.iter().any(|i| matches!(i, ListItem::Row(r) if pull_request_entry_key(r) == key.as_ref()))) {
            self.selected = None;
        }
        self.last_filter = FilterTiming { rows_in: merged.entries.len(), rows_out: self.rows, micros: started.elapsed().as_micros() };
        self.remember(cx);
        cx.notify();
    }

    /// How many pull requests are shown.
    pub fn visible_count(&self) -> usize {
        self.rows
    }

    /// The keys of the rows shown, in order.
    pub fn visible_keys(&self) -> Vec<String> {
        self.items.iter().filter_map(|i| if let ListItem::Row(r) = i { Some(pull_request_entry_key(r)) } else { None }).collect()
    }

    fn row_indices(&self) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, i)| matches!(i, ListItem::Row(_))).map(|(ix, _)| ix).collect()
    }

    fn move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let rows = self.row_indices();
        if rows.is_empty() {
            return;
        }
        let current = self.selected.as_ref().and_then(|key| {
            rows.iter().position(|&ix| matches!(&self.items[ix], ListItem::Row(r) if pull_request_entry_key(r) == key.as_ref()))
        });
        let next = match current {
            None => if delta > 0 { 0 } else { rows.len() - 1 },
            Some(i) => (i as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize,
        };
        if let ListItem::Row(row) = &self.items[rows[next]] {
            self.selected = Some(pull_request_entry_key(row).into());
        }
        self.list.scroll_to_reveal_item(rows[next]);
        cx.notify();
    }

    fn open_row(&mut self, row: &Row, cx: &mut Context<Self>) {
        self.selected = Some(pull_request_entry_key(row).into());
        cx.emit(PullRequestsPageEvent::Open { environment_id: row.environment_id.clone(), reference: pull_request_ref(&row.entry) });
        cx.notify();
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.selected.clone() else { return };
        let row = self.items.iter().find_map(|i| match i {
            ListItem::Row(r) if pull_request_entry_key(r) == key.as_ref() => Some(r.clone()),
            _ => None,
        });
        if let Some(row) = row {
            self.open_row(&row, cx);
        }
    }

    /// Runs a row's quick action, showing its result at once and settling when the host agrees.
    pub fn run_quick_action(&mut self, row: &Row, action: PullRequestAction, cx: &mut Context<Self>) {
        let Some(source) = self.sources.iter().find(|s| s.environment_id == row.environment_id).map(|s| s.source.clone()) else { return };
        self.next_token += 1;
        let now = self.now();
        let iso = jiff::Timestamp::from_millisecond(now).map(|t| t.to_string()).unwrap_or_default();
        if let Some(o) = pull_request_override_after_action(&row.entry, action, now as f64, &iso, self.next_token, None) {
            self.overrides.insert(pull_request_entry_key(row), o);
        }
        let reference = pull_request_ref(&row.entry);
        let request = source.run_action(PullRequestActionInput {
            stack_number: None,
            expected_stack_heads: None,
            expected_head_sha: None,
            project_id: reference.project_id,
            host: reference.host,
            expected_account_id: None,
            allow_stale: None,
            repository: reference.repository,
            number: reference.number,
            action,
            merge_method: None,
            update_method: None,
            reply_with_summary: None,
        });
        let key = pull_request_entry_key(row);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if result.is_err() {
                    this.overrides.remove(&key);
                }
                this.load(false, cx);
            })
            .ok();
        })
        .detach();
        self.refilter(cx);
    }

    /// `listError`: the first failed environment's label and message.
    fn list_error(&self) -> Option<SharedString> {
        self.sources.iter().find_map(|s| match self.loads.get(&s.environment_id) {
            Some(EnvironmentLoad::Failed(message)) => Some(SharedString::from(format!("{}: {message}", s.label))),
            _ => None,
        })
    }

    // --- render ----------------------------------------------------------

    /// `PullRequestListEmptyState`: no projects, no match for the query, or nothing listed.
    fn render_empty(&self, theme: &AppTheme, can_continue: bool, cx: &mut Context<Self>) -> AnyElement {
        let refreshing = self.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
        let mark = gpui_kit::svg().path("icons/ui/branch-mark-open.svg").w(theme.sp(32.)).h(theme.sp(20.)).text_color(theme.colors.muted_foreground);
        let check_again = Button::new("pr-check-again")
            .variant(ButtonVariant::Outline)
            .size(ButtonSize::Sm)
            .icon(IconName::RefreshCw)
            .icon_size(IconSize::Sm)
            .label(if refreshing { t!("Checking...") } else { t!("Check again") })
            .disabled(refreshing)
            .on_click(cx.listener(|this, _, _, cx| this.load(false, cx)));
        let buttons = div().flex().flex_wrap().justify_center().gap(theme.sp(2.));
        if self.projects_known && self.projects.is_empty() {
            return Empty::new(t!("No projects in this workspace"))
                .media(mark)
                .description(t!("Add a project, and the pull requests from its repository appear here."))
                .into_any_element();
        }
        let query = self.query.trim();
        if !query.is_empty() {
            let shown: String = if query.chars().count() > 48 { query.chars().take(48).chain(['…']).collect() } else { query.to_owned() };
            return Empty::new(SharedString::from(t_rich!("Nothing matches “{shownQuery}”", shownQuery = shown.as_str()).plain()))
                .media(mark)
                .description(t!("The hosts were searched for it. Try fewer words, or search by number, author or branch."))
                .child(
                    buttons
                        .child(
                            Button::new("pr-clear-search")
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Sm)
                                .icon(IconName::Search)
                                .icon_size(IconSize::Sm)
                                .label(t!("Clear search"))
                                .on_click(cx.listener(|this, _, window, cx| this.set_query("", window, cx))),
                        )
                        .child(check_again),
                )
                .into_any_element();
        }
        let filtered = has_pull_request_filters(&self.menu_filters)
            || self.state != PullRequestListState::Open
            || self.involvement != PullRequestInvolvement::All
            || self.project.is_some()
            || self.host.is_some();
        let truncated = self.merged.as_ref().is_some_and(|m| m.truncated);
        Empty::new(if filtered { t!("Nothing under these filters") } else { t!("No pull requests") })
            .media(mark)
            .description(if filtered {
                t!("Widen the state, involvement or project filter to see more.")
            } else {
                t!("Pull requests from every project in this workspace appear here.")
            })
            .child(
                buttons
                    .when(truncated && can_continue, |b| {
                        b.child(
                            Button::new("pr-empty-load-more")
                                .variant(ButtonVariant::Outline)
                                .size(ButtonSize::Sm)
                                .label(if self.loading_more { t!("Loading...") } else { t!("Load more pull requests") })
                                .disabled(self.loading_more)
                                .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
                        )
                    })
                    .child(check_again),
            )
            .into_any_element()
    }

    /// The hosts the list read, as the provider filter offers them (`hostMenuOptions`).
    fn host_options(&self) -> Vec<FilterOption> {
        let providers = self.merged.as_ref().map(|m| m.providers.as_slice()).unwrap_or_default();
        let mut options = vec![FilterOption::new("", t!("All", context = "all hosts"), IconName::Plug2)];
        for provider in providers {
            let definition = crate::source_control_presentation::source_control_definition(Some(provider.kind.as_str()));
            let sharing = providers.iter().filter(|p| p.kind == provider.kind).count();
            let label: SharedString = if sharing > 1 { provider.host.clone().into() } else { definition.label.into() };
            let mut option = FilterOption::new(&provider.host, label, IconName::GitPullRequestArrow);
            if !provider.configured {
                option.unavailable = Some(provider.detail.clone().map(SharedString::from).unwrap_or_else(|| t!("This host could not be read.")));
            }
            options.push(option);
        }
        options
    }

    fn server_options(&self) -> Vec<FilterOption> {
        std::iter::once(FilterOption::new("", t!("All servers"), IconName::Layers))
            .chain(self.sources.iter().map(|s| FilterOption::new(s.environment_id.as_str(), s.label.clone(), IconName::Server)))
            .collect()
    }

    /// `projectOptions`: projects whose read failed are marked unavailable and listed last.
    fn project_options(&self) -> Vec<FilterOption> {
        let errors = self.merged.as_ref().map(|m| m.errors.as_slice()).unwrap_or_default();
        let mut projects: Vec<FilterOption> = self
            .projects
            .iter()
            .map(|p| {
                let mut option =
                    FilterOption::new(&format!("{}\u{0}{}", p.environment_id.as_str(), p.project_id.as_str()), p.title.clone(), IconName::FolderGit2);
                option.unavailable = errors
                    .iter()
                    .find(|(e, error)| e == &p.environment_id && error.project_id == p.project_id)
                    .map(|(_, error)| SharedString::from(error.message.clone()));
                option
            })
            .collect();
        projects.sort_by_key(|o| o.unavailable.is_some());
        std::iter::once(FilterOption::new("", t!("All projects"), IconName::Layers)).chain(projects).collect()
    }

    /// `filterCount`: every control away from its default.
    fn filter_count(&self) -> usize {
        let f = &self.menu_filters;
        [
            self.state != PullRequestListState::Open,
            self.involvement != PullRequestInvolvement::All,
            self.host.is_some(),
            self.environment.is_some(),
            self.project.is_some(),
            f.draft.is_some(),
            f.review.is_some(),
            f.checks.is_some(),
            f.author.is_some(),
        ]
        .into_iter()
        .filter(|on| *on)
        .count()
            + f.labels.iter().flatten().flatten().count()
    }

    fn set_filters_submenu(&mut self, submenu: Option<FilterSubmenu>, cx: &mut Context<Self>) {
        if self.filters_submenu != submenu {
            self.filters_submenu = submenu;
            cx.notify();
        }
    }

    fn render_toolbar(&self, theme: &AppTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let sort = self.sort;
        let sort_menu = {
            let this = this.clone();
            dropdown_menu(
                "pr-sort",
                Button::new("pr-sort-trigger").variant(ButtonVariant::Outline).icon(IconName::ArrowDownUp).label(t!("Sort", context = "verb, sort a list")),
                move |_, _| {
                    let mut menu = MenuPopup::new();
                    for option in SORT_MENU {
                        let this = this.clone();
                        menu = menu.item(
                            MenuItem::new(sort_label(option))
                                .icon(sort_icon(option))
                                .icon_size(IconSize::Sm)
                                .kind(MenuItemKind::Radio(option == sort))
                                .on_select(move |_, cx| this.update(cx, |p, cx| p.set_sort(option, cx))),
                        );
                    }
                    menu
                },
            )
        };
        let count = self.filter_count();
        let filters_trigger = Button::new("pr-filters-trigger")
            .variant(ButtonVariant::Outline)
            .icon(IconName::ListFilter)
            .label(t!("Filters"))
            .when(count > 0, |b| {
                b.accessory(
                    div()
                        .rounded_full()
                        .bg(theme.colors.muted)
                        .px(theme.sp(1.5))
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.muted_foreground)
                        .child(count.to_string()),
                )
            });
        let filters_menu = {
            let this = this.clone();
            Popover::new("pr-filters", filters_trigger, move |handle, _, cx| filters_popup(&this, handle, cx))
        };
        let host = self.host.clone();
        let host_options = self.host_options();
        let current_host = host_options.iter().find(|o| Some(o.value.as_ref()) == host.as_deref()).map(|o| (o.icon, o.label.clone()));
        let provider_trigger = match (&host, current_host) {
            (Some(_), Some((icon, _))) => Button::new("pr-provider-trigger").variant(ButtonVariant::Outline).size(ButtonSize::Icon).icon(icon),
            _ => Button::new("pr-provider-trigger")
                .variant(ButtonVariant::Outline)
                .icon(IconName::Plug2)
                .label(t!("All", context = "all providers")),
        };
        let provider_menu = {
            let this = this.clone();
            dropdown_menu("pr-provider", provider_trigger, move |_, _| {
                let mut menu = MenuPopup::new();
                for option in &host_options {
                    let this = this.clone();
                    let value = (!option.value.is_empty()).then(|| option.value.to_string());
                    let on = value == host;
                    menu = menu.item(
                        MenuItem::new(option.label.clone())
                            .icon(option.icon)
                            .icon_size(IconSize::Sm)
                            .kind(MenuItemKind::Radio(on))
                            .disabled(option.unavailable.is_some())
                            .on_select(move |_, cx| this.update(cx, |p, cx| p.set_host(value.clone(), cx))),
                    );
                }
                menu
            })
        };
        let refreshing = self.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(2.))
            .child(div().id("pr-search").min_w_0().flex_1().child(TextInput::new(&self.search).size(InputSize::Lg).leading_icon(IconName::Search)))
            .child(sort_menu)
            .child(filters_menu)
            .child(self.views_menu.clone())
            .child(provider_menu)
            .child(
                Button::new("pr-refresh")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Icon)
                    .icon(IconName::RefreshCw)
                    .disabled(refreshing)
                    .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
            )
    }

}

fn sort_label(sort: PullRequestListSort) -> SharedString {
    match sort {
        PullRequestListSort::Ready => t!("Merge readiness"),
        PullRequestListSort::Blocked => t!("Blocked on me"),
        PullRequestListSort::Updated => t!("Recently updated"),
        PullRequestListSort::Newest => t!("Newest shown"),
        PullRequestListSort::Oldest => t!("Oldest shown"),
        PullRequestListSort::Largest => t!("Largest shown"),
        PullRequestListSort::Smallest => t!("Smallest shown"),
    }
}

/// One choice in a filter submenu or the provider menu (`PullRequestFilterOption`).
#[derive(Clone)]
struct FilterOption {
    value: SharedString,
    label: SharedString,
    icon: IconName,
    unavailable: Option<SharedString>,
}

impl FilterOption {
    fn new(value: &str, label: SharedString, icon: IconName) -> Self {
        Self { value: SharedString::from(value.to_owned()), label, icon, unavailable: None }
    }
}

/// `SORT_OPTIONS`, in menu order.
const SORT_MENU: [PullRequestListSort; 7] = [
    PullRequestListSort::Ready,
    PullRequestListSort::Blocked,
    PullRequestListSort::Updated,
    PullRequestListSort::Newest,
    PullRequestListSort::Oldest,
    PullRequestListSort::Largest,
    PullRequestListSort::Smallest,
];

fn sort_icon(sort: PullRequestListSort) -> IconName {
    match sort {
        PullRequestListSort::Ready => IconName::ListChecks,
        PullRequestListSort::Blocked => IconName::UserLock,
        PullRequestListSort::Updated => IconName::Clock,
        PullRequestListSort::Newest => IconName::CalendarArrowDown,
        PullRequestListSort::Oldest => IconName::CalendarArrowUp,
        PullRequestListSort::Largest => IconName::Maximize2,
        PullRequestListSort::Smallest => IconName::Minimize2,
    }
}

const UNFILTERED: &str = "__all__";

/// The Filters menu: one submenu per control, the open one beside its row.
/// Ports `PullRequestFiltersMenu`.
fn filters_popup(this: &Entity<PullRequestsPage>, handle: PopupHandle, cx: &mut App) -> AnyElement {
    let page = this.read(cx);
    let theme = cx.app_theme().clone();
    let f = page.menu_filters.clone();
    let state_options = vec![
        FilterOption::new("all", t!("All", context = "pull request filter"), IconName::Layers),
        FilterOption::new("open", t!("Open", context = "pull request state"), IconName::GitPullRequestArrow),
        FilterOption::new("closed", t!("Closed", context = "pull request state"), IconName::GitPullRequestClosed),
        FilterOption::new("merged", t!("Merged", context = "pull request state"), IconName::GitMerge),
    ];
    let involvement_options = vec![
        FilterOption::new("all", t!("All", context = "pull request filter"), IconName::Layers),
        FilterOption::new("reviewing", t!("Reviewing", context = "pull request filter"), IconName::Eye),
        FilterOption::new("authored", t!("Authored", context = "pull request filter"), IconName::PenLine),
    ];
    let all = || FilterOption::new(UNFILTERED, t!("All", context = "pull request filter"), IconName::Layers);
    let draft_options = vec![
        all(),
        FilterOption::new("only", t!("Drafts only"), IconName::GitPullRequestDraft),
        FilterOption::new("hide", t!("Hide drafts"), IconName::EyeOff),
    ];
    let review_options = vec![
        all(),
        FilterOption::new("approved", t!("Approved", context = "review decision"), IconName::CircleCheck),
        FilterOption::new("changes-requested", t!("Changes requested"), IconName::CircleX),
        FilterOption::new("review-required", t!("Review required"), IconName::CircleDashed),
        FilterOption::new("none", t!("No reviews"), IconName::CircleSlash),
    ];
    let checks_options = vec![
        all(),
        FilterOption::new("passing", t!("Passing", context = "checks filter"), IconName::CircleCheck),
        FilterOption::new("failing", t!("Failing", context = "checks filter"), IconName::CircleX),
    ];
    let host_options = page.host_options();
    let server_options = page.server_options();
    let project_options = page.project_options();
    let facets = page.merged.as_ref().map(|m| collect_pull_request_list_facets(&m.entries, page.state)).unwrap_or_default();
    let state_value = page.state.as_str().to_owned();
    let involvement_value = page.involvement.as_str().to_owned();
    let draft_value = f.draft.map(|d| d.as_str().to_owned()).unwrap_or_else(|| UNFILTERED.into());
    let review_value = f.review.map(|r| r.as_str().to_owned()).unwrap_or_else(|| UNFILTERED.into());
    let checks_value = f.checks.map(|c| c.as_str().to_owned()).unwrap_or_else(|| UNFILTERED.into());
    let host_value = page.host.clone().unwrap_or_default();
    let server_value = page.environment.as_ref().map(|e| e.as_str().to_owned()).unwrap_or_default();
    let project_value = page.project.as_ref().map(|(e, p)| format!("{}\u{0}{}", e.as_str(), p.as_str())).unwrap_or_default();
    let open = page.filters_submenu;
    let selected_labels: Vec<String> = f.labels.iter().flatten().flatten().cloned().collect();

    // Rows of the main menu, top to bottom, for placing the open submenu.
    let mut offsets: Vec<(FilterSubmenu, Pixels)> = Vec::new();
    let mut y = Pixels::ZERO;
    let row_h = theme.sp(7.);
    let sep_h = theme.sp(2.) + px(1.);
    let mut menu = MenuPopup::new();
    let trigger = |menu: MenuPopup, y: &mut Pixels, offsets: &mut Vec<(FilterSubmenu, Pixels)>, submenu: FilterSubmenu, label: SharedString, options: &[FilterOption], value: &str| {
        let current = options.iter().find(|o| o.value.as_ref() == value).or(options.first());
        offsets.push((submenu, *y));
        *y += row_h;
        let hover = this.clone();
        let click = this.clone();
        let mut item = MenuItem::new(label)
            .id(format!("pr-filter-sub-{submenu:?}").to_lowercase())
            .kind(MenuItemKind::Submenu)
            .icon(current.map_or(IconName::Layers, |c| c.icon))
            .icon_size(IconSize::Sm)
            .detail(current.map(|c| c.label.clone()).unwrap_or_default())
            .on_hover(move |hovered, _, cx| {
                if *hovered {
                    hover.update(cx, |p, cx| p.set_filters_submenu(Some(submenu), cx));
                }
            })
            .on_select(move |_, cx| click.update(cx, |p, cx| p.set_filters_submenu(Some(submenu), cx)));
        if open == Some(submenu) {
            item = item.highlighted();
        }
        menu.item(item)
    };
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::State, t!("State", context = "pull request filter"), &state_options, &state_value);
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Involvement, t!("Involvement"), &involvement_options, &involvement_value);
    menu = menu.separator();
    y += sep_h;
    let author_options: Vec<FilterOption> = std::iter::once(FilterOption::new(UNFILTERED, t!("Anyone"), IconName::User))
        .chain(facets.0.iter().map(|a| FilterOption::new(&a.actor.login, a.actor.login.clone().into(), IconName::User)))
        .collect();
    let author_value = f.author.clone().unwrap_or_else(|| UNFILTERED.into());
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Author, t!("Author"), &author_options, &author_value);
    let labels_detail = if selected_labels.is_empty() {
        t!("Any", context = "no label filter")
    } else {
        t!("{count, plural, one {{count} selected} other {{count} selected}}", count = selected_labels.len())
    };
    let label_options = [FilterOption::new(UNFILTERED, labels_detail, IconName::Tag)];
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Labels, t!("Labels"), &label_options, UNFILTERED);
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Draft, t!("Draft", context = "pull request filter"), &draft_options, &draft_value);
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Review, t!("Review", context = "pull request filter"), &review_options, &review_value);
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Checks, t!("Checks", context = "pull request filter"), &checks_options, &checks_value);
    if host_options.len() > 2 {
        menu = menu.separator();
        y += sep_h;
        menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Host, t!("Host", context = "pull request filter"), &host_options, &host_value);
    }
    if server_options.len() > 2 {
        menu = menu.separator();
        y += sep_h;
        menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Server, t!("Server", context = "pull request filter"), &server_options, &server_value);
    }
    menu = menu.separator();
    y += sep_h;
    menu = trigger(menu, &mut y, &mut offsets, FilterSubmenu::Project, t!("Project", context = "pull request filter"), &project_options, &project_value);
    let close = handle.clone();
    let reset = this.clone();
    let menu = menu.on_dismiss(move |_, cx| {
        reset.update(cx, |p, cx| p.set_filters_submenu(None, cx));
        close.close(cx);
    });

    let submenu = open.map(|submenu| {
        let page = this.clone();
        let update = move |change: Box<dyn Fn(&mut PullRequestsPage, &str, &mut Context<PullRequestsPage>)>| {
            let page = page.clone();
            let change = Rc::new(change);
            move |value: SharedString| {
                let page = page.clone();
                let change = change.clone();
                move |_: &mut Window, cx: &mut App| page.update(cx, |p, cx| change(p, value.as_ref(), cx))
            }
        };
        let (label, options, value, pick): (SharedString, Vec<FilterOption>, String, _) = match submenu {
            FilterSubmenu::State => (t!("State", context = "pull request filter"), state_options, state_value, update(Box::new(|p, v, cx| {
                if let Some(s) = list_state_from(v) {
                    p.set_state(s, cx)
                }
            }))),
            FilterSubmenu::Involvement => (t!("Involvement"), involvement_options, involvement_value, update(Box::new(|p, v, cx| {
                if let Some(i) = involvement_from(v) {
                    p.set_involvement(i, cx)
                }
            }))),
            FilterSubmenu::Author => (t!("Author"), author_options, author_value, update(Box::new(|p, v, cx| {
                let author = (v != UNFILTERED).then(|| v.to_owned());
                p.set_menu_filters(PullRequestListFilters { author, ..p.menu_filters.clone() }, cx)
            }))),
            FilterSubmenu::Draft => (t!("Draft", context = "pull request filter"), draft_options, draft_value, update(Box::new(|p, v, cx| {
                let draft = PullRequestListFiltersDraft::ALL.iter().copied().find(|d| d.as_str() == v);
                p.set_menu_filters(PullRequestListFilters { draft, ..p.menu_filters.clone() }, cx)
            }))),
            FilterSubmenu::Review => (t!("Review", context = "pull request filter"), review_options, review_value, update(Box::new(|p, v, cx| {
                let review = PullRequestListFiltersReview::ALL.iter().copied().find(|r| r.as_str() == v);
                p.set_menu_filters(PullRequestListFilters { review, ..p.menu_filters.clone() }, cx)
            }))),
            FilterSubmenu::Checks => (t!("Checks", context = "pull request filter"), checks_options, checks_value, update(Box::new(|p, v, cx| {
                let checks = PullRequestListFiltersChecks::ALL.iter().copied().find(|c| c.as_str() == v);
                p.set_menu_filters(PullRequestListFilters { checks, ..p.menu_filters.clone() }, cx)
            }))),
            FilterSubmenu::Host => (t!("Host", context = "pull request filter"), host_options, host_value, update(Box::new(|p, v, cx| {
                p.set_host((!v.is_empty()).then(|| v.to_owned()), cx)
            }))),
            FilterSubmenu::Server => (t!("Server", context = "pull request filter"), server_options, server_value, update(Box::new(|p, v, cx| {
                p.set_environment((!v.is_empty()).then(|| EnvironmentId::new(v)), cx)
            }))),
            FilterSubmenu::Project => (t!("Project", context = "pull request filter"), project_options, project_value, update(Box::new(|p, v, cx| {
                let project = v.split_once('\u{0}').map(|(e, id)| (EnvironmentId::new(e), ProjectId::new(id)));
                p.set_project(project, cx)
            }))),
            FilterSubmenu::Labels => {
                // Labels toggle; the menu stays open between picks.
                let mut popup = MenuPopup::new().label(t!("Labels"));
                if facets.1.is_empty() {
                    popup = popup.item(MenuItem::new(t!("No labels found")).disabled(true));
                }
                for facet in facets.1.iter() {
                    let name = facet.label.name.clone();
                    let on = selected_labels.iter().any(|l| l.eq_ignore_ascii_case(&name));
                    let held = selected_labels.clone();
                    let page = this.clone();
                    popup = popup.item(MenuItem::new(SharedString::from(name.clone())).kind(MenuItemKind::Checkbox(on)).on_select(move |_, cx| {
                        let next: Vec<String> = if on {
                            held.iter().filter(|l| !l.eq_ignore_ascii_case(&name)).cloned().collect()
                        } else {
                            held.iter().cloned().chain([name.clone()]).take(10).collect()
                        };
                        page.update(cx, |p, cx| {
                            let labels = (!next.is_empty()).then(|| next.iter().map(|l| vec![l.clone()]).collect());
                            p.set_menu_filters(PullRequestListFilters { labels, ..p.menu_filters.clone() }, cx)
                        })
                    }));
                }
                return (submenu, popup);
            }
        };
        let mut popup = MenuPopup::new().label(label);
        for option in options {
            let on = option.value.as_ref() == value;
            popup = popup.item(
                MenuItem::new(option.label.clone())
                    .id(format!("pr-filter-opt-{}", option.value))
                    .icon(option.icon)
                    .icon_size(IconSize::Sm)
                    .kind(MenuItemKind::Radio(on))
                    .disabled(option.unavailable.is_some())
                    .detail(if option.unavailable.is_some() { format!("· {}", t!("Unavailable", context = "filter option")).into() } else { SharedString::default() })
                    .on_select(pick(option.value.clone())),
            );
        }
        let close = handle.clone();
        let reset = this.clone();
        (submenu, popup.on_dismiss(move |_, cx| {
            reset.update(cx, |p, cx| p.set_filters_submenu(None, cx));
            close.close(cx);
        }))
    });
    let mut row = div().flex().items_start().child(menu);
    if let Some((submenu, popup)) = submenu {
        let top = offsets.iter().find(|(s, _)| *s == submenu).map_or(Pixels::ZERO, |(_, y)| *y);
        // Base UI's sub popup: inline-end, aligned to its trigger row 5px up.
        row = row.child(div().mt(px(1.) + theme.sp(1.) + top - px(5.)).max_h(px(320.)).child(popup));
    }
    row.into_any_element()
}

fn list_state_from(value: &str) -> Option<PullRequestListState> {
    [PullRequestListState::Open, PullRequestListState::Closed, PullRequestListState::Merged, PullRequestListState::All].into_iter().find(|s| s.as_str() == value)
}

fn involvement_from(value: &str) -> Option<PullRequestInvolvement> {
    [PullRequestInvolvement::All, PullRequestInvolvement::Reviewing, PullRequestInvolvement::Authored].into_iter().find(|i| i.as_str() == value)
}

/// Height of a list row: `py-2.5` around two lines.
pub const ROW_HEIGHT: f32 = 57.;

fn render_row(
    row: &Row,
    selected: bool,
    now: i64,
    host_actions: &[PullRequestAction],
    page: &Entity<PullRequestsPage>,
    theme: &AppTheme,
) -> AnyElement {
    let entry = &row.entry;
    let key = SharedString::from(pull_request_entry_key(row));
    let muted = theme.colors.muted_foreground;
    let quick = pull_request_quick_actions(entry, host_actions);
    let open_page = page.clone();
    let open_row = row.clone();
    let signals = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .when_some(entry.checks_state, |el, state| {
            let (icon, color) = match state {
                t3code_contracts::PullRequestChecksState::Passing => (IconName::CircleCheck, open_tone(theme)),
                t3code_contracts::PullRequestChecksState::Failing => (IconName::CircleX, closed_tone(theme)),
                t3code_contracts::PullRequestChecksState::Pending => (IconName::CircleDashed, pending_tone(theme)),
            };
            el.child(Icon::new(icon).size(IconSize::Xs).color(color))
        })
        .when_some(entry.review_decision, |el, decision| {
            let (icon, color, _) = review_decision_icon(theme, decision);
            el.child(Icon::new(icon).size(IconSize::Xs).color(color))
        });
    let status = div()
        .ml_auto()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .ui_text(theme, TextSize::Xs3, weight::NORMAL)
        .when_some(entry.stack.as_ref(), |el, stack| {
            el.child(
                div()
                    .id(SharedString::from(format!("stack-{key}")))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .px(px(5.))
                    .h(px(16.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.colors.border)
                    .text_color(muted)
                    .child(Icon::new(IconName::Layers).size(IconSize::Custom(px(10.))).color(muted))
                    .child(format!("{}/{}", stack.position, stack.size)),
            )
        })
        .children(diff_stat(theme, entry.additions, entry.deletions));
    let author = entry.author.as_ref().map(|a| a.login.as_str());
    let meta = div()
        .flex()
        .min_w_0()
        .items_center()
        .gap(px(6.))
        .overflow_hidden()
        .ui_text(theme, TextSize::Xs3, weight::NORMAL)
        .text_color(muted)
        .child(div().flex().items_center().gap(px(4.)).child(avatar(theme, author, 14.)).child(SharedString::from(author.unwrap_or("ghost").to_owned())))
        .child(SharedString::from(entry.repository.clone()))
        .children(entry.labels.iter().take(3).map(|l| label_chip(theme, &l.name, l.color.as_deref())))
        .child(div().ml_auto().flex_none().whitespace_nowrap().child(relative_label(&entry.updated_at, now)));
    let actions = div().flex().flex_none().gap(px(2.)).pr(px(4.)).invisible().group_hover("pr-row", |s| s.visible()).children(quick.into_iter().map(|q| {
        let page = page.clone();
        let row = row.clone();
        let (icon, label) = match q {
            PullRequestQuickAction::Close => (IconName::GitPullRequestClosed, t!("Close")),
            PullRequestQuickAction::Merge => (IconName::GitMerge, t!("Merge")),
            PullRequestQuickAction::Ready => (IconName::Check, t!("Ready for review")),
            PullRequestQuickAction::Reopen => (IconName::RefreshCw, t!("Reopen")),
        };
        Button::new(SharedString::from(format!("quick-{}-{key}", q.action().as_str())))
            .variant(ButtonVariant::Ghost)
            .size(ButtonSize::IconSm)
            .icon(icon)
            .on_click(move |_, _, cx| page.update(cx, |p, cx| p.run_quick_action(&row, q.action(), cx)))
            .into_any_element()
            .into_element_with_tooltip(label)
    }));
    div()
        .id(SharedString::from(format!("pr-row-{key}")))
        .test_support()
        .group("pr-row")
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .rounded(theme.radii.md)
        .when(selected, |el| el.bg(theme.colors.accent_surface))
        .when(!selected, |el| el.hover(|s| s.bg(theme.colors.accent_surface.opacity(0.6))))
        .cursor_pointer()
        .on_click(move |_, _, cx| open_page.update(cx, |p, cx| p.open_row(&open_row, cx)))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .gap(px(8.))
                .px(px(12.))
                .py(px(10.))
                .child(div().pt(px(3.)).child(state_glyph(theme, entry.state, entry.is_draft, Some(entry.mergeability))))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            div()
                                .flex()
                                .min_w_0()
                                .items_center()
                                .gap(px(6.))
                                .child(div().flex_none().mono_text(px(12.), weight::NORMAL).text_color(muted).child(format!("#{}", entry.number)))
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .ui_text(theme, TextSize::Sm, weight::NORMAL)
                                        .text_color(theme.colors.text)
                                        .child(SharedString::from(entry.title.clone())),
                                )
                                .child(signals)
                                .child(status),
                        )
                        .child(meta),
                ),
        )
        .child(actions)
        .into_any_element()
}

trait WithTooltip {
    fn into_element_with_tooltip(self, label: SharedString) -> AnyElement;
}

impl WithTooltip for AnyElement {
    fn into_element_with_tooltip(self, label: SharedString) -> AnyElement {
        div().id(SharedString::from(format!("tip-{label}"))).tooltip(crate::components::ui::tooltip::tooltip(label)).child(self).into_any_element()
    }
}

impl Render for PullRequestsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let page = cx.entity();
        let now = self.now();
        let host_actions = self.merged.as_ref().map(|m| pull_request_host_actions(&m.providers)).unwrap_or_default();
        let items = self.items.clone();
        let selected = self.selected.clone();
        let theme_rows = theme.clone();
        let loading_more = self.loading_more.then(|| self.merged.as_ref().is_some_and(|m| !m.next_cursors.is_empty()));
        let can_continue = self.merged.as_ref().is_some_and(|m| !m.next_cursors.is_empty()) || self.page_size < MAX_PAGE_SIZE;
        let loading = self.merged.is_none() && self.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
        let body: AnyElement = if loading {
            pull_request_list_ghost(&theme, 7, Some(t!("Loading pull requests…"))).into_any_element()
        } else if items.is_empty()
            && self.merged.is_none()
            && let Some(error) = self.list_error()
        {
            let refreshing = self.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
            Empty::new(t!("Could not load pull requests"))
                .icon(IconName::GitPullRequestArrow)
                .description(error)
                .child(
                    div().flex().flex_wrap().justify_center().gap(theme.sp(2.)).child(
                        Button::new("pr-unavailable-retry")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .icon(IconName::RefreshCw)
                            .icon_size(IconSize::Sm)
                            .label(t!("Retry"))
                            .disabled(refreshing)
                            .on_click(cx.listener(|this, _, _, cx| this.load(true, cx))),
                    ),
                )
                .into_any_element()
        } else if items.is_empty() {
            self.render_empty(&theme, can_continue, cx)
        } else {
            list(self.list.clone(), move |ix, _, _| match &items[ix] {
                ListItem::Header(key, count) => div()
                    .w_full()
                    .flex()
                    .items_end()
                    .gap(px(6.))
                    .h(px(34.))
                    .px(px(12.))
                    .pb(px(6.))
                    .ui_text(&theme_rows, TextSize::Xs, weight::MEDIUM)
                    .text_color(theme_rows.colors.muted_foreground)
                    .child(group_label(*key))
                    .child(div().text_color(theme_rows.colors.muted_foreground.opacity(0.7)).child(count.to_string()))
                    .into_any_element(),
                ListItem::LoadMore => render_load_more(&page, &theme_rows, loading_more, can_continue),
                ListItem::Failed(error) => render_failed_banner(&page, &theme_rows, error.clone()),
                ListItem::Row(row) => {
                    let host = row.entry.host.to_lowercase();
                    let actions = host_actions.get(&host).cloned().unwrap_or_default();
                    let is_selected = selected.as_ref().is_some_and(|k| *k == pull_request_entry_key(row));
                    render_row(row, is_selected, now, &actions, &page, &theme_rows)
                }
            })
            .flex_1()
            .into_any_element()
        };
        let _ = window;
        div()
            .id("pull-requests-page")
            .test_support()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &OpenSelected, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| this.search.update(cx, |s, cx| s.focus(window, cx))))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.canvas)
            .child(
                div()
                    .flex()
                    .items_center()
                    // `WorkspacePageHeader`: the topbar height and gutter, no rule below.
                    .h(theme.spacing.workspace_topbar_height)
                    .flex_none()
                    .px(theme.spacing.workspace_gutter)
                    .gap(theme.sp(3.))
                    .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(theme.colors.text).child(t!("Pull requests"))),
            )
            .child(
                // `scrollbar-gutter: stable both-edges` around `WorkspacePageContainer width="expanded"`:
                // `max-w-6xl mx-auto px-6 pt-6 pb-12 gap-4`.
                div().flex().flex_col().flex_1().min_h_0().px(theme.spacing.app_scrollbar_width).child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .max_w(theme.rem(72.))
                        .mx_auto()
                        .gap(theme.sp(4.))
                        .px(theme.sp(6.))
                        .pt(theme.sp(6.))
                        .pb(theme.sp(12.))
                        .child(self.render_toolbar(&theme, cx))
                        .child(div().flex().flex_col().flex_1().min_h_0().child(body)),
                ),
            )
    }
}

/// `listError && shownCount > 0`: the failure under the rows, with Retry for the failed reads.
fn render_failed_banner(page: &Entity<PullRequestsPage>, theme: &AppTheme, error: SharedString) -> AnyElement {
    let page = page.clone();
    div()
        .id("pr-failed")
        .test_support()
        .flex()
        .items_center()
        .justify_between()
        .gap(theme.sp(3.))
        .rounded(theme.radii.lg)
        .border_1()
        .border_color(theme.colors.warning.opacity(0.3))
        .bg(theme.colors.warning_surface)
        .px(theme.sp(3.))
        .py(theme.sp(2.))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(theme.colors.text)
        .child(t!("{error} Showing the last pull requests loaded.", error = error.as_ref()))
        .child(
            Button::new("pr-retry-failed")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Xs)
                .label(t!("Retry"))
                .on_click(move |_, _, cx| page.update(cx, |p, cx| p.load(true, cx))),
        )
        .into_any_element()
}

/// The row under a truncated listing: Load more, its progress, or why there is no more.
/// `loading` is `Some(continuing)` while a read travels.
fn render_load_more(page: &Entity<PullRequestsPage>, theme: &AppTheme, loading: Option<bool>, can_continue: bool) -> AnyElement {
    let row = div()
        .w_full()
        .flex()
        .justify_center()
        .py(theme.sp(3.))
        .ui_text(theme, TextSize::Xs, weight::NORMAL)
        .text_color(theme.colors.muted_foreground);
    match loading {
        Some(continuing) => row
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .child(Spinner::new(SpinnerSize::Sm))
                    .child(if continuing { t!("Loading more") } else { t!("Updating pull requests") }),
            )
            .into_any_element(),
        None if can_continue => {
            let page = page.clone();
            row.child(
                Button::new("pr-load-more")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t!("Load more pull requests"))
                    .on_click(move |_, _, cx| page.update(cx, |p, cx| p.load_more(cx))),
            )
            .into_any_element()
        }
        None => row.child(t!("Narrow your search to find more pull requests.")).into_any_element(),
    }
}

/// `PullRequestListGhost`: placeholder rows while the first read travels. The fork pulses
/// them; this draws them still.
pub fn pull_request_list_ghost(theme: &AppTheme, rows: usize, caption: Option<SharedString>) -> impl IntoElement {
    const TITLE: [f32; 7] = [0.6, 0.4, 0.5, 0.667, 0.4, 0.6, 0.5];
    const META: [f32; 7] = [0.4, 0.333, 0.4, 0.25, 0.333, 0.4, 0.333];
    let bar = |h: Pixels| div().h(h).rounded(theme.radii.sm).bg(theme.colors.muted_foreground.opacity(0.15));
    div()
        .flex()
        .flex_col()
        .gap(theme.sp(0.5))
        .children(caption.map(|c| {
            div().px(theme.sp(3.)).pb(theme.sp(1.)).ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.muted_foreground).child(c)
        }))
        .children((0..rows).map(|i| {
            div()
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .rounded(theme.radii.md)
                .px(theme.sp(3.))
                .py(theme.sp(2.5))
                .child(bar(theme.sp(4.)).w(theme.sp(4.)).rounded_full())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(theme.sp(1.5))
                        .child(bar(theme.sp(4.)).w(gpui_kit::relative(TITLE[i % 7])))
                        .child(bar(theme.sp(3.5)).w(gpui_kit::relative(META[i % 7]))),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(theme.sp(1.5))
                        .child(bar(theme.sp(3.)).w(theme.sp(12.)))
                        .child(bar(theme.sp(3.)).w(theme.sp(16.))),
                )
        }))
}

fn group_label(key: PullRequestGroupKey) -> SharedString {
    match key {
        PullRequestGroupKey::Authored => t!("Authored"),
        PullRequestGroupKey::ReviewRequested => t!("Review requested"),
        PullRequestGroupKey::Others => t!("Others"),
    }
}
