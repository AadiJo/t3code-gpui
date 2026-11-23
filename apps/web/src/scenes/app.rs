//! The whole window: titlebar, sidebar, the showcase's working thread in the
//! chat view with the real composer, and the panel toggles.

use gpui_kit::{AnyView, App, AppContext as _, Window};

use crate::AppRoot;
use crate::app_root::AppRootOptions;
use crate::components::app_sidebar_layout::TitlebarPlatform;
use crate::router::Route;

/// The thread the scene opens (the showcase thread with every row kind).
const SCENE_THREAD: &str = "/env-local/a1";

fn build_for(platform: TitlebarPlatform, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let root = AppRoot::with_options(AppRootOptions::fixtures(platform), window, cx);
        root.layout().update(cx, |layout, _| layout.force_caption_buttons(true));
        let timeline = root.thread_view().read(cx).chat().read(cx).timeline().clone();
        timeline.update(cx, |timeline, _| {
            timeline.set_now_millis(crate::components::chat::timeline_fixtures::FIXTURE_NOW)
        });
        root.navigate(Route::parse(SCENE_THREAD), cx);
        root
    })
    .into()
}

/// Windows/Linux layout: 40px topbar with caption buttons.
pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    build_for(TitlebarPlatform::Overlay, window, cx)
}

/// macOS layout: 52px topbar, traffic-light inset.
pub fn build_mac(window: &mut Window, cx: &mut App) -> AnyView {
    build_for(TitlebarPlatform::Mac, window, cx)
}
