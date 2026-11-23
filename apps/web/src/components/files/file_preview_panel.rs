//! The Files tab of the right panel. Ports `FilePreviewPanel.tsx` and
//! `FileExplorerPane.tsx`: the open file on the left with its breadcrumb bar
//! and actions, the file tree in a resizable pane on the right (the whole
//! width when no file is open).
//!
//! Text opens in the editor and saves itself 500ms after the last edit, as
//! the web's `FileSaveCoordinator` does; Mod+S saves at once. Before each
//! write the file is read again: when it changed on disk since it was opened,
//! the save stops and a notice offers to keep the edits or take the disk copy.
//! Files over 1 MB open read-only in the code view; markdown and CSV toggle
//! between source and rendered; images get the zoomable viewer; every other
//! kind opens in this machine's default app.

use std::{sync::Arc, time::Duration};

use gpui_kit::{
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
            media_actions::MediaKind,
            media_source::{MediaBytes, load_image},
            video_preview::{ExternalFileCard, open_with_default_app},
        },
        ui::{
            button::{Button, ButtonSize, ButtonVariant},
            dialog::{DialogSurface, Modal},
            menu::{MenuItem, MenuPopup, dropdown_menu},
            spinner::{Spinner, SpinnerSize},
            toggle::{Toggle, ToggleSize, ToggleVariant},
            tooltip::tooltip,
        },
    },
    file_context_menu::{FileContextMenuAction, FileContextMenuCapabilities, composer_file_mention, editor_label},
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
}

/// Where a text file's saving stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveState {
    Saved,
    /// Edited; a save is scheduled.
    Pending,
    Saving,
    Failed(SharedString),
    /// The file changed on disk since it was opened; holds the disk copy.
    Conflict(Arc<str>),
}

struct TextFile {
    editor: Entity<FileEditor>,
    /// The server copy the editor started from or last wrote.
    base: Arc<str>,
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
    /// A file to open once the user answers the unsaved-changes prompt.
    prompt: Option<(SharedString, Option<u32>)>,
    expanded: Option<(Entity<ExpandedImageDialog>, Subscription)>,
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
            prompt: None,
            expanded: None,
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

    /// Whether the unsaved-changes prompt is showing.
    pub fn prompt_open(&self) -> bool {
        self.prompt.is_some()
    }

