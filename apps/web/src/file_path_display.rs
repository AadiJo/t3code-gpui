//! Paths as the UI shows them. Ports `filePathDisplay.ts` and the position
//! helpers of `packages/shared/src/fileLinks.ts` it relies on.

/// A path with an optional `:line[:column]` suffix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePathPosition {
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// `splitFilePathPosition` for the `path:line[:column]` suffix form.
pub fn split_file_path_position(value: &str) -> FilePathPosition {
    fn trailing_number(value: &str) -> Option<(&str, u32)> {
        let (head, tail) = value.rsplit_once(':')?;
        let valid = !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit());
        valid.then(|| tail.parse().ok().map(|n| (head, n))).flatten()
    }
    let (path, line, column) = match trailing_number(value) {
        Some((head, column)) => match trailing_number(head) {
            Some((path, line)) => (path, line, Some(column)),
            None => (head, column, None),
        },
        None => return FilePathPosition { path: value.to_owned(), line: None, column: None },
    };
    if path.is_empty() || line == 0 {
        return FilePathPosition { path: value.to_owned(), line: None, column: None };
    }
    FilePathPosition { path: path.to_owned(), line: Some(line), column: column.filter(|c| *c > 0) }
}

/// `formatFilePathPosition`.
pub fn format_file_path_position(position: &FilePathPosition) -> String {
    match (position.line, position.column) {
        (Some(line), Some(column)) => format!("{}:{line}:{column}", position.path),
        (Some(line), None) => format!("{}:{line}", position.path),
        _ => position.path.clone(),
    }
}

/// `fileBasename`.
pub fn file_basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return path;
    }
    trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed)
}

fn strip_slash_prefixed_windows_drive(path: &str) -> &str {
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' { &path[1..] } else { path }
}

fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\") || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// `isHostPath`: absolute, Windows-absolute, or `~/`.
pub fn is_host_path(path: &str) -> bool {
    path.starts_with('/') || is_windows_absolute(path) || path.starts_with("~/") || path.starts_with("~\\")
}

/// `formatWorkspaceRelativePath`: `<workspace folder>/<relative path>`, keeping
/// a `:line` suffix. Paths outside the workspace stay absolute.
pub fn format_workspace_relative_path(path_with_position: &str, workspace_root: Option<&str>) -> String {
    let position = split_file_path_position(path_with_position);
    let normalized = position.path.replace('\\', "/");
    let normalized_path = strip_slash_prefixed_windows_drive(&normalized).to_owned();
    let mut display = normalized_path.clone();
    if let Some(root) = workspace_root.filter(|r| !r.is_empty()) {
        let trimmed = root.trim_end_matches(['/', '\\']).replace('\\', "/");
        let root_normalized = strip_slash_prefixed_windows_drive(&trimmed).to_owned();
        let label = file_basename(&root_normalized).to_owned();
        let insensitive = is_windows_absolute(strip_slash_prefixed_windows_drive(root));
        let fold = |s: &str| if insensitive { s.to_lowercase() } else { s.to_owned() };
        let compare_path = fold(&normalized_path);
        let compare_root = fold(&root_normalized);
        if compare_path == compare_root {
            display = label;
        } else if compare_path.starts_with(&format!("{compare_root}/")) {
            display = format!("{label}/{}", &normalized_path[root_normalized.len() + 1..]);
        } else if !is_host_path(&normalized_path) {
            let relative = normalized_path.trim_start_matches("./").trim_start_matches('/');
            display = if compare_path.starts_with(&format!("{}/", fold(&label))) {
                normalized_path.clone()
            } else {
                format!("{label}/{relative}")
            };
        }
    }
    format_file_path_position(&FilePathPosition { path: display, ..position })
}
