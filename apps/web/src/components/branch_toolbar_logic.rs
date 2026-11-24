//! The decisions behind the branch and worktree controls. Ports
//! `BranchToolbar.logic.ts` and the ref-name helpers it re-exports from
//! `packages/shared/src/git.ts`.

use gpui_kit::SharedString;
use t3code_contracts::{EnvironmentId, VcsRef};

use crate::i18n::t;

/// Where a new thread runs: the project's checkout or a fresh worktree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EnvMode {
    #[default]
    Local,
    Worktree,
}

impl EnvMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Worktree => "worktree",
        }
    }
}

/// The kind of machine an environment runs on, which picks its icon
/// (`resolveEnvironmentMachineKind`).
pub use t3code_contracts::EnvironmentMachineKind as MachineKind;

/// One machine the run-context menu offers (`EnvironmentOption`).
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentOption {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub is_primary: bool,
    pub machine: MachineKind,
}

const GENERIC_LOCAL_ENVIRONMENT_LABELS: [&str; 2] = ["local", "local environment"];

fn normalize_display_label(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// `resolveEnvironmentOptionLabel`: a primary machine's generic name reads "This device".
pub fn resolve_environment_option_label(
    is_primary: bool,
    environment_id: &EnvironmentId,
    runtime_label: Option<&str>,
    saved_label: Option<&str>,
) -> SharedString {
    let runtime = normalize_display_label(runtime_label);
    let saved = normalize_display_label(saved_label);
    if is_primary {
        return [runtime, saved]
            .into_iter()
            .flatten()
            .find(|label| !GENERIC_LOCAL_ENVIRONMENT_LABELS.contains(&label.to_lowercase().as_str()))
            .map(|label| SharedString::from(label.to_owned()))
            .unwrap_or_else(|| t!("This device"));
    }
    SharedString::from(runtime.or(saved).unwrap_or(environment_id.as_str()).to_owned())
}

/// `shouldShowEnvironmentIndicator`.
pub fn should_show_environment_indicator(active_is_primary: Option<bool>, can_pick_environment: bool) -> bool {
    can_pick_environment || active_is_primary == Some(false)
}

/// `shouldShowComposerContextStrip`: a new thread always shows it.
pub fn should_show_composer_context_strip(
    is_draft_hero_state: bool,
    persist_in_active_threads: bool,
    has_active_project: bool,
    is_git_repo: bool,
    show_environment_indicator: bool,
) -> bool {
    has_active_project
        && (is_draft_hero_state || (persist_in_active_threads && (is_git_repo || show_environment_indicator)))
}

/// Hysteresis that keeps labels and icons from flapping at the width boundary.
const CONTEXT_STRIP_COMPACT_EXPAND_HYSTERESIS_PX: f32 = 16.;

/// `resolveContextStripLabelsCompact`.
pub fn resolve_context_strip_labels_compact(compact: bool, needed_width: f32, available_width: f32) -> bool {
    if compact {
        needed_width > available_width - CONTEXT_STRIP_COMPACT_EXPAND_HYSTERESIS_PX
    } else {
        needed_width > available_width
    }
}

/// `resolveEnvModeLabel`.
pub fn resolve_env_mode_label(mode: EnvMode) -> SharedString {
    match mode {
        EnvMode::Worktree => t!("New worktree"),
        EnvMode::Local => t!("Current checkout"),
    }
}

/// `resolveCurrentWorkspaceLabel`.
pub fn resolve_current_workspace_label(active_worktree_path: Option<&str>) -> SharedString {
    if active_worktree_path.is_some() { t!("Current worktree") } else { resolve_env_mode_label(EnvMode::Local) }
}

/// `resolveLockedWorkspaceLabel`: a started thread names its worktree folder.
pub fn resolve_locked_workspace_label(active_worktree_path: Option<&str>, effective_env_mode: EnvMode) -> SharedString {
    if let Some(path) = active_worktree_path {
        return path
            .rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .map(|s| SharedString::from(s.to_owned()))
            .unwrap_or_else(|| t!("Worktree"));
    }
    match effective_env_mode {
        EnvMode::Worktree => resolve_env_mode_label(EnvMode::Worktree),
        EnvMode::Local => t!("Local checkout"),
    }
}

/// A worktree a recent thread of the project used (`PreviousWorktreeSeed`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviousWorktreeSeed {
    pub branch: Option<String>,
    pub worktree_path: String,
}

/// The thread fields [`resolve_previous_worktree_seed`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeThread {
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    /// Milliseconds since the epoch, `None` when the timestamp does not parse.
    pub updated_at_ms: Option<i64>,
    pub archived: bool,
}

