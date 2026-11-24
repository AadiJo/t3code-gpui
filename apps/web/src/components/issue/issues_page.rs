//! The issues page: issues from every connected tracker across environments,
//! grouped by tracker, with state, involvement and sort filters, search,
//! "Load more" per tracker, a retry that asks only failed environments again,
//! and a template-driven new issue form. Ports `IssuesPage.tsx` and
//! `IssueRow.tsx`.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::base::TestSupportExt as _;
use serde::{Deserialize, Serialize};
use gpui_kit::{rems, 
    AnyElement, App, Pixels, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use t3code_client_runtime::issue_list::{HeldIssuePage, IssueTrackerGroup, continue_issue_page, group_issues, hold_issue_pages, issue_cursors_for_host};
use t3code_contracts::*;

use super::issue_presentation::issue_state_presentation;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::pull_request::pull_request_presentation::{avatar, label_chip, relative_label};
use crate::components::pull_request::pull_request_source::IssueSource;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{InputSize, TextInput};
use crate::client_persistence_storage::LocalStorage;
use crate::components::saved_views_menu::{ApplySavedView, SavedViewsKind, SavedViewsList, SavedViewsMenu};
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::components::ui::popover::{Popover, PopupHandle};
use crate::i18n::t;
use crate::ActiveAppTheme as _;

/// One environment the page reads.
#[derive(Clone)]
pub struct EnvironmentIssueSource {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub source: Rc<dyn IssueSource>,
}

/// What the page asks its owner to do.
#[derive(Clone, Debug)]
pub enum IssuesPageEvent {
    Open { environment_id: EnvironmentId, reference: IssueRef },
}

/// What the issues page remembers between visits and a saved view keeps, in the web's stored
/// shape (`t3.issues.preferences`). Ports `IssueListPreferences`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueListPreferences {
    pub who: IssueInvolvement,
    pub status: IssueListState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide_projects: Option<Vec<(EnvironmentId, ProjectId)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<IssueListSort>,
}

impl Default for IssueListPreferences {
    fn default() -> Self {
        Self { who: IssueInvolvement::All, status: IssueListState::Open, hide_projects: None, host: None, q: None, label: None, assignee: None, order: None }
    }
}

impl IssueListPreferences {
    /// `issueListPreferences`: drops empty fields and the default order.
    pub fn normalized(&self) -> Self {
        let text = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
        Self {
            hide_projects: self.hide_projects.clone().filter(|p| !p.is_empty()),
            host: text(&self.host),
            q: text(&self.q),
            label: self.label.clone().filter(|l| !l.is_empty()),
            assignee: text(&self.assignee),
            order: self.order.filter(|o| *o == IssueListSort::Created),
            ..self.clone()
        }
    }
}

/// The issue list's saved views (`ISSUE_SAVED_VIEWS`).
pub struct IssueSavedViews;

impl SavedViewsKind for IssueSavedViews {
    type Preferences = IssueListPreferences;
    const STORAGE_KEY: &'static str = "t3.issues.savedViews";
    const LIST: SavedViewsList = SavedViewsList::Issue;

    fn placeholder() -> SharedString {
        t!("Issues Assigned to Me")
    }

    fn suggested_name(p: &IssueListPreferences) -> SharedString {
        if p.who == IssueInvolvement::Assigned { t!("Issues Assigned to Me") } else { SharedString::default() }
    }

    fn matches(view: &IssueListPreferences, current: &IssueListPreferences) -> bool {
        let key = |p: &IssueListPreferences| {
            let mut p = p.normalized();
            if let Some(l) = &mut p.label {
                l.sort();
            }
            if let Some(h) = &mut p.hide_projects {
                h.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));
            }
            p
        };
        key(view) == key(current)
    }
}

const PREFERENCES_KEY: &str = "t3.issues.preferences";

/// Which filter submenu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IssueSubmenu {
    State,
    Involvement,
    Labels,
    Assignee,
}

enum Load {
    Loading,
    Loaded(IssueListResult),
    Failed(SharedString),
}


