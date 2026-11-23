//! The fork's desktop notification inbox, as the UI sees it. Ports
//! `packages/contracts/src/desktopNotifications.ts` (the wire and file types),
//! `apps/web/src/desktopNotifications.ts` (the read-state helpers), and the
//! inbox side of `DesktopNotificationCoordinator.tsx`.
//!
//! The inbox itself (reconcile, persistence, native notifications, Dock and
//! taskbar badge) lives in the desktop shell, which installs an
//! [`InboxHost`] on macOS and Windows. [`NotificationInbox`] holds the items
//! the host last committed, for the sidebar bell and the thread rows' Done.

use crate::i18n::t;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, EventEmitter, Global, SharedString};
use serde::{Deserialize, Serialize};

use crate::components::sidebar::model::{
    EnvironmentId, LatestRun, RunStatus, ThreadId, ThreadKey, TimestampMs,
};
use crate::components::sidebar_logic::ThreadStatus;
use t3code_contracts::NotificationMode;

/// What an inbox entry is about (`DesktopNotificationKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DesktopNotificationKind {
    Approval,
    Input,
    Failed,
    Completed,
}

impl DesktopNotificationKind {
    /// Every kind, in the model's reconcile order.
    pub const ALL: [Self; 4] = [Self::Approval, Self::Input, Self::Failed, Self::Completed];

    /// A turn end reads as seen once any client visits the thread; requests wait for an answer.
    pub fn is_turn_end(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// The notification and inbox title (`desktopNotificationTitles`).
    pub fn title(self) -> SharedString {
        match self {
            Self::Approval => t!("Approval required"),
            Self::Input => t!("Input required"),
            Self::Failed => t!("Turn failed"),
            Self::Completed => t!("Turn completed"),
        }
    }
}

/// One inbox entry (`DesktopNotification`). Persisted in `notifications.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotification {
    pub id: SharedString,
    pub environment_id: SharedString,
    pub thread_id: SharedString,
    pub kind: DesktopNotificationKind,
    pub thread_title: SharedString,
    pub project_title: SharedString,
    pub environment_title: SharedString,
    /// ISO 8601 with milliseconds, as `DateTime.formatIso` writes it.
    pub created_at: SharedString,
    pub read: bool,
    pub resolved: bool,
}

impl DesktopNotification {
    /// The thread this entry opens.
    pub fn thread_key(&self) -> ThreadKey {
        ThreadKey {
            environment_id: EnvironmentId(self.environment_id.clone()),
            thread_id: ThreadId(self.thread_id.clone()),
        }
    }

    /// Whether this entry belongs to `key`.
    pub fn is_for(&self, key: &ThreadKey) -> bool {
        self.environment_id == key.environment_id.0 && self.thread_id == key.thread_id.0
    }
}

/// How the latest turn ended, as the inbox reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnState {
    Running,
    Interrupted,
    Completed,
    Error,
}

/// The shared visit time against the latest turn's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadState {
    /// A client viewed the thread after the turn ended.
    Seen,
    /// The visit is older than the turn's end: the sidebar shows Done.
    Unread,
    /// No visit, no finished turn, or a server without visit tracking.
    Unknown,
}

/// One thread as the inbox reconciles it (`DesktopNotificationThread`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotificationThread {
    pub environment_id: SharedString,
    pub thread_id: SharedString,
    pub thread_title: SharedString,
    pub project_title: SharedString,
    pub environment_title: SharedString,
    pub turn_id: Option<SharedString>,
    pub turn_state: Option<TurnState>,
    pub completed_at: Option<SharedString>,
    pub approval: bool,
    pub input: bool,
    pub archived: bool,
    pub settled: bool,
    /// Hidden by a snooze that has neither elapsed nor been woken early.
    pub snoozed: bool,
    /// Background work that will wake the agent is still running after the turn.
    pub background_working: bool,
    pub read_state: ReadState,
    /// Open in a focused window: its new notifications arrive read.
    pub on_screen: bool,
}

