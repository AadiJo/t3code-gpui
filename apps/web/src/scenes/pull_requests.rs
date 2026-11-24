//! The pull request and issue surfaces on fixture data.
//!
//! - `pull-requests`: the pull requests page across two environments, one of
//!   which fails its first load and shows the retry banner.
//! - `pull-requests-summary`, `-guide`, `-timeline`, `-code`: the page with a
//!   pull request open in the right panel on that tab.
//! - `pull-requests-save-view`: the page with the Save PR view dialog open.
//! - `pull-requests-empty`: the page with no pull requests listed.
//! - `issues-page`: the issues page alone, full width.
//! - `issues`: the issues page with the issue panel open.
//!
//! `-light` variants render in the light appearance.
//! `examples/pull_requests_check.rs` scripts these scenes.

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Window};
use t3code_contracts::EnvironmentId;

use crate::components::pull_request::pull_request_fixture::FixtureSource;
use crate::components::pull_request::pull_requests_page::{EnvironmentListSource, PullRequestsPage};

/// 2026-11-23T10:00:00Z: the scenes' clock, so relative times do not drift.
pub const SCENE_NOW_MS: i64 = 1_795_428_000_000;

/// The two environments the page reads; `devbox` fails its first load.
pub fn fixture_sources(fixture: &FixtureSource) -> Vec<EnvironmentListSource> {
    let devbox_list = fixture.state.lock().unwrap().data.list_devbox.clone();
    vec![
        EnvironmentListSource { environment_id: EnvironmentId::new("env-studio"), label: "Studio".into(), source: Rc::new(fixture.clone()) },
        EnvironmentListSource { environment_id: EnvironmentId::new("env-devbox"), label: "devbox".into(), source: Rc::new(fixture.with_list(devbox_list, true)) },
    ]
}

/// The pull requests page with nothing listed (the fork capture's empty workspace).
pub fn build_empty(window: &mut Window, cx: &mut App) -> AnyView {
    let fixture = FixtureSource::standard();
    let mut empty = fixture.state.lock().unwrap().data.list_studio.clone();
    empty.entries.clear();
    empty.errors.clear();
    empty.truncated = false;
    let sources = vec![EnvironmentListSource { environment_id: EnvironmentId::new("env-studio"), label: "Studio".into(), source: Rc::new(fixture.with_list(empty, false)) }];
    cx.new(|cx| {
        let mut page = PullRequestsPage::new(sources, window, cx);
        page.now_ms = Some(SCENE_NOW_MS);
        page
    })
    .into()
}

/// The pull requests page with the Save PR view dialog open.
pub fn build_save_view(window: &mut Window, cx: &mut App) -> AnyView {
    let fixture = FixtureSource::standard();
    let sources = fixture_sources(&fixture);
    let page = cx.new(|cx| {
        let mut page = PullRequestsPage::new(sources, window, cx);
        page.now_ms = Some(SCENE_NOW_MS);
        page
    });
    let menu = page.read(cx).views_menu().clone();
    menu.update(cx, |m, cx| m.open_editor(None, window, cx));
    page.into()
}

/// The pull requests page.
pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    let fixture = FixtureSource::standard();
    let sources = fixture_sources(&fixture);
    cx.new(|cx| {
        let mut page = PullRequestsPage::new(sources, window, cx);
        page.now_ms = Some(SCENE_NOW_MS);
        page
    })
    .into()
}

use gpui_kit::{Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Subscription, div, px};

use crate::components::pull_request::pull_request_detail_panel::{DetailTab, PullRequestDetailEvent, PullRequestDetailPanel};
use crate::components::pull_request::pull_request_source::{PullRequestDetailSource, pull_request_ref};
use crate::components::pull_request::pull_requests_page::PullRequestsPageEvent;
use crate::ActiveAppTheme as _;

/// The page with a pull request open beside it, as the page's right panel lays it out.
pub struct PullRequestsWorkspace {
    pub page: Entity<PullRequestsPage>,
    pub detail: Option<Entity<PullRequestDetailPanel>>,
    pub fixture: FixtureSource,
    _events: Subscription,
    _detail_events: Option<Subscription>,
}

impl PullRequestsWorkspace {
    /// The page over the fixture environments, with `#482` open on `tab` when given.
    pub fn new(tab: Option<DetailTab>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fixture = FixtureSource::standard();
        let sources = fixture_sources(&fixture);
        let page = cx.new(|cx| {
            let mut page = PullRequestsPage::new(sources, window, cx);
            page.now_ms = Some(SCENE_NOW_MS);
            page
        });
        let events = cx.subscribe_in(&page, window, |this, _, event: &PullRequestsPageEvent, window, cx| {
            let PullRequestsPageEvent::Open { reference, .. } = event;
            this.open(reference.clone(), DetailTab::Summary, window, cx);
        });
        let mut workspace = Self { page, detail: None, fixture, _events: events, _detail_events: None };
        if let Some(tab) = tab {
            let entry = workspace.fixture.state.lock().unwrap().data.list_studio.entries[0].clone();
            workspace.open(pull_request_ref(&entry), tab, window, cx);
        }
        workspace
    }

