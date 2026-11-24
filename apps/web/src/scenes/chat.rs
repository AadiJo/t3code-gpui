//! The chat view on a fixture thread that holds every timeline row kind and
//! every markdown element. `chat-light` renders it in the light appearance.

use gpui_kit::{AnyView, App, Window};

use crate::{
    components::{
        chat::timeline_fixtures::{FIXTURE_NOW, fixture_thread},
        chat_view::chat_view,
    },
    theme_palette::Appearance,
};

pub fn build(_window: &mut Window, cx: &mut App) -> AnyView {
    let view = chat_view(cx);
    view.update(cx, |view, cx| {
        view.timeline().update(cx, |timeline, _| timeline.set_now_millis(FIXTURE_NOW));
        view.set_thread("fixture".into(), "t3code".into(), "Sync engine audit".into(), fixture_thread(), cx);
    });
    view.into()
}

pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    crate::theme_palette::ThemeSettings::set_appearance(Appearance::Light, cx);
    build(window, cx)
}

/// The chat view on a real thread projection: `T3CODE_PROJECTION` names a JSON file holding an
/// array of wire-encoded projections (the first is shown), mapped the way live data is.
pub fn build_projection(_window: &mut Window, cx: &mut App) -> AnyView {
    let view = chat_view(cx);
    let path = std::env::var("T3CODE_PROJECTION").expect("T3CODE_PROJECTION names a projection file");
    let raw = std::fs::read(&path).expect("projection file");
    let mut projections: Vec<t3code_shared::thread_projection::ThreadProjection> = serde_json::from_slice(&raw).expect("projection JSON");
    let projection = projections.remove(0);
    let input = crate::session_logic::timeline_input(&mut crate::session_logic::TimelineMapper::default(), &projection, &[]);
    let title = projection.thread.title.clone();
    view.update(cx, |view, cx| {
        view.set_thread("projection".into(), "t3code".into(), title.into(), input, cx);
    });
    view.into()
}
