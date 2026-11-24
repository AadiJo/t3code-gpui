//! Turns thread status changes into sounds, in-app toasts, system
//! notifications, inbox snapshots, the unread badge and screen-reader
//! announcements. Ports `ThreadNotificationCoordinator.tsx`, the inbox side of
//! `DesktopNotificationCoordinator.tsx`, and `startBackgroundThreadAnnouncements`.
//!
//! [`NotificationEngine`] is pure: it takes [`NotificationInput`]s (environment
//! shells, focus, the open thread, settings, inbox items, the clock) and
//! returns [`NotificationEffect`]s. [`ThreadNotificationCoordinator`] runs it in
//! the app and carries the effects out; the fixture driver runs it headless.

use crate::i18n::t;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, SharedString, Subscription, Task, Window,
};
use t3code_contracts::NotificationMode;

use crate::components::background_thread_announcer::{
    BackgroundThreadAnnouncer, BackgroundThreadMemory, BackgroundThreadSnapshot,
    next_background_thread_announcement, resolve_background_thread_request,
};
use crate::components::icons::IconName;
use crate::components::sidebar::model::{
    EnvironmentId, RunStatus, SettledOverride, SidebarThread, ThreadId, ThreadKey, TimestampMs,
};
use crate::components::sidebar_logic::{ThreadStatus, resolve_thread_status};
use crate::components::thread_notification_coordinator_logic::{
    NotificationGate, ObserveOptions, SnoozeState, ThreadNotificationKind,
    ThreadNotificationObservation, is_snoozed_for_notification,
};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::desktop_notifications::{
    DesktopNotification, DesktopNotificationCommand, DesktopNotificationSnapshot,
    DesktopNotificationThread, NotificationInbox, ReadState, derive_acknowledged_completions,
    desktop_turn_state, format_iso, latest_run_read_state,
};
use crate::thread_notifications::{
    NotificationSettings, has_desktop_notifications, has_notification_sound,
    play_notification_sound, thread_notification_tag,
};
use crate::{ActiveAppTheme as _, AppTheme};

/// Where a thread sits relative to a parent. Children are not rows of their own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadLineage {
    /// A row of its own.
    #[default]
    Root,
    /// A subagent: always under its parent, never in the inbox.
    Subagent,
    /// A side chat: under its parent, but its turns reach the inbox.
    SideChat,
    /// A delegated task whose parent is listed: under it, but listed and in the inbox.
    DelegatedUnderParent,
}

impl ThreadLineage {
    /// `isShownUnderParent`: the parent's roll-up covers its notifications.
    fn shown_under_parent(self) -> bool {
        self != Self::Root
    }

    /// `isListedThreadLineage`: announced on its own.
    fn listed(self) -> bool {
        matches!(self, Self::Root | Self::DelegatedUnderParent)
    }
}

/// One thread as notifications read it: the sidebar row plus what only
/// notifications need. The sync layer builds these from the shell; replace
/// the `Arc` when the thread changes, since unchanged ones are skipped by pointer.
#[derive(Clone, Debug)]
pub struct NotificationThread {
    pub row: Arc<SidebarThread>,
    pub project_title: SharedString,
    pub lineage: ThreadLineage,
    pub latest_run_id: Option<SharedString>,
    /// `runtime.activeRunId`: the run doing work now.
    pub active_run_id: Option<SharedString>,
    /// `source.pendingRuntimeRequest.id`.
    pub pending_request_id: Option<SharedString>,
    pub snooze: SnoozeState,
    /// `latestRunStopRequested`.
    pub stop_requested: bool,
    /// `latestRunBackgroundWake`: the run answers a progress wake.
    pub background_wake: bool,
    /// `monitorHoldsWakeCompletion`: an armed Monitor holds the completion.
    pub monitor_holds_wake: bool,
}

/// One environment's shell, as the sync layer reports it.
#[derive(Clone, Debug)]
pub struct EnvironmentShell {
    pub id: EnvironmentId,
    pub label: SharedString,
    /// The live snapshot's threads, or `None` while not live (connecting, stale).
    pub threads: Option<Arc<[Arc<NotificationThread>]>>,
}

/// What the engine reacts to.
#[derive(Clone, Debug)]
pub enum NotificationInput {
    Settings(NotificationSettings),
    /// An environment appeared, or its shell changed.
    Environment(EnvironmentShell),
    RemoveEnvironment(EnvironmentId),
    /// The main window gained or lost focus.
    Focus(bool),
    /// The thread the main pane shows.
    Route(Option<ThreadKey>),
    /// The inbox host committed new items.
    InboxItems(Arc<[DesktopNotification]>),
    /// Time passed: release completion graces that ended.
    Tick,
    /// The threads the servers' tag filters hide, and while any environment hides tags, every
    /// thread the shells list (`hiddenThreadKeysAtom`, `threadRefsAtom`).
    TagFilter { hidden: Arc<HashSet<ThreadKey>>, live: Option<Arc<HashSet<ThreadKey>>> },
}

