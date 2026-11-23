//! The Files tab: the file tree, the file viewer and editor, and go to file.
//! Ports `components/files`. Everything reads through [`WorkspaceFiles`](crate::file_source::WorkspaceFiles).

pub mod delimited_table;
pub mod editor_buffer;
pub mod editor_highlight;
pub mod file_editor;
pub mod file_tree;
pub mod preview_image;

/// Registers key bindings for the Files tab views.
pub fn init(cx: &mut gpui_kit::App) {
    file_editor::init(cx);
    file_tree::init(cx);
}
