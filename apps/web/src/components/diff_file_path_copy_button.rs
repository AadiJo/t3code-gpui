//! Copy-path support for diff file headers. Ports `DiffFilePathCopyButton.tsx`
//! and `formatFilePathForClipboard`; the button itself is drawn by the code
//! view's header, which owns the 1.2s "copied" state.

/// The path as the clipboard gets it: repository-relative, with backslashes
/// when the workspace lives on a Windows host.
pub fn format_file_path_for_clipboard(path: &str, workspace_root: Option<&str>) -> String {
    let windows_host = workspace_root.is_some_and(|root| {
        let bytes = root.as_bytes();
        root.starts_with("\\\\") || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/'))
    });
    if windows_host { path.replace('/', "\\") } else { path.to_owned() }
}
