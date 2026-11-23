//! Placeholder while the page is built.

use gpui_kit::{Context, IntoElement, Render, Window, div};

pub struct ArchivedThreadsPanel;

impl ArchivedThreadsPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for ArchivedThreadsPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
