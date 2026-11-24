//! Settings pages, their nav labels, icons and groups, and the search over
//! the full catalog. Ports `settingsSearch.ts`; the catalog itself is
//! [`super::settings_search_catalog`].

use gpui_kit::SharedString;
use unicode_normalization::UnicodeNormalization as _;

use super::settings_search_catalog::{SETTINGS_SEARCH_ITEMS, flag, item_terms, item_title, item_title_source};
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
    /// Reached from Maintenance; not in the nav.
    Diagnostics,
    /// Behind a feature flag on the web; not in the nav.
    Secrets,
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
            Self::Diagnostics => "diagnostics",
            Self::Secrets => "secrets",
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
            Self::Diagnostics => t!("Diagnostics"),
            Self::Secrets => t!("Secrets"),
        }
    }

    /// The English section label, which search also matches.
    pub fn section_label_source(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Chat => "Chat",
            Self::Diffs => "Diffs & pull requests",
            Self::Keybindings => "Keybindings",
            Self::Voice => "Voice",
            Self::SnapShot => "SnapShots",
            Self::Providers => "Providers",
            Self::Threads => "Threads",
            Self::Browser => "Browser",
            Self::Devices => "Devices",
            Self::ScheduledTasks => "Scheduled tasks",
            Self::Projects => "Projects",
            Self::SourceControl => "Source control & issues",
            Self::Worktrees => "Worktrees",
            Self::Connections => "Environments",
            Self::Access => "Sharing & access",
            Self::Maintenance => "Maintenance",
            Self::Labs => "Labs",
            Self::About => "About",
            Self::Archived => "Archive",
            Self::Diagnostics => "Diagnostics",
            Self::Secrets => "Secrets",
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
            Self::SnapShot => IconName::SnapShot,
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
            Self::Diagnostics => IconName::Wrench,
            Self::Secrets => IconName::KeyRound,
        }
    }

    /// Whether this port has the page. The others show a placeholder.
    pub fn is_built(self) -> bool {
        matches!(
            self,
            Self::Connections
                | Self::General
                | Self::Appearance
                | Self::Chat
                | Self::Diffs
                | Self::Keybindings
                | Self::Voice
                | Self::SnapShot
                | Self::Labs
                | Self::Archived
                | Self::About
        )
    }

    /// `SETTINGS_DEVICE_ONLY_PATHS`: pages saved on this device, which have no scope picker.
    pub fn is_device_only(self) -> bool {
        matches!(
            self,
            Self::General
                | Self::Appearance
                | Self::Chat
                | Self::Diffs
                | Self::SnapShot
                | Self::Connections
                | Self::Access
                | Self::About
        )
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

/// `SettingsSearchScope`: where a setting is stored when not on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    Environment,
    EnvironmentDefaults,
    ProjectDefaults,
    Project,
    Checkout,
    Connections,
}

/// A search hit: a setting's title, the page it lives on, and the row to reveal.
#[derive(Clone, Debug)]
pub struct SearchItem {
    pub id: &'static str,
    pub title: SharedString,
    pub path: SettingsPath,
    /// The row to scroll to, when it is not `id`.
    pub target_id: Option<&'static str>,
}

/// `SettingsSearchAvailability` for this app: a desktop client with no
/// local environment. `has_environment` and `has_cloud` follow the user's setup.
#[derive(Clone, Copy, Debug)]
pub struct SearchAvailability {
    pub has_environment: bool,
    pub has_cloud: bool,
}

impl Default for SearchAvailability {
    fn default() -> Self {
        Self { has_environment: true, has_cloud: true }
    }
}

