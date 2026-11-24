//! What the right panel shows for fixture threads: a real directory per
//! project for the Files tab, and diff patches built from each thread's
//! changed-files cards for the Diff tab. The changed files exist in the
//! project's directory, so opening one from the diff or the timeline works.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::SharedString;

use crate::components::chat::timeline_entries::{FileChange, TurnDiffSummary};
use crate::components::diff_panel::{CommitSummary, DiffPanelContent, DiffPanelInput, TurnSummary};
use crate::diff_panel_store::DiffPanelSelection;
use crate::diff_patch::renderable_patch;

/// Files the fixture turns change, all present in every project directory.
pub const CHANGED_PATHS: [&str; 4] = ["src/orbit.rs", "src/main.rs", "src/components/App.tsx", "README.md"];

/// The directory standing in for a project's workspace, written on first use.
pub fn project_directory(project_id: &str, changed: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join("t3code-fixture-workspaces").join(sanitize(project_id));
    if !root.join("README.md").exists() {
        crate::scenes::files::write_fixture(&root).ok();
    }
    // The fixture replies cite `src/replay.rs` as a file chip.
    for path in changed.iter().chain(&["src/replay.rs"]) {
        let full = root.join(path);
        if !full.exists() {
            write_generated(&full, path).ok();
        }
    }
    root
}

fn sanitize(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

fn write_generated(full: &Path, path: &str) -> std::io::Result<()> {
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(full, generated_lines(path, 0, 80).join("\n") + "\n")
}

/// Code-looking lines for `path`, numbered from `from`.
fn generated_lines(path: &str, from: u32, count: u32) -> Vec<String> {
    let stem = path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("item").replace('-', "_");
    (from..from + count)
        .map(|n| match n % 4 {
            0 => format!("// {stem}: step {n}"),
            1 => format!("fn {stem}_{n}(input: &Input) -> Output {{"),
            2 => format!("    apply(input, {n})"),
            _ => "}".to_owned(),
        })
        .collect()
}

/// A unified diff for the files of one turn: each update replaces its first
/// `deletions` lines with `additions` lines; creates and deletes are whole files.
pub fn turn_patch(files: &[FileChange]) -> String {
    let mut patch = String::new();
    for file in files {
        let path = &file.path;
        let (adds, dels) = (file.additions.min(400), file.deletions.min(400));
        patch.push_str(&format!("diff --git a/{path} b/{path}\n"));
        match file.operation.as_ref() {
            "create" => patch.push_str(&format!("new file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{adds} @@\n")),
            "delete" => patch.push_str(&format!("deleted file mode 100644\n--- a/{path}\n+++ /dev/null\n@@ -1,{dels} +0,0 @@\n")),
            _ => patch.push_str(&format!("--- a/{path}\n+++ b/{path}\n@@ -1,{} +1,{} @@\n", dels + 3, adds + 3)),
        }
        for line in generated_lines(path, 1_000, dels) {
            patch.push_str(&format!("-{line}\n"));
        }
        for line in generated_lines(path, 0, adds) {
            patch.push_str(&format!("+{line}\n"));
        }
        if !matches!(file.operation.as_ref(), "create" | "delete") {
            for line in generated_lines(path, adds, 3) {
                patch.push_str(&format!(" {line}\n"));
            }
        }
    }
    patch
}

/// Every file the summaries touch, once, with stats summed.
fn merged(summaries: &[TurnDiffSummary]) -> Vec<FileChange> {
    let mut files: Vec<FileChange> = Vec::new();
    for file in summaries.iter().flat_map(|s| s.files.iter()) {
        match files.iter_mut().find(|f| f.path == file.path) {
            Some(existing) => {
                existing.additions += file.additions;
                existing.deletions += file.deletions;
            }
            None => files.push(file.clone()),
        }
    }
    files
}

/// The Diff tab's input for a thread and the scope it shows.
pub fn diff_input(
    thread_key: SharedString,
    workspace_root: SharedString,
    branch: Option<SharedString>,
    summaries: &[TurnDiffSummary],
    selection: &DiffPanelSelection,
) -> DiffPanelInput {
    let turns: Vec<TurnSummary> = summaries
        .iter()
        .enumerate()
        .rev()
        .map(|(index, summary)| TurnSummary {
            turn_id: summary.assistant_message_id.0.clone(),
            turn_count: index as u32 + 1,
            completed_label: format!("Turn {}", index + 1).into(),
        })
        .collect();
    let files = match selection {
        DiffPanelSelection::Turn { turn_id, .. } => summaries
            .iter()
            .find(|s| s.assistant_message_id.as_str() == turn_id)
            .map(|s| s.files.to_vec())
            .unwrap_or_default(),
        DiffPanelSelection::Unstaged => summaries.last().map(|s| s.files.to_vec()).unwrap_or_default(),
        DiffPanelSelection::Commit { .. } => summaries.first().map(|s| s.files.to_vec()).unwrap_or_default(),
        DiffPanelSelection::Branch { .. } | DiffPanelSelection::ToolCall { .. } => merged(summaries),
    };
    let content = match renderable_patch(&turn_patch(&files)) {
        Some(patch) => DiffPanelContent::Patch { patch: Arc::new(patch), notices: Vec::new(), stats: None },
        None if summaries.is_empty() => DiffPanelContent::NoTurns,
        None => DiffPanelContent::Empty { no_net_changes: true },
    };
    DiffPanelInput {
        thread_key: Some(thread_key),
        workspace_root: Some(workspace_root),
        turns,
        commits_status: crate::components::diff_panel::CommitsStatus::Loaded { truncated: false },
        base_ref_choices: vec![
            crate::components::diff_panel::BaseRefChoice { label: "main".into(), local: Some("main".into()), remote: Some("origin/main".into()) },
            crate::components::diff_panel::BaseRefChoice { label: "release/0.1".into(), local: Some("release/0.1".into()), remote: None },
            crate::components::diff_panel::BaseRefChoice { label: "origin/feature/relay".into(), local: None, remote: Some("origin/feature/relay".into()) },
        ],
        commits: vec![
            CommitSummary { sha: "4f1c2ab9d0e".into(), subject: "feat: replay loop batches writes".into(), relative_label: "2h ago".into(), url: None },
            CommitSummary { sha: "9a8b7c6d5e4".into(), subject: "fix: persist the cursor after commit".into(), relative_label: "1d ago".into(), url: None },
        ],
        head_ref: branch.or(Some("main".into())),
        base_ref: Some("origin/main".into()),
        can_refresh: true,
        refreshing: false,
        content,
    }
}

/// The Diff tab's input for the Mac reference world: the checkout's uncommitted changes in every
/// scope, with no turns or commits (the reference thread's turns changed no tracked files).
pub fn reference_diff_input(thread_key: SharedString, workspace_root: SharedString, branch: Option<SharedString>, patch: &str) -> DiffPanelInput {
    let content = match renderable_patch(patch) {
        Some(patch) => DiffPanelContent::Patch { patch: Arc::new(patch), notices: Vec::new(), stats: None },
        None => DiffPanelContent::Empty { no_net_changes: true },
    };
    DiffPanelInput {
        thread_key: Some(thread_key),
        workspace_root: Some(workspace_root),
        commits_status: crate::components::diff_panel::CommitsStatus::Loaded { truncated: false },
        head_ref: branch.or(Some("main".into())),
        base_ref: Some("origin/main".into()),
        can_refresh: true,
        content,
        ..DiffPanelInput::default()
    }
}
