//! What the command palette lists, behind [`CommandPaletteSource`]. The app
//! implements the trait over live state; [`FixtureSource`] fills it from a
//! sidebar snapshot for scenes and checks.

use crate::i18n::t;
use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_shared::palette_search::{PullRequestSubject, SearchText, ThreadOpenStats, ThreadSearchSubject};

use crate::components::sidebar::model::{EnvironmentId, ProjectRef, SidebarProvider, SidebarSnapshot, SidebarThread, ThreadKey};
use crate::timestamp_format::format_relative_elapsed;

/// A thread the palette can open, with its search subject built once.
#[derive(Debug)]
pub struct PaletteThread {
    pub key: ThreadKey,
    pub project: Option<ProjectRef>,
    pub title: SharedString,
    pub branch: Option<SharedString>,
    /// The project's name, for the subtitle.
    pub project_title: Option<SharedString>,
    /// Where the thread runs: the environment's label, "Local" or "Remote".
    pub environment_label: SharedString,
    /// The branch is checked out in a worktree (folder-git icon, not folder).
    pub in_worktree: bool,
    /// The provider mark the subtitle ends with.
    pub provider: Option<SidebarProvider>,
    /// Epoch ms of the latest user activity, for the timestamp and recency.
    pub activity_at: i64,
    pub subject: ThreadSearchSubject,
}

impl PaletteThread {
    /// A thread from the sidebar model; `project_title` and `environment_label` feed search.
    pub fn from_sidebar(
        thread: &SidebarThread,
        project_title: Option<&str>,
        environment_label: Option<&str>,
        opens: Option<ThreadOpenStats>,
    ) -> Self {
        let activity_at = thread.latest_user_message_at.unwrap_or(thread.updated_at).0;
        let mut subject = ThreadSearchSubject::new(thread.title.as_ref()).with_branch(thread.branch.as_deref());
        subject.id = Some(thread.key.thread_id.0.as_ref().into());
        subject.project_title = project_title.map(SearchText::new);
        subject.environment_label = environment_label.map(SearchText::new);
        subject.activity_at = Some(activity_at);
        subject.opens = opens;
        if let Some(pr) = &thread.pull_request {
            subject.pull_requests.push(PullRequestSubject {
                number: pr.number.into(),
                number_text: SearchText::new(format!("#{}", pr.number)),
                title: None,
                references: Vec::new(),
            });
        }
        Self {
            key: thread.key.clone(),
            project: Some(ProjectRef { environment_id: thread.key.environment_id.clone(), project_id: thread.project_id.clone() }),
            title: thread.title.clone(),
            branch: thread.branch.clone(),
            project_title: project_title.map(|t| SharedString::from(t.to_owned())),
            environment_label: environment_label.map(|l| SharedString::from(l.to_owned())).unwrap_or_else(|| {
                if thread.remote_machine {
                    t!("Remote", context = "machine location")
                } else {
                    t!("Local", context = "machine location")
                }
            }),
            in_worktree: thread.in_worktree,
            provider: thread.provider.clone(),
            activity_at,
            subject,
        }
    }

    /// "3h ago" against `now_ms`.
    pub fn timestamp(&self, now_ms: i64) -> SharedString {
        format_relative_elapsed((now_ms - self.activity_at).max(0)).label
    }
}

/// A project the palette can start a thread in.
#[derive(Clone, Debug)]
pub struct PaletteProject {
    pub project_ref: ProjectRef,
    pub title: SharedString,
    /// The logical project (sidebar group) it belongs to, and that group's display name
    /// (`projectPickerEntries`, `group.displayName`).
    pub group_key: SharedString,
    pub group_label: SharedString,
    pub workspace_root: SharedString,
    pub pinned: bool,
}

/// The snapshot's unarchived projects, each with its sidebar group (`buildProjectGroups` with
/// the snapshot's grouping; per-project overrides are already folded into the project keys).
pub fn palette_projects(snapshot: &SidebarSnapshot) -> Arc<[PaletteProject]> {
    let groups = crate::components::sidebar::sidebar_project_grouping::build_project_groups(
        &snapshot.projects,
        snapshot.settings.grouping_mode,
        &Default::default(),
    );
    snapshot
        .projects
        .iter()
        .filter(|p| !p.archived)
        .map(|p| {
            let group = groups.iter().find(|g| g.member_refs.contains(&p.project_ref));
            PaletteProject {
                project_ref: p.project_ref.clone(),
                title: p.title.clone(),
                group_key: group.map_or_else(|| p.title.clone(), |g| g.key.clone()),
                group_label: group.map_or_else(|| p.title.clone(), |g| g.label.clone()),
                workspace_root: p.workspace_root.clone(),
                pinned: p.pinned,
            }
        })
        .collect()
}

/// A connected environment, for Add project.
#[derive(Clone, Debug)]
pub struct PaletteEnvironment {
    pub id: EnvironmentId,
    pub label: SharedString,
    pub connected: bool,
}

/// A port the user forwards, or one a server reports listening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PalettePort {
    pub environment_id: EnvironmentId,
    pub environment_label: SharedString,
    pub remote_port: u16,
    /// The local port, once forwarded.
    pub local_port: Option<u16>,
}

/// A setting the palette can jump to (`settingsSearch.ts` items).
#[derive(Clone, Debug)]
pub struct PaletteSetting {
    pub id: SharedString,
    pub title: SharedString,
    pub section: SharedString,
    pub terms: Vec<SharedString>,
    /// Listed after the other settings in its group.
    pub secondary: bool,
}

