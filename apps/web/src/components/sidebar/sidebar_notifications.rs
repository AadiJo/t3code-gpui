//! The fork's notification bell and inbox popover. Ports
//! `SidebarNotifications.tsx` (`05-ui-shell.md` 6.11).

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::icons::{ShellIcon, icon};
use super::model::NotificationKind;
use super::sidebar_thread_header::{HeaderControl, sidebar_icon_button};
use super::style::*;
use super::{InboxFilter, Sidebar, SidebarEvent};

fn kind_look(kind: NotificationKind, colors: &ShellColors) -> (ShellIcon, Hsla, &'static str) {
    match kind {
        NotificationKind::Approval => (ShellIcon::ShieldAlert, colors.warning_foreground, "Approval required"),
        NotificationKind::Input => (ShellIcon::CircleHelp, colors.info_foreground, "Input required"),
        NotificationKind::Failed => (ShellIcon::CircleAlert, colors.destructive_foreground, "Turn failed"),
        NotificationKind::Completed => (ShellIcon::Check, colors.success_foreground, "Turn completed"),
    }
}

impl Sidebar {
    /// The bell with its 6px unread dot, plus the popover when open.
    pub(crate) fn render_notification_bell(
        &mut self,
        colors: &ShellColors,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let unread = self.snapshot.notifications.iter().filter(|n| n.unread).count();
        let hovered = self.hovered_control == Some(HeaderControl::Notifications);
        let bell = sidebar_icon_button("sidebar-notifications", ShellIcon::Bell, px(32.), hovered || self.notifications_open, colors)
            .tooltip(super::tooltip::tooltip("Notifications"))
            .on_hover(self.control_hover_listener(HeaderControl::Notifications, cx))
            .on_click(cx.listener(|this, _, _, cx| {
                this.notifications_open = !this.notifications_open;
                this.context_menu = None;
                cx.notify();
            }))
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_1()
                        .right_1()
                        .size(px(6.))
                        .rounded_full()
                        .bg(colors.destructive),
                )
            });
        div()
            .relative()
            .child(bell)
            .when(self.notifications_open, |d| {
                d.child(
                    deferred(
                        anchored()
                            .anchor(Anchor::BottomLeft)
                            .offset(point(px(0.), px(-10.)))
                            .snap_to_window_with_margin(px(16.))
                            .child(self.render_inbox(colors, cx)),
                    )
                    .with_priority(1),
                )
            })
    }

    fn render_inbox(&mut self, colors: &ShellColors, cx: &mut Context<Self>) -> impl IntoElement {
        let unread: Vec<_> = self.snapshot.notifications.iter().filter(|n| n.unread).collect();
        let unread_count = unread.len();
        let show_all = self.inbox_filter == InboxFilter::All;
        let visible: Vec<_> = if show_all {
            self.snapshot.notifications.iter().collect()
        } else {
            unread
        };
        let tab = |id: &'static str, label: String, selected: bool, filter: InboxFilter, cx: &mut Context<Self>| {
            div()
                .id(id)
                .cursor_pointer()
                .text_color(if selected {
                    colors.foreground
                } else {
                    colors.muted_foreground
                })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.inbox_filter = filter;
                    cx.notify();
                }))
        };
        let items = visible.into_iter().enumerate().map(|(ix, item)| {
            let (glyph, color, kind_title) = kind_look(item.kind, colors);
            let id = item.id.clone();
            let read_id = item.id.clone();
            let thread = item.thread.clone();
            div()
                .flex()
                .items_center()
                .gap_2()
                .when(ix > 0, |d| d.border_t_1().border_color(colors.border))
                .child(
                    div()
                        .id(("notification", ix))
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .py_3()
                        .cursor_pointer()
                        .hover(|s| s.bg(colors.accent).text_color(colors.accent_foreground))
                        .child(div().mt(px(2.)).child(icon(glyph, px(16.), color)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .text_xs()
                                .line_height(px(16.))
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(item.title.clone()),
                                )
                                .child(div().text_color(colors.muted_foreground).child(if item.resolved {
                                    format!("{kind_title} · Resolved")
                                } else {
                                    kind_title.to_string()
                                }))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(colors.muted_foreground)
                                        .child(format!("{} · {}", item.project_label, item.environment_label)),
                                ),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.notifications_open = false;
                            this.activate_thread(thread.clone(), cx);
                            cx.emit(SidebarEvent::OpenNotification(id.clone()));
                        })),
                )
                .when(item.unread, |d| {
                    d.child(
                        div()
                            .id(("notification-read", ix))
                            .p_1()
                            .cursor_pointer()
                            .child(icon(ShellIcon::Check, px(14.), colors.muted_foreground))
                            .tooltip(super::tooltip::tooltip("Mark as read"))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SidebarEvent::MarkNotificationRead(read_id.clone()));
                            })),
                    )
                })
        });
        let items: Vec<_> = items.collect();
        let empty = items.is_empty();
        div()
            .id("notifications-inbox")
            .occlude()
            .w(NOTIFICATIONS_WIDTH)
            .p_4()
            .rounded(px(8.))
            .border_1()
            .border_color(colors.glass_border)
            .bg(colors.glass)
            .text_color(colors.foreground)
            .shadow(vec![BoxShadow {
                color: hsla(0., 0., 0., if colors.dark { 0.8 } else { 0.55 }),
                offset: point(px(0.), px(18.)),
                blur_radius: px(44.),
                spread_radius: px(-18.),
            inset: false,
            }])
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.notifications_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .mb_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .line_height(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Notifications"),
                    )
                    .child(
                        div()
                            .id("notifications-mark-all")
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .when(unread_count == 0, |d| d.opacity(0.4))
                            .when(unread_count > 0, |d| {
                                d.cursor_pointer()
                                    .hover(|s| s.text_color(colors.foreground))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(SidebarEvent::MarkAllNotificationsRead)
                                    }))
                            })
                            .child("Mark all read"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_4()
                    .pb_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .text_xs()
                    .child(tab(
                        "notifications-unread",
                        if unread_count > 0 {
                            format!("Unread ({unread_count})")
                        } else {
                            "Unread".to_string()
                        },
                        !show_all,
                        InboxFilter::Unread,
                        cx,
                    ))
                    .child(tab("notifications-all", "All".into(), show_all, InboxFilter::All, cx)),
            )
            .child(
                div()
                    .id("notifications-list")
                    .max_h(px(384.))
                    .overflow_y_scroll()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .py_6()
                                .flex()
                                .justify_center()
                                .text_xs()
                                .text_color(colors.muted_foreground)
                                .child(if show_all {
                                    "No notifications"
                                } else {
                                    "No unread notifications"
                                }),
                        )
                    })
                    .children(items),
            )
    }
}