/// The issues page.
pub struct IssuesPage {
    sources: Vec<EnvironmentIssueSource>,
    loads: BTreeMap<EnvironmentId, Load>,
    held: HashMap<EnvironmentId, HeldIssuePage>,
    groups: Vec<IssueTrackerGroup>,
    search: Entity<InputState>,
    query: String,
    state: IssueListState,
    involvement: IssueInvolvement,
    sort: IssueListSort,
    selected: Option<String>,
    labels: Vec<String>,
    assignee: Option<String>,
    host: Option<String>,
    submenu: Option<IssueSubmenu>,
    keep_filters_open: bool,
    views_menu: Entity<SavedViewsMenu<IssueSavedViews>>,
    /// The list column's width, for the `@lg/issue-list` container query.
    list_width: std::rc::Rc<std::cell::Cell<Pixels>>,
    pub now_ms: Option<i64>,
    _search: Subscription,
    _views: Subscription,
}

impl EventEmitter<IssuesPageEvent> for IssuesPage {}

impl IssuesPage {
    /// The page over `sources`; loads every environment at once.
    pub fn new(sources: Vec<EnvironmentIssueSource>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search issues")));
        let sub = cx.subscribe_in(&search, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.regroup(cx);
            }
        });
        let remembered: IssueListPreferences = LocalStorage::get(cx, PREFERENCES_KEY).unwrap_or_default();
        let views_menu = cx.new(|cx| SavedViewsMenu::new(remembered.normalized(), cx));
        let views = cx.subscribe_in(&views_menu, window, |this, _, event: &ApplySavedView<IssueListPreferences>, window, cx| {
            this.restore(&event.0, window, cx);
            this.load(false, cx);
        });
        let mut page = Self {
            sources,
            loads: BTreeMap::new(),
            held: HashMap::new(),
            groups: Vec::new(),
            search,
            query: String::new(),
            state: IssueListState::Open,
            involvement: IssueInvolvement::All,
            sort: IssueListSort::Updated,
            selected: None,
            labels: Vec::new(),
            assignee: None,
            host: None,
            submenu: None,
            keep_filters_open: false,
            views_menu,
            list_width: Default::default(),
            now_ms: None,
            _search: sub,
            _views: views,
        };
        page.restore(&remembered, window, cx);
        page.load(false, cx);
        page
    }

    fn input(&self) -> IssueListInput {
        IssueListInput {
            state: self.state,
            involvement: Some(self.involvement),
            project_id: None,
            exclude_project_ids: None,
            host: self.host.clone(),
            limit: Some(50),
            cursors: None,
            refresh: None,
            query: None,
            labels: (!self.labels.is_empty()).then(|| self.labels.clone()),
            assignee: self.assignee.clone(),
            sort: Some(self.sort),
        }
    }

    /// Loads every environment, or only the failed ones.
    pub fn load(&mut self, only_failed: bool, cx: &mut Context<Self>) {
        for env in self.sources.clone() {
            if only_failed && !matches!(self.loads.get(&env.environment_id), Some(Load::Failed(_))) {
                continue;
            }
            self.loads.insert(env.environment_id.clone(), Load::Loading);
            let request = env.source.list(self.input());
            let id = env.environment_id.clone();
            cx.spawn(async move |this, cx| {
                let result = request.await;
                this.update(cx, |this, cx| {
                    this.loads.insert(id, match result {
                        Ok(r) => Load::Loaded(r),
                        Err(e) => Load::Failed(e.0),
                    });
                    this.rehold(cx);
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn rehold(&mut self, cx: &mut Context<Self>) {
        let answers: Vec<(EnvironmentId, IssueListResult)> = self
            .loads
            .iter()
            .filter_map(|(id, l)| if let Load::Loaded(r) = l { Some((id.clone(), r.clone())) } else { None })
            .collect();
        self.held = hold_issue_pages(&answers, &self.held);
        self.regroup(cx);
    }

    fn regroup(&mut self, cx: &mut Context<Self>) {
        let held: Vec<(EnvironmentId, HeldIssuePage)> =
            self.sources.iter().filter_map(|s| self.held.get(&s.environment_id).map(|p| (s.environment_id.clone(), p.clone()))).collect();
        self.groups = group_issues(&held, &self.query, self.sort, |_, _| None);
        let preferences = self.preferences();
        LocalStorage::set(cx, PREFERENCES_KEY, Some(&preferences));
        self.views_menu.update(cx, |m, cx| m.set_current(preferences, cx));
        cx.notify();
    }

    /// The controls as the page remembers them and a saved view keeps them.
    pub fn preferences(&self) -> IssueListPreferences {
        IssueListPreferences {
            who: self.involvement,
            status: self.state,
            hide_projects: None,
            host: self.host.clone(),
            q: Some(self.query.clone()),
            label: Some(self.labels.clone()),
            assignee: self.assignee.clone(),
            order: Some(self.sort),
        }
        .normalized()
    }

    fn restore(&mut self, p: &IssueListPreferences, window: &mut Window, cx: &mut Context<Self>) {
        self.involvement = p.who;
        self.state = p.status;
        self.host = p.host.clone();
        self.labels = p.label.clone().unwrap_or_default();
        self.assignee = p.assignee.clone();
        self.sort = p.order.unwrap_or(IssueListSort::Updated);
        let q = p.q.clone().unwrap_or_default();
        self.search.update(cx, |input, cx| input.set_value(q.clone(), window, cx));
        self.query = q;
    }

    /// The saved views menu.
    pub fn views_menu(&self) -> &Entity<SavedViewsMenu<IssueSavedViews>> {
        &self.views_menu
    }

    /// The order within trackers.
    pub fn set_sort(&mut self, sort: IssueListSort, cx: &mut Context<Self>) {
        self.sort = sort;
        self.load(false, cx);
    }

    fn set_labels(&mut self, labels: Vec<String>, cx: &mut Context<Self>) {
        self.labels = labels;
        self.load(false, cx);
    }

    fn set_assignee(&mut self, assignee: Option<String>, cx: &mut Context<Self>) {
        self.assignee = assignee;
        self.load(false, cx);
    }

    fn set_host(&mut self, host: Option<String>, cx: &mut Context<Self>) {
        self.host = host;
        self.load(false, cx);
    }

    fn set_submenu(&mut self, submenu: Option<IssueSubmenu>, cx: &mut Context<Self>) {
        if self.submenu != submenu {
            self.submenu = submenu;
            cx.notify();
        }
    }

    /// `count`: controls away from their defaults.
    fn filter_count(&self) -> usize {
        [self.state != IssueListState::Open, self.involvement != IssueInvolvement::All, self.assignee.is_some()].into_iter().filter(|b| *b).count() + self.labels.len()
    }

    /// Labels and people the loaded issues carry, for the filter submenus.
    fn facets(&self) -> (Vec<String>, Vec<String>) {
        let mut labels: Vec<String> = Vec::new();
        let mut people: Vec<String> = Vec::new();
        for row in self.groups.iter().flat_map(|g| g.rows.iter()) {
            for l in &row.entry.labels {
                if !labels.contains(&l.name) {
                    labels.push(l.name.clone());
                }
            }
            for a in &row.entry.assignees {
                if !people.contains(&a.login) {
                    people.push(a.login.clone());
                }
            }
        }
        labels.sort_by_key(|l| l.to_lowercase());
        people.sort_by_key(|p| p.to_lowercase());
        (labels, people)
    }

    /// The tracker hosts the list read, one per host, a configured one winning.
    fn trackers(&self) -> Vec<IssueTrackerSummary> {
        let mut trackers: Vec<IssueTrackerSummary> = self.held.values().flat_map(|p| p.answer.trackers.iter().cloned()).collect();
        trackers.sort_by(|a, b| (a.tracker.as_str(), a.host.as_str()).cmp(&(b.tracker.as_str(), b.host.as_str())));
        let mut out: Vec<IssueTrackerSummary> = Vec::new();
        for t in trackers {
            match out.iter_mut().find(|o| o.host == t.host) {
                Some(existing) if !existing.configured && t.configured => *existing = t,
                Some(_) => {}
                None => out.push(t),
            }
        }
        out
    }

    /// The rows shown, as `display key` per tracker group.
    pub fn visible(&self) -> Vec<(String, Vec<String>)> {
        self.groups.iter().map(|g| (g.tracker.host.clone(), g.rows.iter().map(|r| r.entry.display_key.clone()).collect())).collect()
    }

    /// The state filter.
    pub fn set_state(&mut self, state: IssueListState, cx: &mut Context<Self>) {
        self.state = state;
        self.load(false, cx);
        self.regroup(cx);
    }

    /// The involvement filter.
    pub fn set_involvement(&mut self, involvement: IssueInvolvement, cx: &mut Context<Self>) {
        self.involvement = involvement;
        self.load(false, cx);
    }

    /// Asks one tracker host for its next page in `environment_id`.
    pub fn load_more(&mut self, environment_id: &EnvironmentId, host: &str, cx: &mut Context<Self>) {
        let Some(page) = self.held.get(environment_id) else { return };
        let Some(source) = self.sources.iter().find(|s| &s.environment_id == environment_id).map(|s| s.source.clone()) else { return };
        let asked = issue_cursors_for_host(&page.next_cursors, host);
        let request = source.list(IssueListInput { cursors: Some(asked.clone()), ..self.input() });
        let id = environment_id.clone();
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if let Some(page) = this.held.get(&id) {
                    let next = match &result {
                        Ok(r) => continue_issue_page(page, Ok(r), &asked),
                        Err(e) => continue_issue_page(page, Err(&e.0), &asked),
                    };
                    this.held.insert(id, next);
                }
                this.regroup(cx);
            })
            .ok();
        })
        .detach();
    }

    fn open(&mut self, environment_id: EnvironmentId, entry: &IssueListEntry, cx: &mut Context<Self>) {
        self.selected = Some(entry.url.clone());
        cx.emit(IssuesPageEvent::Open {
            environment_id,
            reference: IssueRef { project_id: entry.project_id.clone(), tracker: entry.tracker.clone(), host: Some(entry.host.clone()), scope: entry.scope.clone(), key: entry.key.clone() },
        });
        cx.notify();
    }

}

impl Render for IssuesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let now = self.now_ms.unwrap_or_else(crate::timestamp_format::now_ms);
        let this = cx.entity();
        let count = self.filter_count();
        let filters_trigger = Button::new("issues-filters-trigger")
            .variant(ButtonVariant::Outline)
            .icon(IconName::ListFilter)
            .label(t!("Filters"))
            .when(count > 0, |b| {
                b.accessory(
                    div()
                        .rounded_full()
                        .bg(theme.colors.muted)
                        .px(theme.sp(1.5))
                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.muted_foreground)
                        .child(count.to_string()),
                )
            });
        let filters = {
            let this = this.clone();
            Popover::new("issues-filters", filters_trigger, move |handle, _, cx| issue_filters_popup(&this, handle, cx))
        };
        let sort = self.sort;
        let sort_menu = {
            let this = this.clone();
            dropdown_menu(
                "issues-sort",
                Button::new("issues-sort-trigger").variant(ButtonVariant::Outline).icon(IconName::ArrowDownUp).label(t!("Sort", context = "verb, button that sorts a list")),
                move |_, _| {
                    let mut menu = MenuPopup::new();
                    for (option, label, icon) in [
                        (IssueListSort::Updated, t!("Recently updated"), IconName::Clock),
                        (IssueListSort::Created, t!("Newest", context = "sort order"), IconName::CalendarArrowDown),
                    ] {
                        let this = this.clone();
                        menu = menu.item(
                            MenuItem::new(label)
                                .icon(icon)
                                .icon_size(IconSize::Sm)
                                .kind(MenuItemKind::Radio(option == sort))
                                .on_select(move |_, cx| this.update(cx, |p, cx| p.set_sort(option, cx))),
                        );
                    }
                    menu
                },
            )
        };
        // `hostOptionsFor` and `FilterMenu`: All, then each tracker host, shown once any tracker answered.
        let trackers = self.trackers();
        let tracker_menu = (!trackers.is_empty() || self.host.is_some()).then(|| {
            let this = this.clone();
            let current = self.host.clone();
            let several = trackers.len() > 1;
            let mut options: Vec<(Option<String>, SharedString, IconName)> = vec![(None, t!("All", context = "issue filter"), IconName::Plug2)];
            if let Some(chosen) = &current
                && !trackers.iter().any(|t| &t.host == chosen)
            {
                options.push((Some(chosen.clone()), chosen.clone().into(), IconName::Plug2));
            }
            for tracker in &trackers {
                let (name, icon) = issue_tracker_presentation(tracker.tracker.as_str());
                let label = if several && name == tracker.host { tracker.host.clone() } else { format!("{name} ({})", tracker.host) };
                options.push((Some(tracker.host.clone()), label.into(), icon));
            }
            let shown = options.iter().find(|o| o.0 == current).cloned().unwrap_or_else(|| options[0].clone());
            dropdown_menu(
                "issues-tracker",
                Button::new("issues-tracker-trigger").variant(ButtonVariant::Outline).icon(shown.2).icon_size(IconSize::Sm).label(shown.1).label_max_width(theme.rem(12.)),
                move |_, _| {
                    let mut menu = MenuPopup::new();
                    for (host, label, icon) in options.clone() {
                        let this = this.clone();
                        let on = host == current;
                        menu = menu.item(MenuItem::new(label).icon(icon).icon_size(IconSize::Sm).kind(MenuItemKind::Radio(on)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_host(host.clone(), cx))));
                    }
                    menu
                },
            )
        });
        let failed: Vec<(SharedString, SharedString)> = self
            .sources
            .iter()
            .filter_map(|s| match self.loads.get(&s.environment_id) {
                Some(Load::Failed(m)) => Some((s.label.clone(), m.clone())),
                _ => None,
            })
            .collect();
        let mut list = div().flex().flex_col().gap(rems(1.));
        for group in &self.groups {
            let mut column = div().flex().flex_col().child(
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.375))
                    .px(rems(0.75))
                    .pb(rems(0.375))
                    .ui_text(&theme, TextSize::Xs, weight::MEDIUM)
                    .text_color(theme.colors.muted_foreground)
                    .child(SharedString::from(format!("{} · {}", group.tracker.tracker.as_str(), group.tracker.host)))
                    .child(group.rows.len().to_string()),
            );
            for row in &group.rows {
                let entry = row.entry.clone();
                let env = row.environment_id.clone();
                let (icon, tone) = issue_state_presentation(&theme, &entry.state);
                let selected = self.selected.as_deref() == Some(&entry.url);
                column = column.child(
                    div()
                        .id(SharedString::from(format!("issue-row-{}", entry.url)))
                        .test_support()
                        .flex()
                        .gap(rems(0.5))
                        .px(rems(0.75))
                        .py(rems(0.625))
                        .rounded(theme.radii.md)
                        .when(selected, |el| el.bg(theme.colors.accent_surface))
                        .hover(|s| s.bg(theme.colors.accent_surface.opacity(0.6)))
                        .cursor_pointer()
                        .on_click(cx.listener({
                            let entry = entry.clone();
                            move |this, _, _, cx| this.open(env.clone(), &entry, cx)
                        }))
                        .child(div().pt(rems(0.1875)).child(Icon::new(icon).size(IconSize::Md).color(tone)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(rems(0.125))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(rems(0.375))
                                        .child(div().mono_text(theme.rem(0.75), weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(entry.display_key.clone())))
                                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(SharedString::from(entry.title.clone())))
                                        .children(entry.labels.iter().take(3).map(|l| label_chip(&theme, &l.name, l.color.as_deref())))
                                        .child(div().ml_auto().flex().gap(rems(0.125)).children(entry.assignees.iter().map(|a| avatar(&theme, Some(&a.login), 16.)))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(rems(0.5))
                                        .ui_text(&theme, TextSize::Xs3, weight::NORMAL)
                                        .text_color(theme.colors.muted_foreground)
                                        .child(SharedString::from(entry.state.name.clone()))
                                        .child(SharedString::from(entry.author.as_ref().map_or("ghost".into(), |a| a.login.clone())))
                                        .when(entry.comment_count > 0, |el| el.child(div().flex().gap(theme.rem(0.1875)).items_center().child(Icon::new(IconName::MessageSquare).size(IconSize::Custom(theme.rem(0.625)))).child(entry.comment_count.to_string())))
                                        .child(div().ml_auto().child(relative_label(&entry.updated_at, now))),
                                ),
                        ),
                );
            }
            for error in &group.errors {
                column = column.child(div().px(rems(0.75)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(format!("{}: {}", error.project_title, error.message))));
            }
            for env in group.more.clone() {
                let host = group.tracker.host.clone();
                column = column.child(
                    div().px(rems(0.75)).pt(rems(0.375)).child(
                        Button::new(SharedString::from(format!("issues-more-{}-{host}", env.as_str())))
                            .size(ButtonSize::Xs)
                            .variant(ButtonVariant::Outline)
                            .label(t!("Load more"))
                            .on_click(cx.listener(move |this, _, _, cx| this.load_more(&env, &host, cx))),
                    ),
                );
            }
            list = list.child(column);
        }
        div()
            .id("issues-page")
            .test_support()
            .relative()
            .child(crate::components::pull_request::pull_requests_page::width_probe(cx.entity().downgrade(), self.list_width.clone()))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.canvas)
            .child(div().flex().flex_none().items_center().h(theme.spacing.workspace_topbar_height).px(theme.spacing.workspace_gutter).ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(theme.colors.text).child(t!("Issues")))
            .child(
                div()
                    .id("issues-body")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        // `scrollbar-gutter-both` around `WorkspacePageContainer width="expanded"`.
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .max_w(theme.rem(72.) - theme.spacing.app_scrollbar_width * 2.)
                            .mx_auto()
                            .gap(theme.sp(4.))
                            .px(theme.sp(6.) + theme.spacing.app_scrollbar_width)
                            .pt(theme.sp(6.))
                            .pb(theme.sp(12.))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(theme.sp(2.))
                                    .child({
                                        // `basis-full @lg/issue-list:min-w-48 @lg/issue-list:basis-0 @lg/issue-list:flex-1`.
                                        let wide = self.list_width.get() >= theme.rem(32.);
                                        div().id("issues-search").min_w_0().when(wide, |d| d.flex_1().min_w(theme.rem(12.))).when(!wide, |d| d.w_full()).child(TextInput::new(&self.search).size(InputSize::Lg).leading_icon(IconName::Search))
                                    })
                                    .child(sort_menu)
                                    .child(filters)
                                    .child(self.views_menu.clone())
                                    .children(tracker_menu)
                                    .child(
                                        Button::new("issues-refresh")
                                            .size(ButtonSize::Icon)
                                            .variant(ButtonVariant::Outline)
                                            .icon(IconName::RefreshCw)
                                            .disabled(self.loads.values().any(|l| matches!(l, Load::Loading)))
                                            .on_click(cx.listener(|this, _, _, cx| this.load(false, cx))),
                                    ),
                            )
                            .when(!failed.is_empty(), |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap(rems(0.75))
                                        .rounded(theme.radii.lg)
                                        .border_1()
                                        .border_color(theme.colors.warning.opacity(0.3))
                                        .bg(theme.colors.warning_surface)
                                        .px(rems(0.75))
                                        .py(rems(0.5))
                                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                                        .text_color(theme.colors.text)
                                        .child(div().flex().flex_col().children(failed.iter().map(|(l, m)| div().child(SharedString::from(format!("{l}: {m}"))))))
                                        .child(Button::new("issues-retry").size(ButtonSize::Sm).variant(ButtonVariant::Outline).label(t!("Retry")).on_click(cx.listener(|this, _, _, cx| this.load(true, cx)))),
                                )
                            })
                            // No tracker at all: `Empty` with the issue glyph, as `IssuesPage.tsx` shows it.
                            .when(self.groups.is_empty() && !self.loads.values().any(|l| matches!(l, Load::Loading)), |el| {
                                el.child(
                                    div().flex_1().min_h(theme.rem(28.)).flex().items_center().justify_center().child(
                                        crate::components::ui::empty::Empty::new(t!("No trackers to read"))
                                            .icon(IconName::CircleDot)
                                            .description(t!("Add a project whose remote is on a supported tracker.")),
                                    ),
                                )
                            })
                            .when(!self.groups.is_empty() && self.groups.iter().all(|g| g.rows.is_empty()) && !self.loads.values().any(|l| matches!(l, Load::Loading)), |el| {
                                el.child(div().py(rems(2.)).flex().justify_center().text_color(theme.colors.muted_foreground).child(t!("No issues under these filters.")))
                            })
                            .child(list),
                    ),
            )
    }
}

