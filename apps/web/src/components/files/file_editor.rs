//! The editable text surface of the file viewer: monospace lines with a
//! line-number gutter, syntax colours, selection, clipboard, undo, IME, find
//! and line reveal. Stands in for `@pierre/diffs`' `EditProvider`. Only the
//! visible rows are shaped each frame; GPUI caches shaped lines, so a
//! keystroke reshapes the one line it changed.
//!
//! With word wrap on, a line breaks into visual rows at the width of the text
//! area (after the last space that fits, else at the column limit, as
//! `white-space: pre-wrap` does). Without it every line is one row.

use std::{cell::RefCell, ops::Range, rc::Rc};

use gpui_kit::{
    App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, FontStyle, FontWeight, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollEvent, ShapedLine, SharedString,
    Style, Styled as _, TextAlign, TextRun, UTF16Selection, Window, actions, div, fill, point, prelude::*, px,
    relative, size,
};
use gpui_kit::{
    Subscription,
    component::input::{InputEvent, InputState},
};
use smallvec::{SmallVec, smallvec};

use super::{
    editor_buffer::{Buffer, EditKind, History, Pos, Selection},
    editor_highlight::EditorHighlighter,
};
use crate::{
    ActiveAppTheme as _,
    components::{
        icons::{Icon, IconName, IconSize},
        ui::input::{InputSize, TextInput},
    },
    i18n::t,
    syntax::{SyntaxPalette, TokenSpan, language_for_path},
};

const CONTEXT: &str = "FileEditor";
/// Row height of the code surface (Pierre's `--diffs-line-height`); the font size is the
/// code size setting (`--diffs-font-size`).
pub const EDITOR_LINE_HEIGHT: f32 = 20.;
const TAB_WIDTH: usize = 4;
/// Highlighting work allowed per frame before the rest is deferred to the next.
const HIGHLIGHT_BUDGET_MS: f32 = 4.;

actions!(
    file_editor,
    [
        MoveLeft, MoveRight, MoveUp, MoveDown, SelectLeft, SelectRight, SelectUp, SelectDown, MoveWordLeft,
        MoveWordRight, SelectWordLeft, SelectWordRight, MoveLineStart, MoveLineEnd, SelectLineStart, SelectLineEnd,
        MoveDocStart, MoveDocEnd, SelectDocStart, SelectDocEnd, PageUp, PageDown, Backspace, Delete, DeleteWordLeft,
        Newline, Indent, Undo, Redo, SelectAll, Copy, Cut, Paste, Save, OpenFind, FindNext, FindPrevious, CloseFind
    ]
);

/// Registers the editor's key bindings.
pub fn init(cx: &mut App) {
    let c = Some(CONTEXT);
    let mac = cfg!(target_os = "macos");
    macro_rules! m {
        ($key:literal) => {
            if cfg!(target_os = "macos") { concat!("cmd-", $key) } else { concat!("ctrl-", $key) }
        };
    }
    let mut bindings = vec![
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("home", MoveLineStart, c),
        KeyBinding::new("end", MoveLineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("tab", Indent, c),
        KeyBinding::new(m!("a"), SelectAll, c),
        KeyBinding::new(m!("c"), Copy, c),
        KeyBinding::new(m!("x"), Cut, c),
        KeyBinding::new(m!("v"), Paste, c),
        KeyBinding::new(m!("z"), Undo, c),
        KeyBinding::new(m!("shift-z"), Redo, c),
        KeyBinding::new(m!("s"), Save, c),
        KeyBinding::new(m!("f"), OpenFind, c),
        KeyBinding::new(m!("g"), FindNext, c),
        KeyBinding::new(m!("shift-g"), FindPrevious, c),
        KeyBinding::new("escape", CloseFind, Some("FileEditorFind")),
    ];
    if mac {
        bindings.extend([
            KeyBinding::new("alt-left", MoveWordLeft, c),
            KeyBinding::new("alt-right", MoveWordRight, c),
            KeyBinding::new("alt-shift-left", SelectWordLeft, c),
            KeyBinding::new("alt-shift-right", SelectWordRight, c),
            KeyBinding::new("cmd-left", MoveLineStart, c),
            KeyBinding::new("cmd-right", MoveLineEnd, c),
            KeyBinding::new("cmd-shift-left", SelectLineStart, c),
            KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
            KeyBinding::new("cmd-up", MoveDocStart, c),
            KeyBinding::new("cmd-down", MoveDocEnd, c),
            KeyBinding::new("cmd-shift-up", SelectDocStart, c),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
            KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-left", MoveWordLeft, c),
            KeyBinding::new("ctrl-right", MoveWordRight, c),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
            KeyBinding::new("ctrl-home", MoveDocStart, c),
            KeyBinding::new("ctrl-end", MoveDocEnd, c),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, c),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, c),
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
            KeyBinding::new("ctrl-y", Redo, c),
        ]);
    }
    cx.bind_keys(bindings);
}

/// What the editor tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileEditorEvent {
    /// The text changed; `dirty` compares it with the last saved text.
    Changed { dirty: bool },
    /// mod+s.
    SaveRequested,
}

/// Find state: the query field and every match, in document order.
struct Find {
    input: Entity<InputState>,
    query: String,
    matches: Vec<(usize, Range<usize>)>,
    current: Option<usize>,
    _subscription: Subscription,
}

/// Geometry from the last frame, for hit testing and IME bounds.
#[derive(Clone, Default)]
struct FrameGeometry {
    bounds: Bounds<Pixels>,
    text_left: Pixels,
    ch: Pixels,
    first_row: usize,
    lines: Vec<Rc<ShapedLineEntry>>,
}

/// One visual row: a line, or the part of a wrapped line between two breaks.
struct ShapedLineEntry {
    shaped: ShapedLine,
    /// Display index for each byte of the row when it has tabs.
    map: Option<Vec<u32>>,
    line: usize,
    /// The row's byte range in its line.
    start: usize,
    end: usize,
    /// The line's last row, which owns the position at the line's end.
    last: bool,
}

impl ShapedLineEntry {
    /// Display index of a byte column of the line, clamped to this row.
    fn display_index(&self, col: usize) -> usize {
        let col = col.clamp(self.start, self.end) - self.start;
        self.map.as_ref().map_or(col, |m| m.get(col).copied().unwrap_or(*m.last().unwrap_or(&0)) as usize)
    }

