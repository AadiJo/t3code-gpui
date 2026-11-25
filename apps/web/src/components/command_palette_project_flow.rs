//! The Add project flows inside the palette (`CommandPalette.tsx` browse, clone and new-project
//! modes, `CommandPaletteAddProject.tsx` summaries, `state/projects.ts` path rules). The query is
//! a folder path, a clone URL or a project name, and the input carries a submit button.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, SharedString, Task};

use super::icons::IconName;
use super::sidebar::model::{EnvironmentId, ProjectRef};
use crate::i18n::t;

/// What a project flow needs to know about an environment.
#[derive(Clone, Debug)]
pub struct FlowEnvironment {
    pub id: EnvironmentId,
    pub label: SharedString,
    /// The host runs Windows, so `C:\` paths browse.
    pub windows: bool,
    /// The server's `addProjectBaseDirectory` setting.
    pub base_directory: SharedString,
    /// Where `projects.createNew` puts new projects; `None` when the server cannot.
    pub new_projects_root: Option<SharedString>,
    /// Clones go to `<base>/<host>/<owner>/<repo>` (`ghqCloneLayout`).
    pub ghq_clone_layout: bool,
}

/// One folder of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseEntry {
    pub name: SharedString,
    pub full_path: SharedString,
}

/// A `filesystem.browse` result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseListing {
    pub parent_path: SharedString,
    pub entries: Arc<[BrowseEntry]>,
}

/// Where a finished flow lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowOpened {
    /// A project that was already there: its latest thread opens, else a draft.
    Existing(ProjectRef),
    /// A new project: a draft opens in it.
    Created(ProjectRef),
}

/// The server work of the Add project flows. Failures toast inside the implementation and
/// resolve to `None`.
pub trait ProjectFlowBackend {
    fn environment(&self, id: &EnvironmentId, cx: &App) -> Option<FlowEnvironment>;
    /// `filesystem.browse` of `partial_path` (a directory ending in a separator).
    fn browse(&self, id: &EnvironmentId, partial_path: String, cx: &mut App) -> Task<Result<BrowseListing, SharedString>>;
    /// `handleAddProjectForEnvironment`: opens the project at `path` if one exists there (and
    /// unarchives it), else `project.create` with the folder created when missing.
    fn add_project(&self, id: &EnvironmentId, path: String, cx: &mut App) -> Task<Option<FlowOpened>>;
    /// `useNewProject`: `projects.createNew`, its toasts, and the project once the shell lists it.
    fn create_new_project(&self, id: &EnvironmentId, name: String, cx: &mut App) -> Task<Option<ProjectRef>>;
    /// `projectClone.start` into `destination`; progress shows in the clone toasts. Resolves
    /// once the server accepted the clone, with the project and a task that resolves once the
    /// shell lists it (up to 3 s), so the palette can close first.
    fn start_clone(&self, id: &EnvironmentId, remote_url: String, destination: String, cx: &mut App) -> Task<Option<(ProjectRef, Task<()>)>>;
}

/// The flow a pushed Add project page runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectFlow {
    /// Local folder: the query is a path on the environment.
    Browse { env: EnvironmentId },
    /// Git URL, first step: the query is the clone URL.
    CloneRepository { env: EnvironmentId },
    /// Git URL, second step: the query is the destination path.
    CloneDestination { env: EnvironmentId, remote_url: SharedString, input: SharedString, pinned: SharedString },
    /// New project: the query is the name. `sources` is the environment whose sources page
    /// opened it, for "Add existing project".
    NewProject { env: EnvironmentId, sources: Option<EnvironmentId> },
}

impl ProjectFlow {
    pub fn environment(&self) -> &EnvironmentId {
        match self {
            Self::Browse { env } | Self::CloneRepository { env } | Self::CloneDestination { env, .. } | Self::NewProject { env, .. } => env,
        }
    }

    /// Whether the query is read as a path on this page.
    pub fn browses(&self) -> bool {
        matches!(self, Self::Browse { .. } | Self::CloneDestination { .. })
    }

    /// The page's addon icon.
    pub fn addon_icon(&self) -> IconName {
        match self {
            Self::Browse { .. } => IconName::FolderPlus,
            Self::CloneRepository { .. } | Self::CloneDestination { .. } => IconName::Link,
            Self::NewProject { .. } => IconName::FolderGit2,
        }
    }

    /// `remoteProjectInputPlaceholder`, then the new-project and browse placeholders.
    pub fn placeholder(&self) -> SharedString {
        match self {
            Self::CloneRepository { .. } => t!("Enter Git clone URL"),
            Self::NewProject { .. } => t!("Project name"),
            Self::Browse { .. } | Self::CloneDestination { .. } => t!("Enter path (e.g. ~/projects/my-app)"),
        }
    }
}

