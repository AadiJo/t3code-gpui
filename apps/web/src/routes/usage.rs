//! The usage route. Ports `routes/_chat.usage.tsx` inside `AppRouteModal`:
//! the Usage page in the same large modal as Settings. The caller wraps it in
//! a [`Modal`](crate::components::ui::dialog::Modal) and closes it on
//! [`UsagePageEvent::Close`].

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Subscription, Window, div, prelude::*, px};

use crate::ActiveAppTheme as _;
use crate::components::usage::usage_page::{UsagePage, UsagePageEvent};
use crate::components::usage::usage_source::UsageSource;

/// The open Usage modal.
pub struct UsageRoute {
    page: Entity<UsagePage>,
    _subscription: Subscription,
}

impl EventEmitter<UsagePageEvent> for UsageRoute {}

impl Focusable for UsageRoute {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.page.focus_handle(cx)
    }
}

impl UsageRoute {
    /// Opens the page over `source`; `prefs_path` remembers its choices.
    pub fn new(source: Rc<dyn UsageSource>, prefs_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let page = cx.new(|cx| UsagePage::new(source, prefs_path, window, cx));
        let subscription = cx.subscribe(&page, |_, _, event: &UsagePageEvent, cx| cx.emit(event.clone()));
        Self { page, _subscription: subscription }
    }

    pub fn page(&self) -> &Entity<UsagePage> {
        &self.page
    }
}

impl Render for UsageRoute {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let viewport = window.viewport_size();
        // `sm:p-6 lg:p-10`, `sm:max-w-[80rem] sm:max-h-[60rem]`; the Modal adds 16px.
        let inset = if viewport.width >= px(1024.) { px(40.) } else { px(24.) } - px(16.);
        let width = (viewport.width - inset * 2. - px(32.)).min(theme.rem(80.));
        let height = (viewport.height - inset * 2. - px(32.)).min(theme.rem(60.));
        div()
            .id("usage-modal")
            .occlude()
            .flex()
            .w(width)
            .h(height)
            .overflow_hidden()
            .rounded(theme.radii.xxl)
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.background())
            .shadow(theme.shadows.dialog.clone())
            .child(self.page.clone())
    }
}
