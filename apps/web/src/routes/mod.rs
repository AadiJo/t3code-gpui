//! Pages for the routes that do not show a thread. Ports the page components
//! the web route files mount (`_chat.pull-requests.tsx`, `_chat.issues.tsx`,
//! `_chat.usage.tsx`, `welcome.tsx`, the settings layout). Their bodies are
//! built by other areas; until they land each draws its header and an empty
//! state, so navigation, history and restore already work end to end.
//! Settings is not a page here: it opens the [`settings::SettingsRoute`] modal.

pub mod settings;

use gpui_kit::*;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::empty::Empty;
use crate::router::Route;
use crate::i18n::t;

/// The page's title and icon, for routes that have one.
fn page(route: &Route) -> Option<(SharedString, IconName)> {
    Some(match route {
        Route::PullRequests { .. } => (t!("Pull requests"), IconName::GitPullRequestArrow),
        Route::Issues { .. } => (t!("Issues"), IconName::CircleDot),
        Route::Usage => (t!("Usage"), IconName::ChartNoAxesColumn),
        Route::Settings { .. } => (t!("Settings"), IconName::Settings),
        Route::Welcome => (t!("Welcome"), IconName::House),
        Route::New { .. } => (t!("New thread"), IconName::MessageSquarePlus),
        Route::Index | Route::Thread(_) | Route::Draft(_) => return None,
    })
}

/// A non-thread page: header row and an empty state naming the page.
#[derive(IntoElement)]
pub struct RoutePage {
    pub route: Route,
    pub topbar: Pixels,
    pub leading_inset: Pixels,
}

impl RenderOnce for RoutePage {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let (title, icon) = page(&self.route).unwrap_or_else(|| (t!("Home"), IconName::House));
        div()
            .id("route-page")
            .size_full()
            .flex()
            .flex_col()
            .bg(c.canvas)
            .child(
                div()
                    .flex_none()
                    .h(self.topbar)
                    .flex()
                    .items_center()
                    .px_5()
                    .pl(px(20.) + self.leading_inset)
                    .border_b_1()
                    .border_color(c.border)
                    .ui_text(theme, TextSize::Sm, weight::MEDIUM)
                    .text_color(c.text)
                    .child(title.clone()),
            )
            .child(
                div().flex_1().flex().items_center().justify_center().child(
                    Empty::new(title)
                        .icon(icon)
                        .description(SharedString::from(self.route.path())),
                ),
            )
    }
}