// Path rules from `state/projects.ts`.

fn is_windows_drive(value: &str) -> bool {
    let b = value.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn is_unix_absolute(value: &str) -> bool {
    value.starts_with('/')
}

/// `isFilesystemBrowseQuery`.
pub fn is_browse_query(value: &str, windows: bool) -> bool {
    ["./", "../", ".\\", "..\\", "/", "~/"].iter().any(|p| value.starts_with(p)) || (windows && (is_windows_drive(value) || value.starts_with("\\\\")))
}

fn last_separator(value: &str) -> Option<usize> {
    if is_unix_absolute(value) { value.rfind('/') } else { value.rfind(['/', '\\']) }
}

/// `hasTrailingPathSeparator`.
pub fn has_trailing_separator(value: &str) -> bool {
    if is_unix_absolute(value) { value.ends_with('/') } else { value.ends_with(['/', '\\']) }
}

fn preferred_separator(value: &str) -> char {
    if is_windows_drive(value) || value.starts_with("\\\\") {
        '\\'
    } else if is_unix_absolute(value) {
        '/'
    } else if value.contains('\\') {
        '\\'
    } else {
        '/'
    }
}

/// `getBrowseDirectoryPath`: the query up to and including its last separator.
pub fn directory_path(value: &str) -> &str {
    if has_trailing_separator(value) {
        return value;
    }
    last_separator(value).map_or(value, |i| &value[..=i])
}

/// `getBrowseLeafPathSegment`: what follows the last separator.
pub fn leaf_segment(value: &str) -> &str {
    last_separator(value).map_or(value, |i| &value[i + 1..])
}

/// `appendBrowsePathSegment`.
pub fn append_segment(value: &str, segment: &str) -> String {
    format!("{}{}{}", directory_path(value), segment, preferred_separator(value))
}

/// `getBrowseParentPath` for a directory path.
pub fn parent_path(directory: &str) -> Option<String> {
    let trimmed = directory.trim();
    let separator = preferred_separator(trimmed);
    let body = trimmed.trim_end_matches(['/', '\\']);
    if is_unix_absolute(trimmed) {
        let segments: Vec<&str> = body.split('/').filter(|s| !s.is_empty()).collect();
        return match segments.len() {
            0 => None,
            1 => Some("/".into()),
            n => Some(format!("/{}/", segments[..n - 1].join("/"))),
        };
    }
    if is_windows_drive(trimmed) {
        let root = &trimmed[..3];
        let segments: Vec<&str> = body[2..].split(['/', '\\']).filter(|s| !s.is_empty()).collect();
        return match segments.len() {
            0 => None,
            1 => Some(root.to_string()),
            n => Some(format!("{root}{}\\", segments[..n - 1].join("\\"))),
        };
    }
    let index = last_separator(body)?;
    Some(format!("{}{}", &body[..index], separator).replace(&format!("{separator}{separator}"), &separator.to_string()))
}

/// `canNavigateUp`.
pub fn can_browse_up(directory: &str) -> bool {
    has_trailing_separator(directory) && parent_path(directory).is_some()
}

/// `ensureBrowseDirectoryPath`.
pub fn ensure_directory(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || has_trailing_separator(trimmed) {
        trimmed.to_string()
    } else {
        format!("{trimmed}{}", preferred_separator(trimmed))
    }
}

/// `getAddProjectInitialQuery`: the folder Local folder opens on.
pub fn initial_query(base_directory: &str) -> String {
    let trimmed = base_directory.trim();
    if trimmed.is_empty() {
        return "~/".into();
    }
    let anchored = trimmed == "~" || is_unix_absolute(trimmed) || is_windows_drive(trimmed) || trimmed.starts_with("~/") || trimmed.starts_with("./") || trimmed.starts_with("../");
    ensure_directory(&if anchored { trimmed.to_string() } else { format!("~/{trimmed}") })
}

/// `getCloneDirectoryName`: the repository's folder name from a URL or `owner/name`.
pub fn clone_directory_name(url: &str) -> String {
    let without_query = url.split(['?', '#']).next().unwrap_or_default().trim();
    let scheme = without_query.find("://");
    let has_host = scheme.is_some() || {
        let at = without_query.find('@');
        let colon = without_query.find(':');
        matches!((at, colon), (Some(a), Some(c)) if a < c && !without_query[..c].contains(['/', '\\']))
    };
    let path = scheme.map_or(without_query, |i| &without_query[i + 3..]);
    let segments: Vec<&str> = path.split(['/', '\\', ':']).filter(|s| !s.trim().is_empty()).collect();
    if has_host && segments.len() < 2 {
        return String::new();
    }
    let last = segments.last().map(|s| s.trim()).unwrap_or_default();
    if has_host && segments.len() == 2 && last.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// `getCloneDirectoryPath` with ghq's `<host>/<owner>/<repo>` when the server asks for it.
pub fn clone_directory_path(url: &str, ghq: bool) -> String {
    if ghq && let Some(path) = ghq_path(url) {
        return path;
    }
    clone_directory_name(url)
}

/// `cloneUrlRepositoryPath`: `host/owner/repo` of an https or scp-style URL.
fn ghq_path(url: &str) -> Option<String> {
    let url = url.trim();
    let (host, path) = if let Some(i) = url.find("://") {
        let rest = &url[i + 3..];
        let rest = rest.rsplit_once('@').map_or(rest, |(_, r)| r);
        rest.split_once('/')?
    } else {
        let rest = url.split_once('@')?.1;
        rest.split_once(':')?
    };
    let host = host.split(':').next()?;
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (!host.is_empty() && path.contains('/')).then(|| format!("{host}/{path}"))
}

/// `getCloneDestinationPath`.
pub fn clone_destination(directory: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return directory.to_string();
    }
    let base = ensure_directory(directory);
    let name = if base.ends_with('\\') { name.replace('/', "\\") } else { name.to_string() };
    format!("{base}{name}")
}

/// `getCloneDestinationBrowsePath`: entering a folder keeps the clone's own folder at the end,
/// unless the folder is already named after it.
pub fn clone_destination_browse(directory: &str, selected: &str, pinned: &str, case_sensitive: bool) -> String {
    let selected_path = append_segment(directory, selected);
    let same = if case_sensitive { selected == pinned } else { selected.eq_ignore_ascii_case(pinned) };
    if same { selected_path } else { clone_destination(&selected_path, pinned) }
}

/// `newProjectFolderName`: runs of anything but `a-z0-9` become `-`, at most 64 characters.
/// Diacritics are not folded (the server names the folder; this is only the preview).
pub fn new_project_folder_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(64);
    let slug = slug.trim_end_matches('-');
    const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];
    let reserved = RESERVED.contains(&slug) || (slug.len() == 4 && (slug.starts_with("com") || slug.starts_with("lpt")) && slug.as_bytes()[3].is_ascii_digit());
    match slug {
        "" => "project".into(),
        _ if reserved => format!("{slug}-project"),
        _ => slug.into(),
    }
}

