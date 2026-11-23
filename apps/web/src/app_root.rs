use gpui_kit::{component::ActiveTheme as _, *};

/// Root view of the main window. Ports `AppRoot.tsx`.
pub struct AppRoot;

impl AppRoot {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for AppRoot {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
    }
}
