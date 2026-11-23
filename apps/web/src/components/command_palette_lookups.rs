//! What the command palette lists, behind [`CommandPaletteSource`]. The app
//! implements the trait over live state; [`FixtureSource`] fills it from a
//! sidebar snapshot for scenes and checks.

use std::sync::Arc;

use gpui_kit::{Hsla, SharedString};
use t3code_shared::palette_search::{PullRequestSubject, SearchText, ThreadOpenStats, ThreadSearchSubject};

use crate::components::sidebar::model::{EnvironmentId, ProjectRef, SidebarSnapshot, SidebarThread, ThreadKey};
use crate::theme_palette::{Appearance, ThemeDocument};
use crate::timestamp_format::format_relative_elapsed;

/// A thread the palette can open, with its search subject built once.
#[derive(Debug)]
pub struct PaletteThread {
    pub key: ThreadKey,
    pub project: Option<ProjectRef>,
    pub title: SharedString,
    pub branch: Option<SharedString>,
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
    pub workspace_root: SharedString,
    pub pinned: bool,
}

/// A connected environment, for Add project.
#[derive(Clone, Debug)]
pub struct PaletteEnvironment {
    pub id: EnvironmentId,
    pub label: SharedString,
    pub connected: bool,
}

/// A theme the Change theme page lists.
#[derive(Clone, Debug)]
pub struct PaletteTheme {
    /// `None` is the standard theme that follows the appearance.
    pub id: Option<SharedString>,
    pub label: SharedString,
    /// Preview colours (background, accent) per appearance the theme has.
    pub previews: Vec<(Appearance, [Hsla; 2])>,
    pub document: Arc<ThemeDocument>,
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
    fn themes(&self) -> Vec<PaletteTheme>;
    fn ports(&self) -> Vec<PalettePort>;
    fn settings(&self) -> Arc<[PaletteSetting]>;
    fn supports(&self, feature: PaletteFeature) -> bool;
    /// The clock relative times and search recency read.
    fn now_ms(&self) -> i64;
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
}

impl FixtureSource {
    pub fn new(snapshot: &SidebarSnapshot) -> Self {
        let projects: Arc<[PaletteProject]> = snapshot
            .projects
            .iter()
            .filter(|p| !p.archived)
            .map(|p| PaletteProject {
                project_ref: p.project_ref.clone(),
                title: p.title.clone(),
                workspace_root: p.workspace_root.clone(),
                pinned: p.pinned,
            })
            .collect();
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
            settings: fixture_settings(),
            ports: Vec::new(),
            now_ms: snapshot.now.0,
        }
    }

    pub fn with_ports(mut self, ports: Vec<PalettePort>) -> Self {
        self.ports = ports;
        self
    }
}

fn fixture_settings() -> Arc<[PaletteSetting]> {
    let setting = |id: &str, title: &str, section: &str, terms: &[&str]| PaletteSetting {
        id: id.to_owned().into(),
        title: title.to_owned().into(),
        section: section.to_owned().into(),
        terms: terms.iter().map(|t| SharedString::from(t.to_string())).collect(),
        secondary: false,
    };
    [
        setting("keybindings", "Keybindings", "Keybindings", &["shortcuts", "keys", "hotkeys"]),
        setting("theme", "Theme", "Appearance", &["colors", "dark", "light"]),
        setting("send-shortcut", "Send shortcut", "Chat", &["enter", "submit"]),
        setting("project-grouping", "Project grouping", "General", &["sidebar", "folders"]),
        setting("language", "Language", "General", &["locale", "translation"]),
    ]
    .into()
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

    fn themes(&self) -> Vec<PaletteTheme> {
        let graphite = Arc::new(ThemeDocument::graphite());
        let swatch = |appearance| {
            let palette = graphite.palette(appearance);
            (appearance, [palette.canvas, palette.accent])
        };
        vec![PaletteTheme {
            id: Some(graphite.id.clone().into()),
            label: graphite.name.clone().into(),
            previews: vec![swatch(Appearance::Light), swatch(Appearance::Dark)],
            document: graphite,
        }]
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

    fn now_ms(&self) -> i64 {
        self.now_ms
    }
}