/// Everything one environment tells the inbox (`DesktopNotificationSnapshot`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopNotificationSnapshot {
    pub notification_mode: NotificationMode,
    pub app_icon_badge: bool,
    /// Only complete, connected environment snapshots can resolve missing threads.
    pub environment_ids: Vec<SharedString>,
    pub threads: Vec<DesktopNotificationThread>,
}

/// A read request from the UI (`DesktopNotificationCommand`).
#[derive(Clone, Debug, PartialEq)]
pub enum DesktopNotificationCommand {
    Read(Vec<SharedString>),
    ReadThread(ThreadKey),
}

/// ISO 8601 UTC with milliseconds, the format the TypeScript writes.
pub fn format_iso(at: TimestampMs) -> SharedString {
    jiff::Timestamp::from_millisecond(at.0)
        .map(|ts| ts.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_default()
        .into()
}

/// Parses an ISO 8601 timestamp to milliseconds.
pub fn parse_iso(text: &str) -> Option<TimestampMs> {
    text.parse::<jiff::Timestamp>().ok().map(|ts| TimestampMs(ts.as_millisecond()))
}

/// The server's read state for how the latest run ended (`latestRunReadState`):
/// `Unread` is exactly when the sidebar shows Done without this desktop's inbox.
pub fn latest_run_read_state(
    latest_run: Option<&LatestRun>,
    last_visited_at: Option<TimestampMs>,
) -> ReadState {
    match (latest_run.and_then(|run| run.completed_at), last_visited_at) {
        (Some(completed), Some(visited)) if visited >= completed => ReadState::Seen,
        (Some(_), Some(_)) => ReadState::Unread,
        _ => ReadState::Unknown,
    }
}

/// The latest run as the inbox sees it (`desktopTurnState`). A completion held
/// by background work still reads as completed, so its end cannot announce it.
pub fn desktop_turn_state(latest_run: Option<&LatestRun>, status: ThreadStatus) -> Option<TurnState> {
    let run = latest_run?;
    Some(match run.status {
        RunStatus::Completed if matches!(status, ThreadStatus::Ready | ThreadStatus::Waiting) => {
            TurnState::Completed
        }
        RunStatus::Failed => TurnState::Error,
        RunStatus::Interrupted | RunStatus::Cancelled | RunStatus::RolledBack => TurnState::Interrupted,
        _ => TurnState::Running,
    })
}

/// Whether `thread` has an unread completion in `items` (the sidebar's Done).
pub fn has_unread_desktop_completion(items: &[DesktopNotification], thread: &ThreadKey) -> bool {
    items
        .iter()
        .any(|item| item.kind == DesktopNotificationKind::Completed && !item.read && item.is_for(thread))
}

/// Threads whose last unread completion was just read, with the visit time that
/// clears their Done badge (`deriveAcknowledgedCompletions`). The time is the
/// later of the notification and the thread's current completion.
pub fn derive_acknowledged_completions(
    previous: &[DesktopNotification],
    next: &[DesktopNotification],
    completed_at: impl Fn(&ThreadKey) -> Option<TimestampMs>,
) -> Vec<(ThreadKey, TimestampMs)> {
    let previously_unread: HashSet<&SharedString> = previous
        .iter()
        .filter(|item| item.kind == DesktopNotificationKind::Completed && !item.read)
        .map(|item| &item.id)
        .collect();
    let mut acknowledged: Vec<(ThreadKey, TimestampMs)> = Vec::new();
    for item in next {
        if item.kind != DesktopNotificationKind::Completed
            || !item.read
            || !previously_unread.contains(&item.id)
        {
            continue;
        }
        let thread = item.thread_key();
        if acknowledged.iter().any(|(key, _)| *key == thread)
            || has_unread_desktop_completion(next, &thread)
        {
            continue;
        }
        let created = parse_iso(&item.created_at).unwrap_or_default();
        let visited = completed_at(&thread).filter(|at| *at > created).unwrap_or(created);
        acknowledged.push((thread, visited));
    }
    acknowledged
}

/// The desktop shell's inbox: reconciles snapshots, persists, shows native
/// notifications, and draws the app icon badge. Installed on macOS and Windows.
pub trait InboxHost {
    /// Reconciles one environment's snapshot (`syncNotifications`).
    fn sync(&self, snapshot: DesktopNotificationSnapshot, cx: &mut App);
    /// Marks entries read (`updateNotifications`).
    fn update(&self, command: DesktopNotificationCommand, cx: &mut App);
}

/// Events from the inbox the app reacts to.
#[derive(Clone, Debug)]
pub enum InboxEvent {
    /// A native notification or an inbox row was opened: show its thread.
    OpenThread(ThreadKey),
}

/// The inbox items the host last committed, shared by the bell, the popover
/// and the sidebar's Done. Read with [`NotificationInbox::global`].
pub struct NotificationInbox {
    items: Arc<[DesktopNotification]>,
    ready: bool,
    host: Option<Rc<dyn InboxHost>>,
}

impl EventEmitter<InboxEvent> for NotificationInbox {}

struct GlobalInbox(Entity<NotificationInbox>);

impl Global for GlobalInbox {}

impl NotificationInbox {
    /// Creates the app-wide inbox with no host (Linux, scenes).
    pub fn init(cx: &mut App) {
        let inbox = cx.new(|_| Self { items: Arc::from([]), ready: false, host: None });
        cx.set_global(GlobalInbox(inbox));
    }

    /// The inbox entity. Observe it to re-render on changes.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalInbox>().0.clone()
    }

    /// Installs the desktop shell's host with its saved items (`getNotifications`).
    pub fn install_host(host: Rc<dyn InboxHost>, items: Vec<DesktopNotification>, cx: &mut App) {
        Self::global(cx).update(cx, |inbox, cx| {
            inbox.host = Some(host);
            inbox.items = items.into();
            inbox.ready = true;
            cx.notify();
        });
    }

    /// Replaces the items, from the host after a commit or from a fixture.
    pub fn set_items(items: Arc<[DesktopNotification]>, cx: &mut App) {
        Self::global(cx).update(cx, |inbox, cx| {
            inbox.items = items;
            inbox.ready = true;
            cx.notify();
        });
    }

    /// Asks the app to show a thread (native notification click, inbox row).
    pub fn open_thread(key: ThreadKey, cx: &mut App) {
        Self::global(cx).update(cx, |_, cx| cx.emit(InboxEvent::OpenThread(key)));
    }

    /// Sends a read command to the host, if there is one.
    pub fn update(command: DesktopNotificationCommand, cx: &mut App) {
        if let Some(host) = Self::global(cx).read(cx).host.clone() {
            host.update(command, cx);
        }
    }

    /// Sends a snapshot to the host, if there is one.
    pub fn sync(snapshot: DesktopNotificationSnapshot, cx: &mut App) {
        if let Some(host) = Self::global(cx).read(cx).host.clone() {
            host.sync(snapshot, cx);
        }
    }

    /// Every entry, newest first.
    pub fn items(&self) -> &Arc<[DesktopNotification]> {
        &self.items
    }

    /// Whether a host runs the inbox here (macOS, Windows), or a fixture filled it.
    pub fn is_available(&self) -> bool {
        self.ready
    }

    /// Whether a desktop host is installed (the native notifications come from it).
    pub fn has_host(&self) -> bool {
        self.host.is_some()
    }

    /// How many entries are unread: the badge count before the setting.
    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|item| !item.read).count()
    }

    /// Threads with an unread completion: the sidebar marks these rows Done
    /// (`SidebarThread::has_unread_completion_notification`).
    pub fn unread_completion_threads(&self) -> HashSet<ThreadKey> {
        self.items
            .iter()
            .filter(|item| item.kind == DesktopNotificationKind::Completed && !item.read)
            .map(DesktopNotification::thread_key)
            .collect()
    }
}
