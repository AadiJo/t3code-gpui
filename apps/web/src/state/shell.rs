//! The sidebar's view model from every enabled environment's shell (`state/shell.ts`,
//! `Sidebar.logic.ts` inputs). [`LiveShell`] presents rows with the runtime's
//! [`ShellRows`], filters them with [`navigation_rows`], and maps each to a
//! [`SidebarThread`]. A mapped row keeps its `Arc` while its presented row, draft flag, visit
//! fallback and wake time are unchanged, so an update to one thread rebuilds one row.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, Context, Entity, SharedString, Subscription};
use t3code_client_runtime::state::client_store::ClientStore;
use t3code_client_runtime::state::drafts::{Drafts, scoped_thread_key};
use t3code_client_runtime::state::models::{EpochMs, ThreadRow, now_ms, parse_ms};
use t3code_client_runtime::state::sidebar::last_visited_at;
use t3code_client_runtime::state::thread_settled::thread_woke_at;
use t3code_client_runtime::state::thread_shell::{ShellRows, navigation_rows};
use t3code_client_runtime::state::thread_tag_filter::{hidden_tags, hidden_thread_keys};
use t3code_client_runtime::state::ui_state::UiState;
use t3code_contracts::{
    OrchestrationProjectShell, OrchestrationV2ProviderFailureClass as FailureClass, OrchestrationV2ProviderGoalStatus,
    OrchestrationV2ShellThreadStatus as ShellStatus, OrchestrationV2ThreadShell,
    OrchestrationV2ThreadShellSettledOverride as Settled,
};

use super::{Environments, LiveEnvironment};
use crate::components::sidebar::model::{
    self as model, ErrorClass, LatestRun, ProjectRef, RunStatus, RuntimeStatus, SettledOverride, SidebarProject,
    SidebarSettings, SidebarSnapshot, SidebarThread, SubagentRequest, ThreadKey, ThreadRuntime, TimestampMs,
};

pub type RowKey = (t3code_contracts::EnvironmentId, t3code_contracts::ThreadId);

/// What a mapped row was built from; equal inputs reuse the mapped `Arc`.
struct MappedRow {
    source: Arc<ThreadRow>,
    local_visit: Option<EpochMs>,
    woke_at: Option<EpochMs>,
    has_draft: bool,
    remote: bool,
    unread_completion: bool,
    thread: Arc<SidebarThread>,
}

