//! The Files tab of the right panel. Ports `FilePreviewPanel.tsx` and
//! `FileExplorerPane.tsx`: the open file on the left with its breadcrumb bar
//! and actions, the file tree in a resizable pane on the right (the whole
//! width when no file is open).
//!
//! Text opens in the editor and saves itself 500ms after the last edit, as
//! the web's `FileSaveCoordinator` does; Mod+S saves at once. Leaving a file
//! writes its edits at once (`fileSaveSessions`); edits not yet written stay
//! as that file's draft, so reopening it shows them and writes them again.
//! Files over 1 MB open read-only in the code view; markdown and CSV toggle
//! between source and rendered; images get the zoomable viewer; every other
//! kind offers to open it externally.

use std::{collections::HashMap, sync::Arc, time::Duration};

use gpui_kit::{rems, 
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, KeyBinding, MouseButton, StyleRefinement, MouseMoveEvent, ParentElement as _, Render, actions, SharedString, Styled as _,
    Subscription, Task, Window, div, prelude::*, px,
};
use t3code_contracts::{EditorId, ProjectFileFailure};

use super::{
    delimited_table::{DelimitedTable, parse_delimited},
    file_editor::{FileEditor, FileEditorEvent},
    file_tree::{FileTree, FileTreeEvent},
    preview_image::{ExpandImage, PreviewImage},
    project_file_picker::{FilePickerEvent, ProjectFilePicker},
};
use crate::components::chat::open_in_picker::editor_icon;
use crate::components::search::{
    palette_shell::palette_overlay,
    project_content_search::{ContentSearchEvent, ProjectContentSearch},
};
use crate::{
    ActiveAppTheme as _,
    components::{
        chat::{
            chat_style::ChatColors,
            expanded_image_dialog::{ExpandedImageDialog, ExpandedImageEvent, ExpandedImageItem},
            markdown_stream_blocks::parse_message_blocks,
        },
        chat_markdown::{MarkdownAction, MarkdownUiState, MarkdownView, render_markdown},
        diffs::{CodeView, ViewMode},
        icons::{Icon, IconName, IconSize},
        media::{
            media_actions::{MediaActionSource, MediaKind, media_actions_menu},
            media_source::{MediaBytes, load_image},
            video_preview::ExternalFileCard,
        },
        ui::{
            button::{Button, ButtonSize, ButtonVariant},
            DISABLED_OPACITY,
            menu::{MenuItem, MenuPopup, dropdown_menu},
            spinner::{Spinner, SpinnerSize},
            toggle::{Toggle, ToggleSize, ToggleVariant},
            tooltip::tooltip,
        },
    },
    diff_panel_store::DiffPanelStore,
    editor_preferences::{EDITORS, LastEditor, Platform, editor_label as platform_editor_label, effective_editor},
    file_context_menu::{FileContextMenuAction, FileContextMenuCapabilities, RevealKind, composer_file_mention},
    file_preview::{FilePreviewKind, LARGE_TEXT_PREVIEW_MAX_BYTES, decode_file_preview_text, file_preview_delimiter, workspace_preview_kind},
    file_source::{FileError, SharedWorkspaceFiles, absolute_path},
    i18n::t,
    markdown_pipeline::{MarkdownFlavor, ParsedBlocks},
};

actions!(files_panel, [GoToFile, SearchContents]);

/// Registers Mod+P and Mod+Shift+F inside the Files tab. The app binds the
/// same actions globally and forwards them with [`FilesPanel::open_file_picker`]
/// and [`FilesPanel::open_content_search`].
pub fn init(cx: &mut App) {
    let (p, f) = if cfg!(target_os = "macos") { ("cmd-p", "cmd-shift-f") } else { ("ctrl-p", "ctrl-shift-f") };
    cx.bind_keys([KeyBinding::new(p, GoToFile, Some("FilesPanel")), KeyBinding::new(f, SearchContents, Some("FilesPanel"))]);
}

/// The open palette dialog.
enum Palette {
    Files(Entity<ProjectFilePicker>, #[allow(dead_code)] Subscription),
    Contents(Entity<ProjectContentSearch>, #[allow(dead_code)] Subscription),
}

/// `FileSaveCoordinator`'s debounce.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
const EXPLORER_DEFAULT_WIDTH: f32 = 256.;
const EXPLORER_MIN_WIDTH: f32 = 160.;
const EXPLORER_MAX_WIDTH: f32 = 560.;
const FILE_CONTENT_MIN_WIDTH: f32 = 256.;

/// What the panel asks the rest of the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesPanelEvent {
    /// Insert this mention at the end of the composer ("Add to chat").
    AddToChat(String),
    /// `shell.openInEditor` on the server: an editor, or the file manager to reveal.
    OpenInEditor { absolute_path: String, editor: EditorId, reveal: bool },
    /// "Open file in preview browser" (`openFileInPreview`): an HTML or PDF file
    /// of the workspace, by absolute path, in the thread's preview browser.
    OpenInPreview { absolute_path: String },
}

/// Where a text file's saving stands. A failed write stays `Pending`: its
/// draft is kept and written again on the next edit or when the file reopens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveState {
    Saved,
    /// Edited; a save is scheduled, or the last write failed.
    Pending,
    Saving,
}

struct TextFile {
    editor: Entity<FileEditor>,
    save: SaveState,
    /// Another save was asked for while one was in flight.
    resave: bool,
    table: Option<Entity<DelimitedTable>>,
    markdown: Option<Arc<Vec<ParsedBlocks>>>,
    _timer: Option<Task<()>>,
    _subscription: Subscription,
}

enum FileView {
    Loading,
    Failed { message: SharedString },
    Text(TextFile),
    /// Over 1 MB: read-only, the whole file through its asset URL.
    Large(Entity<CodeView>),
    Image { view: Entity<PreviewImage>, _subscription: Subscription },
    External { size: Option<u64> },
}

