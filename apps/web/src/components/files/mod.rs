//! The Files tab: the file tree, the file viewer and editor, and go to file.
//! Ports `components/files`. Everything reads through [`WorkspaceFiles`](crate::file_source::WorkspaceFiles).

pub mod editor_buffer;
pub mod editor_highlight;
pub mod file_editor;

/// Registers key bindings for the Files tab views.
pub fn init(cx: &mut gpui_kit::App) {
    file_editor::init(cx);
}
