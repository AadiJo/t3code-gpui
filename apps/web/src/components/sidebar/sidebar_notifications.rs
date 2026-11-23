//! The fork's notification bell and inbox popover. Ports
//! `SidebarNotifications.tsx` (`05-ui-shell.md` 6.11): a 384px glass popover
//! above the footer with Unread and All tabs, mark read and mark all read.
//! Rows open their thread and mark it read. Up and Down move through rows,
//! Left and Right switch tabs, Enter opens, Escape closes.
//!
//! [`SidebarNotifications`] is its own view over [`NotificationInbox`]; the
//! sidebar mounts it through [`Sidebar::render_notification_bell`].

use crate::i18n::{t, t_rich};
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::icons::ShellIcon;
use super::sidebar_thread_header::sidebar_icon_button;
use super::style::ShellColors;
use super::Sidebar;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::popover::glass_surface;
use crate::desktop_notifications::{
    DesktopNotification, DesktopNotificationCommand, DesktopNotificationKind, NotificationInbox,
};
use crate::{ActiveAppTheme as _, AppTheme};

/// The icon and its colour per kind.
fn kind_icon(kind: DesktopNotificationKind, theme: &AppTheme) -> (IconName, Hsla) {
    match kind {
        DesktopNotificationKind::Approval => (IconName::ShieldAlert, theme.colors.warning_foreground),
        DesktopNotificationKind::Input => (IconName::CircleQuestionMark, theme.fixed.info_foreground),
        DesktopNotificationKind::Failed => (IconName::CircleAlert, theme.colors.destructive_foreground()),
        DesktopNotificationKind::Completed => (IconName::Check, theme.fixed.success_foreground),
    }
}

/// The bell, its unread dot and the inbox popover.
pub struct SidebarNotifications {
    open: bool,
    show_all: bool,
    /// The row Up and Down point at, in the visible list.
    highlighted: Option<usize>,
    hovered: bool,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Opening a row activates its thread through the sidebar, like a row click.
    sidebar: WeakEntity<Sidebar>,
    _inbox: Subscription,
}

struct GlobalSidebarNotifications(Entity<SidebarNotifications>);

impl Global for GlobalSidebarNotifications {}

impl SidebarNotifications {
    /// The app's bell view, created on first use for `sidebar`.
    pub fn view(sidebar: WeakEntity<Sidebar>, cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx.try_global::<GlobalSidebarNotifications>() {
            return existing.0.clone();
        }
        let view = cx.new(|cx| {
            let inbox = NotificationInbox::global(cx);
            Self {
                open: false,
                show_all: false,
                highlighted: None,
                hovered: false,
                focus: cx.focus_handle(),
                scroll: UniformListScrollHandle::new(),
                sidebar,
                _inbox: cx.observe(&inbox, |this: &mut Self, _, cx| {
                    this.clamp_highlight(cx);
                    cx.notify();
                }),
            }
        });
        cx.set_global(GlobalSidebarNotifications(view.clone()));
        view
    }

    /// Opens or closes the popover (scenes and tests).
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = open;
        self.highlighted = None;
        if open {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Shows every entry instead of the unread ones (scenes and tests).
    pub fn set_show_all(&mut self, show_all: bool, cx: &mut Context<Self>) {
        self.show_all = show_all;
        self.highlighted = None;
        cx.notify();
    }

    /// Whether the popover is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    fn visible(&self, cx: &App) -> Vec<DesktopNotification> {
        let inbox = NotificationInbox::global(cx);
        let items = inbox.read(cx).items();
        items.iter().filter(|item| self.show_all || !item.read).cloned().collect()
    }

    fn clamp_highlight(&mut self, cx: &App) {
        let count = self.visible(cx).len();
        self.highlighted = self.highlighted.filter(|ix| *ix < count);
    }

