//! The Files tab's tree. Ports `FileBrowserPanel.tsx` with `@pierre/trees`
//! drawn natively in its compact density: 24px rows, 18.7px per level,
//! chevrons for folders and Pierre's per-type icons for files, folders with a
//! single folder inside shown as one `a/b` row, ignored entries muted.
//! Folders list lazily through `projects.listEntries` when expanded; typing in
//! the search field asks `projects.searchEntries` and shows only the matches
//! and their folders, like Pierre's `hide-non-matches` search.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
};

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton, MouseDownEvent, Pixels, Point, Render, ScrollStrategy, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, actions, anchored, deferred,
    component::input::{Input, InputEvent, InputState},
    div, prelude::*, px, svg, uniform_list,
};
use t3code_contracts::{ProjectEntry, ProjectEntryKind};

use crate::{
    ActiveAppTheme as _,
    components::{
        icons::{Icon, IconName, IconSize},
        ui::{
            button::{Button, ButtonSize, ButtonVariant},
            menu::{MenuItem, MenuPopup},
            tooltip::tooltip,
        },
    },
    file_context_menu::{FileContextMenuAction, FileContextMenuCapabilities, FileContextMenuItem, composer_file_mention, file_tree_menu_items},
    file_source::{FileError, SharedWorkspaceFiles},
    i18n::t,
    pierre_icons::{PierreIcon, pierre_icon, resolve_file_icon},
};

const CONTEXT: &str = "FileTree";
/// Pierre's compact density: 24px rows, spacing scaled by 0.8.
pub const ROW_HEIGHT: f32 = 24.;
const DENSITY: f32 = 0.8;
const ICON_WIDTH: f32 = 16.;
const ITEM_PADDING_X: f32 = 8. * DENSITY;
const ITEM_MARGIN_X: f32 = 2. * DENSITY;
const ROW_GAP: f32 = 6. * DENSITY;
const LEVEL_GAP: f32 = 8. * DENSITY;
/// The first level's spacing: half an icon (less the guide's half pixel) plus the level gap.
const FIRST_LEVEL: f32 = ICON_WIDTH / 2. - 0.5 + LEVEL_GAP;
/// Each further level adds the row gap, half an icon and the level gap.
const NEXT_LEVEL: f32 = ROW_GAP + ICON_WIDTH / 2. - 0.5 + LEVEL_GAP;
/// `useProjectPathSearch` asks for at most this many matches.
const SEARCH_LIMIT: u64 = 200;

actions!(file_tree, [SelectNext, SelectPrevious, ExpandOrOpen, CollapseOrParent, Confirm, SelectFirst, SelectLast, FocusSearch, CloseSearch]);

/// Registers the tree's key bindings.
pub fn init(cx: &mut App) {
    let mod_f = if cfg!(target_os = "macos") { "cmd-f" } else { "ctrl-f" };
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some(CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(CONTEXT)),
        KeyBinding::new("right", ExpandOrOpen, Some(CONTEXT)),
        KeyBinding::new("left", CollapseOrParent, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new(mod_f, FocusSearch, Some(CONTEXT)),
        KeyBinding::new("escape", CloseSearch, Some("FileTreeSearch")),
        KeyBinding::new("down", SelectNext, Some("FileTreeSearch")),
    ]);
}

/// What the tree asks its owner to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileTreeEvent {
    /// A file row was chosen.
    OpenFile(SharedString),
    /// A context menu row the tree does not handle itself.
    MenuAction { action: FileContextMenuAction, path: SharedString },
}

/// A file dragged out of the tree. Drop targets (the composer) accept this
/// and insert [`FileReferenceDrag::mention`].
#[derive(Clone, Debug)]
pub struct FileReferenceDrag {
    pub path: SharedString,
}

impl FileReferenceDrag {
    /// The composer text for the file: a markdown file link.
    pub fn mention(&self) -> String {
        composer_file_mention(&self.path)
    }
}

/// The chip that follows the pointer while dragging a file.
pub struct FileDragPreview {
    path: SharedString,
}

