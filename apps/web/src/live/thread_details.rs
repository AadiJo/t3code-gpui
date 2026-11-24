//! The thread details card on live data: the shown thread's pull request row (its shell
//! links and `pullRequests.detail`), issues, discovered servers (`subscribeDiscoveredLocalServers`
//! through [`PortForwards`]), bound scheduled tasks (`scheduledTasks.subscribe`) and context
//! window, and the RPCs and commands its rows run.

use std::collections::{HashMap, HashSet};

use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{App, Context, Entity, EventEmitter, SharedString, Task};
use t3code_client_runtime::operations::commands::new_command_id;
use t3code_contracts::rpc::methods::{
    PreviewOpen, PreviewStopLocalServer, PullRequestsDetail, PullRequestsRunAction, ScheduledTasksRunNow, ScheduledTasksSetEnabled,
    ScheduledTasksSubscribe,
};
use t3code_contracts::{
    EnvironmentId, OrchestrationV2Command, OrchestrationV2CommandThreadPullRequestWatch, PreviewOpenInput, PreviewStopLocalServerInput,
    ProjectId, PullRequestAction, PullRequestActionInput, PullRequestDetail, PullRequestMergeMethod, PullRequestRef,
    ScheduledTaskId, ScheduledTaskListResultTasksItem, ScheduledTaskRunNowInput, ScheduledTaskRunStatus, ScheduledTaskSchedule,
    ScheduledTaskSetEnabledInput, ThreadId, ThreadPullRequestLink,
};

use super::rpc::RpcLink;
use crate::components::chat::thread_details_panel::{ThreadDetailsEvent, ThreadDetailsInput};
use crate::components::chat::thread_details_pr_row::PullRequestRowAction;
use t3code_client_runtime::state::models::ThreadRow;
use crate::components::chat::thread_details_sections::{
    UsageLimitsInput, UsageWindowRow,
    AutomationRunStatus, ContextWindowRow, ThreadAutomationRow, ThreadDetailsSectionEvent, ThreadServerRow,
};
use crate::components::command_palette_logic::PortForwardRequest;
use crate::components::settings::scheduled_tasks_settings_logic::schedule_label;
use crate::components::sidebar::model::ThreadKey;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;
use crate::port_discovery_state::PortForwards;
use crate::router::ThreadTarget;
use crate::state::StateBridge;
use crate::state::app_data::{DataEvent, DataEvents, DetailsOutcome, ThreadView};
use crate::state::shell::runtime_ids;
use crate::thread_pull_requests::{current_link, fix_checks_prompt, resolve_conflicts_prompt, row_data};
use crate::ui_state_store::MergeMethod;

/// The bound tasks' source: one subscription for the shown thread's environment.
struct TasksFeed {
    environment: EnvironmentId,
    /// `None` until the first list arrives.
    result: Option<Result<Vec<ScheduledTaskListResultTasksItem>, SharedString>>,
    time_zone: Option<String>,
    _task: Task<()>,
}

/// The shown thread changed what the card shows; the route view re-reads it.
pub struct DetailsChanged(pub ThreadKey);

/// What the card reads beyond the shell row.
#[derive(Default)]
pub struct DetailsFeed {
    shown: Option<ThreadKey>,
    tasks: Option<TasksFeed>,
    /// `pullRequests.detail` per link URL.
    details: HashMap<String, PullRequestDetail>,
    loading: HashSet<String>,
    server_busy: Option<u16>,
    automation_busy: Option<SharedString>,
    /// The Lineage merge or disconnect waiting on the server.
    relationship_busy: Option<crate::components::chat::thread_details_relationships::RelationshipBusy>,
}

impl EventEmitter<DetailsChanged> for DetailsFeed {}

fn toast(title: SharedString, error: impl Into<SharedString>, cx: &mut App) {
    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.into()), cx);
}

/// The scoped key's environment and thread.
fn ids(key: &ThreadKey) -> (EnvironmentId, ThreadId) {
    runtime_ids(key)
}

