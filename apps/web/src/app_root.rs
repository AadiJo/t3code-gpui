use gpui_kit::*;

use crate::ActiveAppTheme as _;

/// Root view of the main window. Ports `AppRoot.tsx`.
pub struct AppRoot;

impl AppRoot {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        div().size_full().bg(colors.canvas).text_color(colors.text)
    }
}
