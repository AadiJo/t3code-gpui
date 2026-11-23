//! The pull request and issue surfaces on fixture data.
//!
//! - `pull-requests`: the pull requests page across two environments, one of
//!   which fails its first load and shows the retry banner.
//! - `pull-requests-summary`, `-guide`, `-timeline`, `-code`: the page with a
//!   pull request open in the right panel on that tab.
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
