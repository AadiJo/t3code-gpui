//! Notifications: the inbox popover on its Unread tab (keyboard highlight on
//! the second row) and its All tab with read and resolved rows, plus the
//! in-app toast stack. The toasts come from the real coordinator: the scene
//! feeds it a focused window on another thread, then three threads change.
//! `notifications-empty` shows both empty states.

use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ActiveAppTheme as _;
use crate::components::background_thread_announcer::BackgroundThreadAnnouncer;
use crate::components::sidebar::fixtures;
use crate::components::sidebar::model::{
    EnvironmentId, ErrorClass, LatestRun, RunStatus, RuntimeStatus, SidebarThread, ThreadId, ThreadKey,
    ThreadRuntime,
};
use crate::components::sidebar::sidebar_notifications::SidebarNotifications;
use crate::components::sidebar::style::ShellColors;
use crate::components::thread_notification_coordinator::{
    EnvironmentShell, NotificationInput, NotificationThread, ThreadNotificationCoordinator,
};
use crate::components::ui::toast::Toaster;
use crate::desktop_notifications::{DesktopNotification, DesktopNotificationKind as Kind, NotificationInbox};
use crate::thread_notifications::NotificationSettings;
use t3code_contracts::NotificationMode;

/// Root view of the scene.
pub struct NotificationsScene {
    bells: Vec<Entity<SidebarNotifications>>,
    caption: SharedString,
}

fn item(id: u32, kind: Kind, thread: &str, title: &str, project: &str, read: bool, resolved: bool) -> DesktopNotification {
    DesktopNotification {
        id: format!("t3-work-{id}").into(),
        environment_id: "env-a".into(),
        thread_id: thread.to_string().into(),
        kind,
        thread_title: title.to_string().into(),
        project_title: project.to_string().into(),
        environment_title: if project == "infra" { "Cloud box".into() } else { "Studio Mac".into() },
        created_at: "2026-11-23T10:00:00.000Z".into(),
        read,
        resolved,
    }
}

/// An inbox with four unread and five read entries, newest first.
fn inbox_items() -> Arc<[DesktopNotification]> {
    Arc::from(vec![
        item(9, Kind::Approval, "t2", "Refactor billing to use the new invoice service", "api", false, false),
        item(8, Kind::Input, "t6", "Deploy script", "infra", false, false),
        item(7, Kind::Completed, "t1", "Fix login bug", "web-app", false, false),
        item(6, Kind::Failed, "t7", "Nightly report", "api", false, false),
        item(5, Kind::Completed, "t3", "Write docs", "docs", true, true),
        item(4, Kind::Approval, "t2", "Refactor billing to use the new invoice service", "api", true, true),
        item(3, Kind::Input, "t6", "Deploy script", "infra", true, false),
        item(2, Kind::Completed, "t8", "Bump dependencies", "web-app", true, false),
        item(1, Kind::Failed, "t9", "Flaky e2e test", "web-app", true, true),
    ])
}

fn thread(id: &str, title: &str, patch: impl FnOnce(&mut SidebarThread)) -> Arc<NotificationThread> {
    let base = fixtures::showcase_snapshot().threads[0].clone();
    let mut row = SidebarThread {
        key: ThreadKey { environment_id: EnvironmentId("env-a".into()), thread_id: ThreadId(id.to_string().into()) },
        title: title.to_string().into(),
        runtime: Some(ThreadRuntime { status: RuntimeStatus::Run(RunStatus::Running), last_error_class: None }),
        latest_run: Some(LatestRun { status: RunStatus::Running, requested_at: None, started_at: None, completed_at: None }),
        ..(*base).clone()
    };
    patch(&mut row);
    Arc::new(NotificationThread {
        row: Arc::new(row),
        project_title: "web-app".into(),
        lineage: Default::default(),
        latest_run_id: Some(format!("run-{id}").into()),
        active_run_id: None,
        pending_request_id: Some(format!("request-{id}").into()),
        snooze: Default::default(),
        stop_requested: false,
        background_wake: false,
        monitor_holds_wake: false,
    })
}