/// Why the gate did not notify a thread that needs attention or completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateReason {
    /// First sighting: a silent baseline.
    FirstSighting,
    ArchivedOrSettled,
    Snoozed,
    /// A stop was requested, or background work holds the thread.
    StoppedOrWaiting,
    /// A wake completion waits out its grace.
    BackgroundWakeGrace,
    /// Already notified for this request or completion, or not settled yet.
    AlreadyNotified,
}

/// Where a notification went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An in-app toast: the window has focus and shows another thread.
    Toast,
    /// A system notification (platforms without the inbox).
    System,
    /// The inbox shows it natively (macOS, Windows), or nothing: see the reason.
    Quiet(QuietReason),
}

/// Why a notification produced neither a toast nor a system notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuietReason {
    /// The desktop inbox owns system notifications here.
    InboxPlatform,
    /// The mode has no system notifications.
    Mode,
    /// The window has focus (and the thread is open, or toasts are off).
    Focused,
}

/// What the tone and icon of an in-app toast are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastTone {
    Completion,
    Approval,
    Limited,
    Failed,
    Input,
}

/// A system notification for one thread; `tag` replaces the thread's last one.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemNotification {
    pub tag: String,
    pub thread: ThreadKey,
    pub title: SharedString,
    pub body: SharedString,
}

/// What the app should do.
#[derive(Clone, Debug)]
pub enum NotificationEffect {
    /// The gate stayed quiet for a thread that needs attention or completed.
    Suppressed { thread: ThreadKey, reason: GateReason },
    /// The gate notified; `outcome` says where it went.
    Notified { thread: ThreadKey, kind: ThreadNotificationKind, title: SharedString, sound: bool, outcome: Outcome },
    Sound(ThreadNotificationKind),
    /// Replaces any toast for the thread.
    Toast { thread: ThreadKey, tone: ToastTone, title: SharedString, description: SharedString },
    /// Closes toasts for threads matching the filter.
    CloseToasts(ToastFilter),
    ShowSystem(SystemNotification),
    CloseSystem(String),
    /// The app icon's unread count.
    Badge(usize),
    /// Sends one environment's snapshot to the inbox host.
    InboxSync(DesktopNotificationSnapshot),
    InboxRead(DesktopNotificationCommand),
    /// Records a visit that clears the thread's Done (`thread.visit`).
    Visit { thread: ThreadKey, at: TimestampMs },
    /// Speaks a background thread change.
    Announce(String),
}

/// Which toasts to close.
#[derive(Clone, Debug, PartialEq)]
pub enum ToastFilter {
    All,
    Thread(ThreadKey),
    /// Toasts whose environment is not in the list.
    EnvironmentsOtherThan(Vec<EnvironmentId>),
    /// Toasts of hidden threads, and while hiding, of threads not in `live`.
    Hidden { hidden: Arc<HashSet<ThreadKey>>, live: Option<Arc<HashSet<ThreadKey>>> },
}

/// The gate's per-thread state (`NotificationState` in the TypeScript).
#[derive(Clone, Debug)]
struct ObservedThread {
    raw: Arc<NotificationThread>,
    status: ThreadStatus,
    waiting: bool,
    observation: ThreadNotificationObservation,
}

#[derive(Default)]
struct WatchedEnvironment {
    previous: HashMap<ThreadId, ObservedThread>,
    reconnecting: bool,
    gate: NotificationGate<ThreadId>,
}

/// `SidebarThreadStatus` as the attention key spells it.
fn status_wire(status: ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Approval => "approval",
        ThreadStatus::Input => "input",
        ThreadStatus::Request => "request",
        ThreadStatus::Preparing => "preparing",
        ThreadStatus::Working => "working",
        ThreadStatus::Waiting => "waiting",
        ThreadStatus::Failed => "failed",
        ThreadStatus::Limited => "limited",
        ThreadStatus::Ready => "ready",
    }
}

/// The notification layer for every connected environment. See the module docs.
pub struct NotificationEngine {
    /// macOS and Windows: the desktop inbox reconciles and shows system notifications.
    inbox_platform: bool,
    settings: NotificationSettings,
    applied_mode: Option<NotificationMode>,
    environments: Vec<EnvironmentShell>,
    watched: Vec<(EnvironmentId, WatchedEnvironment)>,
    focused: bool,
    route: Option<ThreadKey>,
    /// System notifications on screen, by tag (platforms without the inbox).
    pending: Vec<(String, ThreadKey)>,
    badge: usize,
    inbox_ready: bool,
    items: Arc<[DesktopNotification]>,
    last_sent: HashMap<EnvironmentId, DesktopNotificationSnapshot>,
    /// Entries a mark-unread made unread again; being on screen does not read them.
    marked_unread: HashSet<SharedString>,
    announcer: BackgroundThreadMemory,
    /// Threads the tag filter hides: they never notify.
    hidden: Arc<HashSet<ThreadKey>>,
    /// Every listed thread while some environment hides tags.
    live: Option<Arc<HashSet<ThreadKey>>>,
    effects: Vec<NotificationEffect>,
}

