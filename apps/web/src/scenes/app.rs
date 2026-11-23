//! The whole window: titlebar, sidebar, main pane and toggles.

use gpui_kit::{AnyView, App, AppContext as _, Window};

use crate::AppRoot;
use crate::components::app_sidebar_layout::TitlebarPlatform;
use crate::components::sidebar::fixtures;

fn build_for(platform: TitlebarPlatform, window: &mut Window, cx: &mut App) -> AnyView {
    cx.new(|cx| {
        let root = AppRoot::with_snapshot(fixtures::showcase_snapshot(), platform, window, cx);
        root.layout().update(cx, |layout, _| layout.force_caption_buttons(true));
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