    /// Opens a pull request's detail on `tab`.
    pub fn open(&mut self, reference: t3code_contracts::PullRequestRef, tab: DetailTab, window: &mut Window, cx: &mut Context<Self>) {
        let source: Rc<dyn PullRequestDetailSource> = Rc::new(self.fixture.clone());
        let detail = cx.new(|cx| {
            let mut panel = PullRequestDetailPanel::new(source, reference, cx);
            panel.now_ms = Some(SCENE_NOW_MS);
            panel
        });
        detail.update(cx, |p, cx| {
            if tab == DetailTab::Guide {
                p.set_tab(DetailTab::Code, window, cx);
            }
            p.set_tab(tab, window, cx);
        });
        // A host action changes the row the list shows, so the list reads again.
        let page = self.page.clone();
        self._detail_events = Some(cx.subscribe(&detail, move |_, _, event: &PullRequestDetailEvent, cx| {
            if let PullRequestDetailEvent::Acted(_) = event {
                page.update(cx, |p, cx| p.load(false, cx));
            }
        }));
        self.detail = Some(detail);
        cx.notify();
    }
}

impl Render for PullRequestsWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.app_theme().colors.border;
        div()
            .size_full()
            .flex()
            .child(div().flex_1().min_w_0().h_full().child(self.page.clone()))
            .children(self.detail.clone().map(|d| div().w(px(620.)).h_full().border_l_1().border_color(border).child(d)))
    }
}

fn workspace(tab: DetailTab, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| PullRequestsWorkspace::new(Some(tab), window, cx)).into()
}

pub fn build_summary(window: &mut Window, cx: &mut App) -> AnyView {
    workspace(DetailTab::Summary, window, cx)
}

pub fn build_guide(window: &mut Window, cx: &mut App) -> AnyView {
    workspace(DetailTab::Guide, window, cx)
}

pub fn build_timeline(window: &mut Window, cx: &mut App) -> AnyView {
    workspace(DetailTab::Timeline, window, cx)
}

pub fn build_code(window: &mut Window, cx: &mut App) -> AnyView {
    workspace(DetailTab::Code, window, cx)
}

use crate::components::issue::issue_detail_panel::IssueDetailPanel;
use crate::components::issue::issues_page::{EnvironmentIssueSource, IssuesPage, IssuesPageEvent};
use crate::components::pull_request::pull_request_source::IssueSource;

/// The issues page with the issue panel beside it.
pub struct IssuesWorkspace {
    pub page: Entity<IssuesPage>,
    pub panel: Option<Entity<IssueDetailPanel>>,
    pub fixture: FixtureSource,
    _events: Subscription,
}

impl IssuesWorkspace {
    /// The issues page over the fixture, with `#455` open.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fixture = FixtureSource::standard();
        let source: Rc<dyn IssueSource> = Rc::new(fixture.clone());
        let sources = vec![EnvironmentIssueSource { environment_id: EnvironmentId::new("env-studio"), label: "Studio".into(), source: source.clone() }];
        let page = cx.new(|cx| {
            let mut page = IssuesPage::new(sources, window, cx);
            page.now_ms = Some(SCENE_NOW_MS);
            page
        });
        let events = cx.subscribe_in(&page, window, |this, _, event: &IssuesPageEvent, window, cx| {
            if let IssuesPageEvent::Open { reference, .. } = event {
                this.open(reference.clone(), window, cx);
            }
        });
        let entry = fixture.state.lock().unwrap().data.issue_list.entries[0].clone();
        let mut workspace = Self { page, panel: None, fixture, _events: events };
        let reference = t3code_contracts::IssueRef { project_id: entry.project_id, tracker: entry.tracker, host: Some(entry.host), scope: entry.scope, key: entry.key };
        workspace.open(reference, window, cx);
        workspace
    }

    /// Opens an issue in the panel.
    pub fn open(&mut self, reference: t3code_contracts::IssueRef, window: &mut Window, cx: &mut Context<Self>) {
        let source: Rc<dyn IssueSource> = Rc::new(self.fixture.clone());
        self.panel = Some(cx.new(|cx| {
            let mut panel = IssueDetailPanel::new(source, reference, window, cx);
            panel.now_ms = Some(SCENE_NOW_MS);
            panel
        }));
        cx.notify();
    }
}

impl Render for IssuesWorkspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.app_theme().colors.border;
        div()
            .size_full()
            .flex()
            .child(div().flex_1().min_w_0().h_full().child(self.page.clone()))
            .children(self.panel.clone().map(|p| div().w(px(560.)).h_full().border_l_1().border_color(border).child(p)))
    }
}

/// The issues page alone, full width, as the fork capture shows it with no issue open.
pub fn build_issues_page(window: &mut Window, cx: &mut App) -> AnyView {
    let fixture = FixtureSource::standard();
    let source: Rc<dyn IssueSource> = Rc::new(fixture);
    let sources = vec![EnvironmentIssueSource { environment_id: EnvironmentId::new("env-studio"), label: "Studio".into(), source }];
    cx.new(|cx| {
        let mut page = IssuesPage::new(sources, window, cx);
        page.now_ms = Some(SCENE_NOW_MS);
        page
    })
    .into()
}

pub fn build_issues(window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| IssuesWorkspace::new(window, cx)).into()
}