/// `settingsForThisPlatform` plus `filterAvailableSettingsSearchItems`.
fn is_available(item: &super::settings_search_catalog::CatalogItem, availability: SearchAvailability) -> bool {
    let platform_ok = (!item.has(flag::MAC_ONLY) || cfg!(target_os = "macos"))
        && (!item.has(flag::WINDOWS_ONLY) || cfg!(target_os = "windows"))
        && !item.has(flag::SECRETS_FEATURE_ONLY);
    // No local server: local backend, local environment, WSL and the
    // primary environment never exist in this app.
    let unavailable = flag::LOCAL_BACKEND_MANAGEMENT_ONLY
        | flag::LOCAL_ENVIRONMENT_ONLY
        | flag::PRIMARY_ENVIRONMENT_ONLY
        | flag::WSL_AVAILABLE_ONLY
        | flag::MANAGED_TUNNEL_ONLY;
    let environment = flag::ENVIRONMENT_ONLY
        | flag::PROVIDER_SETTINGS_ONLY
        | flag::PEER_LINKS_ONLY
        | flag::FAST_WORKTREE_CHECKOUT_ONLY
        | flag::TEXT_GENERATORS_ONLY
        | flag::REQUIRES_THREAD_AUTO_SETTLEMENT;
    platform_ok
        && item.flags & unavailable == 0
        && (item.flags & environment == 0 || availability.has_environment)
        && (!item.has(flag::CLOUD_ONLY) || availability.has_cloud)
}

/// `normalizeSearchText`: NFKD, marks removed, lowercase, single spaces.
pub fn normalize_search_text(value: &str) -> String {
    let folded: String = value
        .nfkd()
        .filter(|c| !unicode_normalization::char::is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `searchSettings` over the whole catalog with this app's availability.
pub fn search_settings(query: &str) -> Vec<SearchItem> {
    search_settings_with(query, SearchAvailability::default())
}

/// `searchSettings`: every query word must appear in the title, the page
/// label, or the search terms, in English or the active language. Primary
/// items before secondary ones (keybindings), then by rank, then catalog order.
pub fn search_settings_with(query: &str, availability: SearchAvailability) -> Vec<SearchItem> {
    let query = normalize_search_text(query);
    if query.is_empty() {
        return Vec::new();
    }
    let tokens: Vec<&str> = query.split(' ').collect();
    let mut hits: Vec<(bool, u8, usize)> = SETTINGS_SEARCH_ITEMS
        .iter()
        .enumerate()
        .filter(|(_, item)| is_available(item, availability))
        .filter_map(|(index, item)| {
            let title = normalize_search_text(item_title_source(index));
            let mut fields = vec![
                title.clone(),
                normalize_search_text(&item.path.section_label_source()),
                normalize_search_text(&item_title(index)),
                normalize_search_text(&item.path.section_label()),
            ];
            fields.extend(item_terms(index).iter().map(|term| normalize_search_text(term)));
            if !tokens.iter().all(|token| fields.iter().any(|field| field.contains(token))) {
                return None;
            }
            let rank = if title == query {
                5
            } else if title.starts_with(&query) {
                4
            } else if title.contains(&query) {
                3
            } else if tokens.iter().all(|token| title.contains(token)) {
                2
            } else if fields.iter().any(|field| field.contains(&query)) {
                1
            } else {
                0
            };
            Some((item.has(flag::SECONDARY), rank, index))
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    hits.into_iter()
        .map(|(_, _, index)| {
            let item = &SETTINGS_SEARCH_ITEMS[index];
            SearchItem { id: item.id, title: item_title(index), path: item.path, target_id: item.target_id }
        })
        .collect()
}

/// A catalog row's title in the active language, by its id. Panels title
/// their rows with this so the row and its search hit always agree.
pub fn setting_title(id: &str) -> SharedString {
    SETTINGS_SEARCH_ITEMS
        .iter()
        .position(|item| item.id == id)
        .map(item_title)
        .unwrap_or_else(|| SharedString::from(id.to_owned()))
}

/// Every available catalog item as the command palette lists it
/// (`CommandPalette.tsx` settings results), for a `CommandPaletteSource`.
pub fn palette_settings(availability: SearchAvailability) -> std::sync::Arc<[crate::components::command_palette_lookups::PaletteSetting]> {
    SETTINGS_SEARCH_ITEMS
        .iter()
        .enumerate()
        .filter(|(_, item)| is_available(item, availability))
        .map(|(index, item)| crate::components::command_palette_lookups::PaletteSetting {
            id: SharedString::new_static(item.id),
            title: item_title(index),
            section: item.path.section_label(),
            terms: item_terms(index),
            secondary: item.has(flag::SECONDARY),
        })
        .collect()
}

/// The page a catalog item lives on, for opening a palette result.
pub fn setting_path(id: &str) -> Option<SettingsPath> {
    SETTINGS_SEARCH_ITEMS.iter().find(|item| item.id == id).map(|item| item.path)
}