    /// Answers the unsaved-changes prompt: discard the edits and open the file.
    pub fn discard_unsaved(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard_and_open(window, cx);
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

    /// Opens a workspace file, revealing a 1-based line when given. With
    /// unsaved edits that could not be written, asks before leaving them.
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
        if let Some(FileView::Text(text)) = self.open.as_ref().map(|o| &o.view)
            && text.editor.read(cx).is_dirty()
        {
            match text.save {
                SaveState::Failed(_) | SaveState::Conflict(_) => {
                    self.prompt = Some((path, line));
                    cx.notify();
                    return;
                }
                _ => self.save_now(cx),
            }
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
                let editor = cx.new(|cx| FileEditor::new(path.clone(), &file.contents, false, window, cx));
                let subscription = cx.subscribe(&editor, |panel, _, event: &FileEditorEvent, cx| panel.on_editor_event(event, cx));
                open.view = FileView::Text(TextFile {
                    editor,
                    base: file.contents.into(),
                    save: SaveState::Saved,
                    resave: false,
                    table: None,
                    markdown: None,
                    _timer: None,
                    _subscription: subscription,
                });
                self.prepare_rendered(cx);
                self.apply_reveal(cx);
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
        if matches!(text.save, SaveState::Conflict(_)) {
            return;
        }
        text._timer = Some(timer);
        // Re-render the bar only when the state changes, not on every keystroke.
        if !matches!(text.save, SaveState::Saving | SaveState::Pending) {
            text.save = SaveState::Pending;
            cx.notify();
        }
    }

    /// Writes the editor's text now: reads the file first and stops on a conflict.
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.open.as_ref().map(|o| o.path.clone()) else { return };
        let files = self.files.clone();
        let Some(text) = self.text_mut() else { return };
        match text.save {
            SaveState::Saving => {
                text.resave = true;
                return;
            }
            SaveState::Conflict(_) => return,
            _ => {}
        }
        let contents = text.editor.read(cx).text();
        if contents.as_str() == &*text.base && !text.editor.read(cx).is_dirty() {
            text.save = SaveState::Saved;
            cx.notify();
            return;
        }
        text.save = SaveState::Saving;
        text._timer = None;
        let base = text.base.clone();
        let read = files.read_file(path.to_string());
        let work = cx.background_spawn(async move {
            let disk = read.await?;
            if disk.contents.as_str() != &*base && disk.contents != contents {
                return Ok(Err(Arc::<str>::from(disk.contents)));
            }
            files.write_file(path.to_string(), contents.clone()).await?;
            Ok(Ok(contents))
        });
        cx.spawn(async move |this, cx| {
            let result: Result<Result<String, Arc<str>>, FileError> = work.await;
            this.update(cx, |panel, cx| panel.save_finished(result, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    fn save_finished(&mut self, result: Result<Result<String, Arc<str>>, FileError>, cx: &mut Context<Self>) {
        let Some(text) = self.text_mut() else { return };
        match result {
            Ok(Ok(written)) => {
                text.base = written.as_str().into();
                text.save = SaveState::Saved;
                text.editor.update(cx, |editor, cx| editor.mark_saved(written, cx));
                if text.editor.read(cx).is_dirty() || std::mem::take(&mut text.resave) {
                    self.schedule_save(cx);
                }
            }
            Ok(Err(disk)) => text.save = SaveState::Conflict(disk),
            Err(error) => text.save = SaveState::Failed(error.message.into()),
        }
        cx.notify();
    }

    /// Conflict: write the editor's text over the disk copy.
    pub fn keep_my_changes(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.text_mut() else { return };
        if let SaveState::Conflict(disk) = &text.save {
            text.base = disk.clone();
            text.save = SaveState::Pending;
        }
        self.save_now(cx);
    }

    /// Conflict: replace the edits with the disk copy.
    pub fn use_disk_copy(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.text_mut() else { return };
        if let SaveState::Conflict(disk) = text.save.clone() {
            text.base = disk.clone();
            text.save = SaveState::Saved;
            text.editor.update(cx, |editor, cx| editor.replace_contents(&disk, cx));
        }
        cx.notify();
    }

    fn discard_and_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, line)) = self.prompt.take() {
            self.load_file(path, line, window, cx);
        }
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

    fn run_file_action(&mut self, action: FileContextMenuAction, path: SharedString, cx: &mut Context<Self>) {
        let absolute = absolute_path(self.files.root(), &path);
        match action {
            FileContextMenuAction::OpenLocally => open_with_default_app(&self.files, &path, cx),
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
                .px(px(2.))
                .rounded(theme.radii.sm)
                .text_color(colors.muted_foreground)
                .hover(|el| el.bg(colors.accent_surface).text_color(colors.text))
                .cursor_pointer()
                .child(self.project_name.clone())
                .into_any_element(),
        );
        for (index, segment) in segments.iter().enumerate() {
            crumbs.push(div().mx(px(2.)).flex().child(separator()).into_any_element());
            let last = index + 1 == segments.len();
            if last {
                crumbs.push(div().px(px(2.)).text_color(colors.text).whitespace_nowrap().child(segment.to_string()).into_any_element());
            } else {
                let folder: SharedString = segments[..=index].join("/").into();
                let tree = tree.clone();
                crumbs.push(
                    div()
                        .id(("crumb", index))
                        .max_w(px(160.))
                        .px(px(2.))
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
        let absolute: SharedString = absolute_path(self.files.root(), &path).into();
        // Open in: server editors and this machine's default app.
        let editors: Vec<EditorId> = self.capabilities.editor_ids.iter().filter(|id| **id != EditorId::FileManager).cloned().collect();
        let panel = cx.entity().downgrade();
        let open_menu_path = path.clone();
        actions.push(
            div()
                .id("file-open-in")
                .tooltip(tooltip(t!("Open in editor")))
                .child(dropdown_menu(
                    "file-open-in-menu",
                    Button::new("file-open-in-button").variant(ButtonVariant::Ghost).size(ButtonSize::IconSm).icon(IconName::SquareArrowOutUpRight),
                    move |_, _| {
                        let mut menu = MenuPopup::new();
                        let local = panel.clone();
                        let local_path = open_menu_path.clone();
                        menu = menu.item(MenuItem::new(t!("Open", context = "verb, opens a file")).icon(IconName::Pencil).on_select(move |_, cx| {
                            local.update(cx, |panel, cx| panel.run_file_action(FileContextMenuAction::OpenLocally, local_path.clone(), cx)).ok();
                        }));
                        for editor in &editors {
                            let panel = panel.clone();
                            let path = open_menu_path.clone();
                            let editor = editor.clone();
                            menu = menu.item(MenuItem::new(editor_label(&editor)).on_select(move |_, cx| {
                                panel.update(cx, |panel, cx| panel.run_file_action(FileContextMenuAction::OpenWith(editor.clone()), path.clone(), cx)).ok();
                            }));
                        }
                        menu
                    },
                ))
                .into_any_element(),
        );
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
        actions.push(
            div()
                .id("file-copy-path")
                .tooltip(tooltip(t!("Copy path")))
                .child(Button::new("file-copy-path-button").variant(ButtonVariant::Ghost).size(ButtonSize::IconSm).icon(IconName::Copy).on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(absolute.to_string()));
                }))
                .into_any_element(),
        );
        let chat_path = path.clone();
        actions.push(
            div()
                .id("file-add-to-chat")
                .tooltip(tooltip(t!("Add to chat")))
                .child(
                    Button::new("file-add-to-chat-button")
                        .variant(ButtonVariant::Ghost)
                        .size(ButtonSize::IconSm)
                        .icon(IconName::MessageSquarePlus)
                        .on_click(cx.listener(move |panel, _, _, cx| panel.run_file_action(FileContextMenuAction::AddToChat, chat_path.clone(), cx))),
                )
                .into_any_element(),
        );
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

    fn render_save_notice(&self, text: &TextFile, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let notice = |content: AnyElement| {
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .border_b_1()
                .border_color(colors.warning.opacity(0.2))
                .bg(colors.warning_surface)
                .text_size(px(11.))
                .text_color(colors.warning_foreground)
                .child(content)
        };
        match &text.save {
            SaveState::Conflict(_) => Some(
                notice(div().flex_1().child(t!("This file changed on disk since you opened it.")).into_any_element())
                    .child(Button::new("conflict-keep").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Keep my changes")).on_click(cx.listener(|panel, _, _, cx| panel.keep_my_changes(cx))))
                    .child(Button::new("conflict-disk").variant(ButtonVariant::Ghost).size(ButtonSize::Xs).label(t!("Use the file on disk")).on_click(cx.listener(|panel, _, _, cx| panel.use_disk_copy(cx))))
                    .into_any_element(),
            ),
            SaveState::Failed(message) => Some(
                notice(div().flex_1().child(format!("{} {message}", t!("Could not save file"))).into_any_element())
                    .child(Button::new("save-retry").variant(ButtonVariant::Outline).size(ButtonSize::Xs).label(t!("Try again")).on_click(cx.listener(|panel, _, _, cx| {
                        if let Some(text) = panel.text_mut() {
                            text.save = SaveState::Pending;
                        }
                        panel.save_now(cx);
                    })))
                    .into_any_element(),
            ),
            _ => None,
        }
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
                .gap(px(12.))
                .px(px(24.))
                .text_size(px(12.))
                .text_center()
                .child(div().text_color(colors.destructive()).child(message.clone()))
                .child(div().text_color(colors.muted_foreground).child(format!("{}: {}", t!("Attempted path"), open.path)))
                .into_any_element(),
            FileView::External { size } => ExternalFileCard::new(self.files.clone(), open.path.clone(), open.kind, *size).into_any_element(),
            FileView::Image { view: image, .. } => image.clone().into_any_element(),
            FileView::Large(view) => view.clone().into_any_element(),
            FileView::Text(text) => {
                let notice = self.render_save_notice(text, cx);
                let body: AnyElement = match (&text.table, &text.markdown) {
                    (Some(table), _) if open.rendered => table.clone().into_any_element(),
                    (_, Some(markdown)) if open.rendered => self.render_markdown(markdown.clone(), window, cx),
                    // Cached: a keystroke re-renders the editor alone, not the panel around it.
                    _ => text.editor.clone().cached(StyleRefinement::default().size_full()).into_any_element(),
                };
                div().size_full().flex().flex_col().children(notice).child(div().flex_1().min_h_0().child(body)).into_any_element()
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
        };
        div()
            .id("file-markdown")
            .size_full()
            .overflow_y_scroll()
            .child(div().mx_auto().max_w(px(896.)).px(px(24.)).py(px(20.)).child(render_markdown(&view, window, cx)))
            .into_any_element()
    }

    fn render_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (path, _) = self.prompt.as_ref()?;
        let _ = path;
        let name = self.open.as_ref().map(|o| o.path.clone()).unwrap_or_default();
        let surface = DialogSurface::new(t!("Discard unsaved changes?"))
            .description(format!("{} {name}", t!("Your edits have not been saved:")))
            .footer(Button::new("prompt-cancel").variant(ButtonVariant::Ghost).label(t!("Cancel")).on_click(cx.listener(|panel, _, _, cx| {
                panel.prompt = None;
                cx.notify();
            })))
            .footer(Button::new("prompt-discard").variant(ButtonVariant::Destructive).label(t!("Discard")).on_click(cx.listener(|panel, _, window, cx| {
                panel.discard_and_open(window, cx);
            })));
        let close = cx.entity().downgrade();
        Some(
            Modal::dialog("files-unsaved-prompt", surface, move |_, cx| {
                close
                    .update(cx, |panel, cx| {
                        panel.prompt = None;
                        cx.notify();
                    })
                    .ok();
            })
            .into_any_element(),
        )
    }
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
            let saving = match self.open.as_ref().map(|o| &o.view) {
                Some(FileView::Text(text)) => match text.save {
                    SaveState::Pending | SaveState::Saving => Some(t!("Saving…")),
                    _ => None,
                },
                _ => None,
            };
            div()
                .h(px(40.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .border_b_1()
                .border_color(colors.border.opacity(0.6))
                .bg(colors.canvas)
                .child(breadcrumbs)
                .children(saving.map(|label| div().flex_none().text_size(px(11.)).text_color(colors.muted_foreground).child(label)))
                .children(actions)
        });
        let body = self.open.is_some().then(|| self.render_body(window, cx));
        let prompt = self.render_prompt(cx);
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
            .children(prompt)
            .children(palette)
            .children(expanded)
    }
}