    fn open_item(&mut self, item: &DesktopNotification, cx: &mut Context<Self>) {
        self.open = false;
        let key = item.thread_key();
        match self.sidebar.upgrade() {
            Some(sidebar) => sidebar.update(cx, |sidebar, cx| sidebar.activate_thread(key, cx)),
            None => NotificationInbox::open_thread(key, cx),
        }
        NotificationInbox::update(DesktopNotificationCommand::Read(vec![item.id.clone()]), cx);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let visible = self.visible(cx);
        let last = visible.len().checked_sub(1);
        match event.keystroke.key.as_str() {
            "down" => self.highlighted = last.map(|last| self.highlighted.map_or(0, |ix| (ix + 1).min(last))),
            "up" => self.highlighted = last.map(|_| self.highlighted.map_or(0, |ix| ix.saturating_sub(1))),
            "home" => self.highlighted = last.map(|_| 0),
            "end" => self.highlighted = last,
            "left" | "right" => {
                self.show_all = event.keystroke.key == "right";
                self.highlighted = None;
            }
            "enter" => {
                if let Some(item) = self.highlighted.and_then(|ix| visible.get(ix)) {
                    self.open_item(item, cx);
                }
                return;
            }
            "escape" => self.open = false,
            _ => return,
        }
        if let Some(ix) = self.highlighted {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let colors = &theme.colors;
        let inbox = NotificationInbox::global(cx);
        let items = inbox.read(cx).items().clone();
        let unread = items.iter().filter(|item| !item.read).count();
        let visible: Arc<[DesktopNotification]> =
            items.iter().filter(|item| self.show_all || !item.read).cloned().collect();
        let line = px(TextSize::Xs.line_height(theme.fonts.interface));
        let small_line = px(TextSize::Xs2.line_height(theme.fonts.interface));
        let row_height = theme.sp(6.) + line * 2. + small_line + px(1.);
        let list_height = (row_height * visible.len() as f32).min(theme.rem(24.));
        let (muted, popover_fg) = (colors.muted_foreground, colors.foreground());
        let tab = |id: &'static str, label: SharedString, selected: bool, all: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .cursor_pointer()
                .text_color(if selected { popover_fg } else { muted })
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_show_all(all, cx)))
        };
        let unread_label = if unread > 0 {
            t!("Unread ({count})", count = unread)
        } else {
            t!("Unread", context = "filter showing unread notifications")
        };
        let list = if visible.is_empty() {
            div()
                .py(theme.sp(6.))
                .flex()
                .justify_center()
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .text_color(muted)
                .child(if self.show_all { t!("No notifications") } else { t!("No unread notifications") })
                .into_any_element()
        } else {
            let highlighted = self.highlighted;
            let rows = visible.clone();
            uniform_list(
                "notifications-list",
                visible.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    let count = rows.len();
                    range
                        .map(|ix| this.render_row(&rows[ix], ix, ix + 1 == count, highlighted == Some(ix), cx))
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.scroll)
            .h(list_height)
            .into_any_element()
        };
        glass_surface(theme)
            .id("notifications-inbox")
            .occlude()
            .w(theme.rem(24.))
            .max_w(window.viewport_size().width - px(32.))
            .p(theme.sp(4.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.open = false;
                cx.notify();
            }))
            .child(
                div()
                    .mb(theme.sp(3.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(theme.sp(3.))
                    .child(
                        div()
                            .ui_text(theme, TextSize::Sm, weight::SEMIBOLD)
                            .line_height(px(TextSize::Sm.size(theme.fonts.interface)))
                            .child(t!("Notifications")),
                    )
                    .child(
                        div()
                            .id("notifications-mark-all")
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .text_color(muted)
                            .when(unread == 0, |d| d.opacity(0.4))
                            .when(unread > 0, |d| {
                                let ids: Vec<SharedString> =
                                    items.iter().filter(|item| !item.read).map(|item| item.id.clone()).collect();
                                d.cursor_pointer().hover(move |s| s.text_color(popover_fg)).on_click(move |_, _, cx| {
                                    NotificationInbox::update(DesktopNotificationCommand::Read(ids.clone()), cx)
                                })
                            })
                            .child(t_rich!("Mark all read").plain()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(theme.sp(4.))
                    .pb(theme.sp(2.))
                    .border_b_1()
                    .border_color(colors.border)
                    .ui_text(theme, TextSize::Xs, weight::NORMAL)
                    .child(tab("notifications-unread", unread_label, !self.show_all, false, cx))
                    .child(tab(
                        "notifications-all",
                        t!("All", context = "filter showing every notification"),
                        self.show_all,
                        true,
                        cx,
                    )),
            )
            .child(list)
    }

    fn render_row(
        &self,
        item: &DesktopNotification,
        ix: usize,
        last: bool,
        highlighted: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.app_theme();
        let colors = &theme.colors;
        let (icon, color) = kind_icon(item.kind, theme);
        let kind_title = item.kind.title();
        let (accent, accent_fg, muted, popover_fg) =
            (colors.accent, colors.accent_foreground, colors.muted_foreground, colors.foreground());
        let open = item.clone();
        let read_id = item.id.clone();
        let thread_title = item.thread_title.clone();
        div()
            .flex()
            .items_center()
            .gap(theme.sp(2.))
            .when(!last, |d| d.border_b_1().border_color(colors.border))
            .child(
                div()
                    .id(("notification", ix))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(theme.sp(2.))
                    .py(theme.sp(3.))
                    .cursor_pointer()
                    .when(highlighted, |d| d.bg(accent).text_color(accent_fg))
                    .hover(move |s| s.bg(accent).text_color(accent_fg))
                    .child(div().mt(theme.sp(0.5)).flex_none().child(Icon::new(icon).size(IconSize::Md).color(color)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .ui_text(theme, TextSize::Xs, weight::NORMAL)
                            .child(div().truncate().font_weight(weight::MEDIUM).child(item.thread_title.clone()))
                            .child(div().text_color(muted).child(if item.resolved {
                                t!("{title} · Resolved", title = kind_title.as_ref())
                            } else {
                                kind_title
                            }))
                            .child(
                                div()
                                    .truncate()
                                    .ui_text(theme, TextSize::Xs2, weight::NORMAL)
                                    .text_color(muted)
                                    .child(format!("{} · {}", item.project_title, item.environment_title)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_item(&open, cx))),
            )
            .when(!item.read, |d| {
                d.child(
                    div()
                        .id(("notification-read", ix))
                        .flex_none()
                        .p(theme.sp(1.))
                        .cursor_pointer()
                        .text_color(muted)
                        .hover(move |s| s.text_color(popover_fg))
                        .aria_label(SharedString::from(t!("Mark {thread} as read", thread = thread_title.as_ref())))
                        .tooltip(super::tooltip::tooltip(t!("Mark as read")))
                        .child(Icon::new(IconName::Check).size(IconSize::Sm))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            NotificationInbox::update(DesktopNotificationCommand::Read(vec![read_id.clone()]), cx)
                        }),
                )
            })
            .into_any_element()
    }
}

impl Render for SidebarNotifications {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inbox = NotificationInbox::global(cx);
        let unread = inbox.read(cx).unread_count();
        let colors = ShellColors::from_theme(cx.app_theme());
        let label = if unread > 0 { t!("Notifications, unread items") } else { t!("Notifications") };
        let bell = sidebar_icon_button("sidebar-notifications", ShellIcon::Bell, px(32.), self.hovered || self.open, &colors)
            .aria_label(SharedString::from(label))
            .tooltip(super::tooltip::tooltip(t!("Notifications")))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, window, cx| this.set_open(!this.open, window, cx)))
            .when(unread > 0, |d| {
                d.child(div().absolute().top_1().right_1().size(px(6.)).rounded_full().bg(colors.destructive))
            });
        let popover = self.open.then(|| {
            deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .offset(point(px(0.), px(-10.)))
                    .snap_to_window_with_margin(px(16.))
                    .child(self.render_popover(window, cx)),
            )
            .with_priority(1)
        });
        div().relative().flex_none().child(bell).children(popover)
    }
}

impl Sidebar {
    /// The footer's notification bell (fork), shown while the inbox setting is on
    /// and a desktop inbox runs here. Mounted by `sidebar_chrome`.
    pub(crate) fn render_notification_bell(
        &mut self,
        _colors: &ShellColors,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let available = NotificationInbox::global(cx).read(cx).is_available();
        let view = SidebarNotifications::view(cx.weak_entity(), cx);
        div().when(available, |d| d.child(view))
    }
}