struct OpenFile {
    path: SharedString,
    kind: FilePreviewKind,
    delimiter: Option<u8>,
    view: FileView,
    /// Markdown or CSV drawn rendered instead of as source.
    rendered: bool,
    reveal: Option<u32>,
    _load: Option<Task<()>>,
}

/// The Files tab view. Mount it in the right panel's Files slot.
pub struct FilesPanel {
    files: SharedWorkspaceFiles,
    project_name: SharedString,
    tree: Entity<FileTree>,
    explorer_open: bool,
    explorer_width: f32,
    resizing: bool,
    open: Option<OpenFile>,
    /// Edits not yet written, by path (`getUnsavedProjectFileQueryData`).
    drafts: HashMap<SharedString, Arc<str>>,
    /// Paths with a write in flight.
    writing: Vec<SharedString>,
    /// The word-wrap toggle, over the `wordWrap` setting.
    wrap_override: Option<bool>,
    expanded: Option<(Entity<ExpandedImageDialog>, Subscription)>,
    /// The open image's or video's right-click menu (`MediaActions`), where it opened.
    media_menu: Option<gpui_kit::Point<gpui_kit::Pixels>>,
    pending_gallery: Option<(Vec<ExpandedImageItem>, usize)>,
    palette: Option<Palette>,
    markdown_state: MarkdownUiState,
    render_markdown_preferred: bool,
    render_table_preferred: bool,
    capabilities: FileContextMenuCapabilities,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FilesPanelEvent> for FilesPanel {}

impl Focusable for FilesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FilesPanel {
    pub fn new(files: SharedWorkspaceFiles, project_name: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| FileTree::new(files.clone(), window, cx));
        let subscription = cx.subscribe_in(&tree, window, |this, _, event: &FileTreeEvent, window, cx| match event {
            FileTreeEvent::OpenFile(path) => this.open_file(path.clone(), None, window, cx),
            FileTreeEvent::MenuAction { action, path } => this.run_file_action(action.clone(), path.clone(), cx),
        });
        Self {
            files,
            project_name,
            tree,
            explorer_open: true,
            explorer_width: EXPLORER_DEFAULT_WIDTH,
            resizing: false,
            open: None,
            drafts: HashMap::new(),
            writing: Vec::new(),
            wrap_override: None,
            expanded: None,
            media_menu: None,
            pending_gallery: None,
            palette: None,
            markdown_state: MarkdownUiState::default(),
            render_markdown_preferred: false,
            render_table_preferred: true,
            capabilities: FileContextMenuCapabilities::default(),
            focus_handle: cx.focus_handle(),
            _subscriptions: vec![subscription],
        }
    }

    /// Server editors and reveal support, from the server config.
    pub fn set_capabilities(&mut self, capabilities: FileContextMenuCapabilities, cx: &mut Context<Self>) {
        self.tree.update(cx, |tree, _| tree.set_capabilities(capabilities.clone()));
        self.capabilities = capabilities;
        cx.notify();
    }

    pub fn tree(&self) -> &Entity<FileTree> {
        &self.tree
    }

    pub fn open_path(&self) -> Option<&SharedString> {
        self.open.as_ref().map(|open| &open.path)
    }

    /// The editor of the open text file, for checks and for focus.
    pub fn editor(&self) -> Option<&Entity<FileEditor>> {
        match self.open.as_ref().map(|o| &o.view) {
            Some(FileView::Text(text)) => Some(&text.editor),
            _ => None,
        }
    }

    pub fn save_state(&self) -> Option<&SaveState> {
        match self.open.as_ref().map(|o| &o.view) {
            Some(FileView::Text(text)) => Some(&text.save),
            _ => None,
        }
    }

    pub fn expanded_dialog(&self) -> Option<&Entity<ExpandedImageDialog>> {
        self.expanded.as_ref().map(|(dialog, _)| dialog)
    }

    pub fn image_view(&self) -> Option<&Entity<PreviewImage>> {
        match self.open.as_ref().map(|o| &o.view) {
            Some(FileView::Image { view: image, .. }) => Some(image),
            _ => None,
        }
    }

    pub fn table_view(&self) -> Option<&Entity<DelimitedTable>> {
        match self.open.as_ref().map(|o| &o.view) {
            Some(FileView::Text(text)) => text.table.as_ref(),
            _ => None,
        }
    }

    pub fn large_view(&self) -> Option<&Entity<CodeView>> {
        match self.open.as_ref().map(|o| &o.view) {
            Some(FileView::Large(view)) => Some(view),
            _ => None,
        }
    }

    /// True while the open file is still loading.
    pub fn is_loading(&self) -> bool {
        matches!(self.open.as_ref().map(|o| &o.view), Some(FileView::Loading))
    }

