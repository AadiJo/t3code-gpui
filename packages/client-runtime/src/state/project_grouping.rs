//! Projects grouped across environments for display only (`projectGrouping.ts`). Checkouts of
//! one repository share a logical key; a project without a repository identity keeps its own
//! physical key `${environmentId}:${normalizedWorkspaceRoot}`.

use std::collections::HashMap;

use t3code_contracts::{EnvironmentId, OrchestrationProjectShell, SidebarProjectGroupingMode as Mode};

fn is_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes.len() == 2 || matches!(bytes[2], b'/' | b'\\'))
}

fn is_root(value: &str) -> bool {
    value == "/" || value == "\\" || (value.len() == 3 && is_windows_drive_path(value))
}

fn trim_trailing_separators(value: &str) -> String {
    if value.is_empty() || is_root(value) {
        return value.to_owned();
    }
    let trimmed = if value.starts_with('/') { value.trim_end_matches('/') } else { value.trim_end_matches(['/', '\\']) };
    if trimmed.is_empty() {
        return value.to_owned();
    }
    if trimmed.len() == 2 && is_windows_drive_path(trimmed) { format!("{trimmed}\\") } else { trimmed.to_owned() }
}

/// A workspace path as compared across projects (`normalizeProjectPathForComparison`): trimmed,
/// without trailing separators, and case-folded with backslashes on Windows paths.
pub fn normalize_project_path(value: &str) -> String {
    let normalized = trim_trailing_separators(value.trim());
    if is_windows_drive_path(&normalized) || normalized.starts_with("\\\\") {
        normalized.replace('/', "\\").to_lowercase()
    } else {
        normalized
    }
}

/// The physical key of a project in one environment.
pub fn physical_project_key(environment_id: &EnvironmentId, workspace_root: &str) -> String {
    format!("{environment_id}:{}", normalize_project_path(workspace_root))
}

fn repository_relative_path(project: &OrchestrationProjectShell) -> Option<String> {
    let root = project.repository_identity.as_ref()?.root_path.as_deref()?.trim();
    if root.is_empty() {
        return None;
    }
    let path = normalize_project_path(&project.workspace_root);
    let root = normalize_project_path(root);
    if path.is_empty() || root.is_empty() {
        return None;
    }
    if path == root {
        return Some(String::new());
    }
    let separator = if root.contains('\\') { '\\' } else { '/' };
    path.strip_prefix(&format!("{root}{separator}")).map(|rest| rest.replace('\\', "/"))
}

/// Grouping settings: a default mode and per-project overrides keyed by physical key.
#[derive(Debug, Clone)]
pub struct ProjectGroupingSettings {
    pub mode: Mode,
    pub overrides: HashMap<String, Mode>,
}

impl Default for ProjectGroupingSettings {
    fn default() -> Self {
        Self { mode: Mode::Repository, overrides: HashMap::new() }
    }
}

/// The key projects are grouped under for display (`deriveLogicalProjectKeyFromSettings`).
pub fn logical_project_key(environment_id: &EnvironmentId, project: &OrchestrationProjectShell, settings: &ProjectGroupingSettings) -> String {
    let physical = physical_project_key(environment_id, &project.workspace_root);
    let mode = settings.overrides.get(&physical).copied().unwrap_or(settings.mode);
    if mode == Mode::Separate {
        return physical;
    }
    // A NUL reserves the scratch namespace outside repository identities.
    let Some(canonical) = project.repository_identity.as_ref().map(|identity| identity.grouping_key()).filter(|key| !key.is_empty() && !key.contains('\0')) else {
        return physical;
    };
    if mode == Mode::Repository {
        return canonical.to_owned();
    }
    match repository_relative_path(project) {
        Some(relative) if !relative.is_empty() => format!("{canonical}::{relative}"),
        _ => canonical.to_owned(),
    }
}

/// The name a group of projects shows (`deriveProjectGroupLabel`): one shared custom-looking
/// title, else the shared repository display name, else the shared repository name, else the
/// repository's name for a whole-repository group, else the representative's title.
pub fn group_label(group_key: &str, representative: &OrchestrationProjectShell, members: &[&OrchestrationProjectShell]) -> String {
    fn unique<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Vec<&'a str> {
        let mut out: Vec<&str> = Vec::new();
        for value in values.flatten().map(str::trim).filter(|v| !v.is_empty()) {
            if !out.contains(&value) {
                out.push(value);
            }
        }
        out
    }
    let titles = unique(members.iter().map(|m| Some(m.title.as_str())));
    let display_names = unique(members.iter().map(|m| m.repository_identity.as_ref().and_then(|i| i.grouping_display_name())));
    let repository_names = unique(members.iter().map(|m| m.repository_identity.as_ref().and_then(|i| i.name.as_deref())));
    if let [title] = titles.as_slice()
        && (members.iter().any(|m| m.has_custom_title == Some(true)) || (!display_names.contains(title) && !repository_names.contains(title)))
    {
        return (*title).to_owned();
    }
    if let [name] = display_names.as_slice() {
        return (*name).to_owned();
    }
    if let [name] = repository_names.as_slice() {
        return (*name).to_owned();
    }
    if let Some(identity) = representative.repository_identity.as_ref()
        && group_key == identity.grouping_key()
    {
        return identity.grouping_display_name().or(identity.name.as_deref()).unwrap_or(&representative.title).to_owned();
    }
    representative.title.clone()
}
