//! Typed routes and the in-app history. Ports the web app's TanStack route
//! tree (`routes/*`, `router.ts`) as one enum: each variant is a page, and
//! [`Route::parse`] / [`Route::path`] round-trip the web paths so deep links
//! (`t3code://threads/env/thread`) and the persisted last route keep working.
//! Routes the desktop target cannot reach (`/pair`, `/guest`, `/connect`,
//! `/approve-agent`, `/connect-agent`) parse to [`Route::Index`].

use gpui_kit::{Context, EventEmitter, SharedString};

use crate::components::settings::settings_search::SettingsPath;
use crate::components::sidebar::model::{EnvironmentId, ThreadId, ThreadKey};
use crate::navigation_history::{HistoryDirection, NavigationHistory};

/// A draft thread's local id (`DraftId`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DraftId(pub SharedString);

/// The settings panels (`/settings/<panel>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPanel {
    General,
    Appearance,
    Chat,
    Diffs,
    Keybindings,
    Providers,
    Threads,
    Browser,
    Devices,
    ScheduledTasks,
    SourceControl,
    Worktrees,
    Connections,
    Access,
    Secrets,
    Maintenance,
    Labs,
    About,
    Archived,
    Diagnostics,
    Voice,
    SnapShot,
    Projects,
    OpenSourceLicenses,
}

impl SettingsPanel {
    const ALL: [(SettingsPanel, &'static str); 24] = [
        (Self::General, "general"),
        (Self::Appearance, "appearance"),
        (Self::Chat, "chat"),
        (Self::Diffs, "diffs"),
        (Self::Keybindings, "keybindings"),
        (Self::Providers, "providers"),
        (Self::Threads, "threads"),
        (Self::Browser, "browser"),
        (Self::Devices, "devices"),
        (Self::ScheduledTasks, "scheduled-tasks"),
        (Self::SourceControl, "source-control"),
        (Self::Worktrees, "worktrees"),
        (Self::Connections, "connections"),
        (Self::Access, "access"),
        (Self::Secrets, "secrets"),
        (Self::Maintenance, "maintenance"),
        (Self::Labs, "labs"),
        (Self::About, "about"),
        (Self::Archived, "archived"),
        (Self::Diagnostics, "diagnostics"),
        (Self::Voice, "voice"),
        (Self::SnapShot, "snap-shot"),
        (Self::Projects, "projects"),
        (Self::OpenSourceLicenses, "open-source-licenses"),
    ];

    /// The path segment, e.g. `scheduled-tasks`.
    pub fn slug(self) -> &'static str {
        Self::ALL.iter().find(|(panel, _)| *panel == self).map(|(_, slug)| *slug).unwrap_or("general")
    }

    /// A segment, including the renamed ones (`settingsSearch.ts` redirect table).
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "integrations" => Some(Self::Browser),
            "storage" => Some(Self::Maintenance),
            "beta" => Some(Self::Labs),
            _ => Self::ALL.iter().find(|(_, s)| *s == slug).map(|(panel, _)| *panel),
        }
    }
}

impl From<SettingsPath> for SettingsPanel {
    fn from(path: SettingsPath) -> Self {
        Self::from_slug(path.slug()).unwrap_or(Self::General)
    }
}

impl SettingsPanel {
    /// The settings page that draws this panel. Panels the settings shell
    /// has no page for open the page that holds them.
    pub fn settings_path(self) -> SettingsPath {
        match self {
            Self::General => SettingsPath::General,
            Self::Appearance => SettingsPath::Appearance,
            Self::Chat => SettingsPath::Chat,
            Self::Diffs => SettingsPath::Diffs,
            Self::Keybindings => SettingsPath::Keybindings,
            Self::Providers => SettingsPath::Providers,
            Self::Threads => SettingsPath::Threads,
            Self::Browser => SettingsPath::Browser,
            Self::Devices => SettingsPath::Devices,
            Self::ScheduledTasks => SettingsPath::ScheduledTasks,
            Self::SourceControl => SettingsPath::SourceControl,
            Self::Worktrees => SettingsPath::Worktrees,
            Self::Connections | Self::Secrets => SettingsPath::Connections,
            Self::Access => SettingsPath::Access,
            Self::Maintenance | Self::Diagnostics => SettingsPath::Maintenance,
            Self::Labs => SettingsPath::Labs,
            Self::About | Self::OpenSourceLicenses => SettingsPath::About,
            Self::Archived => SettingsPath::Archived,
            Self::Voice => SettingsPath::Voice,
            Self::SnapShot => SettingsPath::SnapShot,
            Self::Projects => SettingsPath::Projects,
        }
    }
}

/// One page of the app. Search strings are kept raw, as the pages that own
/// them parse them (`/new` reads its query unparsed on the web too).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// `/`: starts a draft in the most recent project, or the empty state.
    Index,
    /// `/$environmentId/$threadId`.
    Thread(ThreadKey),
    /// `/draft/$draftId`.
    Draft(DraftId),
    /// `/new?prompt&repo&provider&model&pr`.
    New { query: SharedString },
    /// `/pull-requests?…`.
    PullRequests { query: SharedString },
    /// `/issues?…`.
    Issues { query: SharedString },
    /// `/usage`.
    Usage,
    /// `/settings/<panel>?project&machine&checkout`.
    Settings { panel: SettingsPanel, query: SharedString },
    /// `/welcome`.
    Welcome,
}

