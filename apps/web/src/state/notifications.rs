//! Feeds the notification coordinator from live data: each enabled environment's shell
//! (`NotificationInput::Environment`, `RemoveEnvironment`), the tag filter, and the client
//! settings; and sends `thread.visit` when an inbox read clears a completion. The route comes
//! from `AppRoot`, focus from the window.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::{App, Context, Entity, SharedString, Subscription};
use t3code_client_runtime::state::models::{EpochMs, ThreadRow, format_ms, now_ms};
use t3code_client_runtime::state::notifications::monitor_holds_wake_completion;
use t3code_client_runtime::state::thread_settled::{raised_hand_while_snoozed, thread_woke_at};
use t3code_client_runtime::state::thread_shell::delegated_under_parent;
use t3code_client_runtime::state::environment::SyncStatus;
use t3code_client_runtime::state::drafts::scoped_thread_key;
use t3code_contracts::{EnvironmentId, OrchestrationV2AppThreadLineageRelationshipToParent as Relationship};

use super::shell::{LiveShell, RowExtras, RowKey, environment_key, map_row, runtime_ids, thread_key};
use super::{LiveEnvironment, StateBridge};
use crate::components::sidebar::model::{SidebarThread, ThreadKey, TimestampMs};
use crate::components::thread_notification_coordinator::{
    EnvironmentShell, NotificationInput, NotificationThread, ThreadLineage, ThreadNotificationCoordinator,
};
use crate::components::thread_notification_coordinator_logic::SnoozeState;
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ClientSettingsStore;
use crate::i18n::t;
use crate::thread_notifications::NotificationSettings;

/// What a notification row was built from; equal inputs reuse its `Arc`, so the engine skips it.
struct Mapped {
    source: Arc<ThreadRow>,
    local_visit: Option<EpochMs>,
    lineage: ThreadLineage,
    project_title: SharedString,
    thread: Arc<NotificationThread>,
}

/// The feeder. Create one with [`LiveNotifications::new`] and keep the entity alive.
pub struct LiveNotifications {
    mapped: HashMap<RowKey, Mapped>,
    /// What each environment last sent, to skip unchanged shells.
    sent: HashMap<EnvironmentId, (SharedString, Option<Arc<[Arc<NotificationThread>]>>)>,
    settings: Option<NotificationSettings>,
    tag_filter: Option<(Arc<HashSet<RowKey>>, bool)>,
    _subscriptions: Vec<Subscription>,
}

/// The client settings notifications read.
fn notification_settings(cx: &App) -> NotificationSettings {
    let settings = ClientSettingsStore::get(cx);
    NotificationSettings {
        mode: settings.notification_mode,
        in_app: settings.in_app_notifications_enabled,
        inbox_enabled: settings.notification_inbox_enabled,
        app_icon_badge: settings.app_icon_badge_enabled,
    }
}

/// Where a row sits relative to its parent (`isShownUnderParent`, `isListedThreadLineage`).
fn lineage(row: &ThreadRow, delegated: &HashSet<RowKey>) -> ThreadLineage {
    let lineage = &row.source.lineage;
    if lineage.relationship_to_parent == Some(Relationship::Subagent) {
        ThreadLineage::Subagent
    } else if lineage.side_chat == Some(true) {
        ThreadLineage::SideChat
    } else if delegated.contains(&(row.environment_id.clone(), row.id().clone())) {
        ThreadLineage::DelegatedUnderParent
    } else {
        ThreadLineage::Root
    }
}

impl LiveNotifications {
    /// Follows the live shell and the client settings, and installs the inbox's visit command.
    pub fn new(cx: &mut Context<Self>) -> Self {
        ThreadNotificationCoordinator::on_visit(std::rc::Rc::new(visit), cx);
        let shell = StateBridge::shell(cx);
        let subscriptions = vec![
            cx.observe(&shell, |this, shell, cx| this.sync(&shell, cx)),
            cx.observe_global::<ClientSettingsStore>(|this, cx| this.sync_settings(cx)),
        ];
        let mut this = Self { mapped: HashMap::new(), sent: HashMap::new(), settings: None, tag_filter: None, _subscriptions: subscriptions };
        this.sync_settings(cx);
        this.sync(&shell, cx);
        this
    }

    fn sync_settings(&mut self, cx: &mut Context<Self>) {
        let settings = notification_settings(cx);
        if self.settings != Some(settings) {
            self.settings = Some(settings);
            ThreadNotificationCoordinator::apply(NotificationInput::Settings(settings), cx);
        }
    }