/// Every environment's shell, joined. Notifies once per update cycle when any shell changes.
pub struct LiveShell {
    environments: Entity<Environments>,
    drafts: Rc<ClientStore<Drafts>>,
    ui_state: Rc<ClientStore<UiState>>,
    rows: ShellRows,
    mapped: HashMap<RowKey, MappedRow>,
    /// Every presented row across environments, archived and hidden ones included.
    presented: Vec<Arc<ThreadRow>>,
    snapshot: SidebarSnapshot,
    active_thread: Option<ThreadKey>,
    /// Bumped on each rebuild; sources that cache derived lists compare it.
    revision: u64,
    rebuild_pending: bool,
    rebuilds: u64,
    /// The server's message matches for the sidebar search.
    search_matches: Option<model::SearchContentMatches>,
    /// Threads the servers' tag filters hide.
    hidden: Arc<HashSet<RowKey>>,
    /// Whether some environment's tag filter is on.
    hiding: bool,
    _environment_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

/// Converts a runtime id to the sidebar model's id without copying the string.
pub fn environment_key(id: &t3code_contracts::EnvironmentId) -> model::EnvironmentId {
    model::EnvironmentId(SharedString::from(Arc::clone(&id.0)))
}

/// The sidebar key of a runtime thread.
pub fn thread_key(environment_id: &t3code_contracts::EnvironmentId, thread_id: &t3code_contracts::ThreadId) -> ThreadKey {
    ThreadKey { environment_id: environment_key(environment_id), thread_id: model::ThreadId(SharedString::from(Arc::clone(&thread_id.0))) }
}

/// The runtime ids of a sidebar key.
pub fn runtime_ids(key: &ThreadKey) -> (t3code_contracts::EnvironmentId, t3code_contracts::ThreadId) {
    (
        t3code_contracts::EnvironmentId::new(key.environment_id.0.as_ref()),
        t3code_contracts::ThreadId::new(key.thread_id.0.as_ref()),
    )
}

fn ts(ms: Option<EpochMs>) -> Option<TimestampMs> {
    ms.map(TimestampMs)
}

fn run_status(status: ShellStatus) -> RunStatus {
    match status {
        ShellStatus::Preparing => RunStatus::Preparing,
        ShellStatus::Queued => RunStatus::Queued,
        ShellStatus::Starting => RunStatus::Starting,
        ShellStatus::Running => RunStatus::Running,
        ShellStatus::Waiting => RunStatus::Waiting,
        ShellStatus::Idle | ShellStatus::Completed => RunStatus::Completed,
        ShellStatus::Interrupted => RunStatus::Interrupted,
        ShellStatus::Failed => RunStatus::Failed,
        ShellStatus::Cancelled => RunStatus::Cancelled,
        ShellStatus::RolledBack => RunStatus::RolledBack,
    }
}

/// One provider instance as a sidebar row marks it, before the row's model picks the glyph.
#[derive(Clone)]
struct InstanceMark {
    driver: crate::model_selection::ProviderDriverKind,
    acp_registry: bool,
    initials: SharedString,
    show_badge: bool,
}

/// Every provider instance of one environment's config, by instance id
/// (`SidebarProviderStack` inputs: `ProviderInstanceIcon`, `providerInstanceInitials`,
/// `shouldShowInstanceBadge`).
fn instance_marks(config: Option<&t3code_contracts::ServerConfig>) -> HashMap<String, InstanceMark> {
    use crate::provider_instances::{derive_provider_instance_entries, provider_instance_initials, should_show_instance_badge};
    let Some(config) = config else { return HashMap::new() };
    let providers: Vec<_> = config.providers.iter().cloned().map(Arc::new).collect();
    let entries = derive_provider_instance_entries(&providers);
    entries
        .iter()
        .map(|entry| {
            let mark = InstanceMark {
                driver: driver_kind(&entry.driver_kind),
                acp_registry: entry.driver_kind == "acpRegistry",
                initials: provider_instance_initials(&entry.display_name).into(),
                show_badge: should_show_instance_badge(entry, &entries),
            };
            (entry.instance_id.clone(), mark)
        })
        .collect()
}

fn driver_kind(name: &str) -> crate::model_selection::ProviderDriverKind {
    use crate::model_selection::ProviderDriverKind as Kind;
    match name {
        "codex" => Kind::Codex,
        "claudeAgent" => Kind::ClaudeAgent,
        "cursor" => Kind::Cursor,
        "opencode" => Kind::OpenCode,
        "grok" => Kind::Grok,
        "antigravity" => Kind::Antigravity,
        other => Kind::Other(SharedString::from(other.to_owned())),
    }
}

/// The mark a row shows for the instance and model it runs on.
fn row_provider(row: &ThreadRow, marks: &HashMap<String, InstanceMark>) -> Option<model::SidebarProvider> {
    let selection = &row.source.model_selection;
    let mark = marks.get(&selection.instance_id)?;
    Some(model::SidebarProvider {
        mark: if mark.acp_registry {
            model::SidebarProviderMark::AcpRegistry
        } else {
            model::SidebarProviderMark::Brand(crate::model_selection::ProviderIcon::for_model(&mark.driver, Some(&selection.model)))
        },
        initials: mark.initials.clone(),
        show_badge: mark.show_badge,
    })
}

/// What a row shows beyond its shell row.
#[derive(Clone, Default)]
pub(super) struct RowExtras {
    pub local_visit: Option<EpochMs>,
    pub woke_at: Option<EpochMs>,
    pub has_draft: bool,
    pub remote: bool,
    /// The inbox holds an unread completion for it: the sidebar's Done marker.
    pub unread_completion: bool,
    pub provider: Option<model::SidebarProvider>,
}

/// One presented row as the sidebar draws it (`SidebarThreadSummary`).
pub(super) fn map_row(row: &ThreadRow, extras: RowExtras) -> SidebarThread {
    let RowExtras { local_visit, woke_at, has_draft, remote, unread_completion, provider } = extras;
    let source: &OrchestrationV2ThreadShell = &row.source;
    SidebarThread {
        key: thread_key(&row.environment_id, &source.id),
        project_id: model::ProjectId(SharedString::from(Arc::clone(&source.project_id.0))),
        title: SharedString::from(source.title.clone()),
        provider,
        branch: source.branch.clone().map(SharedString::from),
        in_worktree: source.worktree_path.is_some(),
        created_at: TimestampMs(row.created_at.unwrap_or_default()),
        updated_at: TimestampMs(row.updated_at.or(row.created_at).unwrap_or_default()),
        latest_user_message_at: ts(row.latest_user_message_at),
        latest_user_authored_message_at: ts(row.latest_user_authored_message_at),
        runtime: row.runtime.as_ref().map(|runtime| ThreadRuntime {
            status: match runtime.status {
                ShellStatus::Idle => RuntimeStatus::Idle,
                status => RuntimeStatus::Run(run_status(status)),
            },
            last_error_class: runtime.last_error_class.map(|class| match class {
                FailureClass::UsageLimit => ErrorClass::UsageLimit,
                _ => ErrorClass::Other,
            }),
        }),
        latest_run: row.latest_run.as_ref().map(|run| LatestRun {
            status: run_status(run.status),
            requested_at: ts(run.requested_at),
            started_at: ts(run.started_at),
            completed_at: ts(run.completed_at),
        }),
        blocked_on_thread_wait: source.blocked_on_thread_wait == Some(true),
        has_pending_approvals: row.has_pending_approvals,
        has_pending_user_input: row.has_pending_user_input,
        pending_subagent_request: row.pending_subagent_request.map(|request| match request {
            t3code_client_runtime::state::models::PendingSubagentRequest::Approval => SubagentRequest::Approval,
            t3code_client_runtime::state::models::PendingSubagentRequest::UserInput => SubagentRequest::UserInput,
        }),
        pending_guest_message_count: source.pending_guest_message_count.unwrap_or(0) as u32,
        goal_active: source.goal.as_ref().is_some_and(|goal| goal.status == OrchestrationV2ProviderGoalStatus::Active),
        last_visited_at: ts(last_visited_at(row, local_visit)),
        has_unread_completion_notification: unread_completion,
        settled_override: source.settled_override.map(|value| match value {
            Settled::Settled => SettledOverride::Settled,
            Settled::Active => SettledOverride::Active,
        }),
        settled_at: ts(row.settled_at),
        unsettled_at: ts(row.unsettled_at),
        snoozed_until: ts(row.snoozed_until),
        woke_at: ts(woke_at),
        pinned_at: ts(parse_opt_ms(source.pinned_at.as_deref())),
        pin_order_key: source.pin_order_key.clone().map(SharedString::from),
        active_order_key: source.active_order_key.clone().map(SharedString::from),
        archived: source.archived_at.is_some(),
        has_unsent_draft: has_draft,
        diff: None,
        pull_request: None,
        issue_key: None,
        running_terminal_count: 0,
        remote_machine: remote,
    }
}

fn parse_opt_ms(iso: Option<&str>) -> Option<EpochMs> {
    iso.and_then(parse_ms)
}

/// One project as the sidebar groups it. `logical_key` is the group key with the per-project
/// override applied; it reaches the sidebar as the repository key (grouping runs in repository
/// mode), and a project that stands alone gets none.
fn map_project(
    environment_id: &t3code_contracts::EnvironmentId,
    project: &OrchestrationProjectShell,
    logical_key: &str,
    group_label: Option<String>,
    favicon: Option<Arc<std::path::Path>>,
) -> SidebarProject {
    let physical = t3code_client_runtime::state::project_grouping::physical_project_key(environment_id, &project.workspace_root);
    SidebarProject {
        project_ref: ProjectRef {
            environment_id: environment_key(environment_id),
            project_id: model::ProjectId(SharedString::from(Arc::clone(&project.id.0))),
        },
        title: SharedString::from(project.title.clone()),
        workspace_root: SharedString::from(project.workspace_root.clone()),
        repository_key: (logical_key != physical).then(|| SharedString::from(logical_key.to_owned())),
        repository_relative_path: None,
        repository_display_name: project.repository_identity.as_ref().and_then(|identity| match &identity.origin {
            Some(origin) => Some(origin.display_name.clone().unwrap_or_else(|| origin.canonical_key.clone()).into()),
            None => identity.display_name.clone().map(SharedString::from),
        }),
        repository_name: project.repository_identity.as_ref().and_then(|identity| identity.name.clone().map(SharedString::from)),
        has_custom_title: project.has_custom_title.unwrap_or(false),
        created_at: TimestampMs(parse_ms(&project.created_at).unwrap_or_default()),
        updated_at: TimestampMs(parse_ms(&project.updated_at).unwrap_or_default()),
        pinned: project.pinned_at.is_some(),
        archived: project.archived_at.is_some(),
        group_label: group_label.map(SharedString::from),
        icon: project.project_icon.clone(),
        favicon,
    }
}

/// The sidebar settings the client settings choose, and the grouping settings.
fn sidebar_settings(cx: &App) -> (SidebarSettings, t3code_client_runtime::state::project_grouping::ProjectGroupingSettings) {
    use t3code_contracts::SidebarProjectSortOrder as Sort;
    let settings = crate::hooks::use_settings::ClientSettingsStore::get(cx);
    let sidebar = SidebarSettings {
        project_folders: settings.boring_sidebar_enabled,
        working_shelf: settings.sidebar_working_shelf_enabled,
        // Per-project overrides are folded into each project's key, so the sidebar groups by it.
        grouping_mode: model::ProjectGroupingMode::Repository,
        project_sort_order: match settings.sidebar_project_sort_order {
            Sort::CreatedAt => model::ProjectSortOrder::CreatedAt,
            Sort::Manual => model::ProjectSortOrder::Manual,
            _ => model::ProjectSortOrder::UpdatedAt,
        },
        notification_inbox: settings.notification_inbox_enabled,
        ..SidebarSettings::default()
    };
    let grouping = t3code_client_runtime::state::project_grouping::ProjectGroupingSettings {
        mode: settings.sidebar_project_grouping_mode,
        overrides: settings.sidebar_project_grouping_overrides.iter().map(|(k, v)| (k.clone(), *v)).collect(),
    };
    (sidebar, grouping)
}

impl LiveShell {
    pub(super) fn new(
        environments: Entity<Environments>,
        drafts: Rc<ClientStore<Drafts>>,
        ui_state: Rc<ClientStore<UiState>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = cx.observe_global::<crate::hooks::use_settings::ClientSettingsStore>(|this, cx| this.schedule(cx));
        let inbox = cx.observe(&crate::desktop_notifications::NotificationInbox::global(cx), |this, _, cx| this.schedule(cx));
        let subscription = cx.observe(&environments, |this, environments, cx| {
            let list = environments.read(cx).list.clone();
            this._environment_subscriptions = list.iter().map(|env| cx.observe(env, |this, _, cx| this.schedule(cx))).collect();
            this.schedule(cx);
        });
        let mut this = Self {
            environments,
            drafts,
            ui_state,
            rows: ShellRows::default(),
            mapped: HashMap::new(),
            presented: Vec::new(),
            snapshot: SidebarSnapshot::default(),
            active_thread: None,
            revision: 0,
            rebuild_pending: false,
            rebuilds: 0,
            search_matches: None,
            hidden: Arc::default(),
            hiding: false,
            _environment_subscriptions: Vec::new(),
            _subscriptions: vec![subscription, settings, inbox],
        };
        this.rebuild_now(cx);
        this
    }