/// `resolvePreviousWorktreeSeed`: the most recently updated other, unarchived worktree.
pub fn resolve_previous_worktree_seed(
    threads: &[WorktreeThread],
    current_worktree_path: Option<&str>,
) -> Option<PreviousWorktreeSeed> {
    threads
        .iter()
        .filter_map(|thread| {
            let path = thread.worktree_path.as_deref().filter(|p| !p.is_empty())?;
            (Some(path) != current_worktree_path && !thread.archived).then_some((thread, path, thread.updated_at_ms?))
        })
        // The first of equal timestamps wins, as the strict `>` in the original.
        .fold(None, |latest: Option<(&WorktreeThread, &str, i64)>, candidate| match latest {
            Some(l) if candidate.2 <= l.2 => Some(l),
            _ => Some(candidate),
        })
        .map(|(thread, path, _)| PreviousWorktreeSeed { branch: thread.branch.clone(), worktree_path: path.to_owned() })
}

/// `resolvePreviousWorktreeLabel`.
pub fn resolve_previous_worktree_label(seed: &PreviousWorktreeSeed) -> SharedString {
    match &seed.branch {
        Some(branch) => t!("Previous worktree ({branch})", branch = branch),
        None => t!("Previous worktree"),
    }
}

/// `resolveEffectiveEnvMode`.
pub fn resolve_effective_env_mode(
    active_worktree_path: Option<&str>,
    has_server_thread: bool,
    draft_thread_env_mode: Option<EnvMode>,
    preparing_worktree: bool,
) -> EnvMode {
    if !has_server_thread {
        if active_worktree_path.is_some() {
            return EnvMode::Local;
        }
        return draft_thread_env_mode.unwrap_or_default();
    }
    if active_worktree_path.is_some() || preparing_worktree { EnvMode::Worktree } else { EnvMode::Local }
}

/// `resolveDraftEnvModeAfterBranchChange`.
pub fn resolve_draft_env_mode_after_branch_change(
    next_worktree_path: Option<&str>,
    current_worktree_path: Option<&str>,
    effective_env_mode: EnvMode,
) -> EnvMode {
    if next_worktree_path.is_some() || (effective_env_mode == EnvMode::Worktree && current_worktree_path.is_none()) {
        EnvMode::Worktree
    } else {
        EnvMode::Local
    }
}

/// `resolveBranchToolbarValue`: a new worktree shows its base branch.
pub fn resolve_branch_toolbar_value<'a>(
    env_mode: EnvMode,
    active_worktree_path: Option<&str>,
    active_thread_branch: Option<&'a str>,
    current_git_branch: Option<&'a str>,
) -> Option<&'a str> {
    if env_mode == EnvMode::Worktree && active_worktree_path.is_none() {
        return active_thread_branch.or(current_git_branch);
    }
    current_git_branch.or(active_thread_branch)
}

/// `resolveDefaultWorktreeBaseBranch`: the default ref, else the current one.
pub fn resolve_default_worktree_base_branch(refs: &[VcsRef]) -> Option<&str> {
    refs.iter().find(|r| r.is_default).or_else(|| refs.iter().find(|r| r.current)).map(|r| r.name.as_str())
}

/// `resolveBranchTriggerLabel`.
pub fn resolve_branch_trigger_label(
    active_worktree_path: Option<&str>,
    effective_env_mode: EnvMode,
    resolved_active_branch: Option<&str>,
    resolved_active_branch_is_remote: Option<bool>,
    start_from_origin: bool,
) -> SharedString {
    let Some(branch) = resolved_active_branch else { return t!("Select ref") };
    if effective_env_mode == EnvMode::Worktree && active_worktree_path.is_none() {
        // Only the server can determine a local branch's configured remote.
        return if start_from_origin && resolved_active_branch_is_remote == Some(false) {
            t!("From {branch} (remote)", branch = branch)
        } else {
            t!("From {branch}", branch = branch)
        };
    }
    SharedString::from(branch.to_owned())
}

/// `resolveBranchToolbarPrBranch`.
pub fn resolve_branch_toolbar_pr_branch<'a>(
    active_thread_branch: Option<&'a str>,
    resolved_active_branch: Option<&str>,
) -> Option<&'a str> {
    (active_thread_branch == resolved_active_branch).then_some(active_thread_branch).flatten()
}

/// `resolveLocalCheckoutBranchMismatch`: the thread's branch and the checkout's differ.
pub fn resolve_local_checkout_branch_mismatch<'a>(
    effective_env_mode: EnvMode,
    active_worktree_path: Option<&str>,
    active_thread_branch: Option<&'a str>,
    current_git_branch: Option<&'a str>,
) -> Option<(&'a str, &'a str)> {
    if effective_env_mode != EnvMode::Local || active_worktree_path.is_some() {
        return None;
    }
    match (active_thread_branch, current_git_branch) {
        (Some(thread), Some(current)) if !thread.is_empty() && !current.is_empty() && thread != current => {
            Some((thread, current))
        }
        _ => None,
    }
}

/// Where a ref picked in the branch picker is checked out (`resolveBranchSelectionTarget`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchSelectionTarget {
    pub checkout_cwd: String,
    pub next_worktree_path: Option<String>,
    pub reuse_existing_worktree: bool,
    pub stays_in_active_checkout: bool,
}

