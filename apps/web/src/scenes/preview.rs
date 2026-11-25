//! The streamed browser panel with fixture tabs and generated frames, as the
//! right panel shows it: two tabs, light, and the empty state.

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use t3code_contracts::ThreadId;

use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::components::preview::preview_source::{FixturePreviewSource, fixture_session};
use crate::components::preview::preview_view::PreviewView;
use t3code_client_runtime::preview::server_browser_stream::{Controller, PreviewStreamControl, PreviewStreamEvent};
use crate::theme_palette::{Appearance, ThemeDocument, set_app_theme};

struct PreviewScene {
    view: Entity<PreviewView>,
    source: Rc<FixturePreviewSource>,
    /// Stream events to deliver once the tab's stream opens.
    pending: Vec<PreviewStreamEvent>,
}

impl Render for PreviewScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.pending.is_empty() && self.view.read(cx).active_surface().is_some_and(|s| s.read(cx).settled()) {
            for event in self.pending.drain(..) {
                self.source.emit(event);
            }
            cx.notify();
        }
        let theme = cx.app_theme();
        // The right panel's left edge.
        div().size_full().flex().bg(theme.colors.background()).border_l_1().border_color(theme.colors.border).child(self.view.clone())
    }
}

/// What a variant shows beyond the two settled tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Tabs,
    Empty,
    AgentCursor,
    NoAccess,
    FileNotShown,
}

fn build_variant(light: bool, empty: bool, window: &mut Window, cx: &mut App) -> AnyView {
    build_with(light, if empty { Variant::Empty } else { Variant::Tabs }, window, cx)
}

fn build_with(light: bool, variant: Variant, window: &mut Window, cx: &mut App) -> AnyView {
    if light {
        set_app_theme(AppTheme::new(&ThemeDocument::graphite(), Appearance::Light), cx);
    }
    let thread = ThreadId::new("thread-preview");
    let mut sessions = match variant {
        Variant::Empty => Vec::new(),
        _ => vec![
            fixture_session(&thread, "tab-1", "http://localhost:5173/dashboard", "Dashboard · Acme"),
            fixture_session(&thread, "tab-2", "http://localhost:6006/", "Storybook"),
        ],
    };
    if variant == Variant::FileNotShown {
        sessions[1].nav_status = serde_json::from_value(serde_json::json!({
            "_tag": "LoadFailed", "url": "http://localhost:6006/report.csv", "title": "", "code": -3,
            "description": "ERR_ABORTED", "download": { "id": "dl-1", "fileName": "report.csv" }
        }))
        .expect("fixture status");
    }
    let source = FixturePreviewSource::new(sessions);
    let view = cx.new(|cx| {
        let mut view = PreviewView::new(source.clone() as Rc<_>, thread, "project-acme".into(), None, window, cx);
        if variant == Variant::NoAccess {
            view.set_can_operate(false, cx);
        }
        view
    });
    // An agent driving the page: it holds control and has just clicked at (420, 260).
    let pending = match variant {
        Variant::AgentCursor => vec![
            PreviewStreamEvent::Control(Some(PreviewStreamControl { can_operate: true, controller: Controller::Agent, generation: 2, dialog: None })),
            PreviewStreamEvent::Pointer { click: true, x: 420., y: 260., sequence: 1 },
        ],
        _ => Vec::new(),
    };
    cx.new(|_| PreviewScene { view, source, pending }).into()
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(false, false, window, cx)
}
pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(true, false, window, cx)
}
pub fn build_empty(window: &mut Window, cx: &mut App) -> AnyView {
    build_variant(false, true, window, cx)
}
pub fn build_agent_cursor(window: &mut Window, cx: &mut App) -> AnyView {
    build_with(false, Variant::AgentCursor, window, cx)
}
pub fn build_no_access(window: &mut Window, cx: &mut App) -> AnyView {
    build_with(false, Variant::NoAccess, window, cx)
}
pub fn build_file_not_shown(window: &mut Window, cx: &mut App) -> AnyView {
    build_with(false, Variant::FileNotShown, window, cx)
}