/// The issue Filters menu: State, Involvement, Labels and Assignee submenus, the open one beside
/// its row. Ports `IssueFiltersMenu`.
fn issue_filters_popup(this: &Entity<IssuesPage>, handle: PopupHandle, cx: &mut App) -> AnyElement {
    let page = this.read(cx);
    let theme = cx.app_theme().clone();
    let (label_facets, people) = page.facets();
    let (state, involvement, labels, assignee, open) = (page.state, page.involvement, page.labels.clone(), page.assignee.clone(), page.submenu);
    let state_options = [
        (IssueListState::Open, t!("Open", context = "issue state"), IconName::CircleDot),
        (IssueListState::Closed, t!("Closed", context = "issue state"), IconName::CircleCheck),
        (IssueListState::All, t!("All", context = "issue filter"), IconName::Layers),
    ];
    let involvement_options = [
        (IssueInvolvement::All, t!("Everyone", context = "issue filter"), IconName::Users),
        (IssueInvolvement::Assigned, t!("Assigned to me"), IconName::UserCheck),
        (IssueInvolvement::Authored, t!("Opened by me"), IconName::PenLine),
        (IssueInvolvement::Mentioned, t!("Mentioning me"), IconName::AtSign),
    ];
    let row_h = theme.sp(7.);
    let current_state = state_options.iter().find(|o| o.0 == state).cloned().unwrap_or(state_options[0].clone());
    let current_involvement = involvement_options.iter().find(|o| o.0 == involvement).cloned().unwrap_or(involvement_options[0].clone());
    let labels_detail: SharedString = match labels.len() {
        0 => t!("Any", context = "no label filter"),
        1 => labels[0].clone().into(),
        n => t!("{count, plural, one {{count} label} other {{count} labels}}", count = n),
    };
    let assignee_detail: SharedString = assignee.clone().map(SharedString::from).unwrap_or_else(|| t!("Anyone", context = "no assignee filter"));
    let trigger = |submenu: IssueSubmenu, label: SharedString, icon: IconName, detail: SharedString| {
        let hover = this.clone();
        let click = this.clone();
        let mut item = MenuItem::new(label)
            .id(format!("issue-filter-sub-{submenu:?}").to_lowercase())
            .kind(MenuItemKind::Submenu)
            .icon(icon)
            .icon_size(IconSize::Sm)
            .detail(detail)
            .on_hover(move |hovered, _, cx| {
                if *hovered {
                    hover.update(cx, |p, cx| p.set_submenu(Some(submenu), cx));
                }
            })
            .on_select(move |_, cx| {
                click.update(cx, |p, cx| {
                    p.keep_filters_open = true;
                    p.set_submenu(Some(submenu), cx)
                })
            });
        if open == Some(submenu) {
            item = item.highlighted();
        }
        item
    };
    let close = handle.clone();
    let reset = this.clone();
    let menu = MenuPopup::new()
        .item(trigger(IssueSubmenu::State, t!("State", context = "issue state filter"), current_state.2, current_state.1.clone()))
        .item(trigger(IssueSubmenu::Involvement, t!("Involvement"), current_involvement.2, current_involvement.1.clone()))
        .separator()
        .item(trigger(IssueSubmenu::Labels, t!("Labels"), IconName::Tag, labels_detail))
        .item(trigger(IssueSubmenu::Assignee, t!("Assignee"), IconName::User, assignee_detail))
        .on_dismiss(move |_, cx| {
            let keep = reset.update(cx, |p, _| std::mem::take(&mut p.keep_filters_open));
            if !keep {
                reset.update(cx, |p, cx| p.set_submenu(None, cx));
                close.close(cx);
            }
        });
    let offset: Pixels = match open {
        Some(IssueSubmenu::State) | None => Pixels::ZERO,
        Some(IssueSubmenu::Involvement) => row_h,
        Some(IssueSubmenu::Labels) => row_h * 2. + theme.sp(2.) + px(1.),
        Some(IssueSubmenu::Assignee) => row_h * 3. + theme.sp(2.) + px(1.),
    };
    let sub = open.map(|submenu| {
        let mut popup = MenuPopup::new();
        match submenu {
            IssueSubmenu::State => {
                popup = popup.label(t!("State", context = "issue state filter"));
                for (value, label, icon) in state_options.clone() {
                    let this = this.clone();
                    popup = popup.item(MenuItem::new(label).icon(icon).icon_size(IconSize::Sm).kind(MenuItemKind::Radio(value == state)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_state(value, cx))));
                }
            }
            IssueSubmenu::Involvement => {
                popup = popup.label(t!("Involvement"));
                for (value, label, icon) in involvement_options.clone() {
                    let this = this.clone();
                    popup = popup.item(MenuItem::new(label).icon(icon).icon_size(IconSize::Sm).kind(MenuItemKind::Radio(value == involvement)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_involvement(value, cx))));
                }
            }
            IssueSubmenu::Labels => {
                if label_facets.is_empty() {
                    popup = popup.item(MenuItem::new(t!("No labels found")).disabled(true));
                }
                for name in label_facets.clone() {
                    let on = labels.contains(&name);
                    let held = labels.clone();
                    let this = this.clone();
                    popup = popup.item(MenuItem::new(SharedString::from(name.clone())).kind(MenuItemKind::Checkbox(on)).on_select(move |_, cx| {
                        let next: Vec<String> = if on { held.iter().filter(|l| **l != name).cloned().collect() } else { held.iter().cloned().chain([name.clone()]).collect() };
                        this.update(cx, |p, cx| p.set_labels(next, cx))
                    }));
                }
            }
            IssueSubmenu::Assignee => {
                let anyone = this.clone();
                popup = popup.item(
                    MenuItem::new(t!("Anyone", context = "no assignee filter")).kind(MenuItemKind::Radio(assignee.is_none())).on_select(move |_, cx| anyone.update(cx, |p, cx| p.set_assignee(None, cx))),
                );
                for login in people.clone() {
                    let on = assignee.as_deref() == Some(login.as_str());
                    let this = this.clone();
                    popup = popup.item(MenuItem::new(SharedString::from(login.clone())).kind(MenuItemKind::Radio(on)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_assignee(Some(login.clone()), cx))));
                }
            }
        }
        let close = handle.clone();
        let reset = this.clone();
        popup.on_dismiss(move |_, cx| {
            reset.update(cx, |p, cx| p.set_submenu(None, cx));
            close.close(cx);
        })
    });
    let mut row = div().flex().items_start().child(menu);
    if let Some(popup) = sub {
        row = row.child(div().mt(px(1.) + theme.sp(1.) + offset - theme.rem(0.3125)).max_h(theme.rem(20.)).child(popup));
    }
    row.into_any_element()
}

/// `issueTrackerPresentation`: a git host's tracker takes the host's name; Linear its own; any
/// other its kind.
fn issue_tracker_presentation(kind: &str) -> (String, IconName) {
    match kind {
        "linear" => ("Linear".into(), IconName::Ticket),
        _ => match crate::source_control_presentation::SOURCE_CONTROL_DEFINITIONS.iter().find(|d| d.kind == kind) {
            Some(d) => (d.label.into(), IconName::GitPullRequestArrow),
            None => (kind.into(), IconName::Ticket),
        },
    }
}