impl Route {
    /// Parses a web path with optional query (`/a/b?c`). Aliases redirect the
    /// way the web routes do; unknown and dropped paths become [`Route::Index`].
    pub fn parse(path: &str) -> Self {
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let query = SharedString::from(query.to_string());
        let segments: Vec<String> = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect();
        let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
        match segments.as_slice() {
            [] => Self::Index,
            ["draft", id] => Self::Draft(DraftId(id.to_string().into())),
            ["new"] => Self::New { query },
            ["pull-requests"] => Self::PullRequests { query },
            ["issues"] => Self::Issues { query },
            ["usage"] => Self::Usage,
            ["welcome"] => Self::Welcome,
            ["settings"] => Self::Settings { panel: SettingsPanel::General, query },
            ["settings", panel] => match SettingsPanel::from_slug(panel) {
                Some(panel) => Self::Settings { panel, query },
                None => Self::Settings { panel: SettingsPanel::General, query },
            },
            // `projects.$projectKey.tsx`: redirect to the projects panel.
            ["projects", key] => Self::Settings {
                panel: SettingsPanel::Projects,
                query: format!("project={}", percent_encode(key)).into(),
            },
            ["threads", env, thread] => Self::thread(env, thread),
            ["pair" | "guest" | "connect" | "approve-agent" | "connect-agent", ..] => Self::Index,
            [env, thread] => Self::thread(env, thread),
            _ => Self::Index,
        }
    }

    fn thread(env: &str, thread: &str) -> Self {
        Self::Thread(ThreadKey {
            environment_id: EnvironmentId(env.to_string().into()),
            thread_id: ThreadId(thread.to_string().into()),
        })
    }

    /// The web path for this route, with its query.
    pub fn path(&self) -> String {
        let with_query = |base: &str, query: &SharedString| {
            if query.is_empty() { base.to_string() } else { format!("{base}?{query}") }
        };
        match self {
            Self::Index => "/".into(),
            Self::Thread(key) => format!(
                "/{}/{}",
                percent_encode(&key.environment_id.0),
                percent_encode(&key.thread_id.0)
            ),
            Self::Draft(id) => format!("/draft/{}", percent_encode(&id.0)),
            Self::New { query } => with_query("/new", query),
            Self::PullRequests { query } => with_query("/pull-requests", query),
            Self::Issues { query } => with_query("/issues", query),
            Self::Usage => "/usage".into(),
            Self::Settings { panel, query } => with_query(&format!("/settings/{}", panel.slug()), query),
            Self::Welcome => "/welcome".into(),
        }
    }

    /// Settings on `path`.
    pub fn settings(path: SettingsPath) -> Self {
        Self::Settings { panel: path.into(), query: SharedString::default() }
    }

    /// Settings opens as a modal over the last thread (`isAppModalPage`).
    pub fn is_modal(&self) -> bool {
        matches!(self, Self::Settings { .. })
    }

    /// The thread this route shows in the chat view, if any.
    pub fn thread_target(&self) -> Option<ThreadTarget> {
        match self {
            Self::Thread(key) => Some(ThreadTarget::Server(key.clone())),
            Self::Draft(id) => Some(ThreadTarget::Draft(id.clone())),
            _ => None,
        }
    }
}

/// What a thread route points at (`ThreadRouteTarget`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ThreadTarget {
    Server(ThreadKey),
    Draft(DraftId),
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Emitted after every navigation.
#[derive(Clone, Debug)]
pub struct RouteChanged {
    pub route: Route,
}

/// The current route and the in-app history. The sidebar, links, commands,
/// the palette, the inbox and the app menu all navigate through it, and it
/// handles `navigation.back` / `navigation.forward`.
pub struct Router {
    history: NavigationHistory<Route>,
}

/// Most history entries kept.
const HISTORY_LIMIT: usize = 200;

impl EventEmitter<RouteChanged> for Router {}

impl Router {
    pub fn new(initial: Route) -> Self {
        Self { history: NavigationHistory::new(initial, HISTORY_LIMIT) }
    }

    pub fn current(&self) -> &Route {
        self.history.current()
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_navigate(HistoryDirection::Back)
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_navigate(HistoryDirection::Forward)
    }

    /// Pushes `route` (no-op when it is already current).
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == *self.current() {
            return;
        }
        self.history.push(route);
        self.changed(cx);
    }

    /// Replaces the current entry, as `navigate({ replace: true })` does.
    pub fn replace(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == *self.current() {
            return;
        }
        self.history.replace(route);
        self.changed(cx);
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.history.navigate(HistoryDirection::Back).is_some() {
            self.changed(cx);
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if self.history.navigate(HistoryDirection::Forward).is_some() {
            self.changed(cx);
        }
    }

    /// Leaves a modal route (settings): back to the page under it when that
    /// is the previous entry, else replaces it with [`Route::Index`].
    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        if !self.current().is_modal() {
            return;
        }
        if self.history.previous().is_some_and(|route| !route.is_modal()) {
            self.back(cx);
        } else {
            self.replace(Route::Index, cx);
        }
    }

    /// Drops history entries for which `keep` is false (deleted threads).
    pub fn retain_history(&mut self, keep: impl Fn(&Route) -> bool) {
        self.history.retain(keep);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(RouteChanged { route: self.current().clone() });
        cx.notify();
    }
}

/// Parses a `t3code://` deep link into a route (`t3code://threads/env/thread`).
pub fn route_from_deep_link(url: &str) -> Option<Route> {
    let rest = url.strip_prefix("t3code://")?;
    Some(Route::parse(&format!("/{rest}")))
}
