//! The issues page: issues from every connected tracker across environments,
//! grouped by tracker, with state, involvement and sort filters, search,
//! "Load more" per tracker, a retry that asks only failed environments again,
//! and a template-driven new issue form. Ports `IssuesPage.tsx` and
//! `IssueRow.tsx`.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
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
use crate::components::ui::input::TextInput;
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
use crate::i18n::t;
use crate::{ActiveAppTheme as _, AppTheme};

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
    Created(IssueCreateResult),
}

enum Load {
    Loading,
    Loaded(IssueListResult),
    Failed(SharedString),
}

/// A new issue being written from a template.
struct Draft {
    templates: IssueTemplateList,
    template: Option<String>,
    title: Entity<InputState>,
    body: Entity<TextareaState>,
    labels: Vec<String>,
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
    draft: Option<Draft>,
    pub now_ms: Option<i64>,
    _search: Subscription,
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
            draft: None,
            now_ms: None,
            _search: sub,
        };
        page.load(false, cx);
        page
    }

    fn input(&self) -> IssueListInput {
        IssueListInput {
            state: self.state,
            involvement: Some(self.involvement),
            project_id: None,
            exclude_project_ids: None,
            host: None,
            limit: Some(50),
            cursors: None,
            refresh: None,
            query: None,
            labels: None,
            assignee: None,
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
        cx.notify();
    }

    /// The rows shown, as `display key` per tracker group.
    pub fn visible(&self) -> Vec<(String, Vec<String>)> {
        self.groups.iter().map(|g| (g.tracker.host.clone(), g.rows.iter().map(|r| r.entry.display_key.clone()).collect())).collect()
    }

    /// The state filter.
    pub fn set_state(&mut self, state: IssueListState, cx: &mut Context<Self>) {
        self.state = state;
        self.load(false, cx);
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

    /// Opens the new issue form, reading the first tracker's templates.
    pub fn begin_create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.groups.first() else { return };
        let Some(env) = self.sources.first().cloned() else { return };
        let entry = group.rows.first().map(|r| r.entry.clone());
        let scope = IssueScopeRef {
            project_id: entry.as_ref().map_or_else(|| ProjectId::new("proj"), |e| e.project_id.clone()),
            tracker: group.tracker.tracker.clone(),
            host: Some(group.tracker.host.clone()),
            scope: entry.as_ref().map_or_else(String::new, |e| e.scope.clone()),
        };
        let title = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Issue title")));
        let body = cx.new(|cx| TextareaState::new(window, cx).placeholder(t!("Description")));
        self.draft = Some(Draft { templates: IssueTemplateList { templates: vec![], external_templates: None, blank_issues_enabled: true }, template: None, title, body, labels: vec![] });
        let request = env.source.templates(scope);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if let (Ok(t), Some(d)) = (result, this.draft.as_mut()) {
                    d.templates = t;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Fills the form from a template.
    pub fn apply_template(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_mut() else { return };
        let Some(template) = draft.templates.templates.iter().find(|t| t.key == key).cloned() else { return };
        draft.template = Some(key.to_owned());
        draft.labels = template.labels.clone();
        draft.title.update(cx, |i, cx| i.set_value(template.title.clone(), window, cx));
        draft.body.update(cx, |i, cx| i.set_value(template.body.clone(), window, cx));
        cx.notify();
    }

    /// Files the drafted issue.
    pub fn submit_create(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.take() else { return };
        let Some(group) = self.groups.first() else { return };
        let Some(env) = self.sources.first().cloned() else { return };
        let entry = group.rows.first().map(|r| r.entry.clone());
        let input = IssueCreateInput {
            project_id: entry.as_ref().map_or_else(|| ProjectId::new("proj"), |e| e.project_id.clone()),
            tracker: group.tracker.tracker.clone(),
            host: Some(group.tracker.host.clone()),
            scope: entry.as_ref().map_or_else(String::new, |e| e.scope.clone()),
            title: draft.title.read(cx).value().to_string(),
            body: draft.body.read(cx).value().to_string(),
            labels: draft.labels,
            assignees: vec![],
        };
        let request = env.source.create(input);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if let Ok(created) = result {
                    cx.emit(IssuesPageEvent::Created(created));
                }
                this.load(false, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_draft(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.draft.as_ref()?;
        let templates = draft.templates.templates.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(12.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(theme.colors.border)
                .child(
                    div().flex().flex_wrap().gap(px(6.)).children(templates.into_iter().map(|t| {
                        let key = t.key.clone();
                        let on = draft.template.as_deref() == Some(&t.key);
                        Button::new(SharedString::from(format!("issue-template-{key}")))
                            .size(ButtonSize::Xs)
                            .variant(if on { ButtonVariant::Secondary } else { ButtonVariant::Outline })
                            .label(SharedString::from(t.name.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| this.apply_template(&key, window, cx)))
                    })),
                )
                .child(div().id("issue-new-title").child(TextInput::new(&draft.title)))
                .child(div().id("issue-new-body").h(px(96.)).child(Textarea::new(&draft.body).h(px(96.))))
                .child(div().flex().flex_wrap().gap(px(6.)).children(draft.labels.iter().map(|l| label_chip(theme, l, None))))
                .child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(Button::new("issue-new-submit").size(ButtonSize::Sm).label(t!("Save")).on_click(cx.listener(|this, _, _, cx| this.submit_create(cx))))
                        .child(Button::new("issue-new-cancel").size(ButtonSize::Sm).variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                            this.draft = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }
}

fn state_label(state: IssueListState) -> SharedString {
    match state {
        IssueListState::Open => t!("Open", context = "issue state"),
        IssueListState::Closed => t!("Closed", context = "issue state"),
        IssueListState::All => t!("All", context = "issue filter"),
    }
}

fn involvement_label(involvement: IssueInvolvement) -> SharedString {
    match involvement {
        IssueInvolvement::All => t!("Everyone", context = "issue filter"),
        IssueInvolvement::Assigned => t!("Assigned to me"),
        IssueInvolvement::Authored => t!("Opened by me"),
        IssueInvolvement::Mentioned => t!("Mentioning me"),
    }
}

impl Render for IssuesPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let now = self.now_ms.unwrap_or_else(crate::timestamp_format::now_ms);
        let this = cx.entity();
        let state = self.state;
        let involvement = self.involvement;
        let filters = dropdown_menu("issues-filters", Button::new("issues-filters-trigger").size(ButtonSize::Sm).variant(ButtonVariant::Outline).icon(IconName::ListFilter).label(involvement_label(involvement)), move |_, _| {
            let mut menu = MenuPopup::new().label(t!("State", context = "issue state filter"));
            for s in [IssueListState::Open, IssueListState::Closed, IssueListState::All] {
                let this = this.clone();
                menu = menu.item(MenuItem::new(state_label(s)).kind(MenuItemKind::Radio(s == state)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_state(s, cx))));
            }
            menu = menu.separator().label(t!("Involvement"));
            for i in [IssueInvolvement::All, IssueInvolvement::Assigned, IssueInvolvement::Authored, IssueInvolvement::Mentioned] {
                let this = this.clone();
                menu = menu.item(MenuItem::new(involvement_label(i)).kind(MenuItemKind::Radio(i == involvement)).on_select(move |_, cx| this.update(cx, |p, cx| p.set_involvement(i, cx))));
            }
            menu
        });
        let failed: Vec<(SharedString, SharedString)> = self
            .sources
            .iter()
            .filter_map(|s| match self.loads.get(&s.environment_id) {
                Some(Load::Failed(m)) => Some((s.label.clone(), m.clone())),
                _ => None,
            })
            .collect();
        let mut list = div().flex().flex_col().gap(px(16.));
        for group in &self.groups {
            let mut column = div().flex().flex_col().child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .pb(px(6.))
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
                        .flex()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(10.))
                        .rounded(theme.radii.md)
                        .when(selected, |el| el.bg(theme.colors.accent))
                        .hover(|s| s.bg(theme.colors.accent.opacity(0.6)))
                        .cursor_pointer()
                        .on_click(cx.listener({
                            let entry = entry.clone();
                            move |this, _, _, cx| this.open(env.clone(), &entry, cx)
                        }))
                        .child(div().pt(px(3.)).child(Icon::new(icon).size(IconSize::Md).color(tone)))
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
                                        .items_center()
                                        .gap(px(6.))
                                        .child(div().mono_text(px(12.), weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(entry.display_key.clone())))
                                        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(SharedString::from(entry.title.clone())))
                                        .children(entry.labels.iter().take(3).map(|l| label_chip(&theme, &l.name, l.color.as_deref())))
                                        .child(div().ml_auto().flex().gap(px(2.)).children(entry.assignees.iter().map(|a| avatar(&theme, Some(&a.login), 16.)))),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap(px(8.))
                                        .ui_text(&theme, TextSize::Xs3, weight::NORMAL)
                                        .text_color(theme.colors.muted_foreground)
                                        .child(SharedString::from(entry.state.name.clone()))
                                        .child(SharedString::from(entry.author.as_ref().map_or("ghost".into(), |a| a.login.clone())))
                                        .when(entry.comment_count > 0, |el| el.child(div().flex().gap(px(3.)).items_center().child(Icon::new(IconName::MessageSquare).size(IconSize::Custom(px(10.)))).child(entry.comment_count.to_string())))
                                        .child(div().ml_auto().child(relative_label(&entry.updated_at, now))),
                                ),
                        ),
                );
            }
            for error in &group.errors {
                column = column.child(div().px(px(12.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(format!("{}: {}", error.project_title, error.message))));
            }
            for env in group.more.clone() {
                let host = group.tracker.host.clone();
                column = column.child(
                    div().px(px(12.)).pt(px(6.)).child(
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
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background())
            .child(div().flex().items_center().h(px(48.)).px(px(16.)).border_b_1().border_color(theme.colors.border).ui_text(&theme, TextSize::Sm, weight::MEDIUM).text_color(theme.colors.text).child(t!("Issues")))
            .child(
                div()
                    .id("issues-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .px(px(24.))
                            .py(px(16.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(div().id("issues-search").flex_1().child(TextInput::new(&self.search)))
                                    .child(filters)
                                    .child(Button::new("issues-new").size(ButtonSize::Sm).icon(IconName::Plus).label(SharedString::new_static("New issue")).on_click(cx.listener(|this, _, window, cx| this.begin_create(window, cx))))
                                    .child(Button::new("issues-refresh").size(ButtonSize::Icon).variant(ButtonVariant::Outline).icon(IconName::RefreshCw).on_click(cx.listener(|this, _, _, cx| this.load(false, cx)))),
                            )
                            .children(self.render_draft(&theme, cx))
                            .when(!failed.is_empty(), |el| {
                                el.child(
                                    div()
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
                                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                                        .text_color(theme.colors.text)
                                        .child(div().flex().flex_col().children(failed.iter().map(|(l, m)| div().child(SharedString::from(format!("{l}: {m}"))))))
                                        .child(Button::new("issues-retry").size(ButtonSize::Sm).variant(ButtonVariant::Outline).label(t!("Retry")).on_click(cx.listener(|this, _, _, cx| this.load(true, cx)))),
                                )
                            })
                            .when(self.groups.iter().all(|g| g.rows.is_empty()) && !self.loads.values().any(|l| matches!(l, Load::Loading)), |el| {
                                el.child(div().py(px(32.)).flex().justify_center().text_color(theme.colors.muted_foreground).child(t!("No issues under these filters.")))
                            })
                            .child(list),
                    ),
            )
    }
}
