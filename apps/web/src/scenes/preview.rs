//! The streamed browser panel with fixture tabs and generated frames, as the
//! right panel shows it: two tabs, light, and the empty state.

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Context, Entity, IntoElement, Render, Window, div, prelude::*};
use t3code_contracts::ThreadId;

use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::components::preview::preview_source::{FixturePreviewSource, fixture_session};
use crate::components::preview::preview_view::PreviewView;
use crate::theme_palette::{Appearance, ThemeDocument, set_app_theme};

struct PreviewScene {
    view: Entity<PreviewView>,
}

impl Render for PreviewScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        // The right panel's left edge.
        div().size_full().flex().bg(theme.colors.background()).border_l_1().border_color(theme.colors.border).child(self.view.clone())
    }
}

fn build_variant(light: bool, empty: bool, window: &mut Window, cx: &mut App) -> AnyView {
    if light {
        set_app_theme(AppTheme::new(&ThemeDocument::graphite(), Appearance::Light), cx);
    }
    let thread = ThreadId::new("thread-preview");
    let sessions = if empty {
        Vec::new()
    } else {
        vec![
            fixture_session(&thread, "tab-1", "http://localhost:5173/dashboard", "Dashboard · Acme"),
            fixture_session(&thread, "tab-2", "http://localhost:6006/", "Storybook"),
        ]
    };
    let source = FixturePreviewSource::new(sessions);
    let view = cx.new(|cx| PreviewView::new(source as Rc<_>, thread, "project-acme".into(), None, window, cx));
    cx.new(|_| PreviewScene { view }).into()
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