    pub fn is_rendered(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.rendered)
    }

    pub fn set_explorer_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.explorer_open = open;
        cx.notify();
    }

    /// Whether text files wrap: the toggle, else the `wordWrap` setting.
    pub fn word_wrap(&self, cx: &App) -> bool {
        self.wrap_override.unwrap_or_else(|| cx.try_global::<DiffPanelStore>().is_none_or(|store| store.settings.word_wrap))
    }

    /// The word-wrap toggle; it lasts while the panel does, as the web's does.
    pub fn set_word_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        self.wrap_override = Some(wrap);
        if let Some(FileView::Text(text)) = self.open.as_ref().map(|o| &o.view) {
            text.editor.update(cx, |editor, cx| editor.set_wrap(wrap, cx));
        }
        cx.notify();
    }

    /// True while any file has edits not yet written (the tab's pending marker).
    pub fn has_unsaved(&self) -> bool {
        !self.drafts.is_empty() || matches!(self.save_state(), Some(SaveState::Pending | SaveState::Saving))
    }

    /// Opens a workspace file, revealing a 1-based line when given. Edits to
    /// the file being left are written at once and kept as its draft until
    /// the write lands.
    pub fn open_file(&mut self, path: SharedString, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.open
            && open.path == path
        {
            if let Some(line) = line {
                open.reveal = Some(line);
                self.apply_reveal(cx);
            }
            return;
        }
        if let Some(open) = &self.open
            && let FileView::Text(text) = &open.view
            && text.editor.read(cx).is_dirty()
        {
            let contents: Arc<str> = text.editor.read(cx).text().into();
            let left = open.path.clone();
            self.drafts.insert(left.clone(), contents.clone());
            self.write(left, contents, cx);
        }
        self.load_file(path, line, window, cx);
    }

    fn load_file(&mut self, path: SharedString, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let kind = match workspace_preview_kind(&path) {
            FilePreviewKind::Html | FilePreviewKind::Mermaid => FilePreviewKind::Text,
            kind => kind,
        };
        let delimiter = file_preview_delimiter(&path);
        let rendered = match (kind, delimiter) {
            (_, Some(_)) => self.render_table_preferred,
            (FilePreviewKind::Markdown, _) => self.render_markdown_preferred,
            _ => false,
        };
        self.tree.update(cx, |tree, cx| tree.reveal(&path, cx));
        let mut open = OpenFile { path: path.clone(), kind, delimiter, view: FileView::Loading, rendered, reveal: line, _load: None };
        open._load = Some(match kind {
            FilePreviewKind::Image => self.load_image(path, cx),
            FilePreviewKind::Text | FilePreviewKind::Markdown => self.load_text(path, window, cx),
            _ => {
                open.view = FileView::External { size: None };
                self.load_size(path, cx)
            }
        });
        self.open = Some(open);
        cx.notify();
    }

    fn load_image(&mut self, path: SharedString, cx: &mut Context<Self>) -> Task<()> {
        let bytes = MediaBytes::Workspace { files: self.files.clone(), path: path.clone() };
        let task = load_image(&bytes, path.clone(), cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |panel, cx| {
                let Some(open) = panel.open.as_mut().filter(|o| o.path == path) else { return };
                open.view = match result {
                    Ok(decoded) => {
                        let image = cx.new(|_| PreviewImage::new(decoded, path.clone()));
                        let subscription = cx.subscribe(&image, |panel, _, _: &ExpandImage, cx| panel.expand_open_image(cx));
                        FileView::Image { view: image, _subscription: subscription }
                    }
                    Err(_) => FileView::Failed { message: t!("Unable to load image.") },
                };
                cx.notify();
            })
            .ok();
        })
    }

    fn load_size(&mut self, path: SharedString, cx: &mut Context<Self>) -> Task<()> {
        let read = self.files.read_file(path.to_string());
        let work = cx.background_spawn(read);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |panel, cx| {
                if let Some(open) = panel.open.as_mut().filter(|o| o.path == path)
                    && let Ok(file) = result
                {
                    open.view = FileView::External { size: Some(file.byte_length) };
                    cx.notify();
                }
            })
            .ok();
        })
    }

    fn load_text(&mut self, path: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        let read = self.files.read_file(path.to_string());
        let work = cx.background_spawn(read);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let large = match &result {
                Ok(file) if file.truncated => Some(file.byte_length),
                _ => None,
            };
            if let Some(byte_length) = large {
                let fetch = this.update(cx, |panel, _| panel.files.read_asset(path.to_string())).ok();
                let decoded = match fetch {
                    Some(fetch) if byte_length <= LARGE_TEXT_PREVIEW_MAX_BYTES => cx
                        .background_spawn(async move {
                            let asset = fetch.await?;
                            decode_file_preview_text(&asset.bytes, usize::MAX)
                                .map(|(text, _)| text)
                                .map_err(|error| FileError::new(error.message().to_string()))
                        })
                        .await,
                    _ => Err(FileError::new(t!("Text previews support files up to 256 MB.").to_string())),
                };
                cx.update_window(window_handle, |_, window, cx| {
                    this.update(cx, |panel, cx| panel.text_loaded_large(&path, decoded, window, cx)).ok();
                })
                .ok();
                return;
            }
            cx.update_window(window_handle, |_, window, cx| {
                this.update(cx, |panel, cx| panel.text_loaded(&path, result, window, cx)).ok();
            })
            .ok();
        })
    }

    fn text_loaded_large(&mut self, path: &SharedString, result: Result<String, FileError>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut().filter(|o| &o.path == path) else { return };
        open.view = match result {
            Ok(text) => {
                let view = cx.new(|cx| {
                    let mut view = CodeView::new(ViewMode::File, window, cx);
                    view.set_file_contents(path.clone(), &text, cx);
                    view
                });
                FileView::Large(view)
            }
            Err(error) => FileView::Failed { message: error.message.into() },
        };
        self.apply_reveal(cx);
        cx.notify();
    }

    fn text_loaded(&mut self, path: &SharedString, result: Result<t3code_contracts::ProjectReadFileResult, FileError>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut().filter(|o| &o.path == path) else { return };
        match result {
            Ok(file) => {
                let wrap = self.wrap_override.unwrap_or_else(|| cx.try_global::<DiffPanelStore>().is_none_or(|store| store.settings.word_wrap));
                let draft = self.drafts.get(path).cloned();
                let editor = cx.new(|cx| {
                    let mut editor = FileEditor::new(path.clone(), &file.contents, false, window, cx);
                    editor.set_wrap(wrap, cx);
                    if let Some(draft) = &draft {
                        editor.restore_draft(draft, cx);
                    }
                    editor
                });
                let subscription = cx.subscribe(&editor, |panel, _, event: &FileEditorEvent, cx| panel.on_editor_event(event, cx));
                let Some(open) = self.open.as_mut().filter(|o| &o.path == path) else { return };
                open.view = FileView::Text(TextFile {
                    editor,
                    save: if draft.is_some() { SaveState::Pending } else { SaveState::Saved },
                    resave: false,
                    table: None,
                    markdown: None,
                    _timer: None,
                    _subscription: subscription,
                });
                self.prepare_rendered(cx);
                self.apply_reveal(cx);
                if draft.is_some() {
                    // `recover`: the draft goes through the debounce again.
                    self.schedule_save(cx);
                }
            }
            Err(error) if error.failure == Some(ProjectFileFailure::BinaryFile) => {
                open.view = FileView::External { size: None };
            }
            Err(error) => open.view = FileView::Failed { message: read_error_message(&error) },
        }
        cx.notify();
    }

    /// Builds the table or parses the markdown for a rendered view.
    fn prepare_rendered(&mut self, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else { return };
        if !open.rendered {
            return;
        }
        let (delimiter, kind) = (open.delimiter, open.kind);
        let FileView::Text(text) = &mut open.view else { return };
        let contents = text.editor.read(cx).text();
        if let Some(delimiter) = delimiter {
            let rows = parse_delimited(&contents, delimiter);
            let byte_length = contents.len() as u64;
            text.table = Some(cx.new(|_| DelimitedTable::new(rows, byte_length)));
        } else if kind == FilePreviewKind::Markdown {
            text.markdown = Some(Arc::new(parse_message_blocks(&mask_frontmatter(&contents), MarkdownFlavor::ASSISTANT, false)));
        }
    }

    /// Switches between rendered and source for markdown and CSV.
    pub fn set_rendered(&mut self, rendered: bool, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else { return };
        open.rendered = rendered;
        if open.delimiter.is_some() {
            self.render_table_preferred = rendered;
        } else {
            self.render_markdown_preferred = rendered;
        }
        if let Some(FileView::Text(text)) = self.open.as_mut().map(|o| &mut o.view) {
            text.table = None;
            text.markdown = None;
        }
        self.prepare_rendered(cx);
        cx.notify();
    }

    fn apply_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else { return };
        let Some(line) = open.reveal else { return };
        match &open.view {
            FileView::Text(text) => {
                // A line reveal shows the source, where lines exist.
                if open.rendered {
                    open.rendered = false;
                }
                open.reveal = None;
                text.editor.update(cx, |editor, cx| editor.reveal_line(line, cx));
            }
            FileView::Large(view) => {
                open.reveal = None;
                view.update(cx, |view, cx| view.reveal_line(line, cx));
            }
            _ => {}
        }
        cx.notify();
    }

    fn on_editor_event(&mut self, event: &FileEditorEvent, cx: &mut Context<Self>) {
        match event {
            FileEditorEvent::Changed { dirty: true } => self.schedule_save(cx),
            FileEditorEvent::Changed { dirty: false } => {
                if let Some(text) = self.text_mut()
                    && text.save == SaveState::Pending
                {
                    text.save = SaveState::Saved;
                    text._timer = None;
                }
                cx.notify();
            }
            FileEditorEvent::SaveRequested => self.save_now(cx),
        }
    }

    fn text_mut(&mut self) -> Option<&mut TextFile> {
        match self.open.as_mut().map(|o| &mut o.view) {
            Some(FileView::Text(text)) => Some(text),
            _ => None,
        }
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            this.update(cx, |panel, cx| panel.save_now(cx)).ok();
        });
        let Some(text) = self.text_mut() else { return };
        text._timer = Some(timer);
        // Re-render the bar only when the state changes, not on every keystroke.
        if !matches!(text.save, SaveState::Saving | SaveState::Pending) {
            text.save = SaveState::Pending;
            cx.notify();
        }
    }

    /// Writes the open file's text now, or after the write in flight.
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.open.as_ref().map(|o| o.path.clone()) else { return };
        let writing = self.writing.contains(&path);
        let Some(text) = self.text_mut() else { return };
        text._timer = None;
        if !text.editor.read(cx).is_dirty() {
            text.save = SaveState::Saved;
            cx.notify();
            return;
        }
        if writing {
            text.resave = true;
            return;
        }
        let contents: Arc<str> = text.editor.read(cx).text().into();
        text.save = SaveState::Saving;
        self.drafts.insert(path.clone(), contents.clone());
        self.write(path, contents, cx);
        cx.notify();
    }

    /// One write of `contents` to `path`; one at a time per file, as the web's
    /// coordinator queues them.
    fn write(&mut self, path: SharedString, contents: Arc<str>, cx: &mut Context<Self>) {
        if self.writing.contains(&path) {
            return;
        }
        self.writing.push(path.clone());
        let request = self.files.write_file(path.to_string(), contents.to_string());
        let work = cx.background_spawn(request);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |panel, cx| panel.write_finished(path, contents, result.is_ok(), cx)).ok();
        })
        .detach();
    }

    fn write_finished(&mut self, path: SharedString, contents: Arc<str>, written: bool, cx: &mut Context<Self>) {
        self.writing.retain(|p| p != &path);
        if written && self.drafts.get(&path) == Some(&contents) {
            self.drafts.remove(&path);
        }
        let open = self.open.as_ref().is_some_and(|o| o.path == path);
        if !open {
            // A draft that changed while this write ran goes out next.
            if written && let Some(draft) = self.drafts.get(&path).cloned() {
                self.write(path, draft, cx);
            }
            return;
        }
        let Some(text) = self.text_mut() else { return };
        if written {
            text.editor.update(cx, |editor, cx| editor.mark_saved(contents.to_string(), cx));
        }
        let again = std::mem::take(&mut text.resave) || (written && text.editor.read(cx).is_dirty());
        text.save = if !written || text.editor.read(cx).is_dirty() { SaveState::Pending } else { SaveState::Saved };
        if again {
            self.schedule_save(cx);
        }
        cx.notify();
    }

    fn expand_open_image(&mut self, cx: &mut Context<Self>) {
        let Some(open) = &self.open else { return };
        let FileView::Image { view: image, .. } = &open.view else { return };
        let decoded = image.read(cx).image().clone();
        let item = ExpandedImageItem {
            name: open.path.rsplit('/').next().unwrap_or(&open.path).to_owned().into(),
            kind: MediaKind::Image,
            bytes: MediaBytes::Ready(decoded.bytes.clone()),
            relative_path: Some(open.path.clone()),
            absolute_path: Some(absolute_path(self.files.root(), &open.path).into()),
        };
        let path = open.path.clone();
        let (items, index) = self.image_gallery(&path, item, cx);
        self.open_gallery(items, index, cx);
    }

    /// Images beside `path` in its folder's listing, for gallery navigation.
    fn image_gallery(&self, path: &SharedString, current: ExpandedImageItem, cx: &App) -> (Vec<ExpandedImageItem>, usize) {
        let folder = path.rfind('/').map_or("", |i| &path[..i]);
        let Some(entries) = self.tree.read(cx).folder_entries(folder) else { return (vec![current], 0) };
        let mut items = Vec::new();
        let mut index = 0;
        for entry in entries.iter().filter(|e| !e.directory) {
            if workspace_preview_kind(&entry.path) != FilePreviewKind::Image {
                continue;
            }
            if entry.path == *path {
                index = items.len();
                items.push(current.clone());
                continue;
            }
            let relative = entry.path.clone();
            items.push(ExpandedImageItem {
                name: entry.name.clone(),
                kind: MediaKind::Image,
                bytes: MediaBytes::Workspace { files: self.files.clone(), path: relative.clone() },
                absolute_path: Some(absolute_path(self.files.root(), &relative).into()),
                relative_path: Some(relative),
            });
        }
        if items.is_empty() { (vec![current], 0) } else { (items, index) }
    }

    /// Opens go to file (Mod+P).
    pub fn open_file_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.new(|cx| ProjectFilePicker::new(self.files.clone(), self.project_name.clone(), window, cx));
        let subscription = cx.subscribe_in(&picker, window, |this, _, event: &FilePickerEvent, window, cx| {
            this.palette = None;
            if let FilePickerEvent::Open(path) = event {
                this.open_file(path.clone(), None, window, cx);
            }
            cx.notify();
        });
        self.palette = Some(Palette::Files(picker, subscription));
        cx.notify();
    }

    /// Opens content search (Mod+Shift+F).
    pub fn open_content_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| ProjectContentSearch::new(self.files.clone(), self.project_name.clone(), window, cx));
        let subscription = cx.subscribe_in(&search, window, |this, _, event: &ContentSearchEvent, window, cx| {
            this.palette = None;
            if let ContentSearchEvent::Open { path, line } = event {
                this.open_file(path.clone(), Some(*line), window, cx);
            }
            cx.notify();
        });
        self.palette = Some(Palette::Contents(search, subscription));
        cx.notify();
    }

    pub fn file_picker(&self) -> Option<&Entity<ProjectFilePicker>> {
        match &self.palette {
            Some(Palette::Files(picker, _)) => Some(picker),
            _ => None,
        }
    }

    pub fn content_search(&self) -> Option<&Entity<ProjectContentSearch>> {
        match &self.palette {
            Some(Palette::Contents(search, _)) => Some(search),
            _ => None,
        }
    }

    /// Opens the expanded media viewer over everything on the next render.
    pub fn open_gallery(&mut self, items: Vec<ExpandedImageItem>, index: usize, cx: &mut Context<Self>) {
        self.pending_gallery = Some((items, index));
        cx.notify();
    }

    fn mount_gallery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((items, index)) = self.pending_gallery.take() else { return };
        let dialog = cx.new(|cx| ExpandedImageDialog::new(items, index, window, cx));
        let subscription = cx.subscribe_in(&dialog, window, |this, _, event: &ExpandedImageEvent, window, cx| match event {
            ExpandedImageEvent::Close => {
                this.expanded = None;
                cx.notify();
            }
            ExpandedImageEvent::OpenFile(path) => {
                this.expanded = None;
                this.open_file(path.clone(), None, window, cx);
            }
        });
        self.expanded = Some((dialog, subscription));
    }

    /// The open image or video with its `MediaActions` menu on right click: copy its full and
    /// relative path, save it, and copy an image.
    fn with_media_menu(&self, content: impl IntoElement, cx: &mut Context<Self>) -> AnyElement {
        let menu = self.media_menu.zip(self.open.as_ref()).map(|(at, open)| {
            let (kind, bytes, decoded) = match &open.view {
                FileView::Image { view, .. } => {
                    let decoded = view.read(cx).image().clone();
                    (MediaKind::Image, MediaBytes::Ready(decoded.bytes.clone()), Some(decoded))
                }
                _ => (MediaKind::Video, MediaBytes::Workspace { files: self.files.clone(), path: open.path.clone() }, None),
            };
            let source = MediaActionSource {
                kind,
                name: open.path.rsplit('/').next().unwrap_or(&open.path).to_owned().into(),
                relative_path: Some(open.path.clone()),
                absolute_path: Some(absolute_path(self.files.root(), &open.path).into()),
                bytes,
                decoded,
            };
            let this = cx.weak_entity();
            let close = move |_: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    this.media_menu = None;
                    cx.notify();
                })
                .ok();
            };
            crate::components::ui::pointer_menu::pointer_menu("files-media-menu", at, media_actions_menu(&source, None), close, cx)
        });
        div()
            .id("files-media")
            .size_full()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                    this.media_menu = Some(event.position);
                    cx.notify();
                }),
            )
            .child(content)
            .children(menu)
            .into_any_element()
    }

    /// What right clicks in its Markdown offer on the host (`live::host_menus::right_panel_menus`).
    pub fn set_markdown_menus(&mut self, menus: crate::components::chat_markdown::MarkdownMenus) {
        self.markdown_state.menus = menus;
    }

    /// Runs a file menu row for `path`; the diff panel's header menu uses it too.
    pub fn run_file_action(&mut self, action: FileContextMenuAction, path: SharedString, cx: &mut Context<Self>) {
        let absolute = absolute_path(self.files.root(), &path);
        match action {
            FileContextMenuAction::Open => {
                cx.emit(FilesPanelEvent::OpenInEditor { absolute_path: absolute, editor: EditorId::FileManager, reveal: false })
            }
            FileContextMenuAction::RevealInFolder => {
                cx.emit(FilesPanelEvent::OpenInEditor { absolute_path: absolute, editor: EditorId::FileManager, reveal: true })
            }
            FileContextMenuAction::OpenWith(editor) => {
                cx.emit(FilesPanelEvent::OpenInEditor { absolute_path: absolute, editor, reveal: false })
            }
            FileContextMenuAction::AddToChat => cx.emit(FilesPanelEvent::AddToChat(format!("{} ", composer_file_mention(&path)))),
            FileContextMenuAction::CopyMention => cx.write_to_clipboard(ClipboardItem::new_string(composer_file_mention(&path))),
        }
    }

    fn render_breadcrumbs(&self, path: &SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let segments: Vec<&str> = path.split('/').collect();
        let mut crumbs: Vec<AnyElement> = Vec::new();
        let tree = self.tree.clone();
        let separator = || Icon::new(IconName::ChevronRight).size(IconSize::Sm).color(colors.muted_foreground.opacity(0.6)).into_any_element();
        crumbs.push(
            div()
                .id("crumb-project")
                .px(rems(0.125))
                .rounded(theme.radii.sm)
                .text_color(colors.muted_foreground)
                .hover(|el| el.bg(colors.accent_surface).text_color(colors.text))
                .cursor_pointer()
                .child(self.project_name.clone())
                .into_any_element(),
        );
        for (index, segment) in segments.iter().enumerate() {
            crumbs.push(div().mx(rems(0.125)).flex().child(separator()).into_any_element());
            let last = index + 1 == segments.len();
            if last {
                crumbs.push(div().px(rems(0.125)).text_color(colors.text).whitespace_nowrap().child(segment.to_string()).into_any_element());
            } else {
                let folder: SharedString = segments[..=index].join("/").into();
                let tree = tree.clone();
                crumbs.push(
                    div()
                        .id(("crumb", index))
                        .max_w(rems(10.))
                        .px(rems(0.125))
                        .rounded(theme.radii.sm)
                        .text_color(colors.muted_foreground)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .hover(|el| el.bg(colors.accent_surface).text_color(colors.text))
                        .cursor_pointer()
                        .tooltip(tooltip(folder.clone()))
                        .on_click(move |_, _, cx| {
                            let folder = format!("{folder}/");
                            tree.update(cx, |tree, cx| tree.reveal(&folder, cx));
                        })
                        .child(segment.to_string())
                        .into_any_element(),
                );
            }
        }
        div()
            .id("file-breadcrumbs")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .flex()
            .items_center()
            .text_size(px(12.))
            .children(crumbs)
    }

    fn render_actions(&self, open: &OpenFile, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut actions = Vec::new();
        let path = open.path.clone();
        actions.push(self.render_open_in(&path, cx));
        if let Some(rendered_label) = rendered_toggle_label(open) {
            let icon = if open.rendered {
                IconName::Code
            } else if open.delimiter.is_some() {
                IconName::Table2
            } else {
                IconName::Eye
            };
            let panel = cx.entity().downgrade();
            let rendered = open.rendered;
            actions.push(
                div()
                    .id("file-rendered-toggle")
                    .tooltip(tooltip(rendered_label))
                    .child(Toggle::new("file-rendered").variant(ToggleVariant::Ghost).size(ToggleSize::Sm).icon(icon).pressed(rendered).on_toggle(move |_, _, cx| {
                        panel.update(cx, |panel, cx| panel.set_rendered(!rendered, cx)).ok();
                    }))
                    .into_any_element(),
            );
        }
        if matches!(open.view, FileView::Text(_)) && !open.rendered {
            let wrap = self.word_wrap(cx);
            let panel = cx.entity().downgrade();
            actions.push(
                div()
                    .id("file-wrap-toggle")
                    .tooltip(tooltip(if wrap { t!("Disable word wrap") } else { t!("Enable word wrap") }))
                    .child(Toggle::new("file-wrap").variant(ToggleVariant::Ghost).size(ToggleSize::Sm).icon(IconName::TextWrap).pressed(wrap).on_toggle(move |_, _, cx| {
                        panel.update(cx, |panel, cx| panel.set_word_wrap(!wrap, cx)).ok();
                    }))
                    .into_any_element(),
            );
        }
        if is_browser_preview_file(&path) {
            let absolute = absolute_path(self.files.root(), &path);
            actions.push(
                div()
                    .id("file-open-in-browser")
                    .tooltip(tooltip(t!("Open file in preview browser")))
                    .child(Button::new("file-open-in-browser-button").variant(ButtonVariant::Ghost).size(ButtonSize::IconSm).icon(IconName::Globe).on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(FilesPanelEvent::OpenInPreview { absolute_path: absolute.clone() });
                    })))
                    .into_any_element(),
            );
        }
        let explorer_label = if self.explorer_open { t!("Hide file explorer") } else { t!("Show file explorer") };
        let panel = cx.entity().downgrade();
        let explorer_open = self.explorer_open;
        actions.push(
            div()
                .id("file-explorer-toggle")
                .tooltip(tooltip(explorer_label))
                .child(Toggle::new("file-explorer").variant(ToggleVariant::Ghost).size(ToggleSize::Sm).icon(IconName::FolderTree).pressed(explorer_open).on_toggle(move |_, _, cx| {
                    panel.update(cx, |panel, cx| panel.set_explorer_open(!explorer_open, cx)).ok();
                }))
                .into_any_element(),
        );
        actions
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        let Some(open) = &self.open else { return div().into_any_element() };
        match &open.view {
            FileView::Loading => div().size_full().flex().items_center().justify_center().child(Spinner::new(SpinnerSize::Lg).muted()).into_any_element(),
            FileView::Failed { message } => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(rems(0.75))
                .px(rems(1.5))
                .text_size(px(12.))
                .text_center()
                .child(div().text_color(colors.destructive()).child(message.clone()))
                .child(div().text_color(colors.muted_foreground).child(format!("{}: {}", t!("Attempted path"), open.path)))
                .into_any_element(),
            FileView::External { size } if open.kind == FilePreviewKind::Video => {
                let card = ExternalFileCard::new(self.files.clone(), open.path.clone(), open.kind, *size);
                self.with_media_menu(card, cx)
            }
            FileView::External { size } => ExternalFileCard::new(self.files.clone(), open.path.clone(), open.kind, *size).into_any_element(),
            FileView::Image { view: image, .. } => self.with_media_menu(image.clone(), cx),
            FileView::Large(view) => view.clone().into_any_element(),
            FileView::Text(text) => {
                let body: AnyElement = match (&text.table, &text.markdown) {
                    (Some(table), _) if open.rendered => table.clone().into_any_element(),
                    (_, Some(markdown)) if open.rendered => self.render_markdown(markdown.clone(), window, cx),
                    // Cached: a keystroke re-renders the editor alone, not the panel around it.
                    _ => text.editor.clone().cached(StyleRefinement::default().size_full()).into_any_element(),
                };
                div().size_full().flex().flex_col().child(div().flex_1().min_h_0().child(body)).into_any_element()
            }
        }
    }

    fn render_markdown(&self, pieces: Arc<Vec<ParsedBlocks>>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ChatColors::get(cx);
        let panel = cx.entity().downgrade();
        let actions = std::rc::Rc::new(move |action: MarkdownAction, _: &mut Window, cx: &mut App| {
            panel
                .update(cx, |panel, cx| {
                    match action {
                        MarkdownAction::Copy { key, text } => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                            panel.markdown_state.copied = Some(key);
                        }
                        MarkdownAction::ToggleWrap(key) => {
                            if !panel.markdown_state.unwrapped.remove(&key) {
                                panel.markdown_state.unwrapped.insert(key);
                            }
                        }
                        MarkdownAction::ToggleDetails(key) => {
                            if !panel.markdown_state.open_details.remove(&key) {
                                panel.markdown_state.open_details.insert(key);
                            }
                        }
                        MarkdownAction::OpenLink(target) => cx.open_url(&target),
                        MarkdownAction::RunShellCommand(_) => {}
                    }
                    cx.notify();
                })
                .ok();
        });
        let key = self.open.as_ref().map_or(0, |o| o.path.len() as u64);
        let view = MarkdownView {
            key,
            pieces: &pieces,
            colors: &colors,
            user: false,
            streaming: false,
            piece_opacity: None,
            state: &self.markdown_state,
            actions,
            find: None,
        };
        div()
            .id("file-markdown")
            .size_full()
            .overflow_y_scroll()
            .child(div().mx_auto().max_w(rems(56.)).px(rems(1.5)).py(rems(1.25)).child(render_markdown(&view, window, cx)))
            .into_any_element()
    }

    /// The compact "Open in" group (`OpenInPicker` with `compact`): the
    /// preferred editor's mark, then a chevron whose menu picks the preferred
    /// editor. Opening runs `shell.openInEditor` on the environment's server.
    fn render_open_in(&self, path: &SharedString, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let (c, d) = (&theme.colors, &theme.derived);
        let platform = Platform::current();
        let available = self.capabilities.editor_ids.clone();
        let preferred = effective_editor(LastEditor::get(cx), &available);
        let absolute = absolute_path(self.files.root(), path);
        let reveal = self.capabilities.reveal.is_some();
        let (border, fill, hover) = (c.input, d.outline_fill, d.outline_hover);
        let height = theme.sp(6.);
        let mark = |editor: EditorId| {
            let (icon, brand) = editor_icon(editor, platform);
            Icon::new(icon).size(IconSize::Custom(theme.sp(3.5))).color(if brand { c.foreground() } else { c.muted_foreground })
        };
        let primary = div()
            .id("file-open-in-primary")
            .h(height)
            .min_w(height + px(1.))
            .px(theme.sp(2.) - px(1.))
            .flex()
            .items_center()
            .justify_center()
            .border_l_1()
            .border_t_1()
            .border_b_1()
            .border_color(border)
            .rounded_l(theme.radii.lg)
            .bg(fill)
            .children(preferred.map(mark))
            .tooltip(tooltip(t!("Open file in preferred editor")))
            .when(preferred.is_none(), |el| el.opacity(DISABLED_OPACITY))
            .when_some(preferred, |el, editor| {
                el.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(move |_, _, _, cx| {
                    LastEditor::set(editor, cx);
                    cx.emit(FilesPanelEvent::OpenInEditor {
                        absolute_path: absolute.clone(),
                        editor,
                        reveal: reveal && editor == EditorId::FileManager,
                    });
                }))
            });
        let chevron = div()
            .id("file-open-in-chevron")
            .size(height)
            .flex()
            .items_center()
            .justify_center()
            .border_t_1()
            .border_r_1()
            .border_b_1()
            .border_color(border)
            .rounded_r(theme.radii.lg)
            .bg(fill)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(theme.sp(4.))));
        let panel = cx.entity().downgrade();
        let shortcut = if cfg!(target_os = "macos") { "⌘O" } else { "Ctrl+O" };
        let menu = dropdown_menu("file-open-in-menu", chevron, move |_, cx| {
            let editors: Vec<EditorId> = EDITORS.iter().map(|(id, ..)| *id).filter(|id| available.contains(id)).collect();
            let preferred = effective_editor(LastEditor::get(cx), &available);
            let mut menu = MenuPopup::new();
            if editors.is_empty() {
                menu = menu.item(MenuItem::new(t!("No installed editors found")).disabled(true));
            }
            for editor in editors {
                let panel = panel.clone();
                let mut item = MenuItem::new(platform_editor_label(editor, platform)).icon(editor_icon(editor, platform).0).on_select(move |_, cx| {
                    // Outside the menu presentation, picking only changes the preferred editor.
                    LastEditor::set(editor, cx);
                    panel.update(cx, |_, cx| cx.notify()).ok();
                });
                if Some(editor) == preferred {
                    item = item.shortcut(shortcut);
                }
                menu = menu.item(item);
            }
            menu
        });
        div()
            .id("file-open-in")
            .flex()
            .items_center()
            .child(primary)
            .child(div().w(px(1.)).h(height).bg(border))
            .child(div().id("file-open-in-chooser").tooltip(tooltip(t!("Choose editor"))).child(menu))
            .into_any_element()
    }
}

