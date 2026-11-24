//! The virtualized code surface shared by the diff panel and the file viewer.
//! Ports what T3 uses of Pierre's `CodeView` (`StyledDiffCodeView.tsx`,
//! `AnnotatableCodeView.tsx`): stacked and split diffs, sticky 32px file
//! headers, collapsible files, expandable context, wrapping or horizontal
//! scrolling, line and text selection with copy, find, line comments, and a
//! plain single-file mode with the same gutter and highlighting.
//!
//! Code rows are painted by one canvas: only rows in the viewport are shaped,
//! shaped lines are cached by line and highlight state, and highlighting runs
//! in the background through [`SyntaxStore`]. Headers, separators and
//! comment rows are ordinary elements placed over the canvas, since only a
//! handful are ever visible.

use std::{collections::HashMap, ops::Range, sync::Arc, time::Duration};

use gpui_kit::{
    component::{
        Sizable as _,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
    },
    base::TestSupportExt as _,
    prelude::FluentBuilder as _,
    *,
};
use smallvec::SmallVec;

use super::{
    colors::DiffColors,
    layout::*,
    review_comment::{ReviewComment, ReviewCommentInput, build_review_comment},
};
use crate::{
    appearance_fonts::mono_font,
    ActiveAppTheme as _,
    diff_count::format_diff_count,
    diff_panel_store::{DiffColorScheme, DiffLayout},
    diff_patch::{ChangeType, FileContents, FileDiff},
    syntax::{DocKey, SyntaxPalette, SyntaxStore, TokenSpan},
};

/// Key context of the code view.
pub const CONTEXT: &str = "DiffCodeView";

actions!(
    diff_code_view,
    [CopySelection, SelectAllLines, OpenFind, CloseFind]
);

/// Registers the code view's key bindings.
pub fn init(cx: &mut App) {
    let (copy, find) = if cfg!(target_os = "macos") { ("cmd-c", "cmd-f") } else { ("ctrl-c", "ctrl-f") };
    cx.bind_keys([
        KeyBinding::new(copy, CopySelection, Some(CONTEXT)),
        KeyBinding::new(find, OpenFind, Some(CONTEXT)),
        KeyBinding::new("escape", CloseFind, Some(CONTEXT)),
    ]);
}

/// Diff panel or single-file viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Diff,
    /// One file, no header, every line context. Used by the file viewer.
    File,
}

/// Presentation options from settings and the panel toolbar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodeViewOptions {
    pub layout: DiffLayout,
    pub wrap: bool,
    pub scheme: DiffColorScheme,
    /// Show the gutter "+" that starts a line comment.
    pub comments: bool,
}

impl Default for CodeViewOptions {
    fn default() -> Self {
        Self { layout: DiffLayout::Stacked, wrap: true, scheme: DiffColorScheme::RedGreen, comments: true }
    }
}

/// Rows and header controls an owner draws into the view, such as the pull request Code tab's
/// review threads and Viewed toggles. Heights are reserved in the layout; the elements are built
/// only for rows on screen.
pub trait CodeViewDecorations {
    /// Annotation rows for a file, as `(anchor, height)`.
    fn annotations(&self, file_key: &SharedString) -> Vec<((Side, u32), f32)>;
    /// The element for one annotation row reported by [`Self::annotations`].
    fn render_annotation(&self, file_key: &SharedString, anchor: (Side, u32), window: &mut Window, cx: &mut App) -> Option<AnyElement>;
    /// A control drawn at the right of a file's header, before its counts.
    fn render_header_accessory(&self, file_key: &SharedString, window: &mut Window, cx: &mut App) -> Option<AnyElement>;
}

/// A file handed to the view.
#[derive(Clone)]
pub struct CodeViewFile {
    pub diff: Arc<FileDiff>,
    /// Stable identity within the review scope (`buildFileDiffEntries`).
    pub key: SharedString,
    pub collapsed: bool,
    /// Header counts from metadata when the patch body is partial.
    pub stat: Option<(u32, u32)>,
}