    /// The sidebar's view model as of the last rebuild.
    pub fn snapshot(&self) -> &SidebarSnapshot {
        &self.snapshot
    }

    /// Every presented row, in environment order.
    pub fn presented(&self) -> &[Arc<ThreadRow>] {
        &self.presented
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many times the snapshot was rebuilt, for measurements.
    pub fn rebuild_count(&self) -> u64 {
        self.rebuilds
    }

    /// Threads the servers' tag filters hide, and whether any filter is on.
    pub fn hidden(&self) -> (&Arc<HashSet<RowKey>>, bool) {
        (&self.hidden, self.hiding)
    }

    /// The presented row of a thread.
    pub fn row(&self, key: &ThreadKey) -> Option<&Arc<ThreadRow>> {
        self.presented.iter().find(|row| row.environment_id.as_str() == key.environment_id.0.as_ref() && row.id().as_str() == key.thread_id.0.as_ref())
    }

    /// Marks the thread the main pane shows.
    pub fn set_active_thread(&mut self, key: Option<ThreadKey>, cx: &mut Context<Self>) {
        if self.active_thread != key {
            self.active_thread = key.clone();
            self.snapshot.active_thread = key;
            self.revision += 1;
            cx.notify();
        }
    }

    pub fn active_thread(&self) -> Option<&ThreadKey> {
        self.active_thread.as_ref()
    }

    /// Rebuilds once at the end of the current update cycle, however many shells changed in
    /// it. Each environment's shell already arrives at most once per frame.
    fn schedule(&mut self, cx: &mut Context<Self>) {
        if self.rebuild_pending {
            return;
        }
        self.rebuild_pending = true;
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            _ = this.update(cx, |this, cx| {
                this.rebuild_pending = false;
                this.rebuild_now(cx);
            });
        });
    }

