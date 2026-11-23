//! Sidebar header row and footer. Ports `SidebarChrome.tsx`: the fork removed
//! the wordmark and history buttons, so the header row only holds the
//! optional environment pill; the footer holds the utility buttons and the
//! fork's notification bell.

use crate::components::icons::IconName;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::sidebar_thread_header::{HeaderControl, sidebar_icon_button};
use super::style::*;
use super::{Sidebar, SidebarEvent};

impl Sidebar {
    /// Topbar-height row; part of the window drag strip.
    pub(crate) fn render_header_row(
        &mut self,
        colors: &ShellColors,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_none()
            .h(self.topbar_height)
            .items_center()
            .gap_2()
            .pr_3()
            // The pill clears the floating sidebar toggle.
            .pl(self.header_inset)
            .children(self.snapshot.environment_pill.clone().map(|label| {
                div()
                    .h(px(16.))
                    .px(px(3.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .bg(colors.secondary)
                    .text_size(px(10.))
                    .line_height(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.foreground)
                    .child(label)
            }))
    }

    /// Settings, pull requests, issues, usage and (fork) notifications.
    pub(crate) fn render_footer(
        &mut self,
        colors: &ShellColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let hovered = |control| self.hovered_control == Some(control);
        let button = |this: &Self,
                      cx: &mut Context<Self>,
                      id: &'static str,
                      glyph,
                      control,
                      label: &'static str,
                      event: fn() -> SidebarEvent| {
            sidebar_icon_button(id, glyph, px(32.), hovered(control), colors)
                .tooltip(crate::components::ui::tooltip::tooltip(label))
                .on_hover(this.control_hover_listener(control, cx))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(event())))
        };
        let settings = self.snapshot.settings;
        let row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(button(
                self,
                cx,
                "sidebar-settings",
                IconName::Settings,
                HeaderControl::Settings,
                "Settings",
                || SidebarEvent::OpenSettings,
            ))
            .child(button(
                self,
                cx,
                "sidebar-pull-requests",
                IconName::GitPullRequest,
                HeaderControl::PullRequests,
                "Pull requests",
                || SidebarEvent::OpenPullRequests,
            ))
            .child(button(
                self,
                cx,
                "sidebar-issues",
                IconName::CircleDot,
                HeaderControl::Issues,
                "Issues",
                || SidebarEvent::OpenIssues,
            ))
            .child(button(
                self,
                cx,
                "sidebar-usage",
                IconName::ChartNoAxesColumn,
                HeaderControl::Usage,
                "Usage",
                || SidebarEvent::OpenUsage,
            ))
            .when(settings.notification_inbox, |row| {
                row.child(self.render_notification_bell(colors, window, cx))
            });
        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .px_2()
            .pt_1()
            .pb_2()
            .child(row)
    }
}