    /// Whether `col` of this row's line is drawn on this row.
    fn holds(&self, col: usize) -> bool {
        col >= self.start && (col < self.end || (col == self.end && self.last))
    }

    fn byte_for_display(&self, display: usize) -> usize {
        match &self.map {
            None => display,
            Some(map) => map.partition_point(|&d| (d as usize) < display),
        }
    }
}

/// An editable (or read-only) text document view.
pub struct FileEditor {
    focus_handle: FocusHandle,
    path: SharedString,
    buffer: Buffer,
    history: History,
    selection: Selection,
    marked: Option<Range<Pos>>,
    highlighter: Option<EditorHighlighter>,
    read_only: bool,
    /// Text as last saved or loaded, for the dirty flag.
    saved_text: String,
    dirty: bool,
    scroll: Point<Pixels>,
    scroll_to_caret: bool,
    /// Column the caret keeps while moving up and down.
    goal_x: Option<Pixels>,
    /// A line revealed from a link, tinted until the caret moves.
    revealed_line: Option<usize>,
    find: Option<Find>,
    dragging: bool,
    geometry: Rc<RefCell<FrameGeometry>>,
    wrap: bool,
    /// Row layout for the current wrap width; `None` when wrap is off or the text changed.
    wrap_layout: Option<WrapLayout>,
}

/// Where each line's rows start when wrapped at `cols` columns.
struct WrapLayout {
    cols: usize,
    /// First row of each line, then the total row count.
    row_starts: Vec<u32>,
}

impl EventEmitter<FileEditorEvent> for FileEditor {}

impl Focusable for FileEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FileEditor {
    pub fn new(path: SharedString, contents: &str, read_only: bool, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let highlighter = language_for_path(&path).map(EditorHighlighter::new);
        Self {
            focus_handle: cx.focus_handle(),
            path,
            buffer: Buffer::new(contents),
            history: History::default(),
            selection: Selection::default(),
            marked: None,
            highlighter,
            read_only,
            saved_text: contents.to_owned(),
            dirty: false,
            scroll: Point::default(),
            scroll_to_caret: false,
            goal_x: None,
            revealed_line: None,
            find: None,
            dragging: false,
            geometry: Rc::default(),
            wrap: false,
            wrap_layout: None,
        }
    }

    pub fn path(&self) -> &SharedString {
        &self.path
    }