/// `isBrowserPreviewFile`: HTML and PDF files open in the preview browser.
pub fn is_browser_preview_file(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or(path).to_ascii_lowercase();
    path.ends_with(".html") || path.ends_with(".htm") || path.ends_with(".pdf")
}

/// `renderedToggleLabel`.
fn rendered_toggle_label(open: &OpenFile) -> Option<SharedString> {
    if !matches!(open.view, FileView::Text(_)) {
        return None;
    }
    if open.delimiter.is_some() {
        return Some(if open.rendered { t!("Show source") } else { t!("Show table") });
    }
    (open.kind == FilePreviewKind::Markdown)
        .then(|| if open.rendered { t!("Show markdown source") } else { t!("Show rendered markdown") })
}

/// `filePreviewReadErrorMessage`.
fn read_error_message(error: &FileError) -> SharedString {
    match error.failure {
        Some(ProjectFileFailure::PathNotFile) => t!("The path is a directory or special file, not a regular file."),
        Some(ProjectFileFailure::BinaryFile) => t!("The file is binary and cannot be displayed as text."),
        Some(ProjectFileFailure::WorkspacePathOutsideRoot) => t!("The requested path is outside the workspace."),
        Some(ProjectFileFailure::ResolvedPathOutsideRoot) => t!("The path resolves to a location outside the workspace."),
        Some(ProjectFileFailure::OperationFailed) => t!("The file could not be accessed or read. It may be missing or inaccessible."),
        _ => error.message.clone().into(),
    }
}