    /// Rebuilds the snapshot from the environments' current shells.
    pub fn rebuild_now(&mut self, cx: &mut Context<Self>) {
        let environments: Vec<Entity<LiveEnvironment>> = self.environments.read(cx).list.clone();
        let envs: Vec<&LiveEnvironment> = environments.iter().map(|env| env.read(cx)).collect();
        let shells: Vec<_> = envs
            .iter()
            .filter_map(|env| env.shell.snapshot.as_ref().map(|snapshot| (env.environment_id().clone(), Arc::clone(&snapshot.threads))))
            .collect();
        let borrowed: Vec<_> = shells.iter().map(|(id, threads)| (id.clone(), threads.as_slice())).collect();
        self.presented = self.rows.present(&borrowed);
        let filters: HashMap<_, _> = envs
            .iter()
            .filter_map(|env| Some((env.environment_id().clone(), hidden_tags(env.config.as_deref().map(|config| &config.settings))?)))
            .collect();
        let hidden = hidden_thread_keys(&self.presented, &filters);
        if *self.hidden != hidden {
            self.hidden = Arc::new(hidden);
        }
        self.hiding = !filters.is_empty();
        let listed = navigation_rows(&self.presented, &self.hidden);
        let unread = crate::desktop_notifications::NotificationInbox::global(cx).read(cx).unread_completion_threads();
        let now = now_ms();
        let drafts = self.drafts.current();
        let ui_state = self.ui_state.current();
        let remote = envs.len() > 1;
        let marks: HashMap<_, _> = envs.iter().map(|env| (env.environment_id().clone(), instance_marks(env.config.as_deref()))).collect();
        let no_marks = HashMap::new();
        let mut next = HashMap::with_capacity(listed.len());
        let threads: Vec<Arc<SidebarThread>> = listed
            .iter()
            .map(|row| {
                let scoped = scoped_thread_key(&row.environment_id, row.id());
                let local_visit = ui_state.thread_last_visited_at_by_id.get(&scoped).copied();
                let has_draft = drafts.drafts_by_thread_key.get(&scoped).is_some_and(|draft| !draft.prompt.trim().is_empty());
                let woke_at = thread_woke_at(row, now);
                let key = (row.environment_id.clone(), row.id().clone());
                let provider = row_provider(row, marks.get(&row.environment_id).unwrap_or(&no_marks));
                let unread_completion = !unread.is_empty() && unread.contains(&thread_key(&row.environment_id, row.id()));
                let reused = self.mapped.remove(&key).filter(|mapped| {
                    mapped.thread.provider == provider &&
                    Arc::ptr_eq(&mapped.source, row)
                        && mapped.local_visit == local_visit
                        && mapped.woke_at == woke_at
                        && mapped.has_draft == has_draft
                        && mapped.remote == remote
                        && mapped.unread_completion == unread_completion
                });
                let mapped = reused.unwrap_or_else(|| MappedRow {
                    source: Arc::clone(row),
                    local_visit,
                    woke_at,
                    has_draft,
                    remote,
                    unread_completion,
                    thread: Arc::new(map_row(row, RowExtras { local_visit, woke_at, has_draft, remote, unread_completion, provider })),
                });
                let thread = Arc::clone(&mapped.thread);
                next.insert(key, mapped);
                thread
            })
            .collect();
        self.mapped = next;
        let (settings, grouping) = sidebar_settings(cx);
        let supported = envs.iter().any(|env| env.config_loaded());
        let snooze_presets: std::collections::HashMap<_, std::sync::Arc<[t3code_contracts::SnoozePresetSetting]>> = envs
            .iter()
            .filter_map(|env| {
                let presets = env.config.as_ref()?.settings.snooze_presets.clone()?;
                Some((model::EnvironmentId(env.environment_id().as_str().to_owned().into()), presets.into()))
            })
            .collect();
        let (projects, wanted) = {
        let all: Vec<(t3code_contracts::EnvironmentId, &OrchestrationProjectShell)> = envs
            .iter()
            .flat_map(|env| {
                let id = env.environment_id().clone();
                env.shell.snapshot.iter().flat_map(move |snapshot| snapshot.projects.iter().map({
                    let id = id.clone();
                    move |project| (id.clone(), &**project)
                }))
            })
            .collect();
        let keys: Vec<String> = all
            .iter()
            .map(|(id, project)| t3code_client_runtime::state::project_grouping::logical_project_key(id, project, &grouping))
            .collect();
        let mut members: HashMap<&str, Vec<&OrchestrationProjectShell>> = HashMap::new();
        for (key, (_, project)) in keys.iter().zip(&all) {
            members.entry(key.as_str()).or_default().push(*project);
        }
        let labels: HashMap<&str, String> = members
            .iter()
            .map(|(key, group)| {
                let representative = group.iter().max_by_key(|p| parse_ms(&p.updated_at).unwrap_or_default()).copied().unwrap_or(group[0]);
                (*key, t3code_client_runtime::state::project_grouping::group_label(key, representative, group))
            })
            .collect();
        let wanted: Vec<(t3code_contracts::EnvironmentId, String, Option<String>)> =
            all.iter().map(|(id, p)| (id.clone(), p.workspace_root.clone(), p.favicon_path.clone())).collect();
        let projects: Vec<SidebarProject> = all
            .iter()
            .zip(&keys)
            .map(|((id, project), key)| map_project(id, project, key, labels.get(key.as_str()).cloned(), None))
            .collect();
        (projects, wanted)
        };
        drop(envs);
        // Favicons need the app mutably; the lookups start after the shells are read.
        let projects = projects
            .into_iter()
            .zip(wanted)
            .map(|(project, (id, root, path))| SidebarProject { favicon: super::favicons::favicon(&id, &root, path.as_deref(), cx), ..project })
            .collect();
        self.snapshot = SidebarSnapshot {
            projects,
            threads,
            notifications: Vec::new(),
            settings: SidebarSettings {
                settlement_supported: supported,
                snooze_supported: supported,
                pinning_supported: supported,
                ..settings
            },
            active_thread: self.active_thread.clone(),
            environment_pill: None,
            now: TimestampMs(now),
            auto_settle_disabled: Default::default(),
            hover_details: Default::default(),
            // `AppRoot` gives the sidebar its drafts with `set_drafts`.
            drafts: Vec::new(),
            active_draft: None,
            search_content_matches: self.search_matches.clone(),
            snooze_presets: std::sync::Arc::new(snooze_presets),
        };
        self.revision += 1;
        self.rebuilds += 1;
        cx.notify();
    }

    /// Takes the server's message matches for a sidebar search.
    pub fn set_search_matches(&mut self, matches: Option<model::SearchContentMatches>, cx: &mut Context<Self>) {
        self.search_matches = matches;
        self.snapshot.search_content_matches = self.search_matches.clone();
        self.revision += 1;
        cx.notify();
    }

    /// The environment entities, for handlers.
    pub fn environments<'a>(&self, cx: &'a App) -> &'a [Entity<LiveEnvironment>] {
        &self.environments.read(cx).list
    }
}