/// `PullRequestRef` of a link in the thread's project.
fn link_ref(link: &ThreadPullRequestLink, project_id: ProjectId) -> PullRequestRef {
    PullRequestRef {
        project_id,
        host: Some(link.host.clone()),
        expected_account_id: None,
        allow_stale: None,
        repository: link.repository.clone(),
        number: link.number,
    }
}

fn wire_merge_method(method: MergeMethod) -> PullRequestMergeMethod {
    match method {
        MergeMethod::Merge => PullRequestMergeMethod::Merge,
        MergeMethod::Squash => PullRequestMergeMethod::Squash,
        MergeMethod::Rebase => PullRequestMergeMethod::Rebase,
    }
}

impl DetailsFeed {
    /// Follows `key`: its environment's scheduled tasks and its discovered servers.
    pub fn follow(&mut self, key: Option<ThreadKey>, cx: &mut Context<Self>) {
        if self.shown == key {
            return;
        }
        self.shown = key.clone();
        PortForwards::watch_thread(key.as_ref().map(ids), cx);
        let environment = key.as_ref().map(|key| ids(key).0);
        if self.tasks.as_ref().map(|t| &t.environment) == environment.as_ref() {
            return;
        }
        self.tasks = environment.and_then(|environment| {
            let link = RpcLink::new(&environment, cx)?;
            let mut stream = link.stream::<ScheduledTasksSubscribe>(Default::default());
            let task = cx.spawn(async move |this, cx| {
                while let Some(item) = stream.next().await {
                    let stop = this
                        .update(cx, |this, cx| {
                            if let Some(feed) = &mut this.tasks {
                                match item {
                                    Ok(list) => {
                                        feed.time_zone = list.time_zone;
                                        feed.result = Some(Ok(list.tasks));
                                    }
                                    Err(error) => feed.result = Some(Err(error)),
                                }
                            }
                            this.changed(cx);
                        })
                        .is_err();
                    if stop {
                        break;
                    }
                }
            });
            Some(TasksFeed { environment, result: None, time_zone: None, _task: task })
        });
    }

    fn changed(&self, cx: &mut Context<Self>) {
        if let Some(key) = self.shown.clone() {
            cx.emit(DetailsChanged(key));
        }
    }