impl NotificationEngine {
    /// A new engine. `inbox_platform` is true on macOS and Windows.
    pub fn new(inbox_platform: bool, settings: NotificationSettings) -> Self {
        let mut engine = Self {
            inbox_platform,
            settings,
            applied_mode: None,
            environments: Vec::new(),
            watched: Vec::new(),
            focused: false,
            route: None,
            pending: Vec::new(),
            badge: 0,
            inbox_ready: false,
            items: Arc::from([]),
            last_sent: HashMap::new(),
            marked_unread: HashSet::new(),
            announcer: BackgroundThreadMemory::default(),
            hidden: Arc::default(),
            live: None,
            effects: Vec::new(),
        };
        engine.apply_mode();
        engine.effects.clear();
        engine
    }

    /// The app icon's unread count right now.
    pub fn badge(&self) -> usize {
        self.badge
    }

    /// The inbox items last received.
    pub fn items(&self) -> &Arc<[DesktopNotification]> {
        &self.items
    }

    /// When the next completion grace ends, to wake the engine with [`NotificationInput::Tick`].
    pub fn next_deadline(&self) -> Option<TimestampMs> {
        self.watched.iter().filter_map(|(_, entry)| entry.gate.next_deadline()).min()
    }

    /// Applies one input at `now` and returns what to do.
    pub fn apply(&mut self, input: NotificationInput, now: TimestampMs) -> Vec<NotificationEffect> {
        match input {
            NotificationInput::Settings(settings) => {
                let icon_badge_changed = settings.app_icon_badge != self.settings.app_icon_badge;
                self.settings = settings;
                self.apply_mode();
                self.sync_environments(now);
                if icon_badge_changed && !self.inbox_platform {
                    self.set_badge(self.pending.len());
                }
                self.sync_inbox(now);
                self.inbox_badge();
            }
            NotificationInput::Environment(shell) => {
                let id = shell.id.clone();
                match self.environments.iter_mut().find(|env| env.id == id) {
                    Some(existing) => *existing = shell,
                    None => {
                        self.environments.push(shell);
                        self.prune_removed_environments();
                    }
                }
                self.sync_environments(now);
                self.observe_environment(&id, now);
                self.announce(now);
                self.sync_inbox(now);
            }
            NotificationInput::RemoveEnvironment(id) => {
                self.environments.retain(|env| env.id != id);
                self.last_sent.remove(&id);
                self.prune_removed_environments();
                self.sync_environments(now);
                self.announce(now);
            }
            NotificationInput::Focus(focused) => {
                if focused == self.focused {
                    return Vec::new();
                }
                self.focused = focused;
                if focused && has_desktop_notifications(self.settings.mode) {
                    self.clear_pending();
                }
                self.sync_inbox(now);
                self.read_on_screen();
            }
            NotificationInput::Route(route) => {
                if route == self.route {
                    return Vec::new();
                }
                self.route = route;
                if let Some(route) = &self.route {
                    self.effects.push(NotificationEffect::CloseToasts(ToastFilter::Thread(route.clone())));
                }
                self.announce(now);
                // Opening a thread reads it, including a mark-unread left from before.
                self.marked_unread.clear();
                self.sync_inbox(now);
                self.read_on_screen();
            }
            NotificationInput::InboxItems(items) => self.inbox_items(items, now),
            NotificationInput::TagFilter { hidden, live } => {
                self.hidden = hidden;
                self.live = live;
                self.close_hidden_pending();
            }
            NotificationInput::Tick => {
                for index in 0..self.watched.len() {
                    let due = self.watched[index].1.gate.due(now);
                    let env = self.watched[index].0.clone();
                    for thread_id in due {
                        if let Some(state) = self.watched[index].1.previous.get(&thread_id).cloned() {
                            self.present(&env, &state, ThreadNotificationKind::Completion);
                        }
                    }
                }
            }
        }
        std::mem::take(&mut self.effects)
    }

    fn enabled(&self) -> bool {
        self.settings.mode != NotificationMode::Off || self.settings.in_app
    }

    fn apply_mode(&mut self) {
        if self.applied_mode == Some(self.settings.mode) {
            return;
        }
        self.applied_mode = Some(self.settings.mode);
        self.clear_pending();
    }

    fn set_badge(&mut self, count: usize) {
        // The setting turns the app icon's count off; a focused window clears it (Linux).
        let count = if self.settings.app_icon_badge { count } else { 0 };
        self.badge = if count > 0 && self.focused { 0 } else { count };
        self.effects.push(NotificationEffect::Badge(self.badge));
    }