impl Render for FileDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let name: SharedString = self.path.rsplit('/').next().unwrap_or(&self.path).to_owned().into();
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .h(px(22.))
            .rounded(theme.radii.md)
            .border_1()
            .border_color(theme.colors.border)
            .bg(theme.colors.surface_overlay)
            .text_size(px(12.))
            .text_color(theme.colors.text)
            .child(pierre_icon(resolve_file_icon(&self.path), px(14.), theme.appearance, theme.colors.muted_foreground))
            .child(name)
    }
}

/// One listed entry, with the strings rows share and its icon resolved once.
#[derive(Clone, Debug)]
pub struct TreeEntry {
    pub path: SharedString,
    pub name: SharedString,
    pub directory: bool,
    pub ignored: bool,
    icon: Option<PierreIcon>,
}

impl TreeEntry {
    fn new(entry: ProjectEntry) -> Self {
        let directory = entry.kind == ProjectEntryKind::Directory;
        Self {
            name: name_of(&entry.path).to_owned().into(),
            icon: (!directory).then(|| resolve_file_icon(&entry.path)),
            ignored: entry.ignored == Some(true),
            path: entry.path.into(),
            directory,
        }
    }
}

/// One folder's listing.
#[derive(Clone)]
struct Listing {
    entries: Arc<[TreeEntry]>,
}

#[derive(Clone, Debug)]
struct Row {
    /// Workspace-relative path of the entry the row opens or toggles.
    path: SharedString,
    /// Shown name; flattened folders read `a/b`.
    name: SharedString,
    depth: u16,
    directory: bool,
    expanded: bool,
    ignored: bool,
    loading: bool,
    icon: Option<PierreIcon>,
}

/// The file tree view.
pub struct FileTree {
    files: SharedWorkspaceFiles,
    listings: HashMap<SharedString, Listing>,
    loading: HashSet<SharedString>,
    expanded: HashSet<SharedString>,
    /// Expand every folder that is not ignored as listings arrive.
    expand_all: bool,
    /// Every folder that is not ignored is open (cached at each rebuild).
    all_expanded: bool,
    rows: Vec<Row>,
    rows_dirty: bool,
    selected: Option<SharedString>,
    cursor: Option<usize>,
    search: Entity<InputState>,
    query: String,
    /// Search matches with their folders, or `None` while not searching.
    matches: Option<SearchMatches>,
    search_task: Option<Task<()>>,
    error: Option<SharedString>,
    menu: Option<(Point<Pixels>, SharedString, bool)>,
    capabilities: FileContextMenuCapabilities,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    hovered: bool,
    _subscriptions: Vec<Subscription>,
}

struct SearchMatches {
    entries: Vec<ProjectEntry>,
    truncated: bool,
    pending: bool,
}

impl EventEmitter<FileTreeEvent> for FileTree {}

impl Focusable for FileTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

