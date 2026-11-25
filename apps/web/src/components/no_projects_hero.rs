//! The hero a client with no projects sees (`NoProjectsHero.tsx`); `/welcome` shows it when
//! this client has no local environment (`welcome.tsx`).

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::IconName;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::i18n::t;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// "What should we work on?" with Add project and, when a machine offers a scratch workspace,
/// Start without a project.
#[derive(IntoElement)]
pub struct NoProjectsHero {
    pub topbar: Pixels,
    pub on_add_project: Handler,
    /// `None` when no machine can start a thread without a project.
    pub on_start_without_project: Option<Handler>,
}

impl RenderOnce for NoProjectsHero {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let add = self.on_add_project.clone();
        let buttons = div()
            .mt(theme.sp(6.))
            .flex()
            .justify_center()
            .gap(theme.sp(2.))
            .child(Button::new("hero-add-project").size(ButtonSize::Sm).icon(IconName::Plus).label(t!("Add project")).on_click(move |_, window, cx| add(window, cx)))
            .children(self.on_start_without_project.clone().map(|start| {
                Button::new("hero-without-project")
                    .size(ButtonSize::Sm)
                    .variant(ButtonVariant::Outline)
                    .icon(IconName::MessageSquareDashed)
                    .label(t!("Start without a project"))
                    .on_click(move |_, window, cx| start(window, cx))
            }));
        let description = if self.on_start_without_project.is_some() { t!("Add a project, or start without one.") } else { t!("Add a project to start your first thread.") };
        div()
            .id("no-projects-hero")
            .size_full()
            .flex()
            .flex_col()
            .bg(c.canvas)
            .child(div().flex_none().h(self.topbar))
            .child(
                // `Empty size="hero"`: `gap-6 p-6 md:p-12`, title `text-2xl sm:text-3xl`.
                div().flex_1().min_h_0().flex().flex_col().items_center().justify_center().p(theme.sp(12.)).child(
                    div().w_full().max_w(theme.rem(32.)).px(theme.sp(8.)).py(theme.sp(12.)).child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .text_center()
                            .child(div().ui_text(theme, TextSize::Xl3, weight::SEMIBOLD).text_color(c.foreground()).child(t!("What should we work on?")))
                            .child(div().mt(theme.sp(1.)).ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(description))
                            .child(buttons),
                    ),
                ),
            )
    }
}