    fn clear_pending(&mut self) {
        for (tag, _) in std::mem::take(&mut self.pending) {
            self.effects.push(NotificationEffect::CloseSystem(tag));
        }
        if !self.inbox_platform {
            self.set_badge(0);
        }
    }

    fn prune_removed_environments(&mut self) {
        let ids: Vec<EnvironmentId> = self.environments.iter().map(|env| env.id.clone()).collect();
        self.effects.push(NotificationEffect::CloseToasts(ToastFilter::EnvironmentsOtherThan(ids.clone())));
        let before = self.pending.len();
        let (keep, close): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(_, key)| ids.contains(&key.environment_id));
        self.pending = keep;
        for (tag, _) in close {
            self.effects.push(NotificationEffect::CloseSystem(tag));
        }
        if before != self.pending.len() {
            self.set_badge(self.pending.len());
        }
    }

    /// Closes toasts and system notifications of threads the tag filter hides; while hiding,
    /// also those whose thread left the shells, which cannot be judged (`closeHiddenPending`).
    fn close_hidden_pending(&mut self) {
        let (hidden, live) = (self.hidden.clone(), self.live.clone());
        let close = |key: &ThreadKey| hidden.contains(key) || live.as_ref().is_some_and(|live| !live.contains(key));
        self.effects.push(NotificationEffect::CloseToasts(ToastFilter::Hidden { hidden: hidden.clone(), live: live.clone() }));
        let before = self.pending.len();
        let (closed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending).into_iter().partition(|(_, key)| close(key));
        self.pending = kept;
        for (tag, _) in closed {
            self.effects.push(NotificationEffect::CloseSystem(tag));
        }
        if before != self.pending.len() {
            self.set_badge(self.pending.len());
        }
    }

    /// Watches every environment while notifications are on (`syncEnvironments`).
    fn sync_environments(&mut self, now: TimestampMs) {
        let enabled = self.enabled();
        if !enabled {
            self.effects.push(NotificationEffect::CloseToasts(ToastFilter::All));
        }
        let ids: Vec<EnvironmentId> = if enabled {
            self.environments.iter().map(|env| env.id.clone()).collect()
        } else {
            Vec::new()
        };
        self.watched.retain(|(id, _)| ids.contains(id));
        for id in ids {
            if self.watched.iter().any(|(watched, _)| *watched == id) {
                continue;
            }
            self.watched.push((id.clone(), WatchedEnvironment::default()));
            self.observe_environment(&id, now);
        }
    }

    fn observe_environment(&mut self, id: &EnvironmentId, now: TimestampMs) {
        let Some(index) = self.watched.iter().position(|(watched, _)| watched == id) else { return };
        let threads = self.environments.iter().find(|env| env.id == *id).and_then(|env| env.threads.clone());
        let Some(threads) = threads else {
            let entry = &mut self.watched[index].1;
            entry.reconnecting = true;
            entry.gate.pause();
            return;
        };
        let entry = &mut self.watched[index].1;
        let mut next = HashMap::with_capacity(threads.len());
        let mut changed = Vec::new();
        for raw in threads.iter() {
            if raw.lineage.shown_under_parent() {
                continue;
            }
            let thread_id = raw.row.key.thread_id.clone();
            let prior = entry.previous.get(&thread_id);
            if !entry.reconnecting
                && let Some(prior) = prior
                && Arc::ptr_eq(&prior.raw, raw)
            {
                next.insert(thread_id, prior.clone());
                continue;
            }
            let state = observe_thread(raw);
            changed.push((prior.cloned(), state.clone()));
            next.insert(thread_id, state);
        }
        entry.reconnecting = false;
        entry.previous = next;
        let keys: HashSet<ThreadId> = entry.previous.keys().cloned().collect();
        entry.gate.retain(&keys);
        for (prior, state) in changed {
            let thread = &state.raw.row;
            let options = ObserveOptions {
                // Memory is recorded first, so reopening a settled thread stays silent.
                archived: thread.archived || thread.settled_override == Some(SettledOverride::Settled),
                quiet: state.raw.stop_requested || state.waiting,
                snoozed: is_snoozed_for_notification(&state.raw.snooze, state.observation.completed_at, now),
            };
            let key = thread.key.thread_id.clone();
            let gate = &mut self.watched[index].1.gate;
            let notified = gate.observe(&key, prior.as_ref().map(|p| &p.observation), &state.observation, options, now);
            match notified {
                Some(kind) => self.present(id, &state, kind),
                None if state.observation.attention.is_some() || state.observation.completed_at.is_some() => {
                    let reason = if prior.is_none() {
                        GateReason::FirstSighting
                    } else if options.archived {
                        GateReason::ArchivedOrSettled
                    } else if options.snoozed {
                        GateReason::Snoozed
                    } else if options.quiet {
                        GateReason::StoppedOrWaiting
                    } else if self.watched[index].1.gate.is_releasing(&key) {
                        GateReason::BackgroundWakeGrace
                    } else {
                        GateReason::AlreadyNotified
                    };
                    self.effects.push(NotificationEffect::Suppressed { thread: thread.key.clone(), reason });
                }
                None => {}
            }
        }
    }

    /// Shows one notification (`present`).
    fn present(&mut self, env: &EnvironmentId, state: &ObservedThread, kind: ThreadNotificationKind) {
        let thread = &state.raw.row;
        if self.hidden.contains(&ThreadKey { environment_id: env.clone(), thread_id: thread.key.thread_id.clone() }) {
            return;
        }
        let status = state.status;
        let tone = match kind {
            ThreadNotificationKind::Completion => ToastTone::Completion,
            ThreadNotificationKind::Input => match status {
                ThreadStatus::Approval => ToastTone::Approval,
                ThreadStatus::Limited => ToastTone::Limited,
                ThreadStatus::Failed => ToastTone::Failed,
                _ => ToastTone::Input,
            },
        };
        let title = match (kind, status) {
            (ThreadNotificationKind::Completion, _) => t!("Thread completed"),
            (_, ThreadStatus::Approval) => t!("Approval needed"),
            (_, ThreadStatus::Limited) => t!("Usage limit reached"),
            (_, ThreadStatus::Failed) => t!("Thread failed"),
            (_, ThreadStatus::Request) => t!("Guest request"),
            _ => t!("Input needed"),
        };
        let sound = has_notification_sound(self.settings.mode);
        if sound {
            self.effects.push(NotificationEffect::Sound(kind));
        }
        let key = ThreadKey { environment_id: env.clone(), thread_id: thread.key.thread_id.clone() };
        let outcome = if self.settings.in_app && self.focused && self.route.as_ref() != Some(&key) {
            // A newer alert for the same thread replaces it, like the tag on system notifications.
            self.effects.push(NotificationEffect::CloseToasts(ToastFilter::Thread(key.clone())));
            self.effects.push(NotificationEffect::Toast {
                thread: key.clone(),
                tone,
                title: title.clone(),
                description: thread.title.clone(),
            });
            Outcome::Toast
        } else if self.inbox_platform {
            Outcome::Quiet(QuietReason::InboxPlatform)
        } else if !has_desktop_notifications(self.settings.mode) {
            Outcome::Quiet(QuietReason::Mode)
        } else if self.focused {
            Outcome::Quiet(QuietReason::Focused)
        } else {
            let tag = thread_notification_tag(env, &thread.key.thread_id);
            if let Some(index) = self.pending.iter().position(|(pending, _)| *pending == tag) {
                self.effects.push(NotificationEffect::CloseSystem(tag.clone()));
                self.pending[index].1 = key.clone();
            } else {
                self.pending.push((tag.clone(), key.clone()));
            }
            self.effects.push(NotificationEffect::ShowSystem(SystemNotification {
                tag,
                thread: key.clone(),
                title: title.clone(),
                body: thread.title.clone(),
            }));
            self.set_badge(self.pending.len());
            Outcome::System
        };
        self.effects.push(NotificationEffect::Notified { thread: key, kind, title, sound, outcome });
    }

    /// Speaks status changes of threads other than the open one.
    fn announce(&mut self, now: TimestampMs) {
        let open = self.route.as_ref().map(scoped_thread_key);
        let mut snapshots = Vec::new();
        for env in &self.environments {
            let Some(threads) = &env.threads else { continue };
            for thread in threads.iter() {
                let row = &thread.row;
                if row.archived || !thread.lineage.listed() {
                    continue;
                }
                let (request, request_id) = resolve_background_thread_request(
                    row.has_pending_approvals,
                    row.has_pending_user_input,
                    row.pending_subagent_request,
                    thread.pending_request_id.as_ref(),
                );
                snapshots.push(BackgroundThreadSnapshot {
                    key: scoped_thread_key(&row.key),
                    title: row.title.clone(),
                    request,
                    request_id,
                    run_id: thread.latest_run_id.clone(),
                    run_status: row.latest_run.map(|run| run.status),
                    active_run_id: thread.active_run_id.clone(),
                    snoozed: thread.snooze.effective(now),
                });
            }
        }
        let (memory, announcement) =
            next_background_thread_announcement(&self.announcer, &snapshots, open.as_ref());
        self.announcer = memory;
        if let Some(text) = announcement {
            self.effects.push(NotificationEffect::Announce(text));
        }
    }

    /// Sends each live environment's snapshot to the inbox when it changed.
    fn sync_inbox(&mut self, now: TimestampMs) {
        if !self.inbox_platform || !self.inbox_ready {
            return;
        }
        for env in &self.environments {
            let Some(threads) = &env.threads else { continue };
            let snapshot = self.inbox_snapshot(env, threads, now);
            // Shell updates can include progress. Only notification-relevant changes go out.
            if self.last_sent.get(&env.id) == Some(&snapshot) {
                continue;
            }
            self.last_sent.insert(env.id.clone(), snapshot.clone());
            self.effects.push(NotificationEffect::InboxSync(snapshot));
        }
    }

    fn inbox_snapshot(
        &self,
        env: &EnvironmentShell,
        threads: &[Arc<NotificationThread>],
        now: TimestampMs,
    ) -> DesktopNotificationSnapshot {
        let on_screen = self.focused.then_some(self.route.as_ref()).flatten();
        let threads = threads
            .iter()
            .filter(|thread| thread.lineage != ThreadLineage::Subagent)
            .map(|thread| {
                let row = &thread.row;
                let status = status_without_subagents(row);
                let turn_state = desktop_turn_state(row.latest_run.as_ref(), status);
                let completed_at = row
                    .latest_run
                    .and_then(|run| run.completed_at)
                    .filter(|_| turn_state == Some(crate::desktop_notifications::TurnState::Completed));
                DesktopNotificationThread {
                    environment_id: env.id.0.clone(),
                    thread_id: row.key.thread_id.0.clone(),
                    thread_title: row.title.clone(),
                    project_title: thread.project_title.clone(),
                    environment_title: env.label.clone(),
                    turn_id: thread.latest_run_id.clone(),
                    turn_state,
                    completed_at: completed_at.map(format_iso),
                    approval: row.has_pending_approvals,
                    input: row.has_pending_user_input,
                    archived: row.archived,
                    settled: row.settled_override == Some(SettledOverride::Settled),
                    snoozed: thread.snooze.effective(now),
                    background_working: status == ThreadStatus::Waiting,
                    read_state: latest_run_read_state(row.latest_run.as_ref(), row.last_visited_at),
                    on_screen: on_screen == Some(&row.key),
                }
            })
            .collect();
        DesktopNotificationSnapshot {
            notification_mode: self.settings.mode,
            app_icon_badge: self.settings.app_icon_badge,
            environment_ids: vec![env.id.0.clone()],
            threads,
        }
    }

    fn inbox_items(&mut self, items: Arc<[DesktopNotification]>, now: TimestampMs) {
        let previous = std::mem::replace(&mut self.items, items.clone());
        let was_ready = std::mem::replace(&mut self.inbox_ready, true);
        let was_read: HashSet<&SharedString> =
            previous.iter().filter(|item| item.read).map(|item| &item.id).collect();
        for item in items.iter() {
            if item.read {
                self.marked_unread.remove(&item.id);
            } else if was_read.contains(&item.id) {
                self.marked_unread.insert(item.id.clone());
            }
        }
        // The sidebar's Done follows the inbox: a completion read here counts as a visit.
        let acknowledged = derive_acknowledged_completions(&previous, &items, |key| {
            self.shell_thread(key).and_then(|thread| thread.row.latest_run).and_then(|run| run.completed_at)
        });
        for (thread, at) in acknowledged {
            // Already seen on the server (often read on another client).
            let seen = self.shell_thread(&thread).is_some_and(|shell| {
                latest_run_read_state(shell.row.latest_run.as_ref(), shell.row.last_visited_at) == ReadState::Seen
            });
            if !seen {
                self.effects.push(NotificationEffect::Visit { thread, at });
            }
        }
        if !was_ready {
            self.sync_inbox(now);
        }
        self.inbox_badge();
        self.read_on_screen();
    }

    fn inbox_badge(&mut self) {
        if !self.inbox_platform || !self.inbox_ready {
            return;
        }
        let unread = self.items.iter().filter(|item| !item.read).count();
        let count = if self.settings.app_icon_badge { unread } else { 0 };
        if count != self.badge {
            self.badge = count;
            self.effects.push(NotificationEffect::Badge(count));
        }
    }

    /// Marks the open thread's entries read while the window has focus.
    fn read_on_screen(&mut self) {
        if !self.inbox_platform || !self.focused {
            return;
        }
        let Some(route) = &self.route else { return };
        let ids: Vec<SharedString> = self
            .items
            .iter()
            .filter(|item| !item.read && item.is_for(route) && !self.marked_unread.contains(&item.id))
            .map(|item| item.id.clone())
            .collect();
        if !ids.is_empty() {
            self.effects.push(NotificationEffect::InboxRead(DesktopNotificationCommand::Read(ids)));
        }
    }

    fn shell_thread(&self, key: &ThreadKey) -> Option<&Arc<NotificationThread>> {
        self.environments
            .iter()
            .find(|env| env.id == key.environment_id)?
            .threads
            .as_ref()?
            .iter()
            .find(|thread| thread.row.key.thread_id == key.thread_id)
    }
}