    /// Loads `pullRequests.detail` for the link once, and again after an action.
    fn load_detail(&mut self, environment: &EnvironmentId, reference: PullRequestRef, url: String, cx: &mut Context<Self>) {
        if !self.loading.insert(url.clone()) {
            return;
        }
        let Some(link) = RpcLink::new(environment, cx) else { return };
        let request = link.call::<PullRequestsDetail>(reference);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.loading.remove(&url);
                if let Ok(detail) = result {
                    this.details.insert(url, detail);
                    this.changed(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// The card's input for `view`: `base` plus what the shell row, the feed and the open thread
/// give it.
pub fn input(feed: &Entity<DetailsFeed>, view: &ThreadView, base: ThreadDetailsInput, cx: &mut App) -> ThreadDetailsInput {
    let Some(key) = super::view_target(view) else { return ThreadDetailsInput { is_draft: true, ..base } };
    let (environment_id, thread_id) = ids(&key);
    let row = StateBridge::shell(cx).read(cx).row(&key).cloned();
    let environment = StateBridge::environment(&environment_id, cx);
    let config = environment.as_ref().and_then(|e| e.read(cx).config.clone());
    let project_id = row.as_ref().map(|row| row.source.project_id.clone());
    let links = row.as_ref().and_then(|row| row.source.pull_requests.clone()).unwrap_or_default();

    let project_merge_method = config.as_ref().zip(project_id.as_ref()).and_then(|(config, project)| {
        config.settings.project_settings_overrides.as_ref()?.get(project.as_str())?.pull_request_merge_method
    });
    let last_merge_method = cx
        .try_global::<crate::ui_state_store::UiStateStore>()
        .map_or(PullRequestMergeMethod::Merge, |store| wire_merge_method(store.ui.pull_request_merge_method));
    let mut pull_request = row_data(&links, project_merge_method, last_merge_method);
    if let (Some(pr), Some(link), Some(project)) = (&mut pull_request, current_link(&links), project_id.clone()) {
        let detail = feed.read(cx).details.get(&link.url).cloned();
        match detail {
            Some(detail) => pr.detail = Some(detail),
            None => {
                let reference = link_ref(link, project);
                let url = link.url.clone();
                feed.update(cx, |feed, cx| feed.load_detail(&environment_id, reference, url, cx));
            }
        }
    }

    let can_stop = config.as_ref().is_some_and(|c| c.environment.capabilities.local_server_stop == Some(true));
    let local = super::git::is_loopback(RpcLink::new(&environment_id, cx).as_ref());
    let forwards = PortForwards::forwards(cx);
    let servers = PortForwards::servers(&(environment_id.clone(), thread_id.clone()), cx)
        .iter()
        .map(|server| {
            let port = server.port as u16;
            ThreadServerRow {
                port,
                process_name: server.process_name.clone().map(Into::into),
                from_terminal: server.terminal.is_some(),
                forwarded_to: forwards.iter().find(|f| f.environment_id == environment_id && f.remote_port == port).map(|f| f.local_port),
                can_stop: can_stop && server.pid.is_some(),
                can_forward: !local,
            }
        })
        .collect();

    let feed_ref = feed.read(cx);
    let (automations, automations_error) = match feed_ref.tasks.as_ref().and_then(|t| t.result.as_ref().map(|r| (r, t.time_zone.as_deref()))) {
        Some((Ok(tasks), time_zone)) => (
            tasks
                .iter()
                .filter(|task| task.thread_id.as_deref() == Some(thread_id.as_str()))
                .map(|task| ThreadAutomationRow {
                    id: task.id.clone().into(),
                    title: task.title.clone().into(),
                    schedule_line: schedule_label(&task.schedule, time_zone, task.next_run_at.as_deref()),
                    last_run: match task.last_run_status {
                        ScheduledTaskRunStatus::Running => AutomationRunStatus::Running,
                        ScheduledTaskRunStatus::Succeeded => AutomationRunStatus::Succeeded,
                        ScheduledTaskRunStatus::Failed => AutomationRunStatus::Failed,
                        _ => AutomationRunStatus::Never,
                    },
                    enabled: task.enabled,
                    webhook: matches!(task.schedule, ScheduledTaskSchedule::Webhook(_)),
                })
                .collect(),
            None,
        ),
        Some((Err(error), _)) => (Vec::new(), Some(error.clone())),
        None => (Vec::new(), None),
    };
    let (server_busy, automation_busy) = (feed_ref.server_busy, feed_ref.automation_busy.clone());

    ThreadDetailsInput {
        pull_request,
        issues: row.as_ref().and_then(|row| row.source.issues.clone()).unwrap_or_default(),
        is_draft: false,
        servers,
        server_busy,
        automations,
        automations_error,
        can_operate_automations: true,
        automation_busy,
        context_window: context_window(&key, cx),
        usage_limits: usage_limits(row.as_deref(), config.as_deref(), cx),
        relationships: relationships(&key, config.as_deref(), cx),
        relationship_busy: feed.read(cx).relationship_busy,
        ..base
    }
}

/// The Lineage and Side chats rows: the environment's live and archived shells and the shown
/// thread's projection (`ThreadRelationshipsPanel`).
fn relationships(key: &ThreadKey, config: Option<&t3code_contracts::ServerConfig>, cx: &App) -> Option<crate::state::thread_relationships::RelationshipsInput> {
    let (environment_id, thread_id) = ids(key);
    let environment = StateBridge::environment(&environment_id, cx)?;
    let snapshot = environment.read(cx).shell.snapshot.clone()?;
    let thread = StateBridge::threads(cx).read(cx).find(key, cx).cloned();
    let projection = thread.as_ref().and_then(|t| t.read(cx).projection().cloned());
    let shells = snapshot.threads.iter().chain(snapshot.archived_threads.iter()).collect();
    let providers: Vec<_> = config.map(|c| c.providers.iter().cloned().map(std::sync::Arc::new).collect()).unwrap_or_default();
    let entries = crate::provider_instances::derive_provider_instance_entries(&providers);
    let labels: Vec<(t3code_contracts::EnvironmentId, SharedString)> = StateBridge::environments(cx)
        .read(cx)
        .list
        .iter()
        .filter_map(|e| {
            let env = e.read(cx);
            Some((env.environment_id().clone(), SharedString::from(env.config.as_ref()?.environment.label.clone())))
        })
        .collect();
    crate::state::thread_relationships::derive(crate::state::thread_relationships::RelationshipSources {
        thread_id: &thread_id,
        shells,
        projection: projection.as_deref(),
        is_hidden: &|_| false,
        hiding_anywhere: false,
        show_lineage: true,
        show_side_chats: true,
        any_instance_badge: crate::provider_instances::has_instance_badge(&entries),
        connected_label: &|id| labels.iter().find(|(e, _)| e == id).map(|(_, label)| label.clone()),
    })
}

/// Opens a related thread (`openThread` with `resolveThreadOpenRoute`): a side chat opens as a
/// tab of its parent, anything else in the main pane.
fn open_related(key: &ThreadKey, related: &ThreadId, cx: &mut App) {
    let (environment_id, _) = ids(key);
    let target = crate::state::shell::thread_key(&environment_id, related);
    let parent = StateBridge::environment(&environment_id, cx).and_then(|env| {
        let snapshot = env.read(cx).shell.snapshot.clone()?;
        let shell = snapshot.threads.iter().chain(snapshot.archived_threads.iter()).find(|t| &t.id == related)?.clone();
        (shell.lineage.side_chat == Some(true)).then(|| shell.lineage.parent_thread_id.clone()).flatten()
    });
    match parent {
        Some(parent) => crate::state::open_side_chat(crate::state::shell::thread_key(&environment_id, &parent), target, cx),
        None if &target != key => crate::state::open_thread(target, cx),
        None => {}
    }
}

/// Runs a Lineage or Side chats action.
fn relationship_event(feed: &Entity<DetailsFeed>, key: &ThreadKey, event: &crate::components::chat::thread_details_relationships::RelationshipEvent, cx: &mut App) -> Task<DetailsOutcome> {
    use crate::components::chat::thread_details_relationships::{RelationshipBusy, RelationshipEvent};
    let (environment_id, thread_id) = ids(key);
    match event {
        RelationshipEvent::Open(related) => {
            open_related(key, related, cx);
            Task::ready(DetailsOutcome::Done)
        }
        RelationshipEvent::OpenDelegatedFrom => {
            let from = StateBridge::shell(cx).read(cx).row(key).and_then(|row| row.source.delegated_from.clone());
            if let Some(from) = from {
                crate::state::open_thread(crate::state::shell::thread_key(&from.environment_id, &from.thread_id), cx);
            }
            Task::ready(DetailsOutcome::Done)
        }
        RelationshipEvent::ToggleMenu(_) => Task::ready(DetailsOutcome::Done),
        RelationshipEvent::MergeBack | RelationshipEvent::Disconnect => {
            if feed.read(cx).relationship_busy.is_some() {
                return Task::ready(DetailsOutcome::Done);
            }
            let Some(environment) = StateBridge::environment(&environment_id, cx) else { return Task::ready(DetailsOutcome::Done) };
            let commands = environment.read(cx).commands.clone();
            let projection = StateBridge::threads(cx).read(cx).find(key, cx).and_then(|t| t.read(cx).projection().cloned());
            let merging = matches!(event, RelationshipEvent::MergeBack);
            let (busy, failure) = if merging {
                (RelationshipBusy::Merge, t!("Could not merge back"))
            } else {
                (RelationshipBusy::Detach, t!("Could not disconnect agent session"))
            };
            let merge = projection.as_deref().and_then(|p| {
                let target = t3code_client_runtime::state::thread_relationships::resolve_merge_back_target_thread_id(Some(p))?;
                let run = t3code_client_runtime::state::thread_workflows::resolve_latest_merge_back_run(&p.runs)?.id.clone();
                Some((target, run))
            });
            if merging && merge.is_none() {
                return Task::ready(DetailsOutcome::Done);
            }
            let source = thread_id.clone();
            let request = merge.clone();
            let task = StateBridge::spawn(
                async move {
                    match request {
                        Some((target, run_id)) => {
                            let point = t3code_contracts::OrchestrationV2ThreadForkSourcePoint::Run(t3code_contracts::OrchestrationV2ThreadForkSourcePointRun { run_id });
                            commands.merge_back(&source, target, point).await.map(|_| ())
                        }
                        None => commands.stop_sessions(&source).await.map(|_| ()),
                    }
                },
                cx,
            );
            feed.update(cx, |feed, cx| {
                feed.relationship_busy = Some(busy);
                cx.emit(DetailsChanged(key.clone()));
            });
            let (feed, key) = (feed.clone(), key.clone());
            cx.spawn(async move |cx| {
                let result = task.await;
                cx.update(|cx| {
                    feed.update(cx, |feed, cx| {
                        feed.relationship_busy = None;
                        cx.emit(DetailsChanged(key.clone()));
                    });
                    match result {
                        Some(Err(error)) => toast(failure, error.to_string(), cx),
                        Some(Ok(())) if merging => {
                            if let Some((target, _)) = merge {
                                open_related(&key, &target, cx);
                            }
                        }
                        _ => {}
                    }
                });
                DetailsOutcome::Done
            })
        }
    }
}

/// `ThreadDetailsUsage`'s rows: the instance's entry of the report merged across instances and
/// limit hubs reporting the same login, else the instance's own snapshot (the report keeps one
/// instance per login; another entry may be a different login or workspace).
fn usage_limits(row: Option<&ThreadRow>, config: Option<&t3code_contracts::ServerConfig>, cx: &App) -> Option<UsageLimitsInput> {
    use t3code_shared::usage_limits::{LimitPace, collect_provider_usage_limits, limits_notice_reason, pace_of, pace_remaining_percent, remaining_percent};
    let instance = &row?.source.model_selection.instance_id;
    let config = config?;
    let sources = config.usage_limit_sources.as_deref().unwrap_or_default();
    let report = collect_provider_usage_limits(instance, &config.providers, sources);
    let merged = report.as_ref().and_then(|report| report.accounts.iter().find(|entry| entry.instance_id.as_ref().is_some_and(|id| id.as_str() == instance.as_str())).map(|entry| &entry.limits));
    let own = config.providers.iter().find(|provider| provider.instance_id == *instance).and_then(|provider| provider.usage_limits.as_ref());
    let limits = merged.or(own)?;
    if let Some(reason) = limits_notice_reason(limits) {
        return Some(UsageLimitsInput::Notice(crate::components::usage::usage_limits::limits_notice(reason)));
    }
    // `useNowMinute`: the rows move once a minute.
    let now = (jiff::Timestamp::now().as_millisecond() / 60_000 * 60_000) as f64;
    let format = crate::hooks::settings_bridge::timestamp_format(cx);
    let windows = limits
        .windows
        .iter()
        .map(|window| {
            let left = remaining_percent(window);
            let tick = pace_remaining_percent(window, now);
            let mut lines = Vec::new();
            if let (Some(pace), Some(time_left)) = (pace_of(window, now), tick) {
                let pace = match pace {
                    LimitPace::Ahead => t!("Ahead of pace"),
                    LimitPace::On => t!("On pace"),
                    LimitPace::Under => t!("Under pace"),
                };
                lines.push(SharedString::from(format!("{pace} · {}", t!("{timeLeft}% of the window left", timeLeft = time_left as i64))));
            }
            if let Some(resets_at) = window.resets_at.as_deref()
                && let Some(at) = t3code_shared::usage_format::parse_instant_ms(resets_at).filter(|at| *at as f64 > now)
            {
                lines.push(t!(
                    "Resets in {duration} · {time}",
                    duration = crate::components::usage::usage_limits::format_duration(at as f64 - now).as_ref(),
                    time = crate::timestamp_format::format_upcoming_timestamp(resets_at, format, now as i64, None).as_ref()
                ));
            }
            UsageWindowRow {
                id: window.id.clone().into(),
                label: window.label.clone().into(),
                left: left.clamp(0., 100.) as u8,
                tick: tick.map(|t| t as f32),
                lines,
            }
        })
        .collect();
    Some(UsageLimitsInput::Windows(windows))
}

/// `deriveLatestContextWindowSnapshot` without the compaction case: the active provider
/// thread's latest turn usage, else the usage the provider thread reported.
fn context_window(key: &ThreadKey, cx: &App) -> Option<ContextWindowRow> {
    let thread = StateBridge::threads(cx).read(cx).find(key, cx)?.clone();
    let projection = thread.read(cx).projection()?.clone();
    let provider = projection.thread.active_provider_thread_id.as_ref()?;
    let live = projection
        .provider_turns
        .iter()
        .filter(|turn| &turn.provider_thread_id == provider)
        .filter_map(|turn| turn.token_usage.as_ref())
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at));
    let (used, max, threshold) = match live {
        Some(usage) => (usage.used_tokens, usage.max_tokens, usage.auto_compact_threshold),
        None => {
            let usage = projection.provider_threads.iter().find(|p| &p.id == provider)?.context_usage.as_ref()?;
            (usage.used_tokens, usage.max_tokens, None)
        }
    };
    let used_percentage = max.filter(|m| *m > 0).map(|m| (used as f32 / m as f32 * 100.).min(100.));
    Some(ContextWindowRow { used_tokens: used, max_tokens: max, used_percentage, auto_compact_threshold: threshold })
}

/// Runs a card event on `key`.
pub fn event(feed: &Entity<DetailsFeed>, key: &ThreadKey, event: &ThreadDetailsEvent, cx: &mut App) -> Task<DetailsOutcome> {
    let (environment_id, thread_id) = ids(key);
    let Some(link) = RpcLink::new(&environment_id, cx) else { return Task::ready(DetailsOutcome::Done) };
    let row = StateBridge::shell(cx).read(cx).row(key).cloned();
    let links = row.as_ref().and_then(|row| row.source.pull_requests.clone()).unwrap_or_default();
    let current = current_link(&links).cloned();
    let project_id = row.as_ref().map(|row| row.source.project_id.clone());
    let detail = current.as_ref().and_then(|l| feed.read(cx).details.get(&l.url).cloned());
    match event {
        ThreadDetailsEvent::Relationship(relationship) => return relationship_event(feed, key, relationship, cx),
        ThreadDetailsEvent::OpenUrl(_) => Task::ready(DetailsOutcome::Done),
        ThreadDetailsEvent::PullRequestAction(PullRequestRowAction::Resolve) => {
            Task::ready(detail.map_or(DetailsOutcome::Done, |d| DetailsOutcome::LoadComposer(resolve_conflicts_prompt(&d).into())))
        }
        ThreadDetailsEvent::PullRequestAction(PullRequestRowAction::Fix) => {
            Task::ready(detail.map_or(DetailsOutcome::Done, |d| DetailsOutcome::LoadComposer(fix_checks_prompt(&d).into())))
        }
        ThreadDetailsEvent::PullRequestAction(PullRequestRowAction::Merge) => Task::ready(DetailsOutcome::Done),
        ThreadDetailsEvent::PullRequestAction(PullRequestRowAction::Ready) | ThreadDetailsEvent::Merge(_) => {
            let (Some(current), Some(project_id)) = (current, project_id) else { return Task::ready(DetailsOutcome::Done) };
            let (action, merge_method) = match event {
                ThreadDetailsEvent::Merge(method) => (PullRequestAction::Merge, Some(*method)),
                _ => (PullRequestAction::Ready, None),
            };
            let reference = link_ref(&current, project_id);
            let input = PullRequestActionInput {
                stack_number: None,
                expected_stack_heads: None,
                expected_head_sha: None,
                project_id: reference.project_id.clone(),
                host: reference.host.clone(),
                expected_account_id: None,
                allow_stale: None,
                repository: reference.repository.clone(),
                number: reference.number,
                action,
                merge_method,
                update_method: None,
                reply_with_summary: None,
            };
            let request = link.call::<PullRequestsRunAction>(input);
            let (feed, url) = (feed.clone(), current.url.clone());
            cx.spawn(async move |cx| {
                let result = request.await;
                cx.update(|cx| match result {
                    Ok(_) => feed.update(cx, |feed, cx| {
                        feed.details.remove(&url);
                        feed.load_detail(&environment_id, reference, url, cx);
                    }),
                    Err(error) => toast(t!("Something went wrong."), error, cx),
                });
                DetailsOutcome::Done
            })
        }
        ThreadDetailsEvent::StopWatching => {
            let Some(current) = current else { return Task::ready(DetailsOutcome::Done) };
            let Some(environment) = StateBridge::environment(&environment_id, cx) else { return Task::ready(DetailsOutcome::Done) };
            let commands = environment.read(cx).commands.clone();
            let command = OrchestrationV2Command::ThreadPullRequestWatch(OrchestrationV2CommandThreadPullRequestWatch {
                command_id: new_command_id(),
                thread_id,
                host: current.host,
                repository: current.repository,
                number: current.number,
                watching: false,
                until: None,
                checks_pending: None,
                link: None,
            });
            let task = StateBridge::spawn(async move { commands.dispatch(command).await }, cx);
            cx.spawn(async move |cx| {
                if let Some(Err(error)) = task.await {
                    cx.update(|cx| toast(t!("Something went wrong."), error.to_string(), cx));
                }
                DetailsOutcome::Done
            })
        }
        ThreadDetailsEvent::Section(section) => section_event(feed, &link, (environment_id, thread_id), section, cx),
    }
}

fn section_event(
    feed: &Entity<DetailsFeed>,
    link: &RpcLink,
    (environment_id, thread_id): (EnvironmentId, ThreadId),
    event: &ThreadDetailsSectionEvent,
    cx: &mut App,
) -> Task<DetailsOutcome> {
    let server = |port: u16, cx: &App| {
        PortForwards::servers(&(environment_id.clone(), thread_id.clone()), cx).iter().find(|s| s.port as u16 == port).cloned()
    };
    let feed = feed.clone();
    let palette_id = || crate::components::sidebar::model::EnvironmentId(environment_id.as_str().to_owned().into());
    match event {
        ThreadDetailsSectionEvent::OpenServer { port, external } => {
            let Some(server) = server(*port, cx) else { return Task::ready(DetailsOutcome::Done) };
            if *external {
                if super::git::is_loopback(Some(link)) {
                    cx.open_url(&server.url);
                } else {
                    PortForwards::open_forwarded(environment_id, *port, server.url, cx);
                }
                return Task::ready(DetailsOutcome::Done);
            }
            let input = PreviewOpenInput {
                thread_id,
                url: Some(server.url),
                viewport: None,
                profile_id: None,
                runtime: None,
                color_scheme: None,
                zoom_factor: None,
                reveal: None,
            };
            let request = link.call::<PreviewOpen>(input);
            cx.spawn(async move |cx| match request.await {
                Ok(snapshot) => DetailsOutcome::OpenBrowser(snapshot.tab_id.into()),
                Err(error) => {
                    cx.update(|cx| toast(t!("Unable to open preview"), error, cx));
                    DetailsOutcome::Done
                }
            })
        }
        ThreadDetailsSectionEvent::StopServer(port) => {
            let Some(pid) = server(*port, cx).and_then(|s| s.pid) else { return Task::ready(DetailsOutcome::Done) };
            let request = link.call::<PreviewStopLocalServer>(PreviewStopLocalServerInput { pid, port: u64::from(*port) });
            feed.update(cx, |feed, cx| {
                feed.server_busy = Some(*port);
                feed.changed(cx);
            });
            cx.spawn(async move |cx| {
                let result = request.await;
                cx.update(|cx| {
                    feed.update(cx, |feed, cx| {
                        feed.server_busy = None;
                        feed.changed(cx);
                    });
                    if let Err(error) = result {
                        toast(t!("Could not stop server"), error, cx);
                    }
                });
                DetailsOutcome::Done
            })
        }
        ThreadDetailsSectionEvent::ToggleForward { port, forwarded } => {
            let (environment_id, port) = (palette_id(), *port);
            let request = if *forwarded { PortForwardRequest::Stop { environment_id, port } } else { PortForwardRequest::Start { environment_id, port } };
            PortForwards::handle(request, cx);
            Task::ready(DetailsOutcome::Done)
        }
        ThreadDetailsSectionEvent::ManageAutomations(_) | ThreadDetailsSectionEvent::OpenUsage => Task::ready(DetailsOutcome::Done),
        ThreadDetailsSectionEvent::RunAutomation(id) | ThreadDetailsSectionEvent::SetAutomationEnabled { id, .. } => {
            let task_id = ScheduledTaskId::new(id.as_ref());
            let (request, failure) = match event {
                ThreadDetailsSectionEvent::SetAutomationEnabled { enabled, .. } => (
                    link.call::<ScheduledTasksSetEnabled>(ScheduledTaskSetEnabledInput { id: task_id, enabled: *enabled }).map(|r| r.map(|_| ())).boxed(),
                    t!("Could not update automation"),
                ),
                _ => (link.call::<ScheduledTasksRunNow>(ScheduledTaskRunNowInput { id: task_id }).map(|r| r.map(|_| ())).boxed(), t!("Could not run automation")),
            };
            let id = id.clone();
            feed.update(cx, |feed, cx| {
                feed.automation_busy = Some(id);
                feed.changed(cx);
            });
            cx.spawn(async move |cx| {
                let result = request.await;
                cx.update(|cx| {
                    feed.update(cx, |feed, cx| {
                        feed.automation_busy = None;
                        feed.changed(cx);
                    });
                    if let Err(error) = result {
                        toast(failure, error, cx);
                    }
                });
                DetailsOutcome::Done
            })
        }
    }
}

/// Re-reads the shown thread whenever the feed or its discovered servers change.
pub fn follow_changes(feed: &Entity<DetailsFeed>, events: &Entity<DataEvents>, cx: &mut App) -> Vec<gpui_kit::Subscription> {
    let emit = |events: &Entity<DataEvents>, key: ThreadKey, cx: &mut App| {
        events.update(cx, |_, cx| cx.emit(DataEvent::ThreadChanged(ThreadTarget::Server(key))));
    };
    let changed = {
        let events = events.clone();
        cx.subscribe(feed, move |_, DetailsChanged(key), cx| emit(&events, key.clone(), cx))
    };
    let servers = {
        let (events, feed) = (events.clone(), feed.downgrade());
        cx.observe_global::<PortForwards>(move |cx| {
            if let Some(key) = feed.upgrade().and_then(|feed| feed.read(cx).shown.clone()) {
                emit(&events, key, cx);
            }
        })
    };
    vec![changed, servers]
}