    /// The current text with the file's line endings.
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn line_count(&self) -> usize {
        self.buffer.len_lines()
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    /// Turns soft wrap on or off (`wordWrap`).
    pub fn set_wrap(&mut self, wrap: bool, cx: &mut Context<Self>) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.wrap_layout = None;
            self.scroll.x = px(0.);
            self.scroll_to_caret = true;
            cx.notify();
        }
    }

    pub fn wraps(&self) -> bool {
        self.wrap
    }

    /// Visual rows of the document at the last drawn width.
    pub fn row_count(&self) -> usize {
        self.wrap_layout.as_ref().map_or(self.buffer.len_lines(), |l| *l.row_starts.last().unwrap_or(&0) as usize)
    }

    /// Lays out rows for `cols` columns when wrapping and the layout is stale.
    fn ensure_wrap(&mut self, cols: usize) {
        if !self.wrap {
            self.wrap_layout = None;
            return;
        }
        if self.wrap_layout.as_ref().is_some_and(|l| l.cols == cols) {
            return;
        }
        let mut row_starts = Vec::with_capacity(self.buffer.len_lines() + 1);
        let mut total = 0u32;
        for line in self.buffer.lines() {
            row_starts.push(total);
            total += wrap_starts(line, cols).len() as u32;
        }
        row_starts.push(total);
        self.wrap_layout = Some(WrapLayout { cols, row_starts });
    }

    /// Re-counts one line's rows after an edit inside it.
    fn rewrap_line(&mut self, line: usize) {
        let Some(layout) = &mut self.wrap_layout else { return };
        let rows = wrap_starts(self.buffer.line(line), layout.cols).len() as i64;
        let delta = rows - (layout.row_starts[line + 1] - layout.row_starts[line]) as i64;
        if delta != 0 {
            for start in &mut layout.row_starts[line + 1..] {
                *start = (*start as i64 + delta) as u32;
            }
        }
    }

    /// The first row of a line.
    fn line_row(&self, line: usize) -> usize {
        self.wrap_layout.as_ref().map_or(line, |l| l.row_starts.get(line).copied().unwrap_or(0) as usize)
    }

    /// The line a row belongs to, and which of its rows it is.
    fn row_line(&self, row: usize) -> (usize, usize) {
        match &self.wrap_layout {
            None => (row.min(self.buffer.len_lines() - 1), 0),
            Some(layout) => {
                let line = layout.row_starts.partition_point(|&start| start as usize <= row).saturating_sub(1);
                let line = line.min(self.buffer.len_lines() - 1);
                (line, row - layout.row_starts[line] as usize)
            }
        }
    }

    /// Byte offsets where a line's rows start.
    fn segments(&self, line: usize) -> SmallVec<[usize; 4]> {
        match &self.wrap_layout {
            Some(layout) => wrap_starts(self.buffer.line(line), layout.cols),
            None => smallvec![0],
        }
    }

    /// The row a position is drawn on, and the byte where that row starts.
    fn pos_row(&self, pos: Pos) -> (usize, usize) {
        let segments = self.segments(pos.line);
        let sub = segments.partition_point(|&start| start <= pos.col).saturating_sub(1);
        (self.line_row(pos.line) + sub, segments[sub])
    }

    /// Records `text` as saved; the editor is dirty again only if it now differs.
    pub fn mark_saved(&mut self, text: String, cx: &mut Context<Self>) {
        self.saved_text = text;
        self.history.close_group();
        self.refresh_dirty(cx);
    }

    /// Replaces the whole document (a newer copy from the server), keeping the caret line.
    pub fn replace_contents(&mut self, contents: &str, cx: &mut Context<Self>) {
        let caret = self.selection.head;
        self.buffer = Buffer::new(contents);
        self.history = History::default();
        self.highlighter = language_for_path(&self.path).map(EditorHighlighter::new);
        self.saved_text = contents.to_owned();
        self.dirty = false;
        self.wrap_layout = None;
        self.selection = Selection::caret(self.buffer.clamp(caret));
        self.recompute_find();
        cx.notify();
    }

    /// Shows an unsaved draft over the loaded text: the editor stays dirty
    /// against the loaded copy until the draft is written.
    pub fn restore_draft(&mut self, draft: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::new(draft);
        self.history = History::default();
        self.highlighter = language_for_path(&self.path).map(EditorHighlighter::new);
        self.wrap_layout = None;
        self.dirty = self.buffer.text() != self.saved_text;
        cx.notify();
    }

    /// Selects and centres a 1-based line, as `fileLineReveal` does.
    pub fn reveal_line(&mut self, line: u32, cx: &mut Context<Self>) {
        let index = (line.max(1) as usize - 1).min(self.buffer.len_lines() - 1);
        self.selection = Selection::caret(Pos::new(index, 0));
        self.revealed_line = Some(index);
        let height = self.geometry.borrow().bounds.size.height;
        let row = self.line_row(index);
        self.scroll.y = (px(EDITOR_LINE_HEIGHT) * row as f32 - (height - px(EDITOR_LINE_HEIGHT)) / 2.).max(px(0.));
        cx.notify();
    }

    /// The caret, for checks.
    pub fn caret(&self) -> Pos {
        self.selection.head
    }

    /// Places the caret, for scripted checks and for clicks from elsewhere.
    pub fn set_caret(&mut self, pos: Pos, cx: &mut Context<Self>) {
        self.selection = Selection::caret(self.buffer.clamp(pos));
        self.history.close_group();
        self.scroll_to_caret = true;
        cx.notify();
    }

    /// Bytes held by the text and highlight spans, for memory reports.
    pub fn heap_size(&self) -> usize {
        self.buffer.heap_size() + self.saved_text.capacity() + self.highlighter.as_ref().map_or(0, |h| h.heap_size())
    }

    /// The syntax tokens of a line as last highlighted, for checks.
    pub fn line_tokens(&self, line: usize) -> Option<Vec<crate::syntax::SyntaxToken>> {
        self.highlighter.as_ref()?.line(line).map(|spans| spans.iter().map(|s| s.token).collect())
    }

    /// Opens the find bar with `query`, for scripted checks.
    pub fn find(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(&OpenFind, window, cx);
        if let Some(find) = &mut self.find {
            find.query = query.to_owned();
            find.input.update(cx, |input, cx| input.set_value(query.to_owned(), window, cx));
        }
        self.recompute_find();
        self.find_step(true, cx);
    }

    /// `(current, total)` find matches.
    pub fn find_status(&self) -> Option<(Option<usize>, usize)> {
        self.find.as_ref().map(|f| (f.current, f.matches.len()))
    }

    /// Compares the whole text with the saved copy: after undo, redo and saves.
    fn refresh_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = self.buffer.text() != self.saved_text;
        cx.emit(FileEditorEvent::Changed { dirty: self.dirty });
    }

    /// Replaces `range` with `text` and moves the caret after it.
    fn replace(&mut self, range: Range<Pos>, text: &str, kind: EditKind, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let removed = range.end.line - range.start.line;
        let inserted = text.matches('\n').count();
        let caret = self.buffer.edit(&mut self.history, self.selection, range.clone(), text, kind);
        if let Some(h) = &mut self.highlighter {
            h.edited(range.start.line, removed + 1, inserted + 1);
        }
        if removed == 0 && inserted == 0 {
            self.rewrap_line(range.start.line);
        } else {
            self.wrap_layout = None;
        }
        self.selection = Selection::caret(caret);
        self.revealed_line = None;
        self.goal_x = None;
        self.scroll_to_caret = true;
        if self.find.is_some() {
            self.recompute_find();
        }
        // Any edit counts as unsaved; comparing every keystroke would join the whole file.
        self.dirty = true;
        cx.emit(FileEditorEvent::Changed { dirty: true });
        cx.notify();
    }

    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let kind = if text.contains('\n') { EditKind::Other } else { EditKind::Insert };
        self.replace(self.selection.range(), text, kind, cx);
    }

    fn move_to(&mut self, pos: Pos, select: bool, cx: &mut Context<Self>) {
        let pos = self.buffer.clamp(pos);
        if select {
            self.selection.head = pos;
        } else {
            self.selection = Selection::caret(pos);
        }
        self.history.close_group();
        self.revealed_line = None;
        self.scroll_to_caret = true;
        cx.notify();
    }

    fn horizontal(&mut self, forward: bool, select: bool, word: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        if !select && !self.selection.is_empty() && !word {
            let range = self.selection.range();
            return self.move_to(if forward { range.end } else { range.start }, false, cx);
        }
        let head = self.selection.head;
        let pos = match (forward, word) {
            (true, false) => self.buffer.next_boundary(head),
            (false, false) => self.buffer.prev_boundary(head),
            (true, true) => self.buffer.next_word(head),
            (false, true) => self.buffer.prev_word(head),
        };
        self.move_to(pos, select, cx);
    }

    /// Moves by visual rows: lines, or wrapped rows when word wrap is on.
    fn vertical(&mut self, rows: isize, select: bool, cx: &mut Context<Self>) {
        let head = self.selection.head;
        let (row, _) = self.pos_row(head);
        let target = (row as isize + rows).clamp(0, self.row_count() as isize - 1) as usize;
        if target == row {
            let pos = if rows < 0 { Pos::new(0, 0) } else { self.buffer.end() };
            return self.move_to(pos, select, cx);
        }
        let goal = self.goal_x.unwrap_or_else(|| self.x_for(head));
        let (line, sub) = self.row_line(target);
        let col = self.col_for_x(line, sub, goal);
        self.move_to(Pos::new(line, col), select, cx);
        self.goal_x = Some(goal);
    }

    /// Caret x within its row, from mono cell math (tabs expanded).
    fn x_for(&self, pos: Pos) -> Pixels {
        let ch = self.geometry.borrow().ch;
        let (_, start) = self.pos_row(pos);
        ch * display_columns(&self.buffer.line(pos.line)[start..pos.col]) as f32
    }

    /// The byte column nearest `x` on row `sub` of `line`.
    fn col_for_x(&self, line: usize, sub: usize, x: Pixels) -> usize {
        let ch = self.geometry.borrow().ch.max(px(1.));
        let target = (x / ch).round() as usize;
        let text = self.buffer.line(line);
        let segments = self.segments(line);
        let start = segments.get(sub).copied().unwrap_or(0);
        let next = segments.get(sub + 1).copied();
        let end = next.unwrap_or(text.len());
        let mut columns = 0;
        for (index, c) in text[start..end].char_indices() {
            let width = if c == '\t' { TAB_WIDTH - columns % TAB_WIDTH } else { 1 };
            if columns + width > target {
                return start + if target - columns > width / 2 { index + c.len_utf8() } else { index };
            }
            columns += width;
        }
        // A wrapped row's end is the next row's start; stop before its last character instead.
        match next {
            Some(_) => text[..end].char_indices().next_back().map_or(start, |(index, _)| index.max(start)),
            None => end,
        }
    }

    fn page_lines(&self) -> isize {
        ((self.geometry.borrow().bounds.size.height / px(EDITOR_LINE_HEIGHT)).floor() as isize - 1).max(1)
    }

    fn pos_for_point(&self, position: Point<Pixels>) -> Pos {
        let geometry = self.geometry.borrow();
        let y = position.y - geometry.bounds.origin.y + self.scroll.y;
        let row = ((y / px(EDITOR_LINE_HEIGHT)).floor().max(0.) as usize).min(self.row_count().saturating_sub(1));
        let x = position.x - geometry.bounds.origin.x - geometry.text_left + self.scroll.x;
        if let Some(entry) = row.checked_sub(geometry.first_row).and_then(|i| geometry.lines.get(i)) {
            let display = entry.shaped.closest_index_for_x(x.max(px(0.)));
            let mut col = entry.start + entry.byte_for_display(display);
            if !entry.last && col >= entry.end {
                let text = self.buffer.line(entry.line);
                col = text[..entry.end].char_indices().next_back().map_or(entry.start, |(index, _)| index.max(entry.start));
            }
            return self.buffer.clamp(Pos::new(entry.line, col));
        }
        drop(geometry);
        let (line, sub) = self.row_line(row);
        Pos::new(line, self.col_for_x(line, sub, x))
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let pos = self.pos_for_point(event.position);
        self.goal_x = None;
        match event.click_count {
            2 => {
                let word = self.buffer.word_at(pos);
                self.selection = Selection { anchor: word.start, head: word.end };
            }
            3 => {
                let end = if pos.line + 1 < self.buffer.len_lines() { Pos::new(pos.line + 1, 0) } else { self.buffer.end() };
                self.selection = Selection { anchor: Pos::new(pos.line, 0), head: end };
            }
            _ if event.modifiers.shift => self.selection.head = pos,
            _ => self.selection = Selection::caret(pos),
        }
        self.history.close_group();
        self.revealed_line = None;
        self.dragging = true;
        cx.notify();
    }

    fn on_mouse_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.dragging {
            self.selection.head = self.pos_for_point(position);
            self.scroll_to_caret = true;
            cx.notify();
        }
    }

    fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        self.scroll.y = (self.scroll.y - delta.y).max(px(0.));
        self.scroll.x = (self.scroll.x - delta.x).max(px(0.));
        cx.notify();
    }

    fn open_find(&mut self, _: &OpenFind, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in file"));
            let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| match event {
                InputEvent::Change => {
                    let query = input.read(cx).value().to_string();
                    if let Some(find) = &mut this.find {
                        find.query = query;
                    }
                    this.recompute_find();
                    this.find_step(true, cx);
                }
                InputEvent::PressEnter { shift, .. } => this.find_step(!*shift, cx),
                _ => {}
            });
            self.find = Some(Find { input, query: String::new(), matches: Vec::new(), current: None, _subscription: subscription });
        }
        let selected = (!self.selection.is_empty() && self.selection.anchor.line == self.selection.head.line)
            .then(|| self.buffer.slice(self.selection.range()));
        if let Some(find) = &mut self.find {
            if let Some(text) = &selected {
                find.query = text.clone();
            }
            find.input.update(cx, |input, cx| {
                if let Some(text) = selected {
                    input.set_value(text, window, cx);
                }
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn close_find(&mut self, _: &CloseFind, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn recompute_find(&mut self) {
        let Some(find) = &mut self.find else { return };
        let query = find.query.clone();
        find.matches.clear();
        find.current = None;
        if query.is_empty() {
            return;
        }
        let needle = query.to_lowercase();
        for (index, line) in self.buffer.lines().iter().enumerate() {
            // ASCII lowercasing keeps byte offsets; other text matches exactly.
            let haystack = if line.is_ascii() { line.to_ascii_lowercase() } else { line.clone() };
            let needle = if line.is_ascii() { needle.as_str() } else { query.as_str() };
            let mut from = 0;
            while let Some(found) = haystack[from..].find(needle) {
                let start = from + found;
                find.matches.push((index, start..start + needle.len()));
                from = start + needle.len().max(1);
            }
        }
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        if find.matches.is_empty() {
            find.current = None;
            cx.notify();
            return;
        }
        let caret = self.selection.range().start;
        let next = match find.current {
            Some(current) if forward => (current + 1) % find.matches.len(),
            Some(current) => (current + find.matches.len() - 1) % find.matches.len(),
            None => find.matches.iter().position(|(line, r)| Pos::new(*line, r.start) >= caret).unwrap_or(0),
        };
        find.current = Some(next);
        let (line, range) = find.matches[next].clone();
        self.selection = Selection { anchor: Pos::new(line, range.start), head: Pos::new(line, range.end) };
        self.scroll_to_caret = true;
        cx.notify();
    }

    fn copy_selection(&self, cx: &mut App) {
        if !self.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.slice(self.selection.range())));
        }
    }

    fn render_find_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let find = self.find.as_ref()?;
        let theme = cx.app_theme();
        let colors = theme.colors;
        let status = match (find.current, find.matches.len()) {
            (_, 0) if find.query.is_empty() => SharedString::default(),
            (_, 0) => t!("No results found."),
            (Some(current), total) => format!("{}/{}", current + 1, total).into(),
            (None, total) => total.to_string().into(),
        };
        Some(
            div()
                .key_context("FileEditorFind")
                .on_action(cx.listener(|this, _: &FindNext, _, cx| this.find_step(true, cx)))
                .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.find_step(false, cx)))
                .on_action(cx.listener(Self::close_find))
                .absolute()
                .top(px(8.))
                .right(px(16.))
                .w(px(280.))
                .flex()
                .items_center()
                .gap(px(6.))
                .p(px(4.))
                .rounded(theme.radii.lg)
                .border_1()
                .border_color(colors.border)
                .bg(colors.surface_overlay)
                .shadow(theme.shadows.lg.clone())
                .child(div().flex_1().min_w_0().child(TextInput::new(&find.input).size(InputSize::Compact)))
                .child(div().text_size(px(11.)).text_color(colors.muted_foreground).child(status))
                .child(
                    div()
                        .id("find-close")
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme.radii.md)
                        .hover(|s| s.bg(colors.accent_surface))
                        .on_click(cx.listener(|this, _, window, cx| this.close_find(&CloseFind, window, cx)))
                        .child(Icon::new(IconName::X).size(IconSize::Xs).color(colors.muted_foreground)),
                ),
        )
    }
}