/// What the view asks its owner to do.
#[derive(Clone, Debug)]
pub enum CodeViewEvent {
    /// The user folded or unfolded a file.
    CollapsedChanged { key: SharedString, collapsed: bool },
    /// The file title was clicked.
    OpenFile { path: SharedString },
    /// A separator was expanded on a file whose full contents are not loaded.
    RequestContents { key: SharedString },
    /// A line comment was submitted.
    Comment(ReviewComment),
    DeleteComment { id: SharedString },
    /// The scroll offset changed, for scroll restore.
    Scrolled { offset: f32 },
    /// A file header was right-clicked: show the file menu at `position`.
    HeaderContextMenu { path: SharedString, position: Point<Pixels> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TextPoint {
    file: u32,
    row: u32,
    byte: u32,
}

#[derive(Clone, Copy, Debug)]
struct TextSelection {
    anchor: TextPoint,
    head: TextPoint,
    column: Column,
}

impl TextSelection {
    fn ordered(&self) -> (TextPoint, TextPoint) {
        if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }
}

#[derive(Clone, Copy, Debug)]
struct LineSelection {
    file: u32,
    anchor: u32,
    head: u32,
    column: Column,
}

impl LineSelection {
    fn rows(&self) -> Range<u32> {
        self.anchor.min(self.head)..self.anchor.max(self.head) + 1
    }
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Text,
    Lines,
    Scrollbar { grab: f32 },
}

enum Hit {
    Header,
    Line { file: u32, row: u32, column: Column, gutter: bool, byte: u32 },
    Other,
}

struct Draft {
    file: u32,
    start: (Side, u32),
    end: (Side, u32),
    anchor: (Side, u32),
    label: SharedString,
    editor: Entity<TextareaState>,
    _subscription: Subscription,
}

struct SavedComment {
    file_key: SharedString,
    anchor: (Side, u32),
    comment: ReviewComment,
}

#[derive(Clone, Copy, Debug)]
struct FindMatch {
    file: u32,
    line: LineRef,
    start: u32,
    end: u32,
}

struct Find {
    input: Entity<InputState>,
    query: String,
    matches: Vec<FindMatch>,
    by_line: HashMap<(u32, LineRef), SmallVec<[usize; 2]>>,
    current: Option<usize>,
    _subscription: Subscription,
}

struct FileState {
    diff: Arc<FileDiff>,
    key: SharedString,
    collapsed: bool,
    stat: Option<(u32, u32)>,
    contents: Option<Arc<FileContents>>,
    gaps: Vec<GapExpansion>,
    layout: Option<(LayoutInput, FileLayout)>,
    docs: HashMap<LineSource, Option<DocKey>>,
}

impl FileState {
    fn doc(&mut self, source: LineSource) -> Option<DocKey> {
        let language = self.diff.language?;
        let (diff, contents) = (&self.diff, self.contents.as_deref());
        *self.docs.entry(source).or_insert_with(|| {
            let lines = source_lines(diff, contents, source);
            (!lines.is_empty()).then(|| DocKey::new(lines, language))
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ShapeKey {
    version: u64,
    line: LineRef,
    segment: u16,
    highlighted: bool,
}

/// A code segment ready to paint.
struct PaintText {
    origin: Point<Pixels>,
    line: ShapedLine,
    /// Index into the row's `code_clip`.
    clip: usize,
}

struct PaintRow {
    quads: SmallVec<[(Bounds<Pixels>, Hsla); 8]>,
    stripes: Option<Bounds<Pixels>>,
    hatch: SmallVec<[Bounds<Pixels>; 1]>,
    texts: SmallVec<[PaintText; 2]>,
    numbers: SmallVec<[PaintText; 2]>,
    code_clip: SmallVec<[Bounds<Pixels>; 2]>,
}

/// Everything the canvas paints this frame.
#[derive(Default)]
pub struct PaintPlan {
    rows: Vec<PaintRow>,
    scrollbar: Option<(Bounds<Pixels>, Hsla)>,
    hatch_color: Hsla,
}

const DRAFT_HEIGHT: f32 = 120.;
const SCROLLBAR_WIDTH: f32 = 6.;

/// The code surface. Create with [`CodeView::new`], feed it files with [`CodeView::set_files`].
pub struct CodeView {
    mode: ViewMode,
    options: CodeViewOptions,
    files: Vec<FileState>,
    file_tops: Vec<f32>,
    total_height: f32,
    layout_dirty: bool,
    scroll: Point<f32>,
    bounds: Bounds<Pixels>,
    metrics: Option<Metrics>,
    font: Font,
    font_bold: Font,
    /// The code font size setting this view was last laid out at.
    font_size: f32,
    font_italic: Font,
    shape_cache: HashMap<ShapeKey, (ShapedLine, u64)>,
    number_cache: HashMap<(u32, u8), ShapedLine>,
    frame: u64,
    colors: Option<DiffColors>,
    theme_id: (String, crate::theme_palette::Appearance, DiffColorScheme),
    syntax: Entity<SyntaxStore>,
    selection: Option<TextSelection>,
    line_selection: Option<LineSelection>,
    drag: Option<Drag>,
    hovered: Option<(u32, u32, Column)>,
    hovered_header: Option<u32>,
    draft: Option<Draft>,
    comments: Vec<SavedComment>,
    decorations: Option<std::rc::Rc<dyn CodeViewDecorations>>,
    find: Option<Find>,
    copied_path: Option<SharedString>,
    section: (SharedString, SharedString),
    workspace_root: Option<SharedString>,
    focus_handle: FocusHandle,
    recycled: Vec<PaintRow>,
    /// A reveal asked for before the first layout; applied once laid out.
    pending_reveal: Option<Reveal>,
    _syntax_observer: Subscription,
}

#[derive(Clone, Debug)]
enum Reveal {
    File(SharedString),
    Line(u32),
}

impl EventEmitter<CodeViewEvent> for CodeView {}

impl Focusable for CodeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl CodeView {
    pub fn new(mode: ViewMode, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let syntax = SyntaxStore::global(cx);
        let observer = cx.observe(&syntax, |_, _, cx| cx.notify());
        let font = mono_font(FontWeight::NORMAL);
        Self {
            mode,
            options: CodeViewOptions::default(),
            files: Vec::new(),
            file_tops: Vec::new(),
            total_height: 0.,
            layout_dirty: true,
            scroll: Point::default(),
            bounds: Bounds::default(),
            metrics: None,
            font_bold: Font { weight: FontWeight::BOLD, ..font.clone() },
            font_size: 13.,
            font_italic: Font { style: FontStyle::Italic, ..font.clone() },
            font,
            shape_cache: HashMap::new(),
            number_cache: HashMap::new(),
            frame: 0,
            colors: None,
            theme_id: (String::new(), crate::theme_palette::Appearance::Dark, DiffColorScheme::RedGreen),
            syntax,
            selection: None,
            line_selection: None,
            drag: None,
            hovered: None,
            hovered_header: None,
            draft: None,
            comments: Vec::new(),
            decorations: None,
            find: None,
            copied_path: None,
            section: (SharedString::default(), SharedString::default()),
            workspace_root: None,
            focus_handle: cx.focus_handle(),
            recycled: Vec::new(),
            pending_reveal: None,
            _syntax_observer: observer,
        }
    }

    /// Replaces the files. State of files whose key and version are unchanged is kept.
    pub fn set_files(&mut self, files: Vec<CodeViewFile>, cx: &mut Context<Self>) {
        let mut previous: HashMap<SharedString, FileState> =
            self.files.drain(..).map(|file| (file.key.clone(), file)).collect();
        self.files = files
            .into_iter()
            .map(|file| match previous.remove(&file.key) {
                Some(mut state) if state.diff.version == file.diff.version => {
                    state.collapsed = file.collapsed;
                    state.stat = file.stat;
                    state
                }
                _ => FileState {
                    diff: file.diff,
                    key: file.key,
                    collapsed: file.collapsed,
                    stat: file.stat,
                    contents: None,
                    gaps: Vec::new(),
                    layout: None,
                    docs: HashMap::new(),
                },
            })
            .collect();
        self.selection = None;
        self.line_selection = None;
        if self.draft.as_ref().is_some_and(|d| d.file as usize >= self.files.len()) {
            self.draft = None;
        }
        self.layout_dirty = true;
        self.refresh_find(cx);
        cx.notify();
    }

    /// Shows one file as plain code (file mode).
    pub fn set_file_contents(&mut self, path: SharedString, contents: &str, cx: &mut Context<Self>) {
        let lines = crate::syntax::TextLines::from_contents(contents);
        let diff = FileDiff {
            language: crate::syntax::language_for_path(&path),
            path: path.clone(),
            prev_path: None,
            change: ChangeType::Change,
            old_mode: None,
            new_mode: None,
            binary: false,
            old_lines: Default::default(),
            version: lines.content_hash(),
            new_lines: lines,
            hunks: Vec::new(),
            additions: 0,
            deletions: 0,
        };
        self.set_files(vec![CodeViewFile { diff: Arc::new(diff), key: path, collapsed: false, stat: None }], cx);
    }

    pub fn set_options(&mut self, options: CodeViewOptions, cx: &mut Context<Self>) {
        if self.options != options {
            if self.options.layout != options.layout {
                self.selection = None;
                self.line_selection = None;
            }
            self.options = options;
            self.layout_dirty = true;
            cx.notify();
        }
    }

    pub fn options(&self) -> CodeViewOptions {
        self.options
    }

    /// The monospace family code is drawn in.
    pub fn mono_family(&self) -> SharedString {
        self.font.family.clone()
    }

    /// The review section comments are filed under (`sectionId`, `sectionTitle`).
    pub fn set_section(&mut self, id: SharedString, title: SharedString) {
        self.section = (id, title);
    }

    /// The workspace root that "copy path" joins relative paths to.
    pub fn set_workspace_root(&mut self, root: Option<SharedString>) {
        self.workspace_root = root;
    }

    pub fn set_collapsed(&mut self, key: &str, collapsed: bool, cx: &mut Context<Self>) {
        if let Some(file) = self.files.iter_mut().find(|f| f.key.as_ref() == key)
            && file.collapsed != collapsed
        {
            file.collapsed = collapsed;
            self.layout_dirty = true;
            cx.notify();
        }
    }

    /// Supplies full file contents so collapsed context can expand.
    pub fn set_contents(&mut self, key: &str, contents: Arc<FileContents>, cx: &mut Context<Self>) {
        if let Some(file) = self.files.iter_mut().find(|f| f.key.as_ref() == key) {
            file.contents = Some(contents);
            file.docs.retain(|source, _| matches!(source, LineSource::HunkOld | LineSource::HunkNew));
            self.layout_dirty = true;
            cx.notify();
        }
    }

    pub fn scroll_offset(&self) -> f32 {
        self.scroll.y
    }

    pub fn set_scroll_offset(&mut self, offset: f32, cx: &mut Context<Self>) {
        self.scroll.y = offset.max(0.);
        cx.notify();
    }

    /// Total content height, for tests and measurements.
    pub fn content_height(&mut self) -> f32 {
        self.ensure_layout();
        self.total_height
    }

    fn header_height(&self) -> f32 {
        if self.mode == ViewMode::Diff { HEADER_HEIGHT } else { 0. }
    }

    fn viewport_width(&self) -> f32 {
        f32::from(self.bounds.size.width).max(200.) - SCROLLBAR_WIDTH
    }

    fn file_digits(&self, file: &FileState) -> u32 {
        number_digits(&file.diff, file.contents.as_deref())
    }

    /// Sets the owner's annotation rows and header controls; call again when they change.
    pub fn set_decorations(&mut self, decorations: Option<std::rc::Rc<dyn CodeViewDecorations>>, cx: &mut Context<Self>) {
        self.decorations = decorations;
        self.layout_dirty = true;
        cx.notify();
    }

    fn annotation_inputs(&self, index: usize) -> Vec<((Side, u32), f32)> {
        let file = &self.files[index];
        let mut out: Vec<((Side, u32), f32)> = self.decorations.as_ref().map(|d| d.annotations(&file.key)).unwrap_or_default();
        let width = self.viewport_width();
        for saved in self.comments.iter().filter(|c| c.file_key == file.key) {
            let chars_per_line = ((width - 60.) / 7.).max(20.) as usize;
            let lines: usize = saved.comment.text.split('\n').map(|l| l.len().div_ceil(chars_per_line).max(1)).sum();
            let height = 21. + 20. * lines as f32;
            match out.iter_mut().find(|(anchor, _)| *anchor == saved.anchor) {
                Some((_, total)) => *total += height,
                None => out.push((saved.anchor, height + 1.)),
            }
        }
        if let Some(draft) = self.draft.as_ref().filter(|d| d.file as usize == index) {
            match out.iter_mut().find(|(anchor, _)| *anchor == draft.anchor) {
                Some((_, total)) => *total += DRAFT_HEIGHT,
                None => out.push((draft.anchor, DRAFT_HEIGHT)),
            }
        }
        out
    }

    fn layout_input(&self, index: usize, metrics: Metrics) -> LayoutInput {
        let file = &self.files[index];
        let digits = self.file_digits(file);
        let width = self.viewport_width();
        let wrap_columns = self.options.wrap.then(|| match (self.mode, self.options.layout) {
            (ViewMode::Diff, DiffLayout::Split) => metrics.code_columns(width / 2., digits),
            _ => metrics.code_columns(width, digits),
        });
        LayoutInput {
            layout: if self.mode == ViewMode::File { DiffLayout::Stacked } else { self.options.layout },
            wrap_columns,
            gaps: file.gaps.clone(),
            annotations: self.annotation_inputs(index),
            has_contents: file.contents.is_some(),
            plain: self.mode == ViewMode::File,
        }
    }

    /// Rebuilds stale file layouts and the file offsets.
    fn ensure_layout(&mut self) {
        if !self.layout_dirty {
            return;
        }
        let Some(metrics) = self.metrics else { return };
        self.layout_dirty = false;
        let header = self.header_height();
        let mut top = 0.;
        self.file_tops.clear();
        for index in 0..self.files.len() {
            self.file_tops.push(top);
            top += header;
            if self.files[index].collapsed {
                continue;
            }
            let input = self.layout_input(index, metrics);
            let file = &mut self.files[index];
            let stale = file.layout.as_ref().is_none_or(|(previous, _)| *previous != input);
            if stale {
                let layout = layout_file(&file.diff, file.contents.as_deref(), &input);
                file.layout = Some((input, layout));
            }
            top += file.layout.as_ref().map_or(0., |(_, layout)| layout.height);
        }
        self.file_tops.push(top);
        self.total_height = top;
    }

    fn max_scroll(&self) -> f32 {
        let tail = if self.mode == ViewMode::Diff { 0. } else { 0. };
        (self.total_height + tail - f32::from(self.bounds.size.height)).max(0.)
    }

    fn clamp_scroll(&mut self) {
        self.scroll.y = self.scroll.y.clamp(0., self.max_scroll());
        if self.options.wrap {
            self.scroll.x = 0.;
        }
        self.scroll.x = self.scroll.x.max(0.);
    }

    fn body_top(&self, file: usize) -> f32 {
        self.file_tops[file] + self.header_height()
    }

    fn file_at(&self, y: f32) -> Option<usize> {
        let index = self.file_tops.partition_point(|top| *top <= y);
        (index > 0 && index < self.file_tops.len()).then(|| index - 1)
    }

    /// The file whose header sticks to the top, and how far it is pushed up.
    fn sticky_file(&self) -> Option<(usize, f32)> {
        if self.mode != ViewMode::Diff {
            return None;
        }
        let file = self.file_at(self.scroll.y)?;
        if self.files[file].collapsed || self.file_tops[file] >= self.scroll.y {
            return None;
        }
        let bottom = self.file_tops[file + 1] - self.scroll.y;
        Some((file, (bottom - HEADER_HEIGHT).min(0.)))
    }

    fn cell_geometry(&self, digits: u32, column: Column) -> (f32, f32, f32) {
        let metrics = self.metrics.expect("metrics measured");
        let gutter = metrics.gutter_width(digits);
        let width = self.viewport_width();
        match (self.mode, self.options.layout, column) {
            (ViewMode::Diff, DiffLayout::Split, Column::Left) => (0., width / 2., gutter),
            (ViewMode::Diff, DiffLayout::Split, Column::Right) => (width / 2., width / 2., gutter),
            _ => (0., width, gutter),
        }
    }

    fn row_cell(&self, file: u32, row: u32, column: Column) -> Option<Cell> {
        let (_, layout) = self.files.get(file as usize)?.layout.as_ref()?;
        match layout.rows.get(row as usize)?.kind {
            RowKind::Line(cell) => (column == Column::Left).then_some(cell),
            RowKind::Split(left, right) => if column == Column::Left { left } else { right },
            _ => None,
        }
    }

    fn cell_text(&self, file: u32, cell: Cell) -> &str {
        let state = &self.files[file as usize];
        line_text(&state.diff, state.contents.as_deref(), cell.line)
    }

    fn segments_for(&self, file: u32, text: &str) -> SmallVec<[Range<usize>; 4]> {
        match self.files[file as usize].layout.as_ref().and_then(|(input, _)| input.wrap_columns) {
            Some(columns) => wrap_segments(text, columns),
            None => smallvec::smallvec![0..text.len()],
        }
    }

    fn hit(&self, position: Point<Pixels>) -> Hit {
        let Some(metrics) = self.metrics else { return Hit::Other };
        let local = position - self.bounds.origin;
        let (x, y) = (f32::from(local.x), f32::from(local.y) + self.scroll.y);
        if let Some((_, offset)) = self.sticky_file()
            && f32::from(local.y) < HEADER_HEIGHT + offset
        {
            return Hit::Header;
        }
        let Some(file) = self.file_at(y) else { return Hit::Other };
        let state = &self.files[file];
        let rel = y - self.file_tops[file];
        if rel < self.header_height() {
            return Hit::Header;
        }
        let Some((_, layout)) = state.layout.as_ref().filter(|_| !state.collapsed) else { return Hit::Other };
        let body_y = rel - self.header_height();
        let row = layout.row_at(body_y);
        let Some(row_data) = layout.rows.get(row).filter(|r| r.top <= body_y && body_y < r.top + r.height) else {
            return Hit::Other;
        };
        let column = match self.options.layout {
            DiffLayout::Split if self.mode == ViewMode::Diff && x >= self.viewport_width() / 2. => Column::Right,
            _ => Column::Left,
        };
        let (cell_x, _, gutter) = self.cell_geometry(layout.digits, column);
        let gutter_hit = x < cell_x + gutter;
        let byte = self
            .row_cell(file as u32, row as u32, column)
            .map(|cell| {
                let text = self.cell_text(file as u32, cell);
                let segments = self.segments_for(file as u32, text);
                let visual = (((body_y - row_data.top) / LINE_HEIGHT) as usize).min(segments.len() - 1);
                let segment = segments[visual].clone();
                let code_x = x - cell_x - gutter - metrics.ch + if self.options.wrap { 0. } else { self.scroll.x };
                let target = (code_x / metrics.ch).max(0.);
                let prefix_columns = display_columns(&text[..segment.start]);
                let mut column_at = 0usize;
                let mut byte = segment.end;
                for (offset, ch) in text[segment.clone()].char_indices() {
                    let width = char_columns(ch, prefix_columns + column_at);
                    if column_at as f32 + width as f32 / 2. > target {
                        byte = segment.start + offset;
                        break;
                    }
                    column_at += width;
                }
                byte as u32
            })
            .unwrap_or(0);
        Hit::Line { file: file as u32, row: row as u32, column, gutter: gutter_hit, byte }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let local = event.position - self.bounds.origin;
        if f32::from(local.x) >= self.viewport_width() && self.total_height > f32::from(self.bounds.size.height) {
            let (thumb_top, thumb_height) = self.scrollbar_thumb();
            let y = f32::from(local.y);
            let grab = if (thumb_top..thumb_top + thumb_height).contains(&y) { y - thumb_top } else { thumb_height / 2. };
            self.drag = Some(Drag::Scrollbar { grab });
            self.drag_scrollbar(y, grab, cx);
            return;
        }
        match self.hit(event.position) {
            Hit::Line { file, row, column, gutter: true, .. } if self.mode == ViewMode::Diff || self.options.comments => {
                self.selection = None;
                self.line_selection = match self.line_selection {
                    Some(mut existing) if event.modifiers.shift && existing.file == file && existing.column == column => {
                        existing.head = row;
                        Some(existing)
                    }
                    _ => Some(LineSelection { file, anchor: row, head: row, column }),
                };
                self.drag = Some(Drag::Lines);
            }
            Hit::Line { file, row, column, byte, .. } => {
                self.line_selection = None;
                let point = TextPoint { file, row, byte };
                self.selection = match self.selection {
                    Some(mut existing) if event.modifiers.shift && existing.column == column => {
                        existing.head = point;
                        Some(existing)
                    }
                    _ => Some(TextSelection { anchor: point, head: point, column }),
                };
                if event.click_count >= 2
                    && let Some(cell) = self.row_cell(file, row, column)
                {
                    let text = self.cell_text(file, cell);
                    let (start, end) = if event.click_count >= 3 { (0, text.len()) } else { word_range(text, byte as usize) };
                    self.selection = Some(TextSelection {
                        anchor: TextPoint { file, row, byte: start as u32 },
                        head: TextPoint { file, row, byte: end as u32 },
                        column,
                    });
                }
                self.drag = Some(Drag::Text);
            }
            _ => {
                self.selection = None;
                self.line_selection = None;
            }
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pressed = event.pressed_button == Some(MouseButton::Left);
        if !pressed {
            self.drag = None;
        }
        match (self.drag, self.hit(event.position)) {
            (Some(Drag::Scrollbar { grab }), _) => {
                let y = f32::from((event.position - self.bounds.origin).y);
                self.drag_scrollbar(y, grab, cx);
            }
            (Some(Drag::Text), Hit::Line { file, row, byte, .. }) => {
                if let Some(selection) = self.selection.as_mut() {
                    selection.head = TextPoint { file, row, byte };
                    cx.notify();
                }
            }
            (Some(Drag::Lines), Hit::Line { file, row, .. }) => {
                if let Some(selection) = self.line_selection.as_mut().filter(|s| s.file == file) {
                    selection.head = row;
                    cx.notify();
                }
            }
            (None, Hit::Line { file, row, column, .. }) => {
                if self.hovered != Some((file, row, column)) {
                    self.hovered = Some((file, row, column));
                    cx.notify();
                }
            }
            (None, _) if self.hovered.is_some() => {
                self.hovered = None;
                cx.notify();
            }
            _ => {}
        }
    }

    fn scrollbar_thumb(&self) -> (f32, f32) {
        let view = f32::from(self.bounds.size.height);
        let total = self.total_height.max(1.);
        let height = (view * view / total).clamp(24., view);
        let top = (self.scroll.y / self.max_scroll().max(1.)) * (view - height);
        (top, height)
    }

    fn drag_scrollbar(&mut self, y: f32, grab: f32, cx: &mut Context<Self>) {
        let view = f32::from(self.bounds.size.height);
        let (_, height) = self.scrollbar_thumb();
        let fraction = ((y - grab) / (view - height).max(1.)).clamp(0., 1.);
        self.scroll.y = fraction * self.max_scroll();
        cx.emit(CodeViewEvent::Scrolled { offset: self.scroll.y });
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(window.line_height());
        let (mut dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.shift && dx == 0. {
            dx = dy;
        } else {
            self.scroll.y -= dy;
        }
        if !self.options.wrap {
            self.scroll.x -= dx;
        }
        self.clamp_scroll();
        cx.emit(CodeViewEvent::Scrolled { offset: self.scroll.y });
        cx.stop_propagation();
        cx.notify();
    }

    /// The selected text, lines joined with `\n`.
    pub fn selected_text(&self) -> Option<String> {
        if let Some(lines) = self.line_selection {
            let mut out = Vec::new();
            for row in lines.rows() {
                if let Some(cell) = self.row_cell(lines.file, row, lines.column) {
                    out.push(self.cell_text(lines.file, cell).to_owned());
                }
            }
            return (!out.is_empty()).then(|| out.join("\n"));
        }
        let selection = self.selection?;
        let (start, end) = selection.ordered();
        if start == end {
            return None;
        }
        let mut out = String::new();
        let mut first = true;
        for file in start.file..=end.file {
            let Some((_, layout)) = self.files.get(file as usize).and_then(|f| f.layout.as_ref()) else { continue };
            let rows = if file == start.file { start.row } else { 0 }..if file == end.file { end.row + 1 } else { layout.rows.len() as u32 };
            for row in rows {
                let Some(cell) = self.row_cell(file, row, selection.column) else { continue };
                let text = self.cell_text(file, cell);
                let from = if (file, row) == (start.file, start.row) { start.byte as usize } else { 0 };
                let to = if (file, row) == (end.file, end.row) { end.byte as usize } else { text.len() };
                if !first {
                    out.push('\n');
                }
                first = false;
                out.push_str(text.get(from.min(text.len())..to.min(text.len()).max(from.min(text.len()))).unwrap_or(""));
            }
        }
        Some(out)
    }

    fn copy_selection(&mut self, _: &CopySelection, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn select_all_lines(&mut self, _: &SelectAllLines, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((file, (_, layout))) = self.files.iter().enumerate().find_map(|(i, f)| f.layout.as_ref().map(|l| (i, l))) {
            self.line_selection = Some(LineSelection { file: file as u32, anchor: 0, head: layout.rows.len() as u32 - 1, column: Column::Left });
            cx.notify();
        }
    }

    // --- navigation -------------------------------------------------------

    /// Scrolls so content offset `y` sits just under the sticky header.
    /// Expands a file and scrolls its header to the top. Returns false for an unknown key.
    pub fn reveal_file(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.files.iter().position(|f| f.key.as_ref() == key) else { return false };
        if self.metrics.is_none() {
            self.files[index].collapsed = false;
            self.layout_dirty = true;
            self.pending_reveal = Some(Reveal::File(key.to_owned().into()));
            return true;
        }
        if self.files[index].collapsed {
            self.files[index].collapsed = false;
            self.layout_dirty = true;
            cx.emit(CodeViewEvent::CollapsedChanged { key: self.files[index].key.clone(), collapsed: false });
        }
        self.ensure_layout();
        self.scroll.y = self.file_tops.get(index).copied().unwrap_or(0.);
        self.clamp_scroll();
        cx.emit(CodeViewEvent::Scrolled { offset: self.scroll.y });
        cx.notify();
        true
    }

    /// File mode: scrolls line `number` (1-based) into the middle and selects it.
    pub fn reveal_line(&mut self, number: u32, cx: &mut Context<Self>) {
        if self.metrics.is_none() || self.bounds.size.height == px(0.) {
            self.pending_reveal = Some(Reveal::Line(number));
            return;
        }
        self.ensure_layout();
        let Some((_, layout)) = self.files.first().and_then(|f| f.layout.as_ref()) else { return };
        let Some(row) = layout.rows.iter().position(|r| matches!(r.kind, RowKind::Line(c) if c.number == number)) else { return };
        let top = layout.rows[row].top;
        self.line_selection = Some(LineSelection { file: 0, anchor: row as u32, head: row as u32, column: Column::Left });
        self.scroll.y = top - f32::from(self.bounds.size.height) / 2.;
        self.clamp_scroll();
        cx.notify();
    }

    // --- files ------------------------------------------------------------

    /// Folds or unfolds one file and tells the owner.
    pub fn toggle_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self.files.get_mut(index) else { return };
        file.collapsed = !file.collapsed;
        let (key, collapsed) = (file.key.clone(), file.collapsed);
        // Keep the header where it was when folding a file scrolled past its top.
        if collapsed && self.file_tops.get(index).is_some_and(|top| *top < self.scroll.y) {
            self.scroll.y = self.file_tops[index];
        }
        self.layout_dirty = true;
        cx.emit(CodeViewEvent::CollapsedChanged { key, collapsed });
        cx.notify();
    }

    /// Reveals unchanged lines in gap `gap` of file `index`. `up` reveals lines above
    /// the next hunk; otherwise below the previous one. `all` reveals the whole gap.
    pub fn expand_gap(&mut self, index: usize, gap: usize, up: bool, all: bool, cx: &mut Context<Self>) {
        let Some(file) = self.files.get_mut(index) else { return };
        if file.contents.is_none() {
            cx.emit(CodeViewEvent::RequestContents { key: file.key.clone() });
            return;
        }
        if file.gaps.len() <= gap {
            file.gaps.resize(gap + 1, GapExpansion::default());
        }
        let step = if all { u32::MAX / 2 } else { EXPANSION_STEP };
        let expansion = &mut file.gaps[gap];
        if up {
            expansion.bottom = expansion.bottom.saturating_add(step);
        } else {
            expansion.top = expansion.top.saturating_add(step);
        }
        self.layout_dirty = true;
        cx.notify();
    }

    // --- comments ---------------------------------------------------------

    fn begin_comment(&mut self, file: u32, row: u32, column: Column, window: &mut Window, cx: &mut Context<Self>) {
        let range = match self.line_selection {
            Some(selection) if selection.file == file && selection.rows().contains(&row) => selection.rows(),
            _ => row..row + 1,
        };
        let cell_at = |this: &Self, row: u32| {
            this.row_cell(file, row, column).or_else(|| this.row_cell(file, row, if column == Column::Left { Column::Right } else { Column::Left }))
        };
        let cells: Vec<Cell> = range.clone().filter_map(|row| cell_at(self, row)).collect();
        let (Some(first), Some(last)) = (cells.first().copied(), cells.last().copied()) else { return };
        let state = &self.files[file as usize];
        let label = build_review_comment(ReviewCommentInput {
            id: "draft".into(),
            section_id: self.section.0.clone(),
            section_title: self.section.1.clone(),
            file: &state.diff,
            start: (first.side(), first.number),
            end: (last.side(), last.number),
            text: "",
        })
        .map(|comment| comment.range_label)
        .unwrap_or_else(|| "line".into());
        let editor = cx.new(|cx| TextareaState::new(window, cx).placeholder("Add a comment…"));
        let subscription = cx.subscribe_in(&editor, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event {
                this.submit_draft(window, cx);
            }
        });
        editor.update(cx, |editor, cx| editor.focus(window, cx));
        self.draft = Some(Draft {
            file,
            start: (first.side(), first.number),
            end: (last.side(), last.number),
            anchor: (last.side(), last.number),
            label,
            editor,
            _subscription: subscription,
        });
        self.line_selection = Some(LineSelection { file, anchor: range.start, head: range.end - 1, column });
        self.layout_dirty = true;
        cx.notify();
    }

    fn cancel_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        self.line_selection = None;
        self.layout_dirty = true;
        cx.notify();
    }

    fn submit_draft(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.as_ref() else { return };
        let text = draft.editor.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let state = &self.files[draft.file as usize];
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id: SharedString = format!("diff-comment-{}", NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)).into();
        let Some(comment) = build_review_comment(ReviewCommentInput {
            id,
            section_id: self.section.0.clone(),
            section_title: self.section.1.clone(),
            file: &state.diff,
            start: draft.start,
            end: draft.end,
            text: &text,
        }) else {
            return;
        };
        self.comments.push(SavedComment { file_key: state.key.clone(), anchor: draft.anchor, comment: comment.clone() });
        cx.emit(CodeViewEvent::Comment(comment));
        self.cancel_draft(cx);
    }

    /// Comments already filed in this view, for the composer to mirror.
    pub fn comments(&self) -> impl Iterator<Item = &ReviewComment> {
        self.comments.iter().map(|saved| &saved.comment)
    }

    fn delete_comment(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        self.comments.retain(|saved| &saved.comment.id != id);
        self.layout_dirty = true;
        cx.emit(CodeViewEvent::DeleteComment { id: id.clone() });
        cx.notify();
    }

    // --- find -------------------------------------------------------------

    fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in diff"));
            let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    this.set_find_query(query, cx);
                }
                InputEvent::PressEnter { shift, .. } => this.step_find(!*shift, window, cx),
                _ => {}
            });
            self.find = Some(Find {
                input,
                query: String::new(),
                matches: Vec::new(),
                by_line: HashMap::new(),
                current: None,
                _subscription: subscription,
            });
        }
        if let Some(find) = &self.find {
            find.input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// Opens the find bar, as Mod+F does.
    pub fn open_find_with(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(&OpenFind, window, cx);
    }

    fn close_find(&mut self, _: &CloseFind, window: &mut Window, cx: &mut Context<Self>) {
        if self.draft.is_some() {
            self.cancel_draft(cx);
            return;
        }
        if self.find.take().is_some() {
            window.focus(&self.focus_handle, cx);
            cx.notify();
        } else if self.selection.is_some() || self.line_selection.is_some() {
            self.selection = None;
            self.line_selection = None;
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    /// Opens the find bar with `query` typed into it.
    pub fn find(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(&OpenFind, window, cx);
        if let Some(find) = &self.find {
            let input = find.input.clone();
            input.update(cx, |input, cx| input.set_value(query.to_owned(), window, cx));
        }
        self.set_find_query(query.to_owned(), cx);
    }

    /// Sets the find query (case-insensitive) and recomputes matches.
    pub fn set_find_query(&mut self, query: String, cx: &mut Context<Self>) {
        if let Some(find) = self.find.as_mut() {
            find.query = query;
        }
        self.refresh_find(cx);
        if let Some(find) = self.find.as_mut() {
            find.current = (!find.matches.is_empty()).then_some(0);
        }
        self.reveal_current_match(cx);
    }

    /// Match count and the current match, for the find bar and tests.
    pub fn find_status(&self) -> Option<(usize, Option<usize>)> {
        self.find.as_ref().map(|f| (f.matches.len(), f.current))
    }

    fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else { return };
        find.matches.clear();
        find.by_line.clear();
        let needle = find.query.to_lowercase();
        if needle.is_empty() {
            cx.notify();
            return;
        }
        const MAX_MATCHES: usize = 10_000;
        'files: for (index, file) in self.files.iter().enumerate() {
            for (source, lines) in [(LineSource::HunkOld, &file.diff.old_lines), (LineSource::HunkNew, &file.diff.new_lines)] {
                for line_index in 0..lines.len() {
                    let line = LineRef { source, index: line_index as u32 };
                    // Context lines are in both lists; search them once, on the new side.
                    if source == LineSource::HunkOld && is_context_old(&file.diff, line_index as u32) {
                        continue;
                    }
                    let text = lines.line(line_index);
                    let haystack = if text.is_ascii() { text.to_ascii_lowercase() } else { text.to_lowercase() };
                    if haystack.len() != text.len() {
                        continue;
                    }
                    for (start, _) in haystack.match_indices(&needle) {
                        let position = find.matches.len();
                        find.matches.push(FindMatch { file: index as u32, line, start: start as u32, end: (start + needle.len()) as u32 });
                        find.by_line.entry((index as u32, line)).or_default().push(position);
                        if find.matches.len() >= MAX_MATCHES {
                            break 'files;
                        }
                    }
                }
            }
        }
        find.matches.sort_by_key(|m| (m.file, find_order(&self.files[m.file as usize].diff, m.line), m.start));
        find.by_line.clear();
        for (position, found) in find.matches.iter().enumerate() {
            find.by_line.entry((found.file, found.line)).or_default().push(position);
        }
        cx.notify();
    }

    fn step_find(&mut self, forward: bool, _: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else { return };
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        find.current = Some(match find.current {
            Some(current) if forward => (current + 1) % count,
            Some(current) => (current + count - 1) % count,
            None => 0,
        });
        self.reveal_current_match(cx);
    }

    /// Moves to the next (or previous) find match.
    pub fn find_next(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.step_find(forward, window, cx);
    }

    fn reveal_current_match(&mut self, cx: &mut Context<Self>) {
        let Some(found) = self.find.as_ref().and_then(|f| f.current.map(|c| f.matches[c])) else {
            cx.notify();
            return;
        };
        let index = found.file as usize;
        if self.files[index].collapsed {
            self.files[index].collapsed = false;
            self.layout_dirty = true;
            cx.emit(CodeViewEvent::CollapsedChanged { key: self.files[index].key.clone(), collapsed: false });
        }
        self.ensure_layout();
        let Some((_, layout)) = self.files[index].layout.as_ref() else { return };
        let row = layout.rows.iter().position(|row| match row.kind {
            RowKind::Line(cell) => cell.line == found.line || is_same_context(&self.files[index].diff, cell.line, found.line),
            RowKind::Split(left, right) => [left, right].into_iter().flatten().any(|cell| cell.line == found.line),
            _ => false,
        });
        if let Some(row) = row {
            let y = self.body_top(index) + layout.rows[row].top;
            let view = f32::from(self.bounds.size.height);
            if y < self.scroll.y + HEADER_HEIGHT || y + LINE_HEIGHT > self.scroll.y + view {
                self.scroll.y = y - view / 3.;
                self.clamp_scroll();
            }
        }
        cx.notify();
    }

    // --- painting -----------------------------------------------------------

    fn ensure_metrics(&mut self, window: &mut Window, cx: &App) {
        let theme = cx.app_theme();
        let theme_id = (theme.id.clone(), theme.appearance, self.options.scheme);
        if self.colors.is_none() || self.theme_id != theme_id {
            self.colors = Some(DiffColors::new(theme, self.options.scheme));
            self.theme_id = theme_id;
            self.shape_cache.clear();
            self.number_cache.clear();
        }
        // `--diffs-font-size` is the code font size setting, in the code font family setting.
        let font = mono_font(FontWeight::NORMAL);
        if self.font != font || self.font_size != theme.fonts.code {
            self.font_bold = Font { weight: FontWeight::BOLD, ..font.clone() };
            self.font_italic = Font { style: FontStyle::Italic, ..font.clone() };
            self.font = font;
            self.font_size = theme.fonts.code;
            self.shape_cache.clear();
            self.number_cache.clear();
            self.layout_dirty = true;
        }
        let size = px(self.font_size);
        let ch = {
            let text_system = window.text_system();
            let font_id = text_system.resolve_font(&self.font);
            text_system.advance(font_id, size, 'm').map(|s| f32::from(s.width)).unwrap_or(7.8)
        };
        let metrics = Metrics { ch };
        if self.metrics != Some(metrics) {
            self.metrics = Some(metrics);
            self.layout_dirty = true;
            self.shape_cache.clear();
        }
    }

    /// Lays out and shapes the visible code rows. Called from the canvas.
    fn prepaint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> PaintPlan {
        if self.bounds.size != bounds.size {
            self.layout_dirty = true;
            cx.notify();
        }
        self.bounds = bounds;
        self.ensure_layout();
        self.clamp_scroll();
        self.frame += 1;
        let (Some(metrics), Some(colors)) = (self.metrics, self.colors) else { return PaintPlan::default() };
        let palette = SyntaxPalette::for_appearance(colors.appearance);
        let view_height = f32::from(bounds.size.height);
        let (top, bottom) = (self.scroll.y, self.scroll.y + view_height);
        let mut plan = PaintPlan { rows: Vec::with_capacity(64), scrollbar: None, hatch_color: colors.buffer };
        let mut requests: SmallVec<[(DocKey, crate::syntax::TextLines, usize); 8]> = SmallVec::new();
        let origin = bounds.origin;
        let first_file = self.file_at(top).unwrap_or(0);
        for index in first_file..self.files.len() {
            let file_top = self.file_tops[index];
            if file_top > bottom {
                break;
            }
            if self.files[index].collapsed {
                continue;
            }
            let body_top = file_top + self.header_height();
            let Some(rows) = self.files[index].layout.as_ref().map(|(_, l)| l.rows_in(top - body_top..bottom - body_top)) else {
                continue;
            };
            for row_index in rows {
                let (row, digits, wrap) = {
                    let (input, layout) = self.files[index].layout.as_ref().expect("laid out");
                    (layout.rows[row_index], layout.digits, input.wrap_columns)
                };
                let cells: SmallVec<[(Column, Option<Cell>); 2]> = match row.kind {
                    RowKind::Line(cell) => smallvec::smallvec![(Column::Left, Some(cell))],
                    RowKind::Split(left, right) => smallvec::smallvec![(Column::Left, left), (Column::Right, right)],
                    _ => continue,
                };
                let y = body_top + row.top - self.scroll.y;
                let mut paint = self.recycled.pop().unwrap_or_else(|| PaintRow {
                    quads: SmallVec::new(),
                    stripes: None,
                    hatch: SmallVec::new(),
                    texts: SmallVec::new(),
                    numbers: SmallVec::new(),
                    code_clip: SmallVec::new(),
                });
                for (column, cell) in cells {
                    let (cell_x, cell_width, gutter) = self.cell_geometry(digits, column);
                    let row_bounds = |x: f32, w: f32| Bounds::new(origin + point(px(x), px(y)), size(px(w), px(row.height)));
                    let gutter_bounds = row_bounds(cell_x, gutter - 2.);
                    let code_bounds = row_bounds(cell_x + gutter, cell_width - gutter);
                    let Some(cell) = cell else {
                        paint.hatch.push(code_bounds);
                        continue;
                    };
                    let selected_rows = self.line_selection.filter(|s| s.file == index as u32 && s.column == column && s.rows().contains(&(row_index as u32)));
                    if selected_rows.is_some() {
                        paint.quads.push((gutter_bounds, colors.selected_number));
                        paint.quads.push((code_bounds, colors.selected_line));
                    } else if self.mode == ViewMode::Diff
                        && let Some((line_bg, number_bg, _)) = colors.line_backgrounds(cell.kind)
                    {
                        paint.quads.push((gutter_bounds, number_bg));
                        paint.quads.push((code_bounds, line_bg));
                    }
                    if self.mode == ViewMode::Diff {
                        match cell.kind {
                            LineKind::Addition => paint.quads.push((row_bounds(cell_x, 4.), colors.addition_base)),
                            LineKind::Deletion => paint.stripes = Some(row_bounds(cell_x, 4.)),
                            LineKind::Context => {}
                        }
                        if selected_rows.is_some() {
                            paint.quads.push((row_bounds(cell_x, 4.), colors.modified_base));
                        }
                    }
                    // Line number, right-aligned in the gutter's padded box.
                    let number_color = match (self.mode, cell.kind) {
                        (ViewMode::Diff, LineKind::Addition) => (colors.addition_base, 1u8),
                        (ViewMode::Diff, LineKind::Deletion) => (colors.deletion_base, 2u8),
                        _ => (colors.number, 0u8),
                    };
                    let number = self
                        .number_cache
                        .entry((cell.number, number_color.1))
                        .or_insert_with(|| {
                            let text: SharedString = cell.number.to_string().into();
                            let run = TextRun { len: text.len(), font: self.font.clone(), color: number_color.0, background_color: None, underline: None, strikethrough: None };
                            window.text_system().shape_line(text, px(self.font_size), &[run], None)
                        })
                        .clone();
                    let number_x = cell_x + gutter - 2. - metrics.ch - f32::from(number.width);
                    paint.numbers.push(PaintText { origin: origin + point(px(number_x), px(y)), line: number, clip: 0 });

                    // Code text, one shaped line per wrapped segment.
                    let state = &mut self.files[index];
                    let doc = state.doc(cell.line.source);
                    let state = &self.files[index];
                    let text = line_text(&state.diff, state.contents.as_deref(), cell.line);
                    let spans = doc.and_then(|doc| self.syntax.read(cx).line(doc, cell.line.index as usize));
                    if let Some(doc) = doc
                        && spans.is_none()
                    {
                        let lines = source_lines(&state.diff, state.contents.as_deref(), cell.line.source);
                        if !requests.iter().any(|(key, _, _)| *key == doc) {
                            requests.push((doc, lines.clone(), cell.line.index as usize));
                        } else if let Some(request) = requests.iter_mut().find(|(key, _, _)| *key == doc) {
                            request.2 = request.2.max(cell.line.index as usize);
                        }
                    }
                    let segments = match wrap {
                        Some(columns) => wrap_segments(text, columns),
                        None => smallvec::smallvec![0..text.len()],
                    };
                    let code_x = cell_x + gutter + metrics.ch - if wrap.is_some() { 0. } else { self.scroll.x };
                    let find_ranges = self.find_ranges(index as u32, cell.line);
                    let selection = self.selection_range(index as u32, row_index as u32, column, text.len());
                    for (segment_index, segment) in segments.iter().enumerate() {
                        let key = ShapeKey { version: state.diff.version, line: cell.line, segment: segment_index as u16, highlighted: spans.is_some() };
                        let frame = self.frame;
                        let shaped = match self.shape_cache.get_mut(&key) {
                            Some((line, used)) => {
                                *used = frame;
                                line.clone()
                            }
                            None => {
                                let shaped = shape_segment(window, &self.font, &self.font_bold, &self.font_italic, self.font_size, palette, colors.foreground, text, segment.clone(), spans);
                                self.shape_cache.insert(key, (shaped.clone(), frame));
                                shaped
                            }
                        };
                        let line_y = y + segment_index as f32 * LINE_HEIGHT;
                        let segment_origin = origin + point(px(code_x), px(line_y));
                        let x_of = |byte: usize, shaped: &ShapedLine| {
                            let local = byte.clamp(segment.start, segment.end) - segment.start;
                            let display = expand_tabs(&text[segment.start..segment.start + local], display_columns(&text[..segment.start])).0.len();
                            f32::from(shaped.x_for_index(display))
                        };
                        let band = |from: usize, to: usize, color: Hsla, shaped: &ShapedLine| -> Option<(Bounds<Pixels>, Hsla)> {
                            if to <= segment.start || from >= segment.end.max(segment.start + 1) {
                                return None;
                            }
                            let (x0, x1) = (x_of(from, shaped), x_of(to, shaped));
                            let x1 = if to > segment.end || (to == text.len() && to == segment.end && from < to && color == colors.text_selection) { x1 + metrics.ch * 0.5 } else { x1 };
                            Some((Bounds::new(segment_origin + point(px(x0), px(0.)), size(px((x1 - x0).max(1.)), px(LINE_HEIGHT))), color))
                        };
                        for &(start, end, current) in &find_ranges {
                            let color = if current { colors.search_match_current } else { colors.search_match };
                            if let Some(quad) = band(start as usize, end as usize, color, &shaped) {
                                paint.quads.push(quad);
                            }
                        }
                        if let Some((from, to)) = selection
                            && let Some(quad) = band(from, to, colors.text_selection, &shaped)
                        {
                            paint.quads.push(quad);
                        }
                        let clip = paint.code_clip.len();
                        paint.texts.push(PaintText { origin: segment_origin, line: shaped, clip });
                    }
                    paint.code_clip.push(code_bounds);
                }
                plan.rows.push(paint);
            }
        }
        for (doc, lines, through) in requests {
            self.syntax.update(cx, |store, cx| store.request(doc, &lines, through, cx));
        }
        if self.shape_cache.len() > 1_024 {
            let keep_after = self.frame.saturating_sub(4);
            self.shape_cache.retain(|_, (_, used)| *used >= keep_after);
        }
        if self.number_cache.len() > 2_048 {
            self.number_cache.clear();
        }
        if self.total_height > view_height {
            let (thumb_top, thumb_height) = self.scrollbar_thumb();
            let x = f32::from(bounds.size.width) - SCROLLBAR_WIDTH;
            plan.scrollbar = Some((
                Bounds::new(origin + point(px(x), px(thumb_top)), size(px(SCROLLBAR_WIDTH), px(thumb_height))),
                colors.muted.opacity(0.35),
            ));
        }
        plan
    }

    fn find_ranges(&self, file: u32, line: LineRef) -> SmallVec<[(u32, u32, bool); 2]> {
        let Some(find) = self.find.as_ref() else { return SmallVec::new() };
        let key_line = match line.source {
            LineSource::HunkOld if is_context_old(&self.files[file as usize].diff, line.index) => {
                context_new_ref(&self.files[file as usize].diff, line.index).unwrap_or(line)
            }
            _ => line,
        };
        find.by_line
            .get(&(file, key_line))
            .map(|positions| positions.iter().map(|&p| (find.matches[p].start, find.matches[p].end, find.current == Some(p))).collect())
            .unwrap_or_default()
    }

    fn selection_range(&self, file: u32, row: u32, column: Column, len: usize) -> Option<(usize, usize)> {
        let selection = self.selection.filter(|s| s.column == column)?;
        let (start, end) = selection.ordered();
        if start == end {
            return None;
        }
        let here = (file, row);
        if here < (start.file, start.row) || here > (end.file, end.row) {
            return None;
        }
        let from = if here == (start.file, start.row) { start.byte as usize } else { 0 };
        let to = if here == (end.file, end.row) { end.byte as usize } else { len + 1 };
        (to > from).then_some((from, to))
    }

    fn recycle(&mut self, mut plan: PaintPlan) {
        for mut row in plan.rows.drain(..) {
            row.quads.clear();
            row.stripes = None;
            row.hatch.clear();
            row.texts.clear();
            row.numbers.clear();
            row.code_clip.clear();
            self.recycled.push(row);
        }
    }
}

fn paint_plan(plan: &PaintPlan, bounds: Bounds<Pixels>, colors: &DiffColors, window: &mut Window, cx: &mut App) {
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for row in &plan.rows {
            for (quad_bounds, color) in &row.quads {
                window.paint_quad(fill(*quad_bounds, *color));
            }
            if let Some(stripes) = row.stripes {
                // Pierre's deletion bar: 1px stripes of the base colour over the line tint.
                window.paint_quad(fill(stripes, colors.deletion_line));
                let mut y = f32::from(stripes.origin.y);
                let bottom = f32::from(stripes.bottom());
                while y < bottom {
                    let stripe = Bounds::new(point(stripes.origin.x, px(y)), size(stripes.size.width, px(1.)));
                    window.paint_quad(fill(stripe, colors.deletion_base));
                    y += 2.;
                }
            }
            for hatch in &row.hatch {
                paint_hatch(*hatch, plan.hatch_color, window);
            }
            for number in &row.numbers {
                number.line.paint(number.origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx).ok();
            }
            for (index, clip) in row.code_clip.iter().enumerate() {
                window.with_content_mask(Some(ContentMask { bounds: *clip }), |window| {
                    for text in row.texts.iter().filter(|text| text.clip == index) {
                        text.line.paint(text.origin, px(LINE_HEIGHT), TextAlign::Left, None, window, cx).ok();
                    }
                });
            }
        }
        if let Some((thumb, color)) = plan.scrollbar {
            window.paint_quad(fill(thumb, color).corner_radii(px(3.)));
        }
    });
}

