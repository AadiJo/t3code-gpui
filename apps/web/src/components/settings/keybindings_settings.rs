//! Placeholder while the page is built.

use gpui_kit::{Context, IntoElement, Render, Window, div};

pub struct KeybindingsSettings;

impl KeybindingsSettings {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl KeybindingsSettings {
    pub fn start_recording_for_scene(&mut self, _command: &str, _window: &mut Window, _cx: &mut Context<Self>) {}
}

impl Render for KeybindingsSettings {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}