/// `${environmentId}:${threadId}`, the announcer's thread identity.
fn scoped_thread_key(key: &ThreadKey) -> SharedString {
    format!("{}:{}", key.environment_id.0, key.thread_id.0).into()
}

/// The status `presentThreadShell` gives without the child roll-up.
fn status_without_subagents(row: &SidebarThread) -> ThreadStatus {
    if row.pending_subagent_request.is_none() {
        return resolve_thread_status(row);
    }
    resolve_thread_status(&SidebarThread { pending_subagent_request: None, ..row.clone() })
}

/// Derives the gate's view of one thread (`observeThreads`).
fn observe_thread(raw: &Arc<NotificationThread>) -> ObservedThread {
    let row = &raw.row;
    let mut status = resolve_thread_status(row);
    let latest_status = row.latest_run.map(|run| run.status);
    if status == ThreadStatus::Ready && latest_status == Some(RunStatus::Failed) {
        status = ThreadStatus::Failed;
    }
    let needs_attention = matches!(
        status,
        ThreadStatus::Input | ThreadStatus::Request | ThreadStatus::Approval | ThreadStatus::Failed | ThreadStatus::Limited
    );
    let request_id = needs_attention
        .then(|| {
            resolve_background_thread_request(
                row.has_pending_approvals,
                row.has_pending_user_input,
                row.pending_subagent_request,
                raw.pending_request_id.as_ref(),
            )
            .1
        })
        .flatten();
    // A request with an id is keyed by that id alone, so queued runs do not repeat it.
    let attention = needs_attention.then(|| -> SharedString {
        match &request_id {
            Some(id) => format!("{}:{id}", status_wire(status)).into(),
            None => format!("{}:{}", raw.latest_run_id.as_deref().unwrap_or(""), status_wire(status)).into(),
        }
    });
    let completed = latest_status == Some(RunStatus::Completed);
    let settled = status == ThreadStatus::Ready && completed && !raw.monitor_holds_wake;
    // A completion seen while waiting is remembered without an alert.
    let waiting = status == ThreadStatus::Waiting;
    let completed_at = row.latest_run.and_then(|run| run.completed_at);
    ObservedThread {
        raw: raw.clone(),
        status,
        waiting,
        observation: ThreadNotificationObservation {
            attention,
            request_id,
            settled,
            completed_at: if settled || (waiting && completed) { completed_at } else { None },
            background_wake: raw.background_wake,
        },
    }
}

