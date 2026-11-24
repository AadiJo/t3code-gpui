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
    IntoElement, KeyBinding, ListAlignment, ListState, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div, list, prelude::FluentBuilder as _, px,
};
use serde::{Deserialize, Serialize};
use t3code_client_runtime::pull_request_query::{has_pull_request_filters, parse_pull_request_query};
use t3code_client_runtime::saved_views::{SavedView, saved_view_name_error};
use t3code_contracts::{
    EnvironmentId, PullRequestAction, PullRequestActionInput, PullRequestInvolvement, PullRequestListFilters,
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
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::i18n::t;
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

/// The list controls a saved view keeps. Ports `PullRequestListPreferences`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestListPreferences {
    pub involvement: Option<PullRequestInvolvement>,
    pub state: Option<PullRequestListState>,
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
    saved_views: Vec<SavedView<PullRequestListPreferences>>,
    view_editor: Option<Entity<InputState>>,
    view_error: Option<SharedString>,
    /// Fixed clock for scenes; `None` reads the system clock.
    pub now_ms: Option<i64>,
    pub last_filter: FilterTiming,
    focus: FocusHandle,
    _search_events: Subscription,
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
        let mut page = Self {
            sources,
            loads: BTreeMap::new(),
            merged: None,
            search,
            query: String::new(),
            state: PullRequestListState::Open,
            involvement: PullRequestInvolvement::All,
            sort: PullRequestListSort::Updated,
            menu_filters: PullRequestListFilters::default(),
            overrides: HashMap::new(),
            next_token: 0,
            items: Vec::new(),
            rows: 0,
            selected: None,
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            saved_views: Vec::new(),
            view_editor: None,
            view_error: None,
            now_ms: None,
            last_filter: FilterTiming::default(),
            focus: cx.focus_handle(),
            _search_events: events,
        };
        page.load(false, cx);
        page
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

    /// The saved views the menu offers.
    pub fn set_saved_views(&mut self, views: Vec<SavedView<PullRequestListPreferences>>, cx: &mut Context<Self>) {
        self.saved_views = views;
        cx.notify();
    }

    /// The saved views, for the owner to persist.
    pub fn saved_views(&self) -> &[SavedView<PullRequestListPreferences>] {
        &self.saved_views
    }

    /// Loads every environment, or with `only_failed` only those whose last load failed.
    pub fn load(&mut self, only_failed: bool, cx: &mut Context<Self>) {
        let input = PullRequestListInput {
            state: self.state,
            involvement: None,
            filters: None,
            project_id: None,
            project_ids: None,
            host: None,
            limit: Some(100),
            cursors: None,
            query: None,
        };
        for env in self.sources.clone() {
            let failed = matches!(self.loads.get(&env.environment_id), Some(EnvironmentLoad::Failed(_)));
            if only_failed && !failed {
                continue;
            }
            self.loads.insert(env.environment_id.clone(), EnvironmentLoad::Loading);
            let request = env.source.list(input.clone());
            let id = env.environment_id.clone();
            cx.spawn(async move |this, cx| {
                let result = request.await;
                this.update(cx, |this, cx| {
                    let load = match result {
                        Ok(answer) => EnvironmentLoad::Loaded(Arc::new(answer)),
                        Err(error) => EnvironmentLoad::Failed(error.0),
                    };
                    this.loads.insert(id, load);
                    this.remerge(cx);
                })
                .ok();
            })
            .detach();
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
            .filter_map(|s| match self.loads.get(&s.environment_id) {
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
        self.state = state;
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

    /// The controls as a saved view would keep them.
    pub fn preferences(&self) -> PullRequestListPreferences {
        let labels = self.menu_filters.labels.as_ref().map(|g| g.iter().flatten().cloned().collect());
        PullRequestListPreferences {
            involvement: Some(self.involvement),
            state: Some(self.state),
            q: (!self.query.is_empty()).then(|| self.query.clone()),
            draft: self.menu_filters.draft,
            review: self.menu_filters.review,
            checks: self.menu_filters.checks,
            author: self.menu_filters.author.clone(),
            labels,
            sort: Some(self.sort),
        }
    }

    /// Restores the controls a saved view kept.
    pub fn apply_preferences(&mut self, p: &PullRequestListPreferences, window: &mut Window, cx: &mut Context<Self>) {
        self.involvement = p.involvement.unwrap_or(PullRequestInvolvement::All);
        self.state = p.state.unwrap_or(PullRequestListState::Open);
        self.sort = p.sort.unwrap_or_default();
        self.menu_filters = PullRequestListFilters {
            draft: p.draft,
            review: p.review,
            checks: p.checks,
            labels: p.labels.as_ref().map(|l| l.iter().map(|n| vec![n.clone()]).collect()),
            excluded_labels: None,
            author: p.author.clone(),
        };
        let q = p.q.clone().unwrap_or_default();
        self.set_query(&q, window, cx);
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

    /// Recomputes the visible rows from the held answers. Runs per keystroke.
    pub fn refilter(&mut self, cx: &mut Context<Self>) {
        let started = Instant::now();
        let Some(merged) = self.merged.as_ref() else {
            self.items.clear();
            self.rows = 0;
            self.list.reset(0);
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
        let keep = |row: &Row| match checks {
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
        self.list.reset(self.items.len());
        if self.selected.as_ref().is_some_and(|key| !self.items.iter().any(|i| matches!(i, ListItem::Row(r) if pull_request_entry_key(r) == key.as_ref()))) {
            self.selected = None;
        }
        self.last_filter = FilterTiming { rows_in: merged.entries.len(), rows_out: self.rows, micros: started.elapsed().as_micros() };
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

    // --- saved views -----------------------------------------------------

    /// Opens the name editor for a new view of the current controls.
    pub fn begin_save_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let suggested = if self.involvement == PullRequestInvolvement::Reviewing { t!("PRs Requesting My Review").to_string() } else { String::new() };
        let editor = cx.new(|cx| InputState::new(window, cx).placeholder(t!("PRs Requesting My Review")).default_value(suggested));
        editor.update(cx, |e, cx| e.focus(window, cx));
        self.view_editor = Some(editor);
        self.view_error = None;
        cx.notify();
    }

    /// Saves the view under the editor's name, or shows why it cannot.
    pub fn save_view(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(editor) = &self.view_editor else { return false };
        let name = editor.read(cx).value().to_string();
        let error = saved_view_name_error(&name, self.saved_views.iter().map(|v| (v.id.as_str(), v.name.as_str())), None);
        if let Some(error) = error {
            self.view_error = Some(error.message().into());
            cx.notify();
            return false;
        }
        let id = format!("view-{}", self.saved_views.len() + 1);
        self.saved_views.push(SavedView { id, name: name.trim().to_owned(), search: self.preferences() });
        self.view_editor = None;
        self.view_error = None;
        cx.notify();
        true
    }

    fn delete_view(&mut self, id: &str, cx: &mut Context<Self>) {
        self.saved_views.retain(|v| v.id != id);
        cx.notify();
    }

    fn active_view(&self) -> Option<&SavedView<PullRequestListPreferences>> {
        let current = self.preferences();
        self.saved_views.iter().find(|v| v.search == current)
    }

    // --- render ----------------------------------------------------------

    fn render_toolbar(&self, theme: &AppTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let sort = self.sort;
        let sort_menu = {
            let this = this.clone();
            dropdown_menu(
                "pr-sort",
                Button::new("pr-sort-trigger").variant(ButtonVariant::Outline).icon(IconName::ArrowDownUp).label(t!("Sort", context = "verb, sort a list")),
                move |_, _| {
                    let mut menu = MenuPopup::new().label(t!("Sort pull requests"));
                    for option in PullRequestListSort::ALL {
                        let this = this.clone();
                        menu = menu.item(
                            MenuItem::new(sort_label(option))
                                .kind(MenuItemKind::Radio(option == sort))
                                .on_select(move |_, cx| this.update(cx, |p, cx| p.set_sort(option, cx))),
                        );
                    }
                    menu
                },
            )
        };
        let filters_menu = {
            let this = this.clone();
            let state = self.state;
            let involvement = self.involvement;
            let filters = self.menu_filters.clone();
            let facets = self.merged.as_ref().map(|m| collect_pull_request_list_facets(&m.entries, state)).unwrap_or_default();
            dropdown_menu(
                "pr-filters",
                Button::new("pr-filters-trigger").variant(ButtonVariant::Outline).icon(IconName::ListFilter).label(t!("Filters")),
                move |_, _| filters_popup(&this, state, involvement, &filters, &facets),
            )
        };
        let views_menu = {
            let this = this.clone();
            let views: Vec<(String, String)> = self.saved_views.iter().map(|v| (v.id.clone(), v.name.clone())).collect();
            let active = self.active_view().map(|v| v.id.clone());
            let label: SharedString = self.active_view().map(|v| SharedString::from(v.name.clone())).unwrap_or_else(|| t!("Views", context = "saved filter views menu"));
            dropdown_menu(
                "pr-views",
                Button::new("pr-views-trigger").variant(ButtonVariant::Outline).icon(IconName::Bookmark).label(label).trailing_icon(IconName::ChevronDown),
                move |_, cx| {
                    let mut menu = MenuPopup::new().label(t!("{list, select, pullRequest {Saved pull request views} other {Saved issue views}}", list = "pullRequest"));
                    let prefs: Vec<_> = this.read(cx).saved_views.iter().map(|v| (v.id.clone(), v.search.clone())).collect();
                    for (id, name) in &views {
                        let this = this.clone();
                        let search = prefs.iter().find(|(i, _)| i == id).map(|(_, s)| s.clone()).unwrap_or_default();
                        menu = menu.item(
                            MenuItem::new(SharedString::from(name.clone()))
                                .kind(MenuItemKind::Radio(active.as_deref() == Some(id)))
                                .on_select(move |window, cx| this.update(cx, |p, cx| p.apply_preferences(&search, window, cx))),
                        );
                    }
                    if !views.is_empty() {
                        menu = menu.separator();
                    }
                    let save = this.clone();
                    menu = menu.item(MenuItem::new(t!("Save current view…")).icon(IconName::Plus).on_select(move |window, cx| save.update(cx, |p, cx| p.begin_save_view(window, cx))));
                    if let Some(id) = active.clone() {
                        let del = this.clone();
                        menu = menu.item(MenuItem::new(t!("Delete view")).destructive().on_select(move |_, cx| del.update(cx, |p, cx| p.delete_view(&id, cx))));
                    }
                    menu
                },
            )
        };
        let refreshing = self.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(div().id("pr-search").min_w(px(240.)).flex_1().child(TextInput::new(&self.search).size(InputSize::Lg).leading_icon(IconName::Search)))
            .child(sort_menu)
            .child(filters_menu)
            .child(views_menu)
            .child(
                Button::new("pr-refresh")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Icon)
                    .icon(IconName::RefreshCw)
                    .disabled(refreshing)
                    .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
            )
            .when_some(self.view_editor.clone(), |el, editor| {
                el.child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().ui_text(theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.muted_foreground).child(t!("View name")))
                        .child(div().id("pr-view-name").w(px(260.)).child(TextInput::new(&editor).invalid(self.view_error.is_some())))
                        .child(Button::new("pr-view-save").size(ButtonSize::Sm).label(t!("Save")).on_click(cx.listener(|this, _, _, cx| {
                            this.save_view(cx);
                        })))
                        .child(Button::new("pr-view-cancel").variant(ButtonVariant::Ghost).size(ButtonSize::Sm).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                            this.view_editor = None;
                            this.view_error = None;
                            cx.notify();
                        })))
                        .when_some(self.view_error.clone(), |el, error| {
                            el.child(div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.destructive()).child(error))
                        }),
                )
            })
    }

    fn render_chips(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, filters) = self.effective_filters();
        let mut chips: Vec<(SharedString, PullRequestListFilters)> = Vec::new();
        let m = &self.menu_filters;
        if let Some(d) = filters.draft {
            chips.push((if d == PullRequestListFiltersDraft::Only { t!("Drafts only") } else { t!("Hide drafts") }, PullRequestListFilters { draft: None, ..m.clone() }));
        }
        if let Some(r) = filters.review {
            chips.push((review_filter_label(r), PullRequestListFilters { review: None, ..m.clone() }));
        }
        if let Some(c) = filters.checks {
            chips.push((if c == PullRequestListFiltersChecks::Passing { t!("Passing", context = "checks filter") } else { t!("Failing", context = "checks filter") }, PullRequestListFilters { checks: None, ..m.clone() }));
        }
        if let Some(a) = &filters.author {
            chips.push((SharedString::from(format!("{}: {a}", t!("Author"))), PullRequestListFilters { author: None, ..m.clone() }));
        }
        for group in filters.labels.iter().flatten() {
            let rest: Vec<Vec<String>> = m.labels.iter().flatten().filter(|g| *g != group).cloned().collect();
            chips.push((
                SharedString::from(format!("{}: {}", t!("Labels"), group.join(", "))),
                PullRequestListFilters { labels: (!rest.is_empty()).then_some(rest), ..m.clone() },
            ));
        }
        if chips.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(chips.into_iter().enumerate().map(|(i, (label, without))| {
                    div()
                        .id(("pr-chip", i))
                        .test_support()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h(px(22.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.colors.border)
                        .ui_text(theme, TextSize::Xs, weight::MEDIUM)
                        .text_color(theme.colors.text)
                        .child(label)
                        .child(
                            div()
                                .id(("pr-chip-clear", i))
                                .test_support()
                                .rounded_full()
                                .hover(|s| s.bg(theme.colors.accent_surface))
                                .child(Icon::new(IconName::X).size(IconSize::Xs).color(theme.colors.muted_foreground))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_menu_filters(without.clone(), cx))),
                        )
                }))
                .into_any_element(),
        )
    }

    fn render_banners(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let failed: Vec<(SharedString, SharedString)> = self
            .sources
            .iter()
            .filter_map(|s| match self.loads.get(&s.environment_id) {
                Some(EnvironmentLoad::Failed(message)) => Some((s.label.clone(), message.clone())),
                _ => None,
            })
            .collect();
        if !failed.is_empty() {
            out.push(
                div()
                    .id("pr-failed")
                    .test_support()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .rounded(theme.radii.lg)
                    .border_1()
                    .border_color(theme.colors.warning.opacity(0.3))
                    .bg(theme.colors.warning_surface)
                    .px(px(12.))
                    .py(px(8.))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.text)
                    .child(
                        div().flex().flex_col().gap(px(2.)).children(failed.iter().map(|(label, message)| {
                            div().child(t!("{error} Showing the last pull requests loaded.", error = format!("{label}: {message}").as_str()))
                        })),
                    )
                    .child(
                        Button::new("pr-retry-failed")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Sm)
                            .label(t!("Retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.load(true, cx))),
                    )
                    .into_any_element(),
            );
        }
        if let Some(merged) = &self.merged
            && !merged.errors.is_empty()
        {
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .children(merged.errors.iter().map(|(_, e)| {
                        div().flex().gap(px(6.)).child(Icon::new(IconName::TriangleAlert).size(IconSize::Xs).color(theme.colors.warning)).child(
                            SharedString::from(format!("{}: {}", e.project_title, e.message)),
                        )
                    }))
                    .into_any_element(),
            );
        }
        out
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

