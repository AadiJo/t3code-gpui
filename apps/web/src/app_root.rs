//! Root view of the main window. Ports `AppRoot.tsx` / `__root.tsx`: the
//! sidebar layout with the sidebar on the left and route content in the
//! main pane.

use gpui_kit::*;

use crate::components::app_sidebar_layout::{AppSidebarLayout, TitlebarPlatform};
use crate::components::sidebar::model::SidebarSnapshot;
use crate::components::sidebar::{Sidebar, fixtures};

/// Root view of the main window.
pub struct AppRoot {
    layout: Entity<AppSidebarLayout>,
}

impl AppRoot {
    /// The window with fixture sidebar data, until the runtime feeds it.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_snapshot(fixtures::showcase_snapshot(), TitlebarPlatform::current(), window, cx)
    }

    /// The window drawing `snapshot` with the given titlebar layout.
    pub fn with_snapshot(
        snapshot: SidebarSnapshot,
        platform: TitlebarPlatform,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(snapshot, window, cx));
        let layout = cx.new(|cx| AppSidebarLayout::new(sidebar, platform, cx));
        Self { layout }
    }

    pub fn layout(&self) -> &Entity<AppSidebarLayout> {
        &self.layout
    }

    /// Starts live clocks (durations, relative times). Off in scenes.
    pub fn start_clock(&self, cx: &mut App) {
        let sidebar = self.layout.read(cx).sidebar().clone();
        sidebar.update(cx, |sidebar, cx| sidebar.start_clock(cx));
    }
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.layout.clone())
    }
}