fn shell(threads: Vec<Arc<NotificationThread>>) -> NotificationInput {
    NotificationInput::Environment(EnvironmentShell {
        id: EnvironmentId("env-a".into()),
        label: "Studio Mac".into(),
        threads: Some(threads.into()),
    })
}

/// Drives the coordinator into three toasts: a completion, an approval and a usage limit.
fn raise_toasts(cx: &mut App) {
    let settings = NotificationSettings { mode: NotificationMode::Notifications, in_app: true, ..Default::default() };
    ThreadNotificationCoordinator::apply(NotificationInput::Settings(settings), cx);
    ThreadNotificationCoordinator::apply(NotificationInput::Focus(true), cx);
    let open = ThreadKey { environment_id: EnvironmentId("env-a".into()), thread_id: ThreadId("open".into()) };
    ThreadNotificationCoordinator::apply(NotificationInput::Route(Some(open)), cx);
    let running = vec![
        thread("t7", "Nightly report", |_| {}),
        thread("t2", "Refactor billing to use the new invoice service", |_| {}),
        thread("t1", "Fix login bug", |_| {}),
    ];
    ThreadNotificationCoordinator::apply(shell(running), cx);
    let now = fixtures::showcase_snapshot().now;
    let changed = vec![
        thread("t7", "Nightly report", |row| {
            row.runtime = Some(ThreadRuntime {
                status: RuntimeStatus::Run(RunStatus::Failed),
                last_error_class: Some(ErrorClass::UsageLimit),
            });
            row.latest_run = Some(LatestRun { status: RunStatus::Failed, requested_at: None, started_at: None, completed_at: Some(now) });
        }),
        thread("t2", "Refactor billing to use the new invoice service", |row| row.has_pending_approvals = true),
        thread("t1", "Fix login bug", |row| {
            row.runtime = Some(ThreadRuntime { status: RuntimeStatus::Run(RunStatus::Completed), last_error_class: None });
            row.latest_run = Some(LatestRun { status: RunStatus::Completed, requested_at: None, started_at: None, completed_at: Some(now) });
        }),
    ];
    ThreadNotificationCoordinator::apply(shell(changed), cx);
}

fn bell(window: &mut Window, cx: &mut App, show_all: bool, highlighted: Option<usize>) -> Entity<SidebarNotifications> {
    cx.new(|cx| {
        let mut bell = SidebarNotifications::new(WeakEntity::new_invalid(), cx);
        bell.set_open(true, window, cx);
        bell.set_show_all(show_all, cx);
        bell.set_highlighted(highlighted, cx);
        bell
    })
}

/// The popovers on both tabs with the toast stack.
pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    NotificationInbox::set_items(inbox_items(), cx);
    raise_toasts(cx);
    let bells = vec![bell(window, cx, false, Some(1)), bell(window, cx, true, None)];
    let caption = BackgroundThreadAnnouncer::view(cx).read(cx).text().clone();
    cx.new(|_| NotificationsScene { bells, caption }).into()
}

/// The empty states of both tabs.
pub fn build_empty(window: &mut Window, cx: &mut App) -> AnyView {
    NotificationInbox::set_items(Arc::from([]), cx);
    let bells = vec![bell(window, cx, false, None), bell(window, cx, true, None)];
    cx.new(|_| NotificationsScene { bells, caption: SharedString::default() }).into()
}

impl Render for NotificationsScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let colors = ShellColors::from_theme(theme);
        let columns = self.bells.iter().map(|bell| {
            div()
                .w(px(424.))
                .h_full()
                .flex()
                .flex_col()
                .justify_end()
                .px(px(8.))
                .pb(px(8.))
                .bg(colors.sidebar)
                .border_r_1()
                .border_color(colors.sidebar_border)
                .child(bell.clone())
        });
        div()
            .size_full()
            .relative()
            .flex()
            .bg(theme.colors.background())
            .text_color(theme.colors.foreground())
            .children(columns)
            .child(
                div()
                    .absolute()
                    .top(px(8.))
                    .left(px(16.))
                    .text_xs()
                    .text_color(theme.colors.muted_foreground)
                    .when(!self.caption.is_empty(), |d| d.child(format!("Screen reader: {}", self.caption))),
            )
            .child(BackgroundThreadAnnouncer::view(cx))
            .child(Toaster::view(cx))
    }
}