macro_rules! listener {
    ($name:ident, $action:ty, |$this:ident, $cx:ident| $body:expr) => {
        fn $name(&mut self, _: &$action, _: &mut Window, $cx: &mut Context<Self>) {
            let $this = self;
            $body
        }
    };
}

impl FileEditor {
    listener!(move_left, MoveLeft, |this, cx| this.horizontal(false, false, false, cx));
    listener!(move_right, MoveRight, |this, cx| this.horizontal(true, false, false, cx));
    listener!(select_left, SelectLeft, |this, cx| this.horizontal(false, true, false, cx));
    listener!(select_right, SelectRight, |this, cx| this.horizontal(true, true, false, cx));
    listener!(move_word_left, MoveWordLeft, |this, cx| this.horizontal(false, false, true, cx));
    listener!(move_word_right, MoveWordRight, |this, cx| this.horizontal(true, false, true, cx));
    listener!(select_word_left, SelectWordLeft, |this, cx| this.horizontal(false, true, true, cx));
    listener!(select_word_right, SelectWordRight, |this, cx| this.horizontal(true, true, true, cx));
    listener!(move_up, MoveUp, |this, cx| this.vertical(-1, false, cx));
    listener!(move_down, MoveDown, |this, cx| this.vertical(1, false, cx));
    listener!(select_up, SelectUp, |this, cx| this.vertical(-1, true, cx));
    listener!(select_down, SelectDown, |this, cx| this.vertical(1, true, cx));
    listener!(page_up, PageUp, |this, cx| this.vertical(-this.page_lines(), false, cx));
    listener!(page_down, PageDown, |this, cx| this.vertical(this.page_lines(), false, cx));
    listener!(move_line_start, MoveLineStart, |this, cx| this.line_edge(false, false, cx));
    listener!(move_line_end, MoveLineEnd, |this, cx| this.line_edge(true, false, cx));
    listener!(select_line_start, SelectLineStart, |this, cx| this.line_edge(false, true, cx));
    listener!(select_line_end, SelectLineEnd, |this, cx| this.line_edge(true, true, cx));
    listener!(move_doc_start, MoveDocStart, |this, cx| this.move_to(Pos::default(), false, cx));
    listener!(move_doc_end, MoveDocEnd, |this, cx| { let end = this.buffer.end(); this.move_to(end, false, cx) });
    listener!(select_doc_start, SelectDocStart, |this, cx| this.move_to(Pos::default(), true, cx));
    listener!(select_doc_end, SelectDocEnd, |this, cx| { let end = this.buffer.end(); this.move_to(end, true, cx) });