/// Pierre's split-view buffer: diagonal hatching for the side with no line.
fn paint_hatch(bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let mut builder = PathBuilder::stroke(px(1.4));
    let mut offset = -h;
    while offset < w {
        builder.move_to(point(px(x0 + offset.max(0.)), px(y0 + h - (offset.max(0.) - offset))));
        let end_x = (offset + h).min(w);
        builder.line_to(point(px(x0 + end_x), px(y0 + h - (end_x - offset))));
        offset += 8.;
    }
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn shape_segment(
    window: &mut Window,
    font: &Font,
    bold: &Font,
    italic: &Font,
    font_size: f32,
    palette: &SyntaxPalette,
    foreground: Hsla,
    text: &str,
    segment: Range<usize>,
    spans: Option<&[TokenSpan]>,
) -> ShapedLine {
    let start_column = display_columns(&text[..segment.start]);
    let (display, map) = expand_tabs(&text[segment.clone()], start_column);
    let display_of = |byte: usize| match &map {
        Some(map) => map[byte - segment.start] as usize,
        None => byte - segment.start,
    };
    let plain = |len: usize| TextRun { len, font: font.clone(), color: foreground, background_color: None, underline: None, strikethrough: None };
    let mut runs: SmallVec<[TextRun; 16]> = SmallVec::new();
    let mut cursor = 0usize;
    for span in spans.unwrap_or_default() {
        let (start, end) = ((span.start as usize).max(segment.start), (span.end as usize).min(segment.end));
        if start >= end {
            continue;
        }
        let (start, end) = (display_of(start), display_of(end));
        if start > cursor {
            runs.push(plain(start - cursor));
        }
        let style = palette.style(span.token);
        let run_font = if style.font_weight.is_some_and(|w| w.0 >= 600.) {
            bold
        } else if style.font_style == Some(FontStyle::Italic) {
            italic
        } else {
            font
        };
        runs.push(TextRun { len: end - start, font: run_font.clone(), color: style.color.unwrap_or(foreground), background_color: None, underline: None, strikethrough: None });
        cursor = end;
    }
    if cursor < display.len() || runs.is_empty() {
        runs.push(plain(display.len() - cursor));
    }
    window.text_system().shape_line(display.into_owned().into(), px(font_size), &runs, None)
}

/// True when old-side hunk line `index` is a context line (so it is also on the new side).
fn is_context_old(file: &FileDiff, index: u32) -> bool {
    context_new_ref(file, index).is_some()
}

/// The new-side line of an old-side context line.
fn context_new_ref(file: &FileDiff, index: u32) -> Option<LineRef> {
    use crate::diff_patch::HunkBlock;
    for hunk in &file.hunks {
        let (mut old, mut new) = (hunk.old_index, hunk.new_index);
        for block in &hunk.blocks {
            match *block {
                HunkBlock::Context { lines } => {
                    if (old..old + lines).contains(&index) {
                        return Some(LineRef { source: LineSource::HunkNew, index: new + (index - old) });
                    }
                    old += lines;
                    new += lines;
                }
                HunkBlock::Change { deletions, additions } => {
                    if (old..old + deletions).contains(&index) {
                        return None;
                    }
                    old += deletions;
                    new += additions;
                }
            }
        }
    }
    None
}

fn is_same_context(file: &FileDiff, row_line: LineRef, target: LineRef) -> bool {
    row_line.source == LineSource::HunkOld && context_new_ref(file, row_line.index) == Some(target)
}

/// Stacked-order position of a hunk line, for sorting find matches.
fn find_order(file: &FileDiff, line: LineRef) -> u64 {
    use crate::diff_patch::HunkBlock;
    let mut position = 0u64;
    for hunk in &file.hunks {
        let (mut old, mut new) = (hunk.old_index, hunk.new_index);
        for block in &hunk.blocks {
            match *block {
                HunkBlock::Context { lines } => {
                    if line.source == LineSource::HunkNew && (new..new + lines).contains(&line.index) {
                        return position + (line.index - new) as u64;
                    }
                    position += lines as u64;
                    old += lines;
                    new += lines;
                }
                HunkBlock::Change { deletions, additions } => {
                    if line.source == LineSource::HunkOld && (old..old + deletions).contains(&line.index) {
                        return position + (line.index - old) as u64;
                    }
                    if line.source == LineSource::HunkNew && (new..new + additions).contains(&line.index) {
                        return position + deletions as u64 + (line.index - new) as u64;
                    }
                    position += (deletions + additions) as u64;
                    old += deletions;
                    new += additions;
                }
            }
        }
    }
    position
}

fn word_range(text: &str, byte: usize) -> (usize, usize) {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let byte = byte.min(text.len());
    let start = text[..byte].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(byte, |(i, _)| i);
    let end = text[byte..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| byte + i);
    (start, end)
}

// --- rendering ------------------------------------------------------------

fn icon(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg().path(path).size(px(size)).flex_none().text_color(color)
}

impl CodeView {
    fn render_header(&self, index: usize, y: f32, colors: &DiffColors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let file = &self.files[index];
        let accessory = self.decorations.clone().and_then(|d| d.render_header_accessory(&file.key, window, cx));
        let diff = &file.diff;
        let (additions, deletions) = file.stat.unwrap_or((diff.additions, diff.deletions));
        let (change_icon, change_color) = match diff.change {
            ChangeType::New => ("icons/diffs/pierre-symbol-added.svg", colors.icon_new),
            ChangeType::Deleted => ("icons/diffs/pierre-symbol-deleted.svg", colors.icon_deleted),
            ChangeType::RenamePure | ChangeType::RenameChanged => ("icons/diffs/pierre-symbol-moved.svg", colors.icon_modified),
            ChangeType::Change => ("icons/diffs/pierre-symbol-modified.svg", colors.icon_modified),
        };
        let collapsed = file.collapsed;
        let hovered = self.hovered_header == Some(index as u32);
        // The copy button shows while the pointer is over this file (index.css `data-diff-copy-path`).
        let file_hovered = hovered || self.hovered.is_some_and(|(file, _, _)| file == index as u32);
        let path = diff.path.clone();
        let copied = self.copied_path.as_ref() == Some(&diff.path);
        let mono = self.font.family.clone();
        div()
            .id(("diff-header", index))
            .test_support()
            .absolute()
            .top(px(y))
            .left_0()
            // The scrollbar overlays the header, as Chromium's overlay scrollbar does.
            .right_0()
            .h(px(HEADER_HEIGHT))
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(8.))
            .pr(px(12.))
            .bg(colors.background)
            .text_size(px(12.))
            .line_height(px(12.))
            .text_color(colors.code_foreground)
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered_header = hovered.then_some(index as u32).or(this.hovered_header.filter(|h| *h != index as u32));
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_file(index, cx)))
            .on_mouse_down(MouseButton::Right, {
                let path = path.clone();
                cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(CodeViewEvent::HeaderContextMenu { path: path.clone(), position: event.position });
                })
            })
            .when(hovered, |el| {
                el.child(div().absolute().left_0().top_0().bottom_0().w(px(3.)).bg(colors.header_hover_edge))
            })
            .child(
                div()
                    .id(("diff-collapse", index))
                    .test_support()
                    .size(px(20.))
                    .ml(px(-2.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .hover(|s| s.bg(cx.app_theme().colors.toolbar_control_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_file(index, cx);
                    }))
                    .child(icon(
                        if collapsed { "icons/diffs/chevron-right.svg" } else { "icons/diffs/chevron-down.svg" },
                        16.,
                        colors.muted.opacity(0.8),
                    )),
            )
            .child(icon(change_icon, 16., change_color))
            .child(
                div()
                    .id(("diff-title", index))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .flex_shrink(1.)
                    .line_height(px(16.))
                    .when_some(diff.prev_path.clone(), |el, prev| {
                        el.child(div().min_w_0().flex_shrink(1.).opacity(0.7).text_ellipsis_start().whitespace_nowrap().overflow_hidden().child(prev))
                            .child(icon("icons/diffs/pierre-arrow-right-short.svg", 16., colors.code_foreground))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink(1.)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis_start()
                            .hover(|s| s.text_color(colors.title_hover))
                            .child(path.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let path = this.files[index].diff.path.clone();
                        cx.emit(CodeViewEvent::OpenFile { path });
                    })),
            )
            .child(
                div()
                    .id(("diff-copy", index))
                    .test_support()
                    .when(!file_hovered && !copied, |el| el.opacity(0.))
                    .size(px(20.))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .hover(|s| s.bg(cx.app_theme().colors.toolbar_control_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        let path = this.files[index].diff.path.clone();
                        let text = crate::components::diff_file_path_copy_button::format_file_path_for_clipboard(&path, this.workspace_root.as_deref());
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        this.copied_path = Some(path.clone());
                        cx.notify();
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(Duration::from_millis(1200)).await;
                            this.update(cx, |this, cx| {
                                if this.copied_path.as_ref() == Some(&path) {
                                    this.copied_path = None;
                                    cx.notify();
                                }
                            })
                            .ok();
                        })
                        .detach();
                    }))
                    .child(if copied {
                        icon("icons/diffs/check.svg", 12., colors.stat_addition)
                    } else {
                        icon("icons/diffs/copy.svg", 12., colors.muted)
                    }),
            )
            .child(div().flex_1())
            .children(accessory)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(self.metrics.map_or(7., |m| m.ch)))
                    .font_family(mono)
                    .text_size(px(11.))
                    .when(additions > 0, |el| el.child(div().text_color(colors.addition_base).child(format!("+{}", format_diff_count(additions as u64)))))
                    .when(deletions > 0, |el| el.child(div().text_color(colors.deletion_base).child(format!("-{}", format_diff_count(deletions as u64))))),
            )
            .into_any_element()
    }

    fn render_separator(&self, index: usize, gap: u32, hidden: u32, y: f32, colors: &DiffColors, cx: &mut Context<Self>) -> AnyElement {
        let file = &self.files[index];
        let expandable = file.contents.is_some() || matches!(self.mode, ViewMode::Diff);
        let gap = gap as usize;
        let first = gap == 0;
        let last = gap >= file.diff.hunks.len();
        let label = format!("{} unmodified line{}", format_diff_count(hidden as u64), if hidden == 1 { "" } else { "s" });
        let width = self.viewport_width();
        let button = |id: &'static str, path: &'static str, up: bool, flip: bool| {
            div()
                .id((id, index * 10_000 + gap))
                .test_support()
                .w(px(32.))
                .h_full()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .bg(colors.separator)
                .border_r_2()
                .border_color(colors.background)
                .text_color(colors.separator_text)
                .hover(|s| s.bg(colors.separator_hover).text_color(colors.code_foreground))
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.expand_gap(index, gap, up, event.modifiers().alt, cx);
                }))
                .child(icon(path, 16., colors.separator_text).when(flip, |s| s.with_transformation(Transformation::scale(size(1., -1.)))))
        };
        div()
            .absolute()
            .top(px(y))
            .left_0()
            .w(px(width))
            .h(px(SEPARATOR_HEIGHT))
            .flex()
            .bg(colors.separator)
            .text_size(px(11.))
            .text_color(colors.separator_text)
            .when(expandable && !first, |el| el.child(button("sep-down", "icons/diffs/pierre-expand.svg", false, false)))
            .when(expandable && !last, |el| el.child(button("sep-up", "icons/diffs/pierre-expand.svg", true, true)))
            .child(
                div()
                    .id(("sep-label", index * 10_000 + gap))
                    .test_support()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .px(px(self.metrics.map_or(8., |m| m.ch)))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .hover(|s| s.bg(colors.separator_hover).text_color(colors.code_foreground))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.expand_gap(index, gap, !last, true, cx);
                    }))
                    .child(label),
            )
            .into_any_element()
    }

    fn render_notice(&self, notice: Notice, index: usize, y: f32, colors: &DiffColors) -> AnyElement {
        let diff = &self.files[index].diff;
        let text: SharedString = match notice {
            Notice::Binary => "Binary file not shown".into(),
            Notice::ModeChange => match diff.mode_change() {
                Some((old, new)) => format!("File mode changed from {old} to {new}").into(),
                None => "File mode changed".into(),
            },
            Notice::RenameOnly => "File renamed without changes".into(),
            Notice::Empty => if diff.change == ChangeType::New { "Empty file added".into() } else { "No content changes".into() },
        };
        div()
            .absolute()
            .top(px(y))
            .left_0()
            .w(px(self.viewport_width()))
            .h(px(NOTICE_HEIGHT))
            .flex()
            .items_center()
            .px(px(16.))
            .bg(colors.separator)
            .text_size(px(12.))
            .text_color(colors.separator_text)
            .child(text)
            .into_any_element()
    }

    fn render_no_newline(&self, old: bool, new: bool, y: f32, colors: &DiffColors) -> AnyElement {
        let _ = (old, new);
        div()
            .absolute()
            .top(px(y))
            .left_0()
            .w(px(self.viewport_width()))
            .h(px(LINE_HEIGHT))
            .flex()
            .items_center()
            .pl(px(self.metrics.map_or(40., |m| m.gutter_width(3) + m.ch)))
            .text_size(px(12.))
            .text_color(colors.number)
            .child("No newline at end of file")
            .into_any_element()
    }

    fn render_annotation(&self, index: usize, side: Side, number: u32, y: f32, height: f32, colors: &DiffColors, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = self.files[index].key.clone();
        let theme = cx.app_theme().colors;
        let mut column = div().flex().flex_col();
        if let Some(decorations) = self.decorations.clone()
            && let Some(element) = decorations.render_annotation(&key, (side, number), window, cx)
        {
            column = column.child(element);
        }
        for saved in self.comments.iter().filter(|c| c.file_key == key && c.anchor == (side, number)) {
            let id = saved.comment.id.clone();
            column = column.child(
                div()
                    .id(SharedString::from(format!("comment-{id}")))
                    .test_support()
                    .group("comment")
                    .flex()
                    .items_start()
                    .gap(px(10.))
                    .border_l_2()
                    .border_color(colors.primary.opacity(0.55))
                    .bg(colors.primary.opacity(0.045))
                    .px(px(12.))
                    .py(px(10.))
                    .text_size(px(14.))
                    .line_height(px(20.))
                    .text_color(theme.text)
                    .child(div().h(px(20.)).flex().items_center().child(icon("icons/diffs/message-circle.svg", 14., colors.primary.opacity(0.7))))
                    .child(div().flex_1().min_w_0().child(saved.comment.text.clone()))
                    .child(
                        div()
                            .id(SharedString::from(format!("comment-delete-{id}")))
                            .test_support()
                            .invisible()
                            .group_hover("comment", |s| s.visible())
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .hover(|s| s.bg(theme.toolbar_control_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.delete_comment(&id, cx);
                            }))
                            .child(icon("icons/diffs/trash-2.svg", 12., colors.muted)),
                    ),
            );
        }
        if let Some(draft) = self.draft.as_ref().filter(|d| d.file as usize == index && d.anchor == (side, number)) {
            let empty = draft.editor.read(cx).value().trim().is_empty();
            column = column.child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().h(px(70.)).child(Textarea::new(&draft.editor).h(px(70.))))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(div().mr_auto().text_size(px(10.)).text_color(colors.muted.opacity(0.7)).child(format!("Lines {} · ⌘/Ctrl Enter to send", draft.label)))
                            .child(
                                div()
                                    .id("draft-cancel")
                                    .test_support()
                                    .h(px(24.))
                                    .px(px(7.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.))
                                    .text_size(px(12.))
                                    .text_color(colors.muted)
                                    .hover(|s| s.bg(theme.toolbar_control_hover).text_color(theme.text))
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_draft(cx)))
                                    .child("Cancel"),
                            )
                            .child(
                                div()
                                    .id("draft-submit")
                                    .test_support()
                                    .h(px(24.))
                                    .px(px(7.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.))
                                    .text_size(px(12.))
                                    .bg(theme.text)
                                    .text_color(theme.canvas)
                                    .when(empty, |s| s.opacity(0.64))
                                    .on_click(cx.listener(|this, _, window, cx| this.submit_draft(window, cx)))
                                    .child("Comment"),
                            ),
                    ),
            );
        }
        div()
            .id(("annotation", index * 1_000_000 + number as usize))
            .test_support()
            .absolute()
            .top(px(y))
            .left_0()
            .w(px(self.viewport_width()))
            .h(px(height))
            .overflow_hidden()
            .bg(colors.background)
            .border_y_1()
            .border_color(colors.border.opacity(0.3))
            .child(column)
            .into_any_element()
    }

    fn render_find_bar(&self, colors: &DiffColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let theme = cx.app_theme().colors;
        let status = match (find.matches.len(), find.current) {
            (0, _) if find.query.is_empty() => SharedString::default(),
            (0, _) => "No results".into(),
            (count, Some(current)) => format!("{}/{}", current + 1, count).into(),
            (count, None) => format!("{count}").into(),
        };
        let nav = |id: &'static str, path: &'static str, forward: bool| {
            div()
                .id(id)
                .test_support()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .hover(|s| s.bg(theme.toolbar_control_hover))
                .on_click(cx.listener(move |this, _, window, cx| this.step_find(forward, window, cx)))
                .child(icon(path, 14., colors.muted))
        };
        Some(
            div()
                .absolute()
                .top(px(8.))
                .right(px(16.))
                .w(px(300.))
                .flex()
                .items_center()
                .gap(px(4.))
                .p(px(4.))
                .rounded(px(8.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_overlay)
                .shadow_md()
                .key_context("DiffFind")
                .child(
                    div()
                        .flex_1()
                        .h(px(26.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.input)
                        .bg(theme.input.opacity(0.32))
                        .child(Input::new(&find.input).appearance(false).small()),
                )
                .child(div().text_size(px(11.)).text_color(colors.muted).min_w(px(44.)).child(status))
                .child(nav("find-prev", "icons/diffs/arrow-up.svg", false))
                .child(nav("find-next", "icons/diffs/arrow-down.svg", true))
                .child(
                    div()
                        .id("find-close")
                        .test_support()
                        .size(px(24.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .hover(|s| s.bg(theme.toolbar_control_hover))
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(&CloseFind, window, cx)))
                        .child(icon("icons/diffs/x.svg", 14., colors.muted)),
                )
                .into_any_element(),
        )
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_metrics(window, cx);
        if self.bounds.size.width == px(0.) {
            self.bounds.size = window.viewport_size();
        }
        self.ensure_layout();
        match self.pending_reveal.take() {
            Some(Reveal::File(key)) => {
                self.reveal_file(&key, cx);
            }
            Some(Reveal::Line(number)) => self.reveal_line(number, cx),
            None => {}
        }
        self.clamp_scroll();
        let colors = self.colors.expect("colors resolved");
        let view_height = f32::from(self.bounds.size.height);
        let (top, bottom) = (self.scroll.y, self.scroll.y + view_height);
        let mut overlays: Vec<AnyElement> = Vec::new();
        let sticky = self.sticky_file();
        if let Some(first) = self.file_at(top) {
            for index in first..self.files.len() {
                let file_top = self.file_tops[index];
                if file_top > bottom {
                    break;
                }
                if self.mode == ViewMode::Diff && file_top + HEADER_HEIGHT > top && sticky.is_none_or(|(s, _)| s != index) {
                    overlays.push(self.render_header(index, file_top - top, &colors, window, cx));
                }
                if self.files[index].collapsed {
                    continue;
                }
                let body_top = file_top + self.header_height();
                let Some((_, layout)) = self.files[index].layout.as_ref() else { continue };
                let rows: Vec<Row> = layout.rows[layout.rows_in(top - body_top..bottom - body_top)].to_vec();
                for row in rows {
                    let y = body_top + row.top - top;
                    match row.kind {
                        RowKind::Separator { gap, hidden } => overlays.push(self.render_separator(index, gap, hidden, y, &colors, cx)),
                        RowKind::Notice(notice) => overlays.push(self.render_notice(notice, index, y, &colors)),
                        RowKind::NoNewline { old, new } => overlays.push(self.render_no_newline(old, new, y, &colors)),
                        RowKind::Annotation { side, number } => {
                            overlays.push(self.render_annotation(index, side, number, y, row.height, &colors, window, cx))
                        }
                        _ => {}
                    }
                }
            }
        }
        // The gutter "+" that starts a comment on the hovered line.
        if self.options.comments && self.mode == ViewMode::Diff && self.draft.is_none()
            && let Some((file, row, column)) = self.hovered
            && let Some((_, layout)) = self.files.get(file as usize).and_then(|f| f.layout.as_ref())
            && let Some(row_data) = layout.rows.get(row as usize)
            && self.row_cell(file, row, column).is_some()
            && self.metrics.is_some()
        {
            let (cell_x, _, gutter) = self.cell_geometry(layout.digits, column);
            let y = self.body_top(file as usize) + row_data.top - top;
            overlays.push(
                div()
                    .id("gutter-comment")
                    .test_support()
                    .absolute()
                    .top(px(y))
                    .left(px(cell_x + gutter - 2. - LINE_HEIGHT))
                    .size(px(LINE_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(colors.modified_base)
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.begin_comment(file, row, column, window, cx);
                    }))
                    .child(icon("icons/diffs/plus.svg", 14., colors.background))
                    .into_any_element(),
            );
        }
        if let Some((index, offset)) = sticky {
            overlays.push(self.render_header(index, offset, &colors, window, cx));
        }
        if let Some(bar) = self.render_find_bar(&colors, cx) {
            overlays.push(bar);
        }

        let view = cx.entity();
        let paint_view = view.clone();
        div()
            .id("code-view")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::select_all_lines))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::close_find))
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(colors.background)
            .cursor(CursorStyle::IBeam)
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag = None))
            .child(
                canvas(
                    move |bounds, window, cx| view.update(cx, |this, cx| this.prepaint(bounds, window, cx)),
                    move |bounds, plan, window, cx| {
                        let colors = paint_view.read(cx).colors.expect("colors resolved");
                        paint_plan(&plan, bounds, &colors, window, cx);
                        paint_view.update(cx, |this, _| this.recycle(plan));
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(overlays)
    }
}