fn name_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl FileTree {
    pub fn new(files: SharedWorkspaceFiles, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search files")));
        let subscription = cx.subscribe_in(&search, window, |this, input, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                let query = input.read(cx).value().to_string();
                if query != this.query {
                    this.set_query(query, cx);
                }
            }
        });
        let mut tree = Self {
            files,
            listings: HashMap::new(),
            loading: HashSet::new(),
            expanded: HashSet::new(),
            expand_all: false,
            all_expanded: false,
            rows: Vec::new(),
            rows_dirty: false,
            selected: None,
            cursor: None,
            search,
            query: String::new(),
            matches: None,
            search_task: None,
            error: None,
            menu: None,
            capabilities: FileContextMenuCapabilities::default(),
            scroll: UniformListScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            hovered: false,
            _subscriptions: vec![subscription],
        };
        tree.load(SharedString::default(), cx);
        tree
    }

    /// Which server-side open actions the context menu offers.
    pub fn set_capabilities(&mut self, capabilities: FileContextMenuCapabilities) {
        self.capabilities = capabilities;
    }

    /// Rows currently shown, for checks.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Paths of the shown rows, for checks.
    pub fn row_paths(&self) -> impl Iterator<Item = &SharedString> {
        self.rows.iter().map(|row| &row.path)
    }

    /// A listed folder's entries (`""` is the root), for galleries and breadcrumbs.
    pub fn folder_entries(&self, folder: &str) -> Option<Arc<[TreeEntry]>> {
        self.listings.get(folder).map(|listing| listing.entries.clone())
    }

    /// True while any folder listing or search is in flight.
    pub fn is_loading(&self) -> bool {
        !self.loading.is_empty() || self.matches.as_ref().is_some_and(|m| m.pending)
    }

    pub fn scroll_handle(&self) -> &UniformListScrollHandle {
        &self.scroll
    }

    fn load(&mut self, directory: SharedString, cx: &mut Context<Self>) {
        if !self.loading.insert(directory.clone()) {
            return;
        }
        let request = self.files.list_entries((!directory.is_empty()).then(|| directory.to_string()));
        let work = cx.background_spawn(request);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |tree, cx| tree.listing_loaded(directory, result, cx)).ok();
        })
        .detach();
        self.rebuild_rows();
        cx.notify();
    }

    fn listing_loaded(&mut self, directory: SharedString, result: Result<t3code_contracts::ProjectListEntriesResult, FileError>, cx: &mut Context<Self>) {
        self.loading.remove(&directory);
        match result {
            Ok(listing) => {
                let entries: Arc<[TreeEntry]> = listing.entries.into_iter().map(TreeEntry::new).collect();
                // A folder holding a single folder shows as one row; open the inner one too.
                let single_folder = (entries.len() == 1 && entries[0].directory).then(|| entries[0].path.clone());
                let expand_children: Vec<SharedString> = if self.expand_all {
                    entries.iter().filter(|e| e.directory && !e.ignored).map(|e| e.path.clone()).collect()
                } else {
                    single_folder.filter(|_| self.expanded.contains(&directory)).into_iter().collect()
                };
                self.listings.insert(directory, Listing { entries });
                for child in expand_children {
                    self.expanded.insert(child.clone());
                    if !self.listings.contains_key(&child) {
                        self.load(child, cx);
                    }
                }
                self.error = None;
            }
            Err(error) => self.error = Some(error.message.into()),
        }
        // Many listings can land in one frame (expand all); rows rebuild once, at render.
        self.rows_dirty = true;
        cx.notify();
    }

    /// Reloads every listed folder, as the refresh button does.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let folders: Vec<SharedString> = self.listings.keys().cloned().collect();
        for folder in folders {
            self.loading.remove(&folder);
            self.load(folder, cx);
        }
        if !self.query.trim().is_empty() {
            self.run_search(cx);
        }
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        if self.query.trim().is_empty() {
            self.matches = None;
            self.search_task = None;
            self.rebuild_rows();
            cx.notify();
            return;
        }
        self.run_search(cx);
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query: String = self.query.chars().take(256).collect();
        if let Some(matches) = &mut self.matches {
            matches.pending = true;
        } else {
            self.matches = Some(SearchMatches { entries: Vec::new(), truncated: false, pending: true });
        }
        let work = cx.background_spawn(self.files.search_entries(query, SEARCH_LIMIT));
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |tree, cx| {
                match result {
                    Ok(found) => {
                        tree.matches = Some(SearchMatches { entries: found.entries, truncated: found.truncated, pending: false });
                        tree.error = None;
                    }
                    Err(error) => {
                        tree.matches = Some(SearchMatches { entries: Vec::new(), truncated: false, pending: false });
                        tree.error = Some(error.message.into());
                    }
                }
                tree.cursor = None;
                tree.rebuild_rows();
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Opens every folder that is not ignored, loading as it goes, or closes them all.
    pub fn set_all_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.expand_all = expanded;
        if expanded {
            let folders: Vec<SharedString> = self
                .listings
                .values()
                .flat_map(|l| l.entries.iter())
                .filter(|e| e.directory && !e.ignored)
                .map(|e| e.path.clone())
                .collect();
            for folder in folders {
                self.expanded.insert(folder.clone());
                if !self.listings.contains_key(&folder) {
                    self.load(folder, cx);
                }
            }
        } else {
            self.expanded.clear();
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn all_expanded(&self) -> bool {
        self.listings
            .values()
            .flat_map(|l| l.entries.iter())
            .filter(|e| e.directory && !e.ignored)
            .all(|e| self.expanded.contains(&e.path))
    }

    fn rebuild_rows(&mut self) {
        self.rows_dirty = false;
        let mut rows = Vec::with_capacity(self.rows.len().max(64));
        match &self.matches {
            Some(matches) if !matches.pending || !matches.entries.is_empty() => {
                let (children, ignored) = search_tree(&matches.entries, &self.listings);
                let mut walk = SearchWalk { children: &children, ignored: &ignored, rows: &mut rows };
                walk.walk("", 0);
            }
            Some(_) => {}
            None => self.walk_listing("", 0, &mut rows),
        }
        self.rows = rows;
        self.cursor = self.cursor.map(|c| c.min(self.rows.len().saturating_sub(1)));
        self.all_expanded = self.expand_all || self.all_expanded();
        if let Some(path) = self.selected.clone() {
            self.sync_cursor(&path);
        }
    }

    fn walk_listing(&self, directory: &str, depth: u16, rows: &mut Vec<Row>) {
        let Some(listing) = self.listings.get(directory) else { return };
        for entry in listing.entries.iter() {
            if !entry.directory {
                rows.push(Row {
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    depth,
                    directory: false,
                    expanded: false,
                    ignored: entry.ignored,
                    loading: false,
                    icon: entry.icon,
                });
                continue;
            }
            // Flatten a chain of folders that each hold one folder.
            let mut inner = entry;
            let mut chain: Option<String> = None;
            while let Some(only) = self.listings.get(&inner.path).filter(|l| l.entries.len() == 1 && l.entries[0].directory) {
                inner = &only.entries[0];
                let name = chain.get_or_insert_with(|| entry.name.to_string());
                name.push('/');
                name.push_str(&inner.name);
            }
            let expanded = self.expanded.contains(&inner.path);
            rows.push(Row {
                path: inner.path.clone(),
                name: chain.map_or_else(|| entry.name.clone(), SharedString::from),
                depth,
                directory: true,
                expanded,
                ignored: entry.ignored,
                loading: expanded && self.loading.contains(&inner.path),
                icon: None,
            });
            if expanded {
                self.walk_listing(&inner.path, depth + 1, rows);
            }
        }
    }

    fn sync_cursor(&mut self, path: &str) {
        if let Some(index) = self.rows.iter().position(|row| row.path.as_ref() == path) {
            self.cursor = Some(index);
        }
    }

    /// Selects a file, opening and loading its folders, and scrolls it to the
    /// centre. Used when a file opens from elsewhere (a link, go to file).
    pub fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        self.selected = Some(path.to_owned().into());
        if self.matches.is_some() {
            self.query.clear();
            self.matches = None;
        }
        let mut ancestor = String::new();
        for segment in path.split('/').take(path.split('/').count().saturating_sub(1)) {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(segment);
            let folder = SharedString::from(ancestor.clone());
            self.expanded.insert(folder.clone());
            if !self.listings.contains_key(&folder) {
                self.load(folder, cx);
            }
        }
        self.rebuild_rows();
        if let Some(index) = self.rows.iter().position(|row| row.path.as_ref() == path) {
            self.cursor = Some(index);
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Opens a folder and its parents without changing the selection.
    pub fn expand_folder(&mut self, folder: &str, cx: &mut Context<Self>) {
        let mut path = String::new();
        for segment in folder.split('/') {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(segment);
            let folder = SharedString::from(path.clone());
            self.expanded.insert(folder.clone());
            if !self.listings.contains_key(&folder) {
                self.load(folder, cx);
            }
        }
        self.rebuild_rows();
        cx.notify();
    }

    /// Opens the context menu for a row at a window position, as a right click does.
    pub fn open_context_menu(&mut self, path: &str, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(index) = self.rows.iter().position(|row| row.path.as_ref() == path) {
            self.cursor = Some(index);
            self.menu = Some((position, self.rows[index].path.clone(), self.rows[index].directory));
            cx.notify();
        }
    }

    /// The keyboard cursor row, for checks.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// Whether the context menu is open, for checks.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// Clicks a row by path, for scripted checks.
    pub fn activate_path(&mut self, path: &str, cx: &mut Context<Self>) {
        if let Some(index) = self.rows.iter().position(|row| row.path.as_ref() == path) {
            self.activate(index, cx);
        }
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index).cloned() else { return };
        self.menu = None;
        self.cursor = Some(index);
        if row.directory {
            if self.expanded.remove(&row.path) {
                self.expand_all = false;
            } else {
                self.expanded.insert(row.path.clone());
                if !self.listings.contains_key(&row.path) {
                    self.load(row.path.clone(), cx);
                }
            }
            self.rebuild_rows();
        } else {
            self.selected = Some(row.path.clone());
            cx.emit(FileTreeEvent::OpenFile(row.path));
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

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        self.move_cursor(1, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(isize::MIN / 2, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(isize::MAX / 2, cx);
    }

    fn expand_or_open(&mut self, _: &ExpandOrOpen, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor else { return };
        let row = &self.rows[index];
        if row.directory && row.expanded {
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

    fn focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.focus(window, cx));
    }

    fn close_search(&mut self, _: &CloseSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.set_value("", window, cx));
        self.set_query(String::new(), cx);
        window.focus(&self.focus_handle, cx);
    }

    /// Types into the search field, as typing into the focused tree does.
    pub fn type_to_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = format!("{}{text}", self.query);
        self.search.update(cx, |input, cx| {
            input.set_value(value.clone(), window, cx);
            input.focus(window, cx);
        });
        self.set_query(value, cx);
    }

    fn on_key_down(&mut self, event: &gpui_kit::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.key == "escape" && self.menu.take().is_some() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        if let Some(text) = keystroke.key_char.as_ref().filter(|c| c.chars().all(|c| !c.is_control() && c != ' ')) {
            self.type_to_filter(text, window, cx);
            cx.stop_propagation();
        }
    }

    fn run_menu_action(&mut self, action: FileContextMenuAction, path: SharedString, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            FileContextMenuAction::CopyMention => cx.write_to_clipboard(ClipboardItem::new_string(composer_file_mention(&path))),
            action => cx.emit(FileTreeEvent::MenuAction { action, path }),
        }
        cx.notify();
    }

    fn menu_popup(&self, path: SharedString, directory: bool, cx: &mut Context<Self>) -> MenuPopup {
        let tree = cx.entity().downgrade();
        let items = file_tree_menu_items(&self.capabilities, true);
        let items: Vec<FileContextMenuItem> = if directory {
            items.into_iter().filter(|i| !matches!(i.action, Some(FileContextMenuAction::OpenLocally))).collect()
        } else {
            items
        };
        let mut popup = MenuPopup::new();
        for item in items {
            if item.children.is_empty() {
                popup = popup.item(menu_item(&item, &tree, &path));
            } else {
                popup = popup.label(item.label.clone());
                for child in &item.children {
                    popup = popup.item(menu_item(child, &tree, &path).inset());
                }
            }
        }
        let dismiss = tree.clone();
        popup.on_dismiss(move |_, cx| {
            dismiss.update(cx, |tree, cx| {
                tree.menu = None;
                cx.notify();
            })
            .ok();
        })
    }

    fn render_rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.app_theme();
        let appearance = theme.appearance;
        let fg = theme.colors.text;
        let muted = theme.colors.muted_foreground;
        let ring = theme.colors.ring();
        let radius = theme.radii.md;
        let focused = self.focus_handle.is_focused(window);
        let hovered_tree = self.hovered;
        let guide = muted.opacity(0.25 * 0.75);
        range
            .filter_map(|index| self.rows.get(index).map(|row| (index, row)))
            .map(|(index, row)| {
                let selected = !row.directory && self.selected.as_ref() == Some(&row.path);
                let at_cursor = focused && self.cursor == Some(index);
                let text_color = if row.ignored { muted } else { fg };
                let path = row.path.clone();
                let directory = row.directory;
                let indent = indent_width(row.depth);
                let mut el = div()
                    .id(("file-tree-row", index))
                    .relative()
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px(px(ITEM_PADDING_X))
                    .flex()
                    .items_center()
                    .gap(px(ROW_GAP))
                    .rounded(radius)
                    .text_size(px(12.))
                    .text_color(text_color)
                    .cursor_pointer()
                    .when(selected, |el| el.bg(fg.opacity(0.12)))
                    .when(!selected, |el| el.hover(|el| el.bg(fg.opacity(0.07))))
                    .when(at_cursor, |el| el.border_1().border_color(ring))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        this.activate(index, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.cursor = Some(index);
                            this.menu = Some((event.position, path.clone(), directory));
                            cx.notify();
                        }),
                    );
                if !row.directory {
                    let drag = FileReferenceDrag { path: row.path.clone() };
                    el = el.on_drag(drag, |drag, _, _, cx| cx.new(|_| FileDragPreview { path: drag.path.clone() }));
                }
                if row.depth > 0 {
                    el = el.child(div().w(px(indent - ROW_GAP)).h_full().flex_none().relative().when(hovered_tree, |el| {
                        el.children((0..row.depth).map(|level| {
                            div().absolute().top_0().bottom_0().left(px(guide_x(level))).w(px(1.)).bg(guide)
                        }))
                    }));
                }
                let icon: AnyElement = match row.icon {
                    Some(icon) => div()
                        .when(row.ignored, |el| el.opacity(0.5))
                        .child(pierre_icon(icon, px(ICON_WIDTH), appearance, muted))
                        .into_any_element(),
                    None => svg()
                        .path("icons/files/chevron.svg")
                        .size(px(ICON_WIDTH))
                        .flex_none()
                        .text_color(muted)
                        .when(!row.expanded, |el| el.with_transformation(gpui_kit::Transformation::rotate(gpui_kit::radians(-std::f32::consts::FRAC_PI_2))))
                        .into_any_element(),
                };
                let row = el
                    .child(icon)
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(row.name.clone()))
                    .when(row.loading, |el| {
                        el.child(Icon::new(IconName::LoaderCircle).size(IconSize::Custom(px(ICON_WIDTH))).color(muted).opacity(0.6))
                    });
                div().w_full().px(px(ITEM_MARGIN_X)).child(row.test_support()).into_any_element()
            })
            .collect()
    }
}