    fn line_edge(&mut self, end: bool, select: bool, cx: &mut Context<Self>) {
        let line = self.selection.head.line;
        let text = self.buffer.line(line);
        let indent = self.buffer.indentation(line).len();
        // Home toggles between the first non-blank character and column 0.
        let col = if end { text.len() } else if self.selection.head.col == indent { 0 } else { indent };
        self.goal_x = None;
        self.move_to(Pos::new(line, col), select, cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            self.buffer.prev_boundary(self.selection.head)..self.selection.head
        } else {
            self.selection.range()
        };
        self.replace(range, "", EditKind::Delete, cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            self.selection.head..self.buffer.next_boundary(self.selection.head)
        } else {
            self.selection.range()
        };
        self.replace(range, "", EditKind::Delete, cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            self.buffer.prev_word(self.selection.head)..self.selection.head
        } else {
            self.selection.range()
        };
        self.replace(range, "", EditKind::Delete, cx);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        let indent = self.buffer.indentation(self.selection.range().start.line).to_owned();
        self.replace(self.selection.range(), &format!("\n{indent}"), EditKind::Other, cx);
    }

    fn indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        let unit = self.buffer.indent_unit();
        self.insert(unit, cx);
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some((selection, line)) = self.buffer.undo(&mut self.history) {
            self.after_history(selection, line, cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some((selection, line)) = self.buffer.redo(&mut self.history) {
            self.after_history(selection, line, cx);
        }
    }

    fn after_history(&mut self, selection: Selection, line: usize, cx: &mut Context<Self>) {
        self.selection = Selection { anchor: self.buffer.clamp(selection.anchor), head: self.buffer.clamp(selection.head) };
        if let Some(h) = &mut self.highlighter {
            // Line counts may have changed anywhere below; re-parse from the first line touched.
            h.edited(line, 1, 2);
        }
        self.wrap_layout = None;
        self.scroll_to_caret = true;
        self.recompute_find();
        self.refresh_dirty(cx);
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selection = Selection { anchor: Pos::default(), head: self.buffer.end() };
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
        if !self.selection.is_empty() {
            self.replace(self.selection.range(), "", EditKind::Other, cx);
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace("\r\n", "\n");
            self.replace(self.selection.range(), &text, EditKind::Other, cx);
        }
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(FileEditorEvent::SaveRequested);
    }

    fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(true, cx);
    }

    fn find_previous(&mut self, _: &FindPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(false, cx);
    }

    // UTF-16 offsets for the platform input handler, relative to the caret's line:
    // IME composition never crosses lines, so offsets index that line only.
    fn utf16_line(&self) -> usize {
        self.marked.as_ref().map_or(self.selection.head.line, |m| m.start.line)
    }

    fn col_to_utf16(&self, line: usize, col: usize) -> usize {
        self.buffer.line(line)[..col].encode_utf16().count()
    }

    fn utf16_to_col(&self, line: usize, offset: usize) -> usize {
        let text = self.buffer.line(line);
        let mut count = 0;
        for (index, c) in text.char_indices() {
            if count >= offset {
                return index;
            }
            count += c.len_utf16();
        }
        text.len()
    }

