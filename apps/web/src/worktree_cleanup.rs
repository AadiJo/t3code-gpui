//! Which worktree deleting a thread may also remove. Ports `worktreeCleanup.ts`.

/// The thread fields the cleanup rules read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorktreeThreadRef<'a> {
    pub id: &'a str,
    pub worktree_path: Option<&'a str>,
}

/// `normalizeProjectPathForComparison`: trailing separators dropped, and
/// Windows drive and UNC paths compared with backslashes, case-folded.
pub fn normalize_project_path_for_comparison(value: &str) -> String {
    let value = value.trim();
    let cut = if value.starts_with('/') { value.trim_end_matches('/') } else { value.trim_end_matches(['/', '\\']) };
    // A bare root stays a root; a bare drive keeps its separator.
    let normalized = match cut.as_bytes() {
        [] => value.to_owned(),
        [d, b':'] if d.is_ascii_alphabetic() => format!("{cut}\\"),
        _ => cut.to_owned(),
    };
    let b = normalized.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b.len() == 2 || b[2] == b'/' || b[2] == b'\\');
    if drive || normalized.starts_with("\\\\") {
        normalized.replace('/', "\\").to_lowercase()
    } else {
        normalized
    }
}

/// `getOrphanedWorktreePathForThread`: the thread's worktree, unless another
/// thread uses the same checkout (confirming deletion force-removes it for all).
pub fn get_orphaned_worktree_path_for_thread<'a>(threads: &[WorktreeThreadRef<'a>], thread_id: &str) -> Option<&'a str> {
    let target = threads.iter().find(|t| t.id == thread_id)?;
    let path = target.worktree_path.map(str::trim).filter(|p| !p.is_empty())?;
    let key = normalize_project_path_for_comparison(path);
    let shared = threads.iter().any(|t| {
        t.id != thread_id && t.worktree_path.is_some_and(|p| normalize_project_path_for_comparison(p) == key)
    });
    (!shared).then_some(path)
}

/// `formatWorktreePathForDisplay`: the worktree's folder name.
pub fn format_worktree_path_for_display(worktree_path: &str) -> &str {
    let trimmed = worktree_path.trim();
    if trimmed.is_empty() {
        return worktree_path;
    }
    let last = trimmed.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or("").trim();
    if last.is_empty() { trimmed } else { last }
}