/// `resolveBranchSelectionTarget`.
pub fn resolve_branch_selection_target(
    active_project_cwd: &str,
    active_project_checkout_root: Option<&str>,
    active_worktree_path: Option<&str>,
    ref_is_default: bool,
    ref_worktree_path: Option<&str>,
) -> BranchSelectionTarget {
    if let Some(worktree) = ref_worktree_path.filter(|p| !p.is_empty()) {
        // Until the project's checkout root loads, a checkout containing the project counts as its own.
        let is_project_checkout = match active_project_checkout_root {
            None => workspace_relative_file_path(active_project_cwd, Some(worktree)).is_some(),
            Some(root) => workspace_relative_file_path(root, Some(worktree)).as_deref() == Some("."),
        };
        let next = (!is_project_checkout).then(|| worktree.to_owned());
        let stays = next.as_deref() == active_worktree_path;
        return BranchSelectionTarget {
            checkout_cwd: worktree.to_owned(),
            next_worktree_path: next,
            reuse_existing_worktree: true,
            stays_in_active_checkout: stays,
        };
    }
    let next = if active_worktree_path.is_some() && ref_is_default { None } else { active_worktree_path };
    BranchSelectionTarget {
        checkout_cwd: next.unwrap_or(active_project_cwd).to_owned(),
        next_worktree_path: next.map(str::to_owned),
        reuse_existing_worktree: false,
        stays_in_active_checkout: next == active_worktree_path,
    }
}

/// `isWorktreeChangeBlocked`: a running turn pins its worktree.
pub fn is_worktree_change_blocked(
    turn_interruptible: bool,
    current_worktree_path: Option<&str>,
    next_worktree_path: Option<&str>,
) -> bool {
    next_worktree_path != current_worktree_path && turn_interruptible
}

/// `sanitizeNewRefName`: whitespace runs become dashes.
pub fn sanitize_new_ref_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_space = false;
    for ch in raw.trim().chars() {
        // JS `[ \t\n\r\f\v]`.
        if matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{0C}' | '\u{0B}') {
            if !in_space {
                out.push('-');
            }
            in_space = true;
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}

/// `deriveLocalBranchNameFromRemoteRef`: `origin/feature` becomes `feature`.
pub fn derive_local_branch_name_from_remote_ref(name: &str) -> &str {
    match name.find('/') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i + 1..],
        _ => name,
    }
}

/// `shouldIncludeBranchPickerItem`: the picker's search filter.
pub fn should_include_branch_picker_item(
    item_value: &str,
    normalized_query: &str,
    create_branch_item_value: Option<&str>,
    checkout_pull_request_item_value: Option<&str>,
) -> bool {
    if normalized_query.is_empty()
        || create_branch_item_value == Some(item_value)
        || checkout_pull_request_item_value == Some(item_value)
    {
        return true;
    }
    let lower = item_value.to_lowercase();
    if lower.contains(normalized_query) {
        return true;
    }
    let sanitized = sanitize_new_ref_name(normalized_query);
    !sanitized.is_empty() && sanitized != normalized_query && lower.contains(&sanitized)
}

/// `isWindowsAbsolutePath`: a UNC path or `C:` followed by a separator or nothing.
fn is_windows_absolute_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with("\\\\")
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'/' || b[2] == b'\\'))
}

fn strip_slash_prefixed_windows_drive(path: &str) -> &str {
    let b = path.as_bytes();
    if b.len() >= 4 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' && (b[3] == b'/' || b[3] == b'\\') {
        &path[1..]
    } else {
        path
    }
}

/// `workspaceRelativeFilePath`: `path` relative to `root`, `"."` for the root
/// itself, `None` outside it. Windows drive roots compare case-insensitively.
pub fn workspace_relative_file_path(path: &str, workspace_root: Option<&str>) -> Option<String> {
    let root = workspace_root.filter(|r| !r.is_empty())?;
    let is_network = root.len() >= 2 && root.bytes().take(2).all(|b| b == b'/' || b == b'\\');
    let case_insensitive =
        is_windows_absolute_path(strip_slash_prefixed_windows_drive(root)) || (is_network && path.starts_with("\\\\"));
    let normalized_path =
        if case_insensitive { strip_slash_prefixed_windows_drive(&path.replace('\\', "/")).to_owned() } else { path.to_owned() };
    let normalized_root = if case_insensitive {
        strip_slash_prefixed_windows_drive(&root.replace('\\', "/")).to_owned()
    } else {
        root.to_owned()
    };
    let normalized_root = normalized_root.trim_end_matches('/');
    let fold = |s: &str| if case_insensitive { s.to_lowercase() } else { s.to_owned() };
    let (path_cmp, root_cmp) = (fold(&normalized_path), fold(normalized_root));
    if path_cmp.trim_end_matches('/') == root_cmp {
        return Some(".".to_owned());
    }
    if !path_cmp.starts_with(&format!("{root_cmp}/")) {
        return None;
    }
    let relative = &normalized_path[normalized_root.len() + 1..];
    let mut depth = 0i32;
    for segment in relative.split('/') {
        if segment == ".." {
            depth -= 1;
        } else if !segment.is_empty() && segment != "." {
            depth += 1;
        }
        if depth < 0 {
            return None;
        }
    }
    Some(relative.to_owned())
}
