//! The chat view on a fixture thread that holds every timeline row kind and
//! every markdown element. `chat-light` renders it in the light appearance.

use gpui_kit::{AnyView, App, Window};

use crate::{
    components::{
        chat::timeline_fixtures::{FIXTURE_NOW, fixture_thread},
        chat_view::chat_view,
    },
    theme_palette::{Appearance, ThemeDocument, set_app_theme},
    AppTheme,
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
    set_app_theme(AppTheme::new(&ThemeDocument::graphite(), Appearance::Light), cx);
    build(window, cx)
}