    fn utf16_range_to_pos(&self, range: &Range<usize>) -> Range<Pos> {
        let line = self.utf16_line();
        Pos::new(line, self.utf16_to_col(line, range.start))..Pos::new(line, self.utf16_to_col(line, range.end))
    }
}

impl EntityInputHandler for FileEditor {
    fn text_for_range(&mut self, range: Range<usize>, actual: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let range = self.utf16_range_to_pos(&range);
        actual.replace(self.col_to_utf16(range.start.line, range.start.col)..self.col_to_utf16(range.end.line, range.end.col));
        Some(self.buffer.slice(range))
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let line = self.selection.head.line;
        let range = self.selection.range();
        let start = if range.start.line == line { self.col_to_utf16(line, range.start.col) } else { 0 };
        let end = if range.end.line == line { self.col_to_utf16(line, range.end.col) } else { self.col_to_utf16(line, self.buffer.line(line).len()) };
        Some(UTF16Selection { range: start..end, reversed: self.selection.head < self.selection.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| self.col_to_utf16(m.start.line, m.start.col)..self.col_to_utf16(m.end.line, m.end.col))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.history.close_group();
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = range.map(|r| self.utf16_range_to_pos(&r)).or(self.marked.clone()).unwrap_or(self.selection.range());
        self.marked = None;
        let kind = if text.contains('\n') { EditKind::Other } else { EditKind::Insert };
        self.replace(range, text, kind, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|r| self.utf16_range_to_pos(&r)).or(self.marked.clone()).unwrap_or(self.selection.range());
        let start = range.start;
        self.replace(range, text, EditKind::Insert, cx);
        self.marked = (!text.is_empty()).then(|| start..Pos::new(start.line, start.col + text.len()));
        if let Some(selected) = selected {
            let base = self.col_to_utf16(start.line, start.col);
            let a = self.utf16_to_col(start.line, base + selected.start);
            let b = self.utf16_to_col(start.line, base + selected.end);
            self.selection = Selection { anchor: Pos::new(start.line, a), head: Pos::new(start.line, b) };
        }
        cx.notify();
    }

    fn bounds_for_range(&mut self, range: Range<usize>, _: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let range = self.utf16_range_to_pos(&range);
        let geometry = self.geometry.borrow();
        let (row, start) = self.pos_row(range.start);
        let x = geometry.bounds.origin.x + geometry.text_left - self.scroll.x
            + geometry.ch * display_columns(&self.buffer.line(range.start.line)[start..range.start.col]) as f32;
        let y = geometry.bounds.origin.y + px(EDITOR_LINE_HEIGHT) * row as f32 - self.scroll.y;
        Some(Bounds::new(point(x, y), size(geometry.ch, px(EDITOR_LINE_HEIGHT))))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let pos = self.pos_for_point(point);
        (pos.line == self.utf16_line()).then(|| self.col_to_utf16(pos.line, pos.col))
    }
}

impl Render for FileEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.app_theme().colors.code_background;
        div()
            .id("file-editor")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::move_word_left))
            .on_action(cx.listener(Self::move_word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::move_line_start))
            .on_action(cx.listener(Self::move_line_end))
            .on_action(cx.listener(Self::select_line_start))
            .on_action(cx.listener(Self::select_line_end))
            .on_action(cx.listener(Self::move_doc_start))
            .on_action(cx.listener(Self::move_doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::indent))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::open_find))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_previous))
            .relative()
            .size_full()
            .bg(background)
            .child(EditorElement { editor: cx.entity() })
            .children(self.render_find_bar(cx))
    }
}

/// Draws a [`FileEditor`]: gutter, current line, selection, find matches, text, caret.
struct EditorElement {
    editor: Entity<FileEditor>,
}

struct EditorPrepaint {
    hitbox: Hitbox,
    lines: Vec<Rc<ShapedLineEntry>>,
    /// Line numbers, on each line's first row only.
    numbers: Vec<Option<ShapedLine>>,
    first_row: usize,
    gutter_width: Pixels,
    text_left: Pixels,
    ch: Pixels,
    scroll: Point<Pixels>,
    focused: bool,
}