/// The desktop shell's system notifications and app icon badge, for platforms
/// without the inbox (Linux) and for the badge everywhere.
pub trait SystemNotifier {
    /// Shows `notification`, replacing the last one with its tag.
    fn show(&self, notification: SystemNotification);
    /// Removes the notification with `tag`.
    fn close(&self, tag: &str);
    /// Sets the app icon's unread count (0 clears it).
    fn set_badge(&self, count: usize);
}

/// Sends a visit for a thread whose completion was read in the inbox. The sync
/// layer installs this; until then visits are dropped.
pub type VisitHandler = Rc<dyn Fn(ThreadKey, TimestampMs, &mut App)>;

/// Runs a [`NotificationEngine`] in the app and carries out its effects.
/// Install with [`ThreadNotificationCoordinator::init`], then feed it with
/// [`ThreadNotificationCoordinator::apply`].
pub struct ThreadNotificationCoordinator {
    engine: NotificationEngine,
    notifier: Option<Rc<dyn SystemNotifier>>,
    on_visit: Option<VisitHandler>,
    toasts: HashMap<ThreadKey, Vec<u64>>,
    timer: Option<Task<()>>,
    _inbox: Subscription,
}

struct GlobalCoordinator(Entity<ThreadNotificationCoordinator>);

impl Global for GlobalCoordinator {}

