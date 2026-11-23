//! The pull request surfaces: the pull requests page, the detail with its
//! Summary, Guide, Timeline and Code tabs, and their data sources. Mirrors
//! `apps/web/src/components/pullRequest`.

pub mod pull_request_detail_logic;
pub mod pull_request_list_logic;
pub mod pull_request_fixture;
pub mod pull_request_source;
pub mod pull_request_presentation;
pub mod pull_requests_page;

/// Registers the pull request surfaces' key bindings.
pub fn init(cx: &mut gpui_kit::App) {
    pull_requests_page::init(cx);
}
