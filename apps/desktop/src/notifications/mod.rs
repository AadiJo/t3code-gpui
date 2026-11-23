//! Native notifications, the saved inbox, and the app icon badge. Ports
//! `ElectronNotifications.ts` and `ipc/methods/notificationBadge.ts`.
//!
//! On macOS and Windows the inbox reconciles here and its new unread entries
//! become native notifications (one per entry, removed once read). On Linux
//! the web side's coordinator shows one notification per thread, replaced by
//! tag, like the browser `Notification` the web app uses there. Clicking a
//! notification activates the window and opens its thread.

pub mod model;
pub mod store;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::cell::RefCell;
use std::rc::Rc;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::{App, SharedString};
use t3code_contracts::NotificationMode;
use t3code_web::components::sidebar::model::ThreadKey;
use t3code_web::components::thread_notification_coordinator::{
    SystemNotification, SystemNotifier, ThreadNotificationCoordinator,
};
use t3code_web::desktop_notifications::{
    DesktopNotification, DesktopNotificationCommand, DesktopNotificationSnapshot, InboxHost,
    NotificationInbox, format_iso,
};
use t3code_web::components::sidebar::model::TimestampMs;

use store::InboxStore;

/// A click on a native notification.
#[derive(Clone, Debug)]
pub enum Activation {
    /// An inbox entry, by id (macOS, Windows).
    Inbox(SharedString),
    /// A thread's notification (Linux).
    Thread(ThreadKey),
}

/// What a platform backend shows.
#[derive(Clone, Debug)]
pub struct NativeNotification {
    /// The inbox entry id, or the thread tag on Linux. Showing an id again replaces it.
    pub id: String,
    pub title: String,
    pub lines: Vec<String>,
    /// Where a click goes.
    pub activation: Activation,
}

/// One platform's notification center and app icon badge.
pub trait NativeBackend {
    fn show(&self, notification: NativeNotification);
    fn remove(&self, id: &str);
    fn set_badge(&self, count: usize);
}

#[cfg(target_os = "linux")]
fn backend(activations: UnboundedSender<Activation>) -> Option<Rc<dyn NativeBackend>> {
    linux::LinuxBackend::new(activations).map(|backend| Rc::new(backend) as Rc<dyn NativeBackend>)
}

#[cfg(target_os = "macos")]
fn backend(activations: UnboundedSender<Activation>) -> Option<Rc<dyn NativeBackend>> {
    macos::MacBackend::new(activations).map(|backend| Rc::new(backend) as Rc<dyn NativeBackend>)
}

#[cfg(target_os = "windows")]
fn backend(activations: UnboundedSender<Activation>) -> Option<Rc<dyn NativeBackend>> {
    windows::WindowsBackend::new(activations).map(|backend| Rc::new(backend) as Rc<dyn NativeBackend>)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn backend(_: UnboundedSender<Activation>) -> Option<Rc<dyn NativeBackend>> {
    None
}

/// Whether the inbox runs here (`ElectronNotifications.ts:57`).
const INBOX_PLATFORM: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The coordinator's system notifications and badge, through the backend.
struct Notifier(Rc<dyn NativeBackend>);

impl SystemNotifier for Notifier {
    fn show(&self, notification: SystemNotification) {
        self.0.show(NativeNotification {
            id: notification.tag,
            title: notification.title.to_string(),
            lines: vec![notification.body.to_string()],
            activation: Activation::Thread(notification.thread),
        });
    }

    fn close(&self, tag: &str) {
        self.0.remove(tag);
    }

    fn set_badge(&self, count: usize) {
        self.0.set_badge(count);
    }
}

/// The inbox host for macOS and Windows.
struct DesktopInbox {
    store: RefCell<InboxStore>,
    native: Option<Rc<dyn NativeBackend>>,
}

impl DesktopInbox {
    fn publish(&self, commit: store::Commit, cx: &mut App) {
        if !commit.changed {
            return;
        }
        if let Some(native) = &self.native {
            for id in &commit.removed {
                native.remove(id);
            }
        }
        let items: std::sync::Arc<[DesktopNotification]> = self.store.borrow().items().into();
        NotificationInbox::set_items(items, cx);
    }

    fn now() -> SharedString {
        format_iso(TimestampMs(jiff::Timestamp::now().as_millisecond()))
    }
}

impl InboxHost for DesktopInbox {
    fn sync(&self, snapshot: DesktopNotificationSnapshot, cx: &mut App) {
        let result = self.store.borrow_mut().sync(&snapshot, &Self::now());
        let commit = match result {
            Ok(commit) => commit,
            Err(error) => return eprintln!("could not save notifications: {error}"),
        };
        let added = commit.added.clone();
        self.publish(commit, cx);
        let notifications = matches!(
            snapshot.notification_mode,
            NotificationMode::Notifications | NotificationMode::NotificationsAndSound
        );
        let Some(native) = self.native.as_ref().filter(|_| notifications) else { return };
        // A focused window shows its own alerts; the inbox still records these.
        if cx.active_window().is_some() {
            return;
        }
        for item in added {
            native.show(NativeNotification {
                id: item.id.to_string(),
                title: item.kind.title().to_string(),
                lines: vec![
                    item.thread_title.to_string(),
                    format!("{} · {}", item.project_title, item.environment_title),
                ],
                activation: Activation::Inbox(item.id.clone()),
            });
        }
    }

    fn update(&self, command: DesktopNotificationCommand, cx: &mut App) {
        match self.store.borrow_mut().update(&command) {
            Ok(commit) => self.publish(commit, cx),
            Err(error) => eprintln!("could not save notifications: {error}"),
        }
    }
}

/// Activates the main window and opens the clicked notification's thread.
fn activate(activation: Activation, cx: &mut App) {
    cx.activate(true);
    if let Some(window) = cx.windows().first().copied() {
        window.update(cx, |_, window, _| window.activate_window()).ok();
    }
    let thread = match activation {
        Activation::Thread(thread) => thread,
        Activation::Inbox(id) => {
            let inbox = NotificationInbox::global(cx);
            let Some(item) = inbox.read(cx).items().iter().find(|item| item.id == id).cloned() else {
                return;
            };
            NotificationInbox::update(DesktopNotificationCommand::Read(vec![id]), cx);
            item.thread_key()
        }
    };
    NotificationInbox::open_thread(thread, cx);
}

/// Installs native notifications, the inbox (macOS, Windows) and the badge.
/// Call once after `t3code_web::init`.
pub fn install(cx: &mut App) {
    let (activations, mut clicks) = unbounded();
    let native = backend(activations);
    if let Some(native) = &native {
        ThreadNotificationCoordinator::install_notifier(Rc::new(Notifier(native.clone())), cx);
    }
    cx.spawn(async move |cx| {
        while let Some(activation) = clicks.next().await {
            cx.update(|cx| activate(activation, cx));
        }
    })
    .detach();
    if !INBOX_PLATFORM {
        return;
    }
    let Some(path) = store::default_path() else { return };
    let store = match InboxStore::load(path) {
        Ok(store) => store,
        Err(error) => return eprintln!("could not load notifications: {error}"),
    };
    let items = store.items().to_vec();
    if let Some(native) = &native {
        // Entries read while the app was closed lose their native notification.
        for item in items.iter().filter(|item| item.read) {
            native.remove(&item.id);
        }
    }
    NotificationInbox::install_host(Rc::new(DesktopInbox { store: RefCell::new(store), native }), items, cx);
}