fn menu_item(item: &FileContextMenuItem, tree: &gpui_kit::WeakEntity<FileTree>, path: &SharedString) -> MenuItem {
    let mut menu_item = MenuItem::new(item.label.clone());
    if let Some(icon) = item.icon {
        menu_item = menu_item.icon(icon);
    }
    if let Some(action) = item.action.clone() {
        let tree = tree.clone();
        let path = path.clone();
        menu_item = menu_item.on_select(move |_, cx| {
            tree.update(cx, |tree, cx| tree.run_menu_action(action.clone(), path.clone(), cx)).ok();
        });
    }
    menu_item
}

/// Left padding before a row's icon at `depth`.
fn indent_width(depth: u16) -> f32 {
    if depth == 0 { 0. } else { FIRST_LEVEL + NEXT_LEVEL * (depth - 1) as f32 + ROW_GAP }
}

/// x of the indent guide for `level`, inside the indent box.
fn guide_x(level: u16) -> f32 {
    ICON_WIDTH / 2. - 0.5 + NEXT_LEVEL * level as f32
}

/// Folder → children for the search view: every match plus its folders, with
/// listed folders keeping their ignored flag.
fn search_tree<'a>(
    matches: &'a [ProjectEntry],
    listings: &HashMap<SharedString, Listing>,
) -> (HashMap<String, Vec<(String, bool)>>, HashSet<String>) {
    let mut children: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut ignored = HashSet::new();
    for listing in listings.values() {
        for entry in listing.entries.iter().filter(|e| e.ignored) {
            ignored.insert(entry.path.to_string());
        }
    }
    for entry in matches {
        if entry.ignored == Some(true) {
            ignored.insert(entry.path.clone());
        }
        let mut path = entry.path.as_str();
        let mut directory = entry.kind == ProjectEntryKind::Directory;
        while seen.insert(path.to_owned()) {
            children.entry(parent_of(path).to_owned()).or_default().push((path.to_owned(), directory));
            if !path.contains('/') {
                break;
            }
            path = parent_of(path);
            directory = true;
        }
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| (!a.1, a.0.to_lowercase()).cmp(&(!b.1, b.0.to_lowercase())));
    }
    (children, ignored)
}