/// `normalizePastedCloneUrl`: `owner/name` becomes a GitHub URL.
pub fn normalize_clone_url(input: &str) -> String {
    let trimmed = input.trim();
    let shorthand = {
        let parts: Vec<&str> = trimmed.split('/').collect();
        parts.len() == 2
            && !parts[0].is_empty()
            && parts[0].len() <= 39
            && parts[0].chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && parts[0].chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !parts[1].is_empty()
            && parts[1].chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    if shorthand {
        let path = if trimmed.ends_with(".git") { trimmed.to_string() } else { format!("{trimmed}.git") };
        return format!("https://github.com/{path}");
    }
    trimmed.to_string()
}

/// `filterFilesystemBrowseEntries`: hidden folders only when the filter starts with `.`,
/// prefix matches first, then other substring matches; and the entry named exactly.
pub fn filter_entries(entries: &[BrowseEntry], filter: &str) -> (Vec<BrowseEntry>, Option<BrowseEntry>) {
    let show_hidden = filter.starts_with('.');
    let lower = filter.to_lowercase();
    let mut scored: Vec<(usize, &BrowseEntry)> = entries
        .iter()
        .filter(|e| show_hidden || !e.name.starts_with('.'))
        .filter_map(|e| {
            if lower.is_empty() {
                return Some((0, e));
            }
            let name = e.name.to_lowercase();
            if name == lower {
                Some((0, e))
            } else if name.starts_with(&lower) {
                Some((1000 + name.len(), e))
            } else if let Some(i) = name.find(&lower) {
                let boundary = name[..i].ends_with([' ', '-', '_', '.']);
                Some((if boundary { 2000 } else { 3000 } + i, e))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by_key(|(score, _)| *score);
    let visible: Vec<BrowseEntry> = scored.into_iter().map(|(_, e)| e.clone()).collect();
    let exact = (!filter.is_empty()).then(|| visible.iter().find(|e| e.name.as_ref() == filter).cloned()).flatten();
    (visible, exact)
}

/// `getFilesystemBrowseTabCompletion`: one prefix match enters it, several extend the query to
/// their common prefix.
pub fn tab_completion(visible: &[BrowseEntry], filter: &str) -> Option<(SharedString, bool)> {
    let lower = filter.to_lowercase();
    let matches: Vec<&BrowseEntry> = visible.iter().filter(|e| e.name.to_lowercase().starts_with(&lower)).collect();
    let (first, rest) = matches.split_first()?;
    if rest.is_empty() {
        return Some((first.name.clone(), true));
    }
    let mut prefix: Vec<char> = first.name.chars().collect();
    for entry in rest {
        let chars: Vec<char> = entry.name.chars().collect();
        let len = prefix.iter().zip(&chars).take_while(|(a, b)| a.to_lowercase().eq(b.to_lowercase())).count();
        prefix.truncate(len);
    }
    let name: String = prefix.into_iter().collect();
    (name.chars().count() > filter.chars().count()).then(|| (name.into(), false))
}

/// `inferProjectTitleFromPath`: the last path segment.
pub fn title_from_path(path: &str) -> String {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).find(|s| !s.is_empty()).unwrap_or(path).to_string()
}

/// Folders and projects for scenes and checks, with no server.
pub struct FixtureFlows {
    pub environments: Vec<FlowEnvironment>,
    /// Folder paths, each ending in `/`; listings are their direct children.
    pub folders: Vec<&'static str>,
    pub log: Rc<std::cell::RefCell<Vec<String>>>,
}

impl FixtureFlows {
    /// A home with a few code folders on every environment.
    pub fn sample(environments: Vec<FlowEnvironment>) -> Self {
        Self {
            environments,
            folders: vec!["/home/dev/", "/home/dev/code/", "/home/dev/Documents/", "/home/dev/code/infra/", "/home/dev/code/marketing-site/", "/home/dev/code/relay/", "/home/dev/code/t3code/"],
            log: Rc::default(),
        }
    }

    fn expand(path: &str) -> String {
        path.strip_prefix("~/").map_or_else(|| path.to_string(), |rest| format!("/home/dev/{rest}"))
    }
}

impl ProjectFlowBackend for FixtureFlows {
    fn environment(&self, id: &EnvironmentId, _cx: &App) -> Option<FlowEnvironment> {
        self.environments.iter().find(|e| &e.id == id).cloned()
    }

    fn browse(&self, _id: &EnvironmentId, partial_path: String, _cx: &mut App) -> Task<Result<BrowseListing, SharedString>> {
        let directory = Self::expand(&partial_path);
        if !self.folders.contains(&directory.as_str()) {
            return Task::ready(Err(SharedString::from("Directory not found.")));
        }
        let entries: Arc<[BrowseEntry]> = self
            .folders
            .iter()
            .filter_map(|f| {
                let rest = f.strip_prefix(directory.as_str())?.trim_end_matches('/');
                (!rest.is_empty() && !rest.contains('/')).then(|| BrowseEntry { name: rest.to_string().into(), full_path: f.trim_end_matches('/').to_string().into() })
            })
            .collect();
        Task::ready(Ok(BrowseListing { parent_path: directory.trim_end_matches('/').to_string().into(), entries }))
    }

    fn add_project(&self, id: &EnvironmentId, path: String, _cx: &mut App) -> Task<Option<FlowOpened>> {
        self.log.borrow_mut().push(format!("project.create {path}"));
        Task::ready(Some(FlowOpened::Created(ProjectRef { environment_id: id.clone(), project_id: super::sidebar::model::ProjectId(title_from_path(&path).into()) })))
    }

    fn create_new_project(&self, id: &EnvironmentId, name: String, _cx: &mut App) -> Task<Option<ProjectRef>> {
        self.log.borrow_mut().push(format!("projects.createNew {name}"));
        Task::ready(Some(ProjectRef { environment_id: id.clone(), project_id: super::sidebar::model::ProjectId(new_project_folder_name(&name).into()) }))
    }

    fn start_clone(&self, id: &EnvironmentId, remote_url: String, destination: String, _cx: &mut App) -> Task<Option<(ProjectRef, Task<()>)>> {
        self.log.borrow_mut().push(format!("projectClone.start {remote_url} -> {destination}"));
        let project = ProjectRef { environment_id: id.clone(), project_id: super::sidebar::model::ProjectId(title_from_path(&destination).into()) };
        Task::ready(Some((project, Task::ready(()))))
    }
}
