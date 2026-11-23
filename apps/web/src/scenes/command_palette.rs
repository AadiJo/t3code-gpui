//! The command palette over the app: the root page searching "theme", the
//! New thread in… page, and a query with no results.

use std::rc::Rc;

use gpui_kit::*;

use crate::AppRoot;
use crate::command_palette_store::OpenIntent;
use crate::components::command_palette_lookups::FixtureSource;
use crate::components::sidebar::fixtures;
use crate::shortcuts::ShortcutRoot;

fn palette_scene(intent: OpenIntent, window: &mut Window, cx: &mut App) -> AnyView {
    let snapshot = fixtures::showcase_snapshot();
    let source = Rc::new(FixtureSource::new(&snapshot));
    let app = cx.new(|cx| AppRoot::new(window, cx));
    let root = cx.new(|cx| ShortcutRoot::new(app.into(), source, window, cx));
    let palette = root.read(cx).palette().clone();
    // Before the first frame nothing is rendered to judge availability from,
    // so scene rows all look available; the palette check covers availability.
    palette.update(cx, |palette, cx| palette.open(intent, window, cx));
    root.into()
}

/// The root page with "theme" typed.
pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    palette_scene(OpenIntent::Search("theme".into()), window, cx)
}

/// The root page before typing: actions and recent threads.
pub fn build_root(window: &mut Window, cx: &mut App) -> AnyView {
    palette_scene(OpenIntent::Search("".into()), window, cx)
}

/// The New thread in… page.
pub fn build_nested(window: &mut Window, cx: &mut App) -> AnyView {
    palette_scene(OpenIntent::NewThreadIn, window, cx)
}

/// A query nothing matches.
pub fn build_empty(window: &mut Window, cx: &mut App) -> AnyView {
    palette_scene(OpenIntent::Search("zzqxv".into()), window, cx)
}