/// Features the palette offers only when the app supports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaletteFeature {
    Triage,
    PromptLinks,
    LinkPullRequest,
    GuestShare,
    RestartAgentSession,
    PortForwarding,
    NewProject,
    PullRequests,
    Issues,
    UseInAnotherApp,
    ProjectSettings,
    PreviewFind,
}

/// The data behind the command palette. Calls happen when the palette opens
/// and when it filters, so implementations return shared data.
pub trait CommandPaletteSource {
    /// Every thread the palette may list, archived ones left out.
    fn threads(&self) -> Arc<[Arc<PaletteThread>]>;
    fn projects(&self) -> Arc<[PaletteProject]>;
    /// The thread in the main pane.
    fn active_thread(&self) -> Option<ThreadKey>;
    /// The project new threads go to by default.
    fn current_project(&self) -> Option<ProjectRef>;
    fn environments(&self) -> Vec<PaletteEnvironment>;
    fn ports(&self) -> Vec<PalettePort>;
    fn settings(&self) -> Arc<[PaletteSetting]>;
    fn supports(&self, feature: PaletteFeature) -> bool;
    /// The active thread has linked pull requests or issues; without any,
    /// "Show linked items" is disabled. Defaults to `true`.
    fn active_thread_has_linked_items(&self) -> bool {
        true
    }
    /// The clock relative times and search recency read.
    fn now_ms(&self) -> i64;
    /// The server work of the Add project flows; without it their rows only tell the app.
    fn project_flows(&self) -> Option<std::rc::Rc<dyn super::command_palette_project_flow::ProjectFlowBackend>> {
        None
    }
}

/// Palette data from a sidebar snapshot, for scenes and checks.
pub struct FixtureSource {
    threads: Arc<[Arc<PaletteThread>]>,
    projects: Arc<[PaletteProject]>,
    active_thread: Option<ThreadKey>,
    environments: Vec<PaletteEnvironment>,
    settings: Arc<[PaletteSetting]>,
    ports: Vec<PalettePort>,
    now_ms: i64,
    /// Threads that link a pull request or an issue.
    linked: Arc<std::collections::HashSet<ThreadKey>>,
}

impl FixtureSource {
    /// Whether `thread` links a pull request or an issue ("Show linked items").
    pub fn has_linked_items(&self, thread: Option<&ThreadKey>) -> bool {
        thread.is_some_and(|key| self.linked.contains(key))
    }

    pub fn new(snapshot: &SidebarSnapshot) -> Self {
        let projects = palette_projects(snapshot);
        let threads = snapshot
            .threads
            .iter()
            .filter(|t| !t.archived)
            .map(|t| {
                let project = projects.iter().find(|p| p.project_ref.project_id == t.project_id);
                Arc::new(PaletteThread::from_sidebar(t, project.map(|p| p.title.as_ref()), None, None))
            })
            .collect();
        let environment = snapshot.threads.first().map(|t| t.key.environment_id.clone());
        Self {
            threads,
            projects,
            active_thread: snapshot.active_thread.clone(),
            environments: environment
                .into_iter()
                .map(|id| PaletteEnvironment { id, label: "devbox".into(), connected: true })
                .collect(),
            settings: crate::components::settings::settings_search::palette_settings(Default::default()),
            ports: Vec::new(),
            now_ms: snapshot.now.0,
            linked: Arc::new(
                snapshot.threads.iter().filter(|t| t.pull_request.is_some() || t.issue_key.is_some()).map(|t| t.key.clone()).collect(),
            ),
        }
    }

    pub fn with_ports(mut self, ports: Vec<PalettePort>) -> Self {
        self.ports = ports;
        self
    }
}

impl CommandPaletteSource for FixtureSource {
    fn threads(&self) -> Arc<[Arc<PaletteThread>]> {
        self.threads.clone()
    }

    fn projects(&self) -> Arc<[PaletteProject]> {
        self.projects.clone()
    }

    fn active_thread(&self) -> Option<ThreadKey> {
        self.active_thread.clone()
    }

    fn current_project(&self) -> Option<ProjectRef> {
        let active = self.active_thread.as_ref()?;
        self.threads.iter().find(|t| &t.key == active).and_then(|t| t.project.clone())
    }

    fn environments(&self) -> Vec<PaletteEnvironment> {
        self.environments.clone()
    }

    fn ports(&self) -> Vec<PalettePort> {
        self.ports.clone()
    }

    fn settings(&self) -> Arc<[PaletteSetting]> {
        self.settings.clone()
    }

    fn supports(&self, feature: PaletteFeature) -> bool {
        matches!(feature, PaletteFeature::PortForwarding | PaletteFeature::NewProject | PaletteFeature::ProjectSettings)
    }

    fn active_thread_has_linked_items(&self) -> bool {
        self.has_linked_items(self.active_thread.as_ref())
    }

    fn now_ms(&self) -> i64 {
        self.now_ms
    }

    fn project_flows(&self) -> Option<std::rc::Rc<dyn super::command_palette_project_flow::ProjectFlowBackend>> {
        let environments = self
            .environments
            .iter()
            .map(|e| super::command_palette_project_flow::FlowEnvironment {
                id: e.id.clone(),
                label: e.label.clone(),
                windows: false,
                base_directory: SharedString::default(),
                new_projects_root: Some("/home/dev/.t3/projects".into()),
                ghq_clone_layout: false,
                remote_sources: Vec::new(),
                publish_targets: Vec::new(),
            })
            .collect();
        Some(std::rc::Rc::new(super::command_palette_project_flow::FixtureFlows::sample(environments)))
    }
}