    /// Sends the tag filter and each environment whose shell changed; removes environments
    /// that left.
    fn sync(&mut self, shell: &Entity<LiveShell>, cx: &mut Context<Self>) {
        let environments: Vec<Entity<LiveEnvironment>> = StateBridge::environments(cx).read(cx).list.clone();
        let shell = shell.read(cx);
        let presented = shell.presented().to_vec();
        let (hidden, hiding) = shell.hidden();
        let (hidden, hiding) = (hidden.clone(), hiding);
        let mut inputs = Vec::new();
        if self.tag_filter.as_ref().is_none_or(|(sent, sent_hiding)| !Arc::ptr_eq(sent, &hidden) || *sent_hiding != hiding) {
            let keys = |set: &HashSet<RowKey>| -> Arc<HashSet<ThreadKey>> { Arc::new(set.iter().map(|(env, id)| thread_key(env, id)).collect()) };
            let live = hiding.then(|| Arc::new(presented.iter().map(|row| thread_key(&row.environment_id, row.id())).collect()));
            inputs.push(NotificationInput::TagFilter { hidden: keys(&hidden), live });
            self.tag_filter = Some((hidden, hiding));
        }
        let delegated = delegated_under_parent(&presented);
        let ui_state = StateBridge::ui_state(cx).current();
        let now = now_ms();
        let connections = ConnectionRuntime::state(cx).read(cx);
        let mut next = HashMap::with_capacity(self.mapped.len());
        let mut seen = HashSet::new();
        for env in &environments {
            let env = env.read(cx);
            let id = env.environment_id().clone();
            seen.insert(id.clone());
            let label = connections
                .environment(&id)
                .map(|state| SharedString::from(state.entry.label.clone()))
                .or_else(|| env.config.as_ref().map(|config| SharedString::from(config.environment.label.clone())))
                .unwrap_or_else(|| SharedString::from(id.to_string()));
            let live = env.shell.status == SyncStatus::Live && env.shell.snapshot.is_some();
            let threads = live.then(|| {
                let projects = env.shell.snapshot.as_ref().map(|snapshot| snapshot.projects.as_slice()).unwrap_or_default();
                let titles: HashMap<&t3code_contracts::ProjectId, SharedString> =
                    projects.iter().map(|project| (&project.id, SharedString::from(project.title.clone()))).collect();
                presented
                    .iter()
                    .filter(|row| row.environment_id == id)
                    .map(|row| {
                        let key = (row.environment_id.clone(), row.id().clone());
                        let local_visit = ui_state.thread_last_visited_at_by_id.get(&scoped_thread_key(&row.environment_id, row.id())).copied();
                        let lineage = lineage(row, &delegated);
                        let project_title = titles.get(&row.source.project_id).cloned().unwrap_or_else(|| t!("Project"));
                        let reused = self.mapped.remove(&key).filter(|mapped| {
                            Arc::ptr_eq(&mapped.source, row) && mapped.local_visit == local_visit && mapped.lineage == lineage && mapped.project_title == project_title
                        });
                        let mapped = reused.unwrap_or_else(|| Mapped {
                            source: Arc::clone(row),
                            local_visit,
                            lineage,
                            project_title: project_title.clone(),
                            thread: Arc::new(notification_thread(row, shell.mapped_row(row), local_visit, lineage, project_title, now)),
                        });
                        let thread = Arc::clone(&mapped.thread);
                        next.insert(key, mapped);
                        thread
                    })
                    .collect::<Arc<[_]>>()
            });
            let unchanged = self.sent.get(&id).is_some_and(|(sent_label, sent)| {
                *sent_label == label
                    && match (sent, &threads) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| Arc::ptr_eq(a, b)),
                        _ => false,
                    }
            });
            if unchanged {
                continue;
            }
            self.sent.insert(id.clone(), (label.clone(), threads.clone()));
            inputs.push(NotificationInput::Environment(EnvironmentShell { id: environment_key(&id), label, threads }));
        }
        self.mapped = next;
        let gone: Vec<EnvironmentId> = self.sent.keys().filter(|id| !seen.contains(*id)).cloned().collect();
        for id in gone {
            self.sent.remove(&id);
            inputs.push(NotificationInput::RemoveEnvironment(environment_key(&id)));
        }
        for input in inputs {
            ThreadNotificationCoordinator::apply(input, cx);
        }
    }
}

/// One shell row as notifications read it (`presentThreadShell` plus the raw shell fields). The
/// sidebar's mapped row is shared when it was mapped from this row with the same visit: the
/// fields it adds (provider, draft, remote, unread mark) are ones notifications never read.
fn notification_thread(
    row: &ThreadRow,
    sidebar: Option<(Option<EpochMs>, &Arc<SidebarThread>)>,
    local_visit: Option<EpochMs>,
    lineage: ThreadLineage,
    project_title: SharedString,
    now: EpochMs,
) -> NotificationThread {
    let source = &row.source;
    let mapped = match sidebar {
        Some((visit, thread)) if visit == local_visit => Arc::clone(thread),
        _ => Arc::new(map_row(row, RowExtras { local_visit, woke_at: thread_woke_at(row, now), ..RowExtras::default() })),
    };
    NotificationThread {
        row: mapped,
        project_title,
        lineage,
        latest_run_id: row.latest_run.as_ref().map(|run| SharedString::from(Arc::clone(&run.run_id.0))),
        active_run_id: row.runtime.as_ref().and_then(|runtime| runtime.active_run_id.as_ref()).map(|id| SharedString::from(Arc::clone(&id.0))),
        pending_request_id: source.pending_runtime_request.as_ref().map(|request| SharedString::from(Arc::clone(&request.id.0))),
        snooze: SnoozeState {
            snoozed_until: row.snoozed_until.map(TimestampMs),
            snoozed_at: row.snoozed_at.map(TimestampMs),
            hand_raised: raised_hand_while_snoozed(row),
        },
        stop_requested: source.latest_run_stop_requested == Some(true),
        background_wake: source.latest_run_background_wake == Some(true),
        monitor_holds_wake: monitor_holds_wake_completion(source),
    }
}

/// Records a visit for a completion read in the inbox (`useAcknowledgeThreadWoke`): on the
/// server once its config has loaded, otherwise in the local visit map.
fn visit(key: ThreadKey, at: TimestampMs, cx: &mut App) {
    let (environment_id, thread_id) = runtime_ids(&key);
    let Some(environment) = StateBridge::environment(&environment_id, cx) else { return };
    let (loaded, commands) = {
        let env = environment.read(cx);
        (env.config_loaded(), env.commands.clone())
    };
    if !loaded {
        StateBridge::ui_state(cx).update(|ui| ui.mark_local_visit(scoped_thread_key(&environment_id, &thread_id), at.0));
        return;
    }
    let visited_at = format_ms(at.0);
    StateBridge::spawn(async move { commands.visit(&thread_id, visited_at, false, None).await }, cx).detach();
}
