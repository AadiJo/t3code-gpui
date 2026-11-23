//! Native replacements for `@pierre/diffs`: the code view shared by the diff
//! panel and the file viewer, the diff file tree, and review comments.

pub mod code_view;
pub mod colors;
pub mod diff_file_tree;
pub mod layout;
pub mod review_comment;

pub use code_view::{CodeView, CodeViewEvent, CodeViewFile, CodeViewOptions, ViewMode};

/// Registers key bindings for the code view and file tree.
pub fn init(cx: &mut gpui_kit::App) {
    cx.default_global::<crate::diff_panel_store::DiffPanelStore>();
    code_view::init(cx);
    diff_file_tree::init(cx);
}
