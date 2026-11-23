//! The diff panel's file tree. Ports `DiffFileTree.tsx` and
//! `diffFileTree.logic.ts` with `@pierre/trees` drawn natively: compact
//! 24px rows, empty directories flattened, folders open by default, git status
//! colours and letters, ordered like the patch (`diffFileTreePositions`).

use std::{collections::HashSet, ops::Range, sync::Arc};

use gpui_kit::{prelude::FluentBuilder as _, *};

use crate::{
    ActiveAppTheme as _, diff_file_order::diff_file_tree_positions, diff_panel_store::DiffColorScheme,
    diff_patch::{ChangeType, FileDiff}, syntax::hex, theme_palette::Appearance,
};

const CONTEXT: &str = "DiffFileTree";
const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 12.;

actions!(diff_file_tree, [SelectNext, SelectPrevious, ExpandOrOpen, CollapseOrParent, Confirm]);

/// Registers the tree's key bindings.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("right", ExpandOrOpen, Some(CONTEXT)),
        KeyBinding::new("left", CollapseOrParent, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
    ]);
}

/// Pierre trees' git status for a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

impl GitStatus {
    fn letter(self) -> &'static str {
        match self {
            GitStatus::Added => "A",
            GitStatus::Deleted => "D",
            GitStatus::Modified => "M",
            GitStatus::Renamed => "R",
        }
    }

    fn color(self, appearance: Appearance, scheme: DiffColorScheme) -> Hsla {
        let dark = appearance == Appearance::Dark;
        let pick = |d: &str, l: &str| hex(if dark { d } else { l }).unwrap_or_default();
        match (self, scheme) {
            (GitStatus::Added, DiffColorScheme::RedGreen) => pick("#00cab1", "#16a994"),
            (GitStatus::Deleted, DiffColorScheme::RedGreen) => pick("#ff6762", "#ff2e3f"),
            (GitStatus::Added, DiffColorScheme::BlueOrange) => pick("#51a2ff", "#155dfc"),
            (GitStatus::Deleted, DiffColorScheme::BlueOrange) => pick("#ff8904", "#f54900"),
            (GitStatus::Modified, _) => pick("#08c0ef", "#1ca1c7"),
            (GitStatus::Renamed, _) => pick("#ffd452", "#d5a910"),
        }
    }
}

/// A file in the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFileTreeEntry {
    pub path: SharedString,
    pub status: GitStatus,
}

/// One entry per path; a path changed twice (file to symlink) shows as modified.
/// Ports `diffFileTreeEntries`.
pub fn diff_file_tree_entries<'a>(files: impl IntoIterator<Item = &'a Arc<FileDiff>>) -> Vec<DiffFileTreeEntry> {
    let mut entries: Vec<DiffFileTreeEntry> = Vec::new();
    for file in files {
        let status = match file.change {
            ChangeType::New => GitStatus::Added,
            ChangeType::Deleted => GitStatus::Deleted,
            ChangeType::RenamePure | ChangeType::RenameChanged => GitStatus::Renamed,
            ChangeType::Change => GitStatus::Modified,
        };
        match entries.iter_mut().find(|entry| entry.path == file.path) {
            Some(entry) if entry.status != status => entry.status = GitStatus::Modified,
            Some(_) => {}
            None => entries.push(DiffFileTreeEntry { path: file.path.clone(), status }),
        }
    }
    entries
}

#[derive(Debug)]
struct Node {
    /// Full path; directories end in `/`.
    path: SharedString,
    /// Displayed name; flattened directories join segments with `/`.
    name: SharedString,
    status: Option<GitStatus>,
    children: Vec<Node>,
}

#[derive(Clone, Debug)]
struct VisibleRow {
    path: SharedString,
    name: SharedString,
    depth: usize,
    directory: bool,
    expanded: bool,
    status: Option<GitStatus>,
}

/// Emitted when a file row is chosen.
#[derive(Clone, Debug)]
pub struct SelectFile(pub SharedString);

/// The tree view. Feed it entries with [`DiffFileTree::set_entries`].
pub struct DiffFileTree {
    entries: Vec<DiffFileTreeEntry>,
    roots: Vec<Node>,
    collapsed: HashSet<SharedString>,
    rows: Vec<VisibleRow>,
    selected: Option<SharedString>,
    cursor: Option<usize>,
    scheme: DiffColorScheme,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
}

impl EventEmitter<SelectFile> for DiffFileTree {}