struct Paint {
    selection: Hsla,
    current_line: Hsla,
    revealed: Hsla,
    find: Hsla,
    find_current: Hsla,
    caret: Hsla,
    gutter_border: Hsla,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some("file-editor-text".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> EditorPrepaint {
        let theme = cx.app_theme();
        let appearance = theme.appearance;
        let foreground = theme.colors.code_foreground;
        let muted = theme.colors.muted_foreground;
        let code_size = theme.fonts.code;
        let focused = self.editor.read(cx).focus_handle.is_focused(window);
        let entity = self.editor.clone();
        entity.update(cx, |editor, _| {
            let regular = crate::appearance_fonts::mono_font(FontWeight::NORMAL);
            let bold = Font { weight: FontWeight::BOLD, ..regular.clone() };
            let italic = Font { style: FontStyle::Italic, ..regular.clone() };
            let font_size = px(code_size);
            let line_height = px(EDITOR_LINE_HEIGHT);
            let ch = window
                .text_system()
                .shape_line("0".into(), font_size, &[plain_run(1, &regular, foreground)], None)
                .width;
            let count = editor.buffer.len_lines();
            let digits = count.to_string().len().max(3);
            // `2ch` left pad, the digits, `1ch` right pad, then the 2px column border; `1ch` before text.
            let gutter_width = ch * (3 + digits) as f32 + px(2.);
            let text_left = gutter_width + ch;
            // The text area keeps `1ch` free on the right, as Pierre's line padding does.
            let cols = ((bounds.size.width - text_left - ch) / ch.max(px(1.))).floor().max(1.) as usize;
            editor.ensure_wrap(cols);
            let total_rows = editor.row_count();
            let content_height = line_height * total_rows as f32;
            let max_y = (content_height - bounds.size.height + line_height * 3.).max(px(0.));
            if editor.scroll_to_caret {
                editor.scroll_to_caret = false;
                let head = editor.selection.head;
                let (row, start) = editor.pos_row(head);
                let top = line_height * row as f32;
                if top < editor.scroll.y {
                    editor.scroll.y = top;
                } else if top + line_height > editor.scroll.y + bounds.size.height {
                    editor.scroll.y = top + line_height - bounds.size.height;
                }
                let x = ch * display_columns(&editor.buffer.line(head.line)[start..head.col]) as f32;
                let visible = bounds.size.width - text_left - ch * 2.;
                if x < editor.scroll.x {
                    editor.scroll.x = (x - ch * 4.).max(px(0.));
                } else if x > editor.scroll.x + visible {
                    editor.scroll.x = x - visible + ch * 4.;
                }
            }
            if editor.wrap {
                editor.scroll.x = px(0.);
            }
            editor.scroll.y = editor.scroll.y.clamp(px(0.), max_y);
            let first_row = (editor.scroll.y / line_height).floor() as usize;
            let visible_rows = (bounds.size.height / line_height).ceil() as usize + 1;
            let last_row = (first_row + visible_rows).min(total_rows);
            let (first_line, first_sub) = editor.row_line(first_row);
            let last_line = if last_row > first_row { editor.row_line(last_row - 1).0 + 1 } else { first_line + 1 }.min(count);
            let head_line = editor.selection.head.line;
            let number_color = muted.opacity(0.7);
            let mut pending = false;
            if let Some(highlighter) = &mut editor.highlighter {
                pending = !highlighter.ensure(editor.buffer.lines(), last_line.saturating_sub(1), HIGHLIGHT_BUDGET_MS);
            }
            if pending {
                // The rest of the highlight is drawn on the next frame, within budget again.
                let view = entity.entity_id();
                window.on_next_frame(move |_, cx| cx.notify(view));
            }
            let palette = SyntaxPalette::for_appearance(appearance);
            let rows = last_row.saturating_sub(first_row);
            let mut lines = Vec::with_capacity(rows);
            let mut numbers = Vec::with_capacity(rows);
            let (mut index, mut sub) = (first_line, first_sub);
            let mut segments = editor.segments(index);
            while lines.len() < rows && index < count {
                let text = editor.buffer.line(index);
                let start = segments[sub];
                let last = sub + 1 == segments.len();
                let end = if last { text.len() } else { segments[sub + 1] };
                let spans = editor.highlighter.as_ref().and_then(|h| h.line(index));
                let mut entry = if start == 0 && last {
                    shape_code_line(window, &regular, &bold, &italic, font_size, palette, foreground, text, spans)
                } else {
                    let clipped: Vec<TokenSpan> = spans
                        .unwrap_or_default()
                        .iter()
                        .filter(|span| (span.end as usize) > start && (span.start as usize) < end)
                        .map(|span| TokenSpan {
                            start: (span.start as usize).max(start) as u32 - start as u32,
                            end: (span.end as usize).min(end) as u32 - start as u32,
                            token: span.token,
                        })
                        .collect();
                    shape_code_line(window, &regular, &bold, &italic, font_size, palette, foreground, &text[start..end], Some(&clipped))
                };
                (entry.line, entry.start, entry.end, entry.last) = (index, start, end, last);
                lines.push(Rc::new(entry));
                numbers.push((start == 0).then(|| {
                    let number: SharedString = (index + 1).to_string().into();
                    let len = number.len();
                    let color = if index == head_line { foreground } else { number_color };
                    window.text_system().shape_line(number, font_size, &[plain_run(len, &regular, color)], None)
                }));
                if last {
                    index += 1;
                    sub = 0;
                    if index < count {
                        segments = editor.segments(index);
                    }
                } else {
                    sub += 1;
                }
            }
            *editor.geometry.borrow_mut() =
                FrameGeometry { bounds, text_left, ch, first_row, lines: lines.clone() };
            let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
            EditorPrepaint { hitbox, lines, numbers, first_row, gutter_width, text_left, ch, scroll: editor.scroll, focused }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut EditorPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        window.set_cursor_style(CursorStyle::IBeam, &state.hitbox);
        self.register_mouse(&state.hitbox, window);

        let theme = cx.app_theme();
        let c = &theme.colors;
        let paint = Paint {
            selection: c.focus.opacity(if theme.is_dark() { 0.32 } else { 0.22 }),
            current_line: c.code_foreground.opacity(0.04),
            revealed: c.focus.opacity(0.14),
            find: c.search_match_background,
            find_current: c.search_match_active_background,
            caret: c.code_foreground,
            gutter_border: c.border.opacity(0.6),
        };
        let editor = self.editor.read(cx);
        let line_height = px(EDITOR_LINE_HEIGHT);
        let scroll = state.scroll;
        let selection = editor.selection.range();
        let head = editor.selection.head;
        let read_only = editor.read_only;
        let text_origin_x = bounds.origin.x + state.text_left - scroll.x;
        let row_y = |row: usize| bounds.origin.y + line_height * row as f32 - scroll.y;
        let x_of = |entry: &ShapedLineEntry, col: usize| text_origin_x + entry.shaped.x_for_index(entry.display_index(col));
        let visible_lines = state.lines.first().map_or(0..0, |first| first.line..state.lines.last().map_or(first.line, |l| l.line) + 1);
        let find_matches: Vec<(usize, Range<usize>, bool)> = editor
            .find
            .as_ref()
            .map(|find| {
                let start = find.matches.partition_point(|(line, _)| *line < visible_lines.start);
                find.matches[start..]
                    .iter()
                    .enumerate()
                    .take_while(|(_, (line, _))| visible_lines.contains(line))
                    .map(|(i, (line, range))| (*line, range.clone(), find.current == Some(start + i)))
                    .collect()
            })
            .unwrap_or_default();
        let revealed = editor.revealed_line;

        window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
            let text_bounds = Bounds::from_corners(point(bounds.origin.x + state.gutter_width, bounds.origin.y), bounds.bottom_right());
            for (offset, entry) in state.lines.iter().enumerate() {
                let line = entry.line;
                let y = row_y(state.first_row + offset);
                let row = Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, line_height));
                if revealed == Some(line) {
                    window.paint_quad(fill(row, paint.revealed));
                } else if line == head.line && selection.start == selection.end && state.focused {
                    window.paint_quad(fill(row, paint.current_line));
                }
                window.with_content_mask(Some(gpui_kit::ContentMask { bounds: text_bounds }), |window| {
                    for (_, range, current) in find_matches.iter().filter(|(l, ..)| *l == line) {
                        if range.end <= entry.start || range.start > entry.end {
                            continue;
                        }
                        let left = x_of(entry, range.start);
                        let right = x_of(entry, range.end);
                        window.paint_quad(fill(
                            Bounds::from_corners(point(left, y + px(1.)), point(right, y + line_height - px(1.))),
                            if *current { paint.find_current } else { paint.find },
                        ));
                    }
                    if line >= selection.start.line && line <= selection.end.line && selection.start != selection.end {
                        // The selection on this line, with `usize::MAX` for "through the newline".
                        let from = if line == selection.start.line { selection.start.col } else { 0 };
                        let to = if line == selection.end.line { selection.end.col } else { usize::MAX };
                        let starts_here = from < entry.end || (from == entry.end && entry.last);
                        if starts_here && to > entry.start {
                            let start = x_of(entry, from.max(entry.start));
                            let end = if to > entry.end {
                                x_of(entry, entry.end) + if entry.last { state.ch } else { px(0.) }
                            } else {
                                x_of(entry, to)
                            };
                            window.paint_quad(fill(Bounds::from_corners(point(start, y), point(end, y + line_height)), paint.selection));
                        }
                    }
                    entry.shaped.paint(point(text_origin_x, y), line_height, TextAlign::Left, None, window, cx).ok();
                    if line == head.line && entry.holds(head.col) && state.focused && !read_only {
                        let x = x_of(entry, head.col);
                        window.paint_quad(fill(Bounds::new(point(x, y + px(2.)), size(px(1.5), line_height - px(4.))), paint.caret));
                    }
                });
                if let Some(number) = &state.numbers[offset] {
                    let number_x = bounds.origin.x + state.gutter_width - px(2.) - state.ch - number.width;
                    number.paint(point(number_x, y), line_height, TextAlign::Left, None, window, cx).ok();
                }
            }
            window.paint_quad(fill(
                Bounds::new(point(bounds.origin.x + state.gutter_width - px(2.), bounds.origin.y), size(px(1.), bounds.size.height)),
                paint.gutter_border,
            ));
        });
    }
}