fn review_filter_label(review: PullRequestListFiltersReview) -> SharedString {
    match review {
        PullRequestListFiltersReview::Approved => t!("Approved", context = "review decision"),
        PullRequestListFiltersReview::ChangesRequested => t!("Changes requested"),
        PullRequestListFiltersReview::ReviewRequired => t!("Review required"),
        PullRequestListFiltersReview::NoneType => t!("No reviews"),
    }
}

fn state_label(state: PullRequestListState) -> SharedString {
    match state {
        PullRequestListState::Open => t!("Open", context = "pull request state"),
        PullRequestListState::Closed => t!("Closed", context = "pull request state"),
        PullRequestListState::Merged => t!("Merged", context = "pull request state"),
        PullRequestListState::All => t!("All", context = "pull request filter"),
    }
}

fn involvement_label(involvement: PullRequestInvolvement) -> SharedString {
    match involvement {
        PullRequestInvolvement::All => t!("All", context = "pull request filter"),
        PullRequestInvolvement::Reviewing => t!("Reviewing", context = "pull request filter"),
        PullRequestInvolvement::Authored => t!("Authored", context = "pull request filter"),
    }
}

fn filters_popup(
    this: &Entity<PullRequestsPage>,
    state: PullRequestListState,
    involvement: PullRequestInvolvement,
    filters: &PullRequestListFilters,
    facets: &(Vec<PullRequestAuthorFacet>, Vec<PullRequestLabelFacet>),
) -> MenuPopup {
    let mut menu = MenuPopup::new().min_width(px(220.)).label(t!("State", context = "pull request filter"));
    for option in [PullRequestListState::Open, PullRequestListState::Merged, PullRequestListState::Closed, PullRequestListState::All] {
        let this = this.clone();
        menu = menu.item(MenuItem::new(state_label(option)).kind(MenuItemKind::Radio(option == state)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_state(option, cx))));
    }
    menu = menu.separator().label(t!("Involvement"));
    for option in [PullRequestInvolvement::All, PullRequestInvolvement::Reviewing, PullRequestInvolvement::Authored] {
        let this = this.clone();
        menu = menu.item(
            MenuItem::new(involvement_label(option))
                .kind(MenuItemKind::Radio(option == involvement))
                .on_select(move |_, cx| this.update(cx, |p, cx| p.set_involvement(option, cx))),
        );
    }
    let with = |update: Box<dyn Fn(&mut PullRequestListFilters)>| {
        let this = this.clone();
        let mut next = filters.clone();
        update(&mut next);
        move |_: &mut Window, cx: &mut App| this.update(cx, |p, cx| p.set_menu_filters(next.clone(), cx))
    };
    menu = menu.separator().label(t!("Draft", context = "pull request filter"));
    for (label, value) in [(t!("Drafts only"), Some(PullRequestListFiltersDraft::Only)), (t!("Hide drafts"), Some(PullRequestListFiltersDraft::Hide))] {
        let toggled = if filters.draft == value { None } else { value };
        menu = menu.item(MenuItem::new(label).kind(MenuItemKind::Checkbox(filters.draft == value)).on_select(with(Box::new(move |f| f.draft = toggled))));
    }
    menu = menu.separator().label(t!("Review", context = "pull request filter"));
    for value in [PullRequestListFiltersReview::Approved, PullRequestListFiltersReview::ChangesRequested, PullRequestListFiltersReview::ReviewRequired, PullRequestListFiltersReview::NoneType] {
        let toggled = if filters.review == Some(value) { None } else { Some(value) };
        menu = menu.item(MenuItem::new(review_filter_label(value)).kind(MenuItemKind::Checkbox(filters.review == Some(value))).on_select(with(Box::new(move |f| f.review = toggled))));
    }
    menu = menu.separator().label(t!("Checks", context = "pull request filter"));
    for (label, value) in [(t!("Passing", context = "checks filter"), PullRequestListFiltersChecks::Passing), (t!("Failing", context = "checks filter"), PullRequestListFiltersChecks::Failing)] {
        let toggled = if filters.checks == Some(value) { None } else { Some(value) };
        menu = menu.item(MenuItem::new(label).kind(MenuItemKind::Checkbox(filters.checks == Some(value))).on_select(with(Box::new(move |f| f.checks = toggled))));
    }
    if !facets.0.is_empty() {
        menu = menu.separator().label(t!("Author"));
        for facet in facets.0.iter().take(6) {
            let login = facet.actor.login.clone();
            let on = filters.author.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&login));
            let value = (!on).then(|| login.clone());
            menu = menu.item(
                MenuItem::new(SharedString::from(format!("{login} ({})", facet.count)))
                    .kind(MenuItemKind::Checkbox(on))
                    .on_select(with(Box::new(move |f| f.author = value.clone()))),
            );
        }
    }
    if !facets.1.is_empty() {
        menu = menu.separator().label(t!("Labels"));
        for facet in facets.1.iter().take(8) {
            let name = facet.label.name.clone();
            let held: Vec<Vec<String>> = filters.labels.clone().unwrap_or_default();
            let on = held.iter().any(|g| g.iter().any(|l| l.eq_ignore_ascii_case(&name)));
            let next: Vec<Vec<String>> =
                if on { held.into_iter().filter(|g| !g.iter().any(|l| l.eq_ignore_ascii_case(&name))).collect() } else { held.into_iter().chain([vec![name.clone()]]).collect() };
            menu = menu.item(
                MenuItem::new(SharedString::from(format!("{name} ({})", facet.count)))
                    .kind(MenuItemKind::Checkbox(on))
                    .on_select(with(Box::new(move |f| f.labels = (!next.is_empty()).then(|| next.clone())))),
            );
        }
    }
    menu
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
        let loading = self.merged.is_none() && self.loads.values().any(|l| matches!(l, EnvironmentLoad::Loading));
        let body: AnyElement = if loading {
            div().p(px(24.)).text_color(theme.colors.muted_foreground).child(t!("Loading pull requests…")).into_any_element()
        } else if items.is_empty() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.))
                .py(px(48.))
                .text_color(theme.colors.muted_foreground)
                .child(Icon::new(IconName::GitPullRequestArrow).size(IconSize::Xl).color(theme.colors.muted_foreground))
                .child(div().ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(theme.colors.text).child(t!("Nothing under these filters")))
                .child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).child(t!("Widen the state, involvement or project filter to see more.")))
                .into_any_element()
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
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    // `WorkspacePageContainer`: `px-6 pt-6`, then `gap-3` within the toolbar group.
                    .gap(theme.sp(3.))
                    .px(theme.sp(6.))
                    .pt(theme.sp(6.))
                    .child(self.render_toolbar(&theme, cx))
                    .children(self.render_chips(&theme, cx))
                    .children(self.render_banners(&theme, cx))
                    .child(div().flex().flex_col().flex_1().min_h_0().child(body)),
            )
    }
}

fn group_label(key: PullRequestGroupKey) -> SharedString {
    match key {
        PullRequestGroupKey::Authored => t!("Authored"),
        PullRequestGroupKey::ReviewRequested => t!("Review requested"),
        PullRequestGroupKey::Others => t!("Others"),
    }
}