/// `maskMarkdownFrontmatter`: a leading `---` block renders as a code block.
fn mask_frontmatter(text: &str) -> String {
    let Some(rest) = text.strip_prefix("---\n") else { return text.to_owned() };
    match rest.find("\n---") {
        Some(end) => format!("```yaml\n{}\n```{}", &rest[..end], &rest[end + 4..]),
        None => text.to_owned(),
    }
}

impl Render for FilesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.mount_gallery(window, cx);
        let theme = cx.app_theme().clone();
        let colors = theme.colors;
        let width = window.viewport_size().width.as_f32();
        let has_file = self.open.is_some();
        let explorer = self.explorer_open || !has_file;
        let max_width = (width - FILE_CONTENT_MIN_WIDTH).clamp(EXPLORER_MIN_WIDTH, EXPLORER_MAX_WIDTH);
        let explorer_width = self.explorer_width.clamp(EXPLORER_MIN_WIDTH, max_width);
        let subheader = self.open.as_ref().map(|open| {
            let breadcrumbs = self.render_breadcrumbs(&open.path, cx).into_any_element();
            let actions = self.render_actions(open, cx);
            div()
                .h(rems(1.75))
                .mb(rems(0.75))
                .flex_none()
                .flex()
                .items_center()
                .gap(rems(0.5))
                .px(rems(0.75))
                .border_b_1()
                .border_color(gpui_kit::transparent_black())
                .bg(colors.canvas)
                .child(breadcrumbs)
                .children(actions)
        });
        let body = self.open.is_some().then(|| self.render_body(window, cx));
        let expanded = self.expanded.as_ref().map(|(dialog, _)| dialog.clone());
        let close_palette = cx.entity().downgrade();
        let palette = self.palette.as_ref().map(|palette| {
            let content = match palette {
                Palette::Files(picker, _) => picker.clone().into_any_element(),
                Palette::Contents(search, _) => search.clone().into_any_element(),
            };
            palette_overlay(
                content,
                move |_, cx| {
                    close_palette
                        .update(cx, |panel, cx| {
                            panel.palette = None;
                            cx.notify();
                        })
                        .ok();
                },
                window,
                &theme,
            )
        });
        div()
            .id("files-panel")
            .key_context("FilesPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|panel, _: &GoToFile, window, cx| panel.open_file_picker(window, cx)))
            .on_action(cx.listener(|panel, _: &SearchContents, window, cx| panel.open_content_search(window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.canvas)
            .on_mouse_move(cx.listener(move |panel, event: &MouseMoveEvent, window, cx| {
                if panel.resizing {
                    let right = window.viewport_size().width;
                    panel.explorer_width = (right - event.position.x).as_f32();
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|panel, _, _, cx| {
                if panel.resizing {
                    panel.resizing = false;
                    cx.notify();
                }
            }))
            .children(subheader)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .children(body.map(|body| div().flex_1().min_w_0().h_full().overflow_hidden().child(body)))
                    .when(explorer, |el| {
                        el.child(
                            div()
                                .relative()
                                .h_full()
                                .when(has_file, |el| el.w(px(explorer_width)).flex_none().border_l_1().border_color(colors.border.opacity(0.6)))
                                .when(!has_file, |el| el.flex_1())
                                .child(self.tree.clone().cached(StyleRefinement::default().size_full()))
                                .when(has_file, |el| {
                                    el.child(
                                        div()
                                            .id("file-explorer-resize")
                                            .absolute()
                                            .left(px(-3.))
                                            .top_0()
                                            .bottom_0()
                                            .w(px(6.))
                                            .cursor(gpui_kit::CursorStyle::ResizeLeftRight)
                                            .on_mouse_down(MouseButton::Left, cx.listener(|panel, _, _, cx| {
                                                panel.resizing = true;
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                    }),
            )
            .children(palette)
            .children(expanded)
    }
}