fn now() -> TimestampMs {
    TimestampMs(jiff::Timestamp::now().as_millisecond())
}

impl ThreadNotificationCoordinator {
    /// Creates the app-wide coordinator. `inbox_platform` is true where the desktop
    /// shell runs the inbox (macOS, Windows).
    pub fn init(inbox_platform: bool, settings: NotificationSettings, cx: &mut App) {
        let inbox = NotificationInbox::global(cx);
        let coordinator = cx.new(|cx| {
            let subscription = cx.observe(&inbox, |this: &mut Self, inbox, cx| {
                let items = inbox.read(cx).items().clone();
                if !Arc::ptr_eq(&items, this.engine.items()) {
                    this.run(NotificationInput::InboxItems(items), cx);
                }
            });
            Self {
                engine: NotificationEngine::new(inbox_platform, settings),
                notifier: None,
                on_visit: None,
                toasts: HashMap::new(),
                timer: None,
                _inbox: subscription,
            }
        });
        cx.set_global(GlobalCoordinator(coordinator));
    }

    /// The coordinator entity.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCoordinator>().0.clone()
    }

    /// Installs the desktop shell's system notifications and badge.
    pub fn install_notifier(notifier: Rc<dyn SystemNotifier>, cx: &mut App) {
        Self::global(cx).update(cx, |this, _| this.notifier = Some(notifier));
    }

    /// Installs the visit command the inbox's reads send.
    pub fn on_visit(handler: VisitHandler, cx: &mut App) {
        Self::global(cx).update(cx, |this, _| this.on_visit = Some(handler));
    }

    /// Feeds one input (shell change, settings, route).
    pub fn apply(input: NotificationInput, cx: &mut App) {
        Self::global(cx).update(cx, |this, cx| this.run(input, cx));
    }

    /// Tracks the main window's focus.
    pub fn watch_window(window: &mut Window, cx: &mut App) {
        let focused = window.is_window_active();
        Self::apply(NotificationInput::Focus(focused), cx);
        Self::global(cx).update(cx, |_, cx| {
            cx.observe_window_activation(window, |this, window, cx| {
                this.run(NotificationInput::Focus(window.is_window_active()), cx)
            })
            .detach();
        });
    }

    /// The badge count the engine last set.
    pub fn badge(&self) -> usize {
        self.engine.badge()
    }

    fn run(&mut self, input: NotificationInput, cx: &mut Context<Self>) {
        let effects = self.engine.apply(input, now());
        for effect in effects {
            self.carry_out(effect, cx);
        }
        self.schedule_tick(cx);
    }

    fn schedule_tick(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.engine.next_deadline() else {
            self.timer = None;
            return;
        };
        let wait = Duration::from_millis((deadline.0 - now().0).max(0) as u64);
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| this.run(NotificationInput::Tick, cx)).ok();
        }));
    }

    fn carry_out(&mut self, effect: NotificationEffect, cx: &mut Context<Self>) {
        match effect {
            NotificationEffect::Suppressed { .. } | NotificationEffect::Notified { .. } => {}
            NotificationEffect::Sound(kind) => play_notification_sound(kind),
            NotificationEffect::Toast { thread, tone, title, description } => {
                let data = toast_data(tone, title, description, thread.clone(), cx.app_theme());
                let id = Toaster::push(data, cx);
                self.toasts.entry(thread).or_default().push(id);
            }
            NotificationEffect::CloseToasts(filter) => {
                let close: Vec<ThreadKey> = self
                    .toasts
                    .keys()
                    .filter(|key| match &filter {
                        ToastFilter::All => true,
                        ToastFilter::Thread(thread) => *key == thread,
                        ToastFilter::EnvironmentsOtherThan(ids) => !ids.contains(&key.environment_id),
                        ToastFilter::Hidden { hidden, live } => {
                            hidden.contains(*key) || live.as_ref().is_some_and(|live| !live.contains(*key))
                        }
                    })
                    .cloned()
                    .collect();
                for key in close {
                    for id in self.toasts.remove(&key).unwrap_or_default() {
                        Toaster::dismiss(id, cx);
                    }
                }
            }
            NotificationEffect::ShowSystem(notification) => {
                if let Some(notifier) = &self.notifier {
                    notifier.show(notification);
                }
            }
            NotificationEffect::CloseSystem(tag) => {
                if let Some(notifier) = &self.notifier {
                    notifier.close(&tag);
                }
            }
            NotificationEffect::Badge(count) => {
                if let Some(notifier) = &self.notifier {
                    notifier.set_badge(count);
                }
            }
            NotificationEffect::InboxSync(snapshot) => {
                cx.defer(move |cx| NotificationInbox::sync(snapshot, cx));
            }
            NotificationEffect::InboxRead(command) => {
                cx.defer(move |cx| NotificationInbox::update(command, cx));
            }
            NotificationEffect::Visit { thread, at } => {
                if let Some(on_visit) = self.on_visit.clone() {
                    cx.defer(move |cx| on_visit(thread, at, cx));
                }
            }
            NotificationEffect::Announce(text) => BackgroundThreadAnnouncer::announce(text, cx),
        }
    }
}

/// The in-app toast for a notification, with the web app's icon and "Open thread".
pub fn toast_data(
    tone: ToastTone,
    title: SharedString,
    description: SharedString,
    thread: ThreadKey,
    theme: &AppTheme,
) -> ToastData {
    let fixed = &theme.fixed;
    let (kind, icon, color) = match tone {
        ToastTone::Completion => (ToastKind::Success, IconName::CircleCheck, fixed.success_foreground),
        ToastTone::Approval => (ToastKind::Warning, IconName::ShieldQuestionMark, theme.colors.warning_foreground),
        ToastTone::Limited => (ToastKind::Warning, IconName::CircleAlert, theme.colors.warning_foreground),
        ToastTone::Failed => (ToastKind::Error, IconName::CircleAlert, theme.colors.destructive_foreground()),
        ToastTone::Input => (ToastKind::Warning, IconName::MessageCircleQuestionMark, fixed.info_foreground),
    };
    ToastData::new(kind, title)
        .description(description)
        .icon(icon, color)
        .action(t!("Open thread"), move |_, cx| NotificationInbox::open_thread(thread.clone(), cx))
}