impl EditorElement {
    fn register_mouse(&self, hitbox: &Hitbox, window: &mut Window) {
        let editor = self.editor.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && hb.is_hovered(window) {
                editor.update(cx, |editor, cx| editor.on_mouse_down(event, window, cx));
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
                editor.update(cx, |editor, cx| editor.on_mouse_drag(event.position, cx));
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                editor.update(cx, |editor, _| editor.dragging = false);
            }
        });
        let editor = self.editor.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                let delta = event.delta.pixel_delta(px(EDITOR_LINE_HEIGHT));
                editor.update(cx, |editor, cx| editor.scroll_by(delta, cx));
            }
        });
    }
}

fn plain_run(len: usize, font: &Font, color: Hsla) -> TextRun {
    TextRun { len, font: font.clone(), color, background_color: None, underline: None, strikethrough: None }
}

/// Columns a string takes with tabs expanded to the next multiple of [`TAB_WIDTH`].
fn display_columns(text: &str) -> usize {
    text.chars().fold(0, |columns, c| if c == '\t' { columns + TAB_WIDTH - columns % TAB_WIDTH } else { columns + 1 })
}

/// Byte offsets where a line's rows start when wrapped at `cols` columns: after
/// the last space or tab that fits, else at the column limit.
fn wrap_starts(text: &str, cols: usize) -> SmallVec<[usize; 4]> {
    let mut starts: SmallVec<[usize; 4]> = smallvec![0];
    let mut columns = 0;
    // The byte after the row's last space, with the columns up to it.
    let mut last_break: Option<(usize, usize)> = None;
    for (index, c) in text.char_indices() {
        let width = if c == '\t' { TAB_WIDTH - columns % TAB_WIDTH } else { 1 };
        if columns + width > cols && columns > 0 {
            let (at, used) = last_break.filter(|(at, _)| *at < index).unwrap_or((index, columns));
            starts.push(at);
            columns -= used;
            last_break = None;
        }
        columns += width;
        if c == ' ' || c == '\t' {
            last_break = Some((index + c.len_utf8(), columns));
        }
    }
    starts
}

/// Shapes one code line with its syntax spans, expanding tabs to spaces.
#[allow(clippy::too_many_arguments)]
fn shape_code_line(
    window: &mut Window,
    regular: &Font,
    bold: &Font,
    italic: &Font,
    font_size: Pixels,
    palette: &SyntaxPalette,
    foreground: Hsla,
    text: &str,
    spans: Option<&[TokenSpan]>,
) -> ShapedLineEntry {
    let (display, map) = if text.contains('\t') {
        let mut display = String::with_capacity(text.len() + 8);
        let mut map = Vec::with_capacity(text.len() + 1);
        let mut columns = 0;
        for c in text.chars() {
            for _ in 0..c.len_utf8() {
                map.push(display.len() as u32);
            }
            if c == '\t' {
                let width = TAB_WIDTH - columns % TAB_WIDTH;
                display.extend(std::iter::repeat_n(' ', width));
                columns += width;
            } else {
                display.push(c);
                columns += 1;
            }
        }
        map.push(display.len() as u32);
        (SharedString::from(display), Some(map))
    } else {
        (SharedString::from(text.to_owned()), None)
    };
    let to_display = |byte: usize| map.as_ref().map_or(byte, |m| m[byte] as usize);
    let mut runs: SmallVec<[TextRun; 16]> = SmallVec::new();
    let mut cursor = 0;
    for span in spans.unwrap_or_default() {
        let (start, end) = (to_display(span.start as usize), to_display(span.end as usize));
        if start >= end || end > display.len() {
            continue;
        }
        if start > cursor {
            runs.push(plain_run(start - cursor, regular, foreground));
        }
        let style = palette.style(span.token);
        let run_font = if style.font_weight.is_some_and(|w| w.0 >= 600.) {
            bold
        } else if style.font_style == Some(FontStyle::Italic) {
            italic
        } else {
            regular
        };
        runs.push(plain_run(end - start, run_font, style.color.unwrap_or(foreground)));
        cursor = end;
    }
    if cursor < display.len() || runs.is_empty() {
        runs.push(plain_run(display.len() - cursor, regular, foreground));
    }
    let shaped = window.text_system().shape_line(display, font_size, &runs, None);
    ShapedLineEntry { shaped, map, line: 0, start: 0, end: text.len(), last: true }
}

/// The code font family, as the code font setting resolves it (Pierre's `--diffs-font-family`).
pub fn mono_family() -> SharedString {
    crate::appearance_fonts::mono_font(FontWeight::NORMAL).family
}
