//! Settings pages, their nav labels, icons and groups, and the search over
//! them. Ports the parts of `settingsSearch.ts` the shell needs. Only the
//! Environments page is built here; the rest are listed and marked as not
//! built, for the agents porting those panels.

use gpui_kit::SharedString;

use crate::components::icons::IconName;
use crate::i18n::t;

/// A settings page, by its web route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPath {
    General,
    Appearance,
    Chat,
    Diffs,
    Keybindings,
    Voice,
    SnapShot,
    Providers,
    Threads,
    Browser,
    Devices,
    ScheduledTasks,
    Projects,
    SourceControl,
    Worktrees,
    Connections,
    Access,
    Maintenance,
    Labs,
    About,
    Archived,
}

impl SettingsPath {
    /// The web route, `/settings/<slug>`.
    pub fn slug(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Chat => "chat",
            Self::Diffs => "diffs",
            Self::Keybindings => "keybindings",
            Self::Voice => "voice",
            Self::SnapShot => "snap-shot",
            Self::Providers => "providers",
            Self::Threads => "threads",
            Self::Browser => "browser",
            Self::Devices => "devices",
            Self::ScheduledTasks => "scheduled-tasks",
            Self::Projects => "projects",
            Self::SourceControl => "source-control",
            Self::Worktrees => "worktrees",
            Self::Connections => "connections",
            Self::Access => "access",
            Self::Maintenance => "maintenance",
            Self::Labs => "labs",
            Self::About => "about",
            Self::Archived => "archived",
        }
    }

    /// `SETTINGS_SECTION_LABELS`: the breadcrumb and search-result label.
    pub fn section_label(self) -> SharedString {
        match self {
            Self::General => t!("General"),
            Self::Appearance => t!("Appearance"),
            Self::Chat => t!("Chat"),
            Self::Diffs => t!("Diffs & pull requests"),
            Self::Keybindings => t!("Keybindings"),
            Self::Voice => t!("Voice"),
            Self::SnapShot => t!("SnapShots"),
            Self::Providers => t!("Providers"),
            Self::Threads => t!("Threads"),
            Self::Browser => t!("Browser"),
            Self::Devices => t!("Devices"),
            Self::ScheduledTasks => t!("Scheduled tasks"),
            Self::Projects => t!("Projects"),
            Self::SourceControl => t!("Source control & issues"),
            Self::Worktrees => t!("Worktrees"),
            Self::Connections => t!("Environments"),
            Self::Access => t!("Sharing & access"),
            Self::Maintenance => t!("Maintenance"),
            Self::Labs => t!("Labs"),
            Self::About => t!("About"),
            Self::Archived => t!("Archive"),
        }
    }

    /// `SETTINGS_NAV_LABELS`: two pages read differently in the nav.
    pub fn nav_label(self) -> SharedString {
        match self {
            Self::SourceControl => t!("Source control & issues", context = "settings navigation item"),
            Self::Access => t!("Sharing & access", context = "settings navigation item"),
            other => other.section_label(),
        }
    }

    /// `SETTINGS_SECTION_ICONS`. SnapShots uses a custom glyph on the web;
    /// the nearest Lucide icon stands in.
    pub fn icon(self) -> IconName {
        match self {
            Self::General => IconName::Settings2,
            Self::Appearance => IconName::Palette,
            Self::Chat => IconName::MessageSquareText,
            Self::Diffs => IconName::FileDiff,
            Self::Keybindings => IconName::Keyboard,
            Self::Voice => IconName::Mic,
            Self::SnapShot => IconName::Monitor,
            Self::Providers => IconName::Bot,
            Self::Threads => IconName::MessagesSquare,
            Self::Browser => IconName::Globe,
            Self::Devices => IconName::Smartphone,
            Self::ScheduledTasks => IconName::CalendarClock,
            Self::Projects => IconName::PanelsTopLeft,
            Self::SourceControl => IconName::GitBranch,
            Self::Worktrees => IconName::FolderTree,
            Self::Connections => IconName::Server,
            Self::Access => IconName::ShieldCheck,
            Self::Maintenance => IconName::Wrench,
            Self::Labs => IconName::FlaskConical,
            Self::About => IconName::Info,
            Self::Archived => IconName::Archive,
        }
    }

    /// Whether this port has the page. The others show a placeholder.
    pub fn is_built(self) -> bool {
        self == Self::Connections
    }
}

/// One nav group: a heading (hidden for the first and last) and its pages.
pub struct NavGroup {
    pub heading: Option<SharedString>,
    pub paths: &'static [SettingsPath],
}

/// `SETTINGS_NAV_GROUPS`. Secrets is behind a feature flag on the web and is
/// left out, as it is there by default.
pub fn nav_groups() -> [NavGroup; 5] {
    use SettingsPath::*;
    [
        NavGroup { heading: None, paths: &[General, Appearance, Chat, Diffs, Keybindings, Voice, SnapShot] },
        NavGroup { heading: Some(t!("Agents")), paths: &[Providers, Threads, Browser, Devices, ScheduledTasks] },
        NavGroup { heading: Some(t!("Code")), paths: &[Projects, SourceControl, Worktrees] },
        NavGroup { heading: Some(t!("Connections")), paths: &[Connections, Access] },
        NavGroup { heading: None, paths: &[Maintenance, Labs, About, Archived] },
    ]
}

/// A search hit: a setting's title and the page it lives on.
#[derive(Clone, Debug)]
pub struct SearchItem {
    pub id: &'static str,
    pub title: SharedString,
    pub path: SettingsPath,
}

/// What search covers: every page, plus the Environments page's settings.
/// The web catalog has 167 entries; the other pages' entries arrive with them.
fn search_items() -> Vec<SearchItem> {
    let mut items: Vec<SearchItem> = nav_groups()
        .iter()
        .flat_map(|group| group.paths.iter())
        .map(|path| SearchItem { id: path.slug(), title: path.nav_label(), path: *path })
        .collect();
    items.extend([
        SearchItem { id: "remote-environments", title: t!("Environments"), path: SettingsPath::Connections },
        SearchItem { id: "t3-connect", title: t!("T3 Connect"), path: SettingsPath::Connections },
    ]);
    items
}

/// `searchSettings`: case-insensitive substring match on titles and page
/// labels, title matches first.
pub fn search_settings(query: &str) -> Vec<SearchItem> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, SearchItem)> = search_items()
        .into_iter()
        .filter_map(|item| {
            let title = item.title.to_lowercase();
            let section = item.path.section_label().to_lowercase();
            let rank = if title.starts_with(&query) {
                0
            } else if title.contains(&query) {
                1
            } else if section.contains(&query) {
                2
            } else {
                return None;
            };
            Some((rank, item))
        })
        .collect();
    hits.sort_by_key(|(rank, _)| *rank);
    let mut seen = Vec::new();
    hits.into_iter()
        .map(|(_, item)| item)
        .filter(|item| {
            let key = (item.title.clone(), item.path);
            let fresh = !seen.contains(&key);
            seen.push(key);
            fresh
        })
        .collect()
}
