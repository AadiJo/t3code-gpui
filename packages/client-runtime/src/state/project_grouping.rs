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