impl DiffFileTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: Vec::new(),
            roots: Vec::new(),
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected: None,
            cursor: None,
            scheme: DiffColorScheme::RedGreen,
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_scheme(&mut self, scheme: DiffColorScheme) {
        self.scheme = scheme;
    }

    /// Replaces the entries, keeping folded folders and the selection where they still exist.
    pub fn set_entries(&mut self, entries: Vec<DiffFileTreeEntry>, cx: &mut Context<Self>) {
        if entries == self.entries {
            return;
        }
        self.entries = entries;
        self.roots = build_tree(&self.entries);
        self.rebuild_rows();
        cx.notify();
    }

    /// Selects and reveals a file, opening its folders.
    pub fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        let mut ancestor = String::new();
        for segment in path.split('/').take(path.split('/').count().saturating_sub(1)) {
            ancestor.push_str(segment);
            ancestor.push('/');
            self.collapsed.retain(|dir| !ancestor.starts_with(dir.as_ref()));
        }
        self.selected = Some(path.to_owned().into());
        self.rebuild_rows();
        if let Some(index) = self.rows.iter().position(|row| row.path.as_ref() == path) {
            self.cursor = Some(index);
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    pub fn file_count(&self) -> usize {
        self.entries.len()
    }

    fn all_expanded(&self) -> bool {
        self.collapsed.is_empty()
    }

    fn set_all_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.collapsed.clear();
        if !expanded {
            fn collect(nodes: &[Node], out: &mut HashSet<SharedString>) {
                for node in nodes.iter().filter(|n| n.status.is_none()) {
                    out.insert(node.path.clone());
                    collect(&node.children, out);
                }
            }
            collect(&self.roots, &mut self.collapsed);
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        fn walk(nodes: &[Node], depth: usize, collapsed: &HashSet<SharedString>, out: &mut Vec<VisibleRow>) {
            for node in nodes {
                let directory = node.status.is_none();
                let expanded = directory && !collapsed.contains(&node.path);
                out.push(VisibleRow {
                    path: node.path.clone(),
                    name: node.name.clone(),
                    depth,
                    directory,
                    expanded,
                    status: node.status,
                });
                if expanded {
                    walk(&node.children, depth + 1, collapsed, out);
                }
            }
        }
        self.rows.clear();
        walk(&self.roots, 0, &self.collapsed, &mut self.rows);
        self.cursor = self.cursor.map(|c| c.min(self.rows.len().saturating_sub(1)));
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else { return };
        self.cursor = Some(index);
        if row.directory {
            if !self.collapsed.remove(&row.path) {
                self.collapsed.insert(row.path.clone());
            }
            self.rebuild_rows();
        } else {
            self.selected = Some(row.path.clone());
            cx.emit(SelectFile(row.path));
        }
        cx.notify();
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        let next = match self.cursor {
            Some(cursor) => (cursor as isize + delta).clamp(0, self.rows.len() as isize - 1) as usize,
            None => 0,
        };
        self.cursor = Some(next);
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, cx);
    }

    fn expand_or_open(&mut self, _: &ExpandOrOpen, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor else { return };
        let row = self.rows[index].clone();
        if row.directory && !row.expanded {
            self.activate(index, cx);
        } else if row.directory {
            self.move_cursor(1, cx);
        } else {
            self.activate(index, cx);
        }
    }

    fn collapse_or_parent(&mut self, _: &CollapseOrParent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor else { return };
        let row = self.rows[index].clone();
        if row.directory && row.expanded {
            self.activate(index, cx);
        } else if let Some(parent) = self.rows[..index].iter().rposition(|r| r.directory && r.depth + 1 == row.depth) {
            self.cursor = Some(parent);
            self.scroll.scroll_to_item(parent, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.cursor {
            self.activate(index, cx);
        }
    }

    fn render_rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.app_theme();
        let (appearance, colors) = (theme.appearance, theme.colors);
        let fg = colors.text;
        let muted = colors.muted_foreground;
        range
            .filter_map(|index| self.rows.get(index).cloned().map(|row| (index, row)))
            .map(|(index, row)| {
                let status_color = row.status.map(|s| s.color(appearance, self.scheme));
                let selected = !row.directory && self.selected.as_ref() == Some(&row.path);
                let focused = self.cursor == Some(index) && self.focus_handle.is_focused(window);
                div()
                    .id(("tree-row", index))
                    .h(px(ROW_HEIGHT))
                    .mx(px(4.))
                    .pl(px(4. + INDENT * row.depth as f32))
                    .pr(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .text_color(status_color.unwrap_or(fg))
                    .when(selected, |el| el.bg(fg.opacity(0.12)))
                    .when(focused && !selected, |el| el.bg(fg.opacity(0.07)))
                    .hover(|el| el.bg(fg.opacity(if selected { 0.12 } else { 0.07 })))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.activate(index, cx);
                    }))
                    .child(
                        div().w(px(12.)).flex_none().when(row.directory, |el| {
                            el.child(
                                svg()
                                    .path(if row.expanded { "icons/diffs/chevron-down.svg" } else { "icons/diffs/chevron-right.svg" })
                                    .size(px(12.))
                                    .text_color(muted),
                            )
                        }),
                    )
                    .child(
                        svg()
                            .path(match (row.directory, row.expanded) {
                                (true, true) => "icons/diffs/folder-open.svg",
                                (true, false) => "icons/diffs/folder.svg",
                                _ => "icons/diffs/file.svg",
                            })
                            .size(px(14.))
                            .flex_none()
                            .text_color(status_color.unwrap_or(muted)),
                    )
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.name.clone()))
                    .when_some(row.status, |el, status| {
                        el.child(div().flex_none().font_weight(FontWeight::MEDIUM).text_size(px(11.)).child(status.letter()))
                    })
                    .into_any_element()
            })
            .collect()
    }
}