struct SearchWalk<'a> {
    children: &'a HashMap<String, Vec<(String, bool)>>,
    ignored: &'a HashSet<String>,
    rows: &'a mut Vec<Row>,
}

impl SearchWalk<'_> {
    fn walk(&mut self, directory: &str, depth: u16) {
        let Some(list) = self.children.get(directory) else { return };
        for (path, is_directory) in list {
            let mut path = path.as_str();
            let mut name = name_of(path).to_owned();
            if *is_directory {
                while let Some(inner) = self.children.get(path).filter(|c| c.len() == 1 && c[0].1) {
                    path = &inner[0].0;
                    name.push('/');
                    name.push_str(name_of(path));
                }
            }
            self.rows.push(Row {
                path: path.to_owned().into(),
                name: name.into(),
                depth,
                directory: *is_directory,
                expanded: *is_directory,
                ignored: self.ignored.contains(path),
                loading: false,
                icon: (!is_directory).then(|| resolve_file_icon(path)),
            });
            if *is_directory {
                self.walk(path, depth + 1);
            }
        }
    }
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.menu.clone().map(|(position, path, directory)| {
            let popup = self.menu_popup(path, directory, cx);
            deferred(anchored().position(position).snap_to_window_with_margin(px(8.)).child(popup)).with_priority(2)
        });
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        if self.rows_dirty {
            self.rebuild_rows();
        }
        let has_directories = self.listings.get("").is_some_and(|l| l.entries.iter().any(|e| e.directory));
        let all_expanded = has_directories && self.all_expanded;
        let searching = self.matches.is_some();
        let pending = self.matches.as_ref().is_some_and(|m| m.pending) || !self.listings.contains_key("");
        let no_matches = self.matches.as_ref().is_some_and(|m| !m.pending && m.entries.is_empty());
        let truncated = self.matches.as_ref().is_some_and(|m| m.truncated && !m.pending);
        let search_focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let expand_label = if all_expanded { t!("Collapse all folders") } else { t!("Expand all folders") };
        let refresh_label = if self.is_loading() { t!("Refreshing…") } else { t!("Refresh files") };
        div()
            .id("file-tree")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::expand_or_open))
            .on_action(cx.listener(Self::collapse_or_parent))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::focus_search))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
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
                    .child(
                        div().id("file-tree-refresh-tip").tooltip(tooltip(refresh_label)).child(
                            Button::new("file-tree-refresh")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::IconXs)
                                .icon(IconName::RefreshCw)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        ),
                    )
                    .child(
                        div()
                            .id("file-tree-search")
                            .key_context("FileTreeSearch")
                            .on_action(cx.listener(Self::close_search))
                            .on_action(cx.listener(Self::select_next))
                            .flex_1()
                            .min_w_0()
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(theme.radii.lg)
                            .when(search_focused, |el| el.bg(colors.canvas))
                            .when(!search_focused, |el| el.hover(|el| el.bg(colors.muted.opacity(0.4))))
                            .text_size(px(12.))
                            .child(Input::new(&self.search).appearance(false).bordered(false).focus_bordered(false).p_0().h(px(16.)).text_size(px(12.)).w_full()),
                    )
                    .when(has_directories, |el| {
                        el.child(
                            div()
                                .id("file-tree-toggle-all")
                                .tooltip(tooltip(expand_label.clone()))
                                .child(
                                    Button::new("file-tree-toggle-all-button")
                                        .variant(ButtonVariant::Ghost)
                                        .size(ButtonSize::IconXs)
                                        .icon(if all_expanded { IconName::ChevronsDownUp } else { IconName::ChevronsUpDown })
                                        .on_click(cx.listener(move |this, _, _, cx| this.set_all_expanded(!all_expanded, cx))),
                                ),
                        )
                    })
                    ,
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .id("file-tree-error")
                        .p(px(16.))
                        .text_size(px(12.))
                        .text_color(colors.destructive())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                        .child(t!("{error} Click to retry.", error = error)),
                )
            })
            .when(truncated, |el| {
                el.child(
                    div()
                        .px(px(12.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("More matches available. Refine your search.")),
                )
            })
            .when(pending && self.rows.is_empty(), |el| {
                el.child(div().px(px(12.)).py(px(4.)).text_size(px(12.)).text_color(colors.muted_foreground).child(t!("Loading files…")))
            })
            .when(no_matches, |el| {
                el.child(div().px(px(12.)).py(px(16.)).text_size(px(12.)).text_color(colors.muted_foreground).child(t!("No matching files.")))
            })
            .when(!no_matches, |el| {
                el.child(
                    uniform_list("file-tree-rows", self.rows.len(), cx.processor(Self::render_rows))
                        .track_scroll(&self.scroll)
                        .flex_1()
                        .when(searching, |el| el.pt(px(2.))),
                )
            })
            .children(menu)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.menu.is_some() {
                    this.menu = None;
                    cx.notify();
                }
            }))
    }
}