/// Builds the tree ordered like the patch, flattening single-child directory chains.
fn build_tree(entries: &[DiffFileTreeEntry]) -> Vec<Node> {
    let positions = diff_file_tree_positions(entries.iter().map(|e| e.path.as_ref()));
    let mut roots: Vec<Node> = Vec::new();
    for entry in entries {
        let segments: Vec<&str> = entry.path.split('/').collect();
        let mut level = &mut roots;
        let mut prefix = String::new();
        for (i, segment) in segments.iter().enumerate() {
            let last = i + 1 == segments.len();
            prefix.push_str(segment);
            if !last {
                prefix.push('/');
            }
            let position = level.iter().position(|n| n.path.as_ref() == prefix);
            let index = position.unwrap_or_else(|| {
                level.push(Node {
                    path: prefix.clone().into(),
                    name: (*segment).to_owned().into(),
                    status: last.then_some(entry.status),
                    children: Vec::new(),
                });
                level.len() - 1
            });
            level = &mut level[index].children;
        }
    }
    fn finish(nodes: &mut Vec<Node>, positions: &std::collections::HashMap<String, usize>, depth: usize) {
        for node in nodes.iter_mut() {
            while node.status.is_none() && node.children.len() == 1 && node.children[0].status.is_none() {
                let child = node.children.pop().expect("one child");
                node.name = format!("{}/{}", node.name, child.name).into();
                node.path = child.path;
                node.children = child.children;
            }
            finish(&mut node.children, positions, depth + 1);
        }
        nodes.sort_by_key(|node| (positions.get(node.path.as_ref()).copied().unwrap_or(usize::MAX), node.path.clone()));
    }
    finish(&mut roots, &positions, 0);
    roots
}

impl Render for DiffFileTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.app_theme().colors;
        let has_directories = self.rows.iter().any(|row| row.directory);
        let all_expanded = self.all_expanded();
        div()
            .id("diff-file-tree")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::expand_or_open))
            .on_action(cx.listener(Self::collapse_or_parent))
            .on_action(cx.listener(Self::confirm))
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.canvas)
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.6))
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(div().px(px(4.)).font_weight(FontWeight::MEDIUM).text_color(colors.text).child("Files"))
                    .child(div().ml_auto().child(self.entries.len().to_string()))
                    .when(has_directories, |el| {
                        el.child(
                            div()
                                .id("tree-toggle-all")
                                .size(px(24.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(8.))
                                .hover(|s| s.bg(colors.toolbar_control_hover))
                                .on_click(cx.listener(move |this, _, _, cx| this.set_all_expanded(!all_expanded, cx)))
                                .child(
                                    svg()
                                        .path(if all_expanded { "icons/diffs/chevrons-down-up.svg" } else { "icons/diffs/chevrons-up-down.svg" })
                                        .size(px(14.))
                                        .text_color(colors.muted_foreground),
                                ),
                        )
                    }),
            )
            .child(
                uniform_list("diff-file-tree-rows", self.rows.len(), cx.processor(Self::render_rows))
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .py(px(4.)),
            )
    }
}
