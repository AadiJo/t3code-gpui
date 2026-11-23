//! The composer's prompt editor: a custom GPUI text element over the Markdown
//! draft. Ports `ComposerPromptEditorTiptap.tsx` without ProseMirror: the
//! Markdown source is the document, block and inline markers are styled or
//! hidden in place, and chips (file references, mentions, skills, models,
//! pasted text and images) are single caret stops.
//!
//! The editor owns editing, selection, IME, clipboard and undo. Decisions that
//! belong to the composer (send, menus, history, attachments) leave as
//! [`ComposerEditorEvent`]s.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{
    App, ClipboardEntry, ClipboardItem, Context, EntityInputHandler, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, Image, KeyBinding, Pixels, Point, SharedString, UTF16Selection, Window, actions, point, px,
};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::components::composer_prompt_editor_element::{EditorLayout, PromptEditorElement};
use crate::composer_code_block::{
    code_body_range, convert_fence_on_enter, exit_on_trailing_blank_lines, indent_code_lines, indented_newline,
};
use crate::composer_editor_mentions::{InlineToken, collect_line_tokens};
use crate::composer_list_continuation::{list_continuation_for_enter, list_indent_for_tab, list_outdent_for_tab};
use crate::composer_logic::{
    ComposerKeyEvent, ComposerSubmissionIntent, ComposerSubmitContext, composer_submission_intent_for_key,
};
use crate::composer_rich_text::{SourcePiece, parse_inline_markdown_pieces};
use crate::composer_rich_text_doc::{CodeBlock, LineKind, classify_lines};
use crate::composer_text_direction::{TextDirection, line_directions};
use crate::composer_undo_grouping::{ComposerChangeKind, UndoHistory, UndoSnapshot};

/// Messages longer than this stay in the composer (`docs/user/composer.md`).
pub const MAX_PROMPT_CHARS: usize = 120_000;
/// Pastes this large become a text attachment unless pasted with Shift.
pub const LARGE_PASTE_BYTES: usize = 32 * 1024;
/// Key context for the editor's bindings.
pub const EDITOR_CONTEXT: &str = "ComposerPromptEditor";

actions!(
    composer_editor,
    [
        MoveLeft, MoveRight, MoveUp, MoveDown, SelectLeft, SelectRight, SelectUp, SelectDown,
        MoveWordLeft, MoveWordRight, SelectWordLeft, SelectWordRight,
        MoveLineStart, MoveLineEnd, SelectLineStart, SelectLineEnd,
        MoveDocStart, MoveDocEnd, SelectDocStart, SelectDocEnd,
        Backspace, Delete, DeleteWordLeft, DeleteWordRight, DeleteToLineStart,
        SelectAll, Copy, Cut, Paste, PastePlain, Undo, Redo,
        Enter, ShiftEnter, ModEnter, ModAltEnter, Tab, ShiftTab, Escape, PageUp, PageDown,
        ToggleBold, ToggleItalic, ShowCharacterPalette,
    ]
);

/// Binds the editor's keys. Word and line movement follow the platform:
/// Option/Cmd on macOS, Ctrl/Home/End elsewhere.
pub fn bind_keys(cx: &mut App) {
    let c = Some(EDITOR_CONTEXT);
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
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("shift-enter", ShiftEnter, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("shift-tab", ShiftTab, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
    ];
    if cfg!(target_os = "macos") {
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
            KeyBinding::new("alt-delete", DeleteWordRight, c),
            KeyBinding::new("cmd-backspace", DeleteToLineStart, c),
            KeyBinding::new("ctrl-a", MoveLineStart, c),
            KeyBinding::new("ctrl-e", MoveLineEnd, c),
            KeyBinding::new("cmd-a", SelectAll, c),
            KeyBinding::new("cmd-c", Copy, c),
            KeyBinding::new("cmd-x", Cut, c),
            KeyBinding::new("cmd-v", Paste, c),
            KeyBinding::new("cmd-shift-v", PastePlain, c),
            KeyBinding::new("cmd-z", Undo, c),
            KeyBinding::new("cmd-shift-z", Redo, c),
            KeyBinding::new("cmd-b", ToggleBold, c),
            KeyBinding::new("cmd-i", ToggleItalic, c),
            KeyBinding::new("cmd-enter", ModEnter, c),
            KeyBinding::new("cmd-alt-enter", ModAltEnter, c),
            KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, c),
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
            KeyBinding::new("ctrl-delete", DeleteWordRight, c),
            KeyBinding::new("ctrl-a", SelectAll, c),
            KeyBinding::new("ctrl-c", Copy, c),
            KeyBinding::new("ctrl-insert", Copy, c),
            KeyBinding::new("ctrl-x", Cut, c),
            KeyBinding::new("shift-delete", Cut, c),
            KeyBinding::new("ctrl-v", Paste, c),
            KeyBinding::new("shift-insert", Paste, c),
            KeyBinding::new("ctrl-shift-v", PastePlain, c),
            KeyBinding::new("ctrl-z", Undo, c),
            KeyBinding::new("ctrl-shift-z", Redo, c),
            KeyBinding::new("ctrl-y", Redo, c),
            KeyBinding::new("ctrl-b", ToggleBold, c),
            KeyBinding::new("ctrl-i", ToggleItalic, c),
            KeyBinding::new("ctrl-enter", ModEnter, c),
            KeyBinding::new("ctrl-alt-enter", ModAltEnter, c),
        ]);
    }
    cx.bind_keys(bindings);
}

/// Something the composer decides, raised by the editor.
#[derive(Clone, Debug)]
pub enum ComposerEditorEvent {
    /// The draft text changed.
    Changed,
    /// Only the selection moved.
    SelectionChanged,
    /// A send key was pressed outside a code block.
    Submit(ComposerSubmissionIntent),
    /// Tab outside a list or code block (accepts a suggested prompt).
    Tab,
    Escape,
    /// The editor cannot scroll further; the timeline should page instead.
    PageScroll { down: bool },
    /// Pasted or dropped content that becomes attachments.
    Attach(PastedContent),
    /// Focus moved into or out of the editor.
    FocusChanged(bool),
}

/// Clipboard or drop content the composer turns into attachments.
#[derive(Clone, Debug)]
pub enum PastedContent {
    Images(Vec<Image>),
    Files(Vec<std::path::PathBuf>),
    /// A paste of [`LARGE_PASTE_BYTES`] or more.
    LargeText(Arc<str>),
}

/// The parsed inline content of one source line, shared across frames while
/// the line's text is unchanged.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct LineParse {
    pub text: String,
    pub tokens: Vec<InlineToken>,
    pub pieces: Vec<SourcePiece>,
}

/// One source line of the document.
#[derive(Clone, Debug)]
pub struct DocLine {
    /// Byte range in the draft, without the newline.
    pub range: Range<usize>,
    pub kind: LineKind,
    pub direction: TextDirection,
    pub parse: Arc<LineParse>,
}

/// The document model derived from the draft text after every edit.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub lines: Vec<DocLine>,
    pub line_starts: Vec<usize>,
    pub code_blocks: Vec<CodeBlock>,
}

impl Doc {
    /// The line holding `offset` (the later line at a line boundary).
    pub fn line_at(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|start| *start <= offset).saturating_sub(1)
    }
}

/// What a drag selects by.
#[derive(Clone, Debug)]
enum SelectMode {
    Char,
    Word(Range<usize>),
    Line(Range<usize>),
}

fn hash_line(text: &str, newline_after: bool, rich: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    newline_after.hash(&mut h);
    rich.hash(&mut h);
    h.finish()
}

/// The prompt editor entity.
pub struct ComposerPromptEditor {
    pub(crate) focus_handle: FocusHandle,
    text: String,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    pub(crate) doc: Rc<Doc>,
    parse_cache: HashMap<u64, Arc<LineParse>>,
    history: UndoHistory,
    /// Rich text on: Markdown styled in place. Off: plain Markdown.
    pub rich_text: bool,
    pub disabled: bool,
    pub placeholder: SharedString,
    /// What the composer knows about sending; read when a send key is pressed.
    pub submit_context: ComposerSubmitContext,
    /// Written by the element each frame through `Entity::read`, so drawing never
    /// updates the entity (an update mid-draw leaves the window dirty for another frame).
    pub(crate) layout: RefCell<Option<Rc<EditorLayout>>>,
    pub(crate) scroll_y: Cell<Pixels>,
    pub(crate) scroll_to_caret: Cell<bool>,
    goal_x: Option<Pixels>,
    selecting: Option<SelectMode>,
    was_focused: bool,
}

impl EventEmitter<ComposerEditorEvent> for ComposerPromptEditor {}

impl Focusable for ComposerPromptEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ComposerPromptEditor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            text: String::new(),
            selected: 0..0,
            reversed: false,
            marked: None,
            doc: Rc::default(),
            parse_cache: HashMap::new(),
            history: UndoHistory::default(),
            rich_text: true,
            disabled: false,
            placeholder: "Ask anything. @ files, $ skills, / commands".into(),
            submit_context: ComposerSubmitContext::default(),
            layout: RefCell::new(None),
            scroll_y: Cell::new(px(0.)),
            scroll_to_caret: Cell::new(false),
            goal_x: None,
            selecting: None,
            was_focused: false,
        };
        this.rebuild_doc();
        this
    }

    // ---- Read access ----

    /// The Markdown draft.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The selection as source byte offsets.
    pub fn selection(&self) -> Range<usize> {
        self.selected.clone()
    }

    /// The caret (selection head).
    pub fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    pub(crate) fn marked_range_for_paint(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    /// Whether an IME composition is in progress.
    pub fn is_composing(&self) -> bool {
        self.marked.is_some()
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    /// Whether the caret is on the first visual row (for history recall).
    pub fn caret_on_first_row(&self) -> bool {
        self.layout.borrow().as_ref().is_none_or(|l| l.row_of(&self.doc, self.cursor()).0 == 0)
    }

    /// Whether the caret is on the last visual row.
    pub fn caret_on_last_row(&self) -> bool {
        self.layout.borrow().as_ref().is_none_or(|l| {
            let (row, total) = l.row_of(&self.doc, self.cursor());
            row + 1 >= total
        })
    }

    /// The window point at the middle of the caret position for `offset`, from the last frame.
    pub fn window_point_for(&self, offset: usize) -> Option<Point<Pixels>> {
        let layout = self.layout.borrow().clone()?;
        let bounds = layout.window_bounds_for_offset(&self.doc, offset, self.scroll_y.get())?;
        Some(bounds.center())
    }

    /// Whether the caret sits inside a fenced code block's body.
    pub fn caret_in_code(&self) -> bool {
        let line = self.doc.line_at(self.cursor());
        matches!(self.doc.lines.get(line).map(|l| &l.kind), Some(LineKind::Code { .. }))
    }

    // ---- Programmatic edits ----

    /// Replaces the whole draft, as when switching threads or recalling a prompt.
    /// Clears undo history.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        let end = self.text.len();
        self.selected = end..end;
        self.reversed = false;
        self.marked = None;
        self.history.clear();
        self.rebuild_doc();
        self.scroll_to_caret.set(true);
        cx.emit(ComposerEditorEvent::Changed);
        cx.notify();
    }

    /// Replaces a range as one undo step (menu picks, chip insertion).
    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str, cx: &mut Context<Self>) {
        let caret = range.start + replacement.len();
        self.apply_edit(range, replacement, caret..caret, ComposerChangeKind::Other, cx);
    }

    /// Inserts text at the selection as one undo step.
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self.selected.clone();
        let caret = range.start + text.len();
        self.apply_edit(range, text, caret..caret, ComposerChangeKind::Other, cx);
    }

    /// Moves the caret, snapping out of chips.
    pub fn set_cursor(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.move_to(offset, cx);
    }

    /// Selects a range.
    pub fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.selected = self.snap(range.start, false)..self.snap(range.end, true);
        self.reversed = false;
        self.goal_x = None;
        self.scroll_to_caret.set(true);
        cx.emit(ComposerEditorEvent::SelectionChanged);
        cx.notify();
    }

    // ---- Document model ----

    fn rebuild_doc(&mut self) {
        let rich = self.rich_text;
        let text = &self.text;
        let mut line_starts = Vec::with_capacity(self.doc.lines.len() + 1);
        let mut ranges = Vec::with_capacity(self.doc.lines.len() + 1);
        let mut start = 0;
        loop {
            line_starts.push(start);
            match text[start..].find('\n') {
                Some(i) => {
                    ranges.push(start..start + i);
                    start += i + 1;
                }
                None => {
                    ranges.push(start..text.len());
                    break;
                }
            }
        }
        let count = ranges.len();
        let mut parses = Vec::with_capacity(count);
        let mut used = Vec::with_capacity(count);
        for (i, range) in ranges.iter().enumerate() {
            let line = &text[range.clone()];
            let newline_after = i + 1 < count;
            let key = hash_line(line, newline_after, rich);
            used.push(key);
            let parse = match self.parse_cache.get(&key) {
                Some(parse) if parse.text == line => parse.clone(),
                _ => {
                    let parse = Arc::new(LineParse {
                        text: line.to_owned(),
                        tokens: collect_line_tokens(line, newline_after),
                        pieces: if rich { parse_inline_markdown_pieces(line) } else { Vec::new() },
                    });
                    self.parse_cache.insert(key, parse.clone());
                    parse
                }
            };
            parses.push(parse);
        }
        if self.parse_cache.len() > count * 2 + 64 {
            let used: std::collections::HashSet<u64> = used.into_iter().collect();
            self.parse_cache.retain(|k, _| used.contains(k));
        }
        let (kinds, code_blocks) = classify_lines(ranges.iter().map(|r| &text[r.clone()]), rich);
        let directions = line_directions(ranges.iter().map(|r| &text[r.clone()]).zip(kinds.iter()));
        self.doc = Rc::new(Doc {
            lines: ranges
                .into_iter()
                .zip(kinds)
                .zip(directions)
                .zip(parses)
                .map(|(((range, kind), direction), parse)| DocLine { range, kind, direction, parse })
                .collect(),
            line_starts,
            code_blocks,
        });
    }

    /// Byte length of a hidden block marker at the start of a line (rich mode).
    pub(crate) fn hidden_block_marker(line: &DocLine) -> usize {
        match line.kind {
            LineKind::Heading { marker, .. } | LineKind::Quote { marker } => marker,
            _ => 0,
        }
    }

    /// Moves an offset out of a chip or hidden block marker. `forward` picks the
    /// chip's end when the offset is inside one.
    fn snap(&self, offset: usize, forward: bool) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let Some(line) = self.doc.lines.get(self.doc.line_at(offset)) else { return offset };
        let content_start = line.range.start + Self::hidden_block_marker(line);
        if offset < content_start {
            return content_start;
        }
        let rel = offset - line.range.start;
        for token in &line.parse.tokens {
            if rel > token.range.start && rel < token.range.end {
                return line.range.start + if forward { token.range.end } else { token.range.start };
            }
        }
        offset
    }

    /// The chip covering `[offset - 1, offset)` or `[offset, offset + 1)`.
    fn chip_at(&self, offset: usize, before: bool) -> Option<Range<usize>> {
        let line = self.doc.lines.get(self.doc.line_at(offset))?;
        let rel = offset - line.range.start;
        line.parse.tokens.iter().find_map(|t| {
            let hit = if before { t.range.end == rel } else { t.range.start == rel };
            hit.then(|| line.range.start + t.range.start..line.range.start + t.range.end)
        })
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        if let Some(chip) = self.chip_at(offset, true) {
            return chip.start;
        }
        let line = &self.doc.lines[self.doc.line_at(offset)];
        let content_start = line.range.start + Self::hidden_block_marker(line);
        if offset == content_start && line.range.start > 0 {
            return line.range.start - 1;
        }
        let from = line.range.start;
        self.text[from..offset].grapheme_indices(true).next_back().map_or(from, |(i, _)| from + i)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        if let Some(chip) = self.chip_at(offset, false) {
            return chip.end;
        }
        let line = &self.doc.lines[self.doc.line_at(offset)];
        if offset >= line.range.end {
            return self.snap((offset + 1).min(self.text.len()), true);
        }
        let next = self.text[offset..line.range.end].grapheme_indices(true).nth(1).map_or(line.range.end, |(i, _)| offset + i);
        self.snap(next, true)
    }

    /// Start of the word before `offset`; chips count as words.
    fn previous_word(&self, offset: usize) -> usize {
        if let Some(chip) = self.chip_at(offset, true) {
            return chip.start;
        }
        let line = &self.doc.lines[self.doc.line_at(offset)];
        let content_start = line.range.start + Self::hidden_block_marker(line);
        if offset <= content_start {
            return self.previous_boundary(offset);
        }
        let slice = &self.text[content_start..offset];
        let mut target = content_start;
        for (i, word) in slice.split_word_bound_indices().rev() {
            if word.chars().any(char::is_alphanumeric) {
                target = content_start + i;
                break;
            }
        }
        self.snap(target, false)
    }

    /// End of the word after `offset`.
    fn next_word(&self, offset: usize) -> usize {
        if let Some(chip) = self.chip_at(offset, false) {
            return chip.end;
        }
        let line = &self.doc.lines[self.doc.line_at(offset)];
        if offset >= line.range.end {
            return self.next_boundary(offset);
        }
        let slice = &self.text[offset..line.range.end];
        let mut target = line.range.end;
        for (i, word) in slice.split_word_bound_indices() {
            if word.chars().any(char::is_alphanumeric) {
                target = offset + i + word.len();
                break;
            }
        }
        self.snap(target, true)
    }

    fn word_range_at(&self, offset: usize) -> Range<usize> {
        if let Some(chip) = self.chip_at(offset, false).or_else(|| self.chip_at(offset, true)) {
            return chip;
        }
        let line = &self.doc.lines[self.doc.line_at(offset)];
        let rel = offset - line.range.start;
        let mut found = offset..offset;
        for (i, word) in self.text[line.range.clone()].split_word_bound_indices() {
            if rel >= i && rel <= i + word.len() {
                found = line.range.start + i..line.range.start + i + word.len();
                if !word.trim().is_empty() {
                    break;
                }
            }
        }
        found
    }

    fn line_range_at(&self, offset: usize) -> Range<usize> {
        let line = &self.doc.lines[self.doc.line_at(offset)];
        let start = line.range.start + Self::hidden_block_marker(line);
        let end = if line.range.end < self.text.len() { line.range.end + 1 } else { line.range.end };
        start..end
    }

    // ---- Selection ----

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.snap(offset, true);
        self.selected = offset..offset;
        self.reversed = false;
        self.scroll_to_caret.set(true);
        cx.emit(ComposerEditorEvent::SelectionChanged);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.snap(offset, true);
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        self.scroll_to_caret.set(true);
        cx.emit(ComposerEditorEvent::SelectionChanged);
        cx.notify();
    }

    fn vertical_target(&mut self, down: bool, page: bool) -> Option<usize> {
        let layout = self.layout.borrow().clone()?;
        let head = self.cursor();
        let pos = layout.position_for_offset(&self.doc, head)?;
        let x = *self.goal_x.get_or_insert(pos.x);
        let step = if page { layout.visible_height() } else { layout.row_height_at(&self.doc, head) };
        let y = if down { pos.y + step + px(1.) } else { pos.y - px(1.) - if page { step } else { px(0.) } };
        if y < px(0.) {
            return Some(0);
        }
        if y > layout.content_height {
            return Some(self.text.len());
        }
        layout.offset_for_content_point(&self.doc, point(x, y))
    }

    fn move_vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let goal = self.goal_x;
        let Some(target) = self.vertical_target(down, false) else { return };
        let keep_goal = self.goal_x.or(goal);
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
        self.goal_x = keep_goal;
    }

    fn row_bounds(&self) -> Option<Range<usize>> {
        let layout = self.layout.borrow().clone()?;
        layout.row_range_at(&self.doc, self.cursor())
    }

    // ---- Editing ----

    fn snapshot(&self) -> UndoSnapshot {
        UndoSnapshot { text: self.text.clone(), selection: self.selected.clone() }
    }

    /// The one write path: records undo, edits, re-derives the document.
    fn apply_edit(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        selection_after: Range<usize>,
        kind: ComposerChangeKind,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range.start.min(self.text.len())..range.end.min(self.text.len());
        if range.is_empty() && replacement.is_empty() {
            return;
        }
        self.history.record(self.snapshot(), kind, Instant::now());
        self.text.replace_range(range, replacement);
        self.rebuild_doc();
        self.selected = selection_after.start.min(self.text.len())..selection_after.end.min(self.text.len());
        self.reversed = false;
        self.goal_x = None;
        self.scroll_to_caret.set(true);
        cx.emit(ComposerEditorEvent::Changed);
        cx.notify();
    }

    fn insert_text(&mut self, text: &str, kind: ComposerChangeKind, cx: &mut Context<Self>) {
        let range = self.marked.take().unwrap_or(self.selected.clone());
        let caret = range.start + text.len();
        let kind = if range.is_empty() { kind } else { ComposerChangeKind::Other };
        self.apply_edit(range, text, caret..caret, kind, cx);
    }

    fn delete_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let start = range.start;
        self.apply_edit(range, "", start..start, ComposerChangeKind::Delete, cx);
    }

    fn restore(&mut self, snapshot: UndoSnapshot, cx: &mut Context<Self>) {
        self.text = snapshot.text;
        self.selected = snapshot.selection;
        self.reversed = false;
        self.marked = None;
        self.rebuild_doc();
        self.scroll_to_caret.set(true);
        cx.emit(ComposerEditorEvent::Changed);
        cx.notify();
    }

    fn code_block_at(&self, offset: usize) -> Option<(usize, Range<usize>)> {
        let line = self.doc.lines.get(self.doc.line_at(offset))?;
        let block = match line.kind {
            LineKind::Code { block } => block,
            LineKind::FenceClose { block } if offset == line.range.start => block,
            _ => return None,
        };
        let body = code_body_range(&self.text, &self.doc.line_starts, &self.doc.code_blocks[block]);
        Some((block, body))
    }

    /// Enter and Shift+Enter inside text: list continuation, code rules, or a newline.
    fn insert_newline(&mut self, continue_blocks: bool, cx: &mut Context<Self>) {
        let selection = self.selected.clone();
        if let Some((block, body)) = self.code_block_at(selection.start) {
            let edit = (selection.is_empty())
                .then(|| exit_on_trailing_blank_lines(&self.text, body.clone(), &self.doc.code_blocks[block], selection.start))
                .flatten()
                .or_else(|| indented_newline(&self.text, body, selection.clone()));
            if let Some(edit) = edit {
                self.history.close_group();
                self.apply_edit(edit.range, &edit.replacement, edit.selection_after, ComposerChangeKind::Other, cx);
                return;
            }
        }
        if continue_blocks && selection.is_empty() && self.rich_text {
            if let Some(edit) = list_continuation_for_enter(&self.text, selection.start) {
                self.history.close_group();
                self.apply_edit(edit.range, &edit.replacement, edit.cursor_after..edit.cursor_after, ComposerChangeKind::Other, cx);
                return;
            }
            if let Some(edit) = self.quote_continuation(selection.start) {
                self.apply_edit(edit.0, &edit.1, edit.2..edit.2, ComposerChangeKind::Other, cx);
                return;
            }
        }
        self.insert_text("\n", ComposerChangeKind::Insert, cx);
    }

    /// Shift+Enter in a quote continues it; on an empty quote line it leaves the quote.
    fn quote_continuation(&self, caret: usize) -> Option<(Range<usize>, String, usize)> {
        let line = self.doc.lines.get(self.doc.line_at(caret))?;
        let LineKind::Quote { marker } = line.kind else { return None };
        let prefix = &self.text[line.range.start..line.range.start + marker];
        if self.text[line.range.start + marker..line.range.end].trim().is_empty() {
            return Some((line.range.start..line.range.end, String::new(), line.range.start));
        }
        let insertion = format!("\n{}", if prefix.ends_with(' ') { prefix.to_owned() } else { format!("{prefix} ") });
        Some((caret..caret, insertion.clone(), caret + insertion.len()))
    }

    /// Enter at the end of an opening fence opens a closed block.
    fn try_open_fence(&mut self, cx: &mut Context<Self>) -> bool {
        let caret = self.cursor();
        if !self.selected.is_empty() || !self.rich_text {
            return false;
        }
        let line = &self.doc.lines[self.doc.line_at(caret)];
        if !matches!(line.kind, LineKind::FenceOpen { .. } | LineKind::Paragraph) {
            return false;
        }
        let Some(edit) = convert_fence_on_enter(&self.text, line.range.clone(), caret) else {
            // A fence line that already has a closing fence below: step into the block.
            if matches!(line.kind, LineKind::FenceOpen { .. }) && caret == line.range.end {
                self.insert_text("\n", ComposerChangeKind::Insert, cx);
                return true;
            }
            return false;
        };
        self.history.close_group();
        self.apply_edit(edit.range, &edit.replacement, edit.selection_after, ComposerChangeKind::Other, cx);
        true
    }

    fn toggle_mark(&mut self, delimiter: &str, cx: &mut Context<Self>) {
        let range = self.selected.clone();
        let text = &self.text;
        let dl = delimiter.len();
        let wrapped = range.start >= dl
            && text.get(range.start - dl..range.start) == Some(delimiter)
            && text.get(range.end..range.end + dl) == Some(delimiter);
        if wrapped {
            let inner = text[range.clone()].to_owned();
            let start = range.start - dl;
            self.apply_edit(start..range.end + dl, &inner, start..start + inner.len(), ComposerChangeKind::Other, cx);
        } else {
            let inner = text[range.clone()].to_owned();
            let replacement = format!("{delimiter}{inner}{delimiter}");
            let start = range.start + dl;
            self.apply_edit(range, &replacement, start..start + inner.len(), ComposerChangeKind::Other, cx);
        }
    }

    /// Clipboard text for a range: the Markdown source, chips included.
    fn clipboard_text(&self, range: Range<usize>) -> String {
        self.text[range].to_owned()
    }

    fn paste_item(&mut self, item: ClipboardItem, plain: bool, cx: &mut Context<Self>) {
        let mut images = Vec::new();
        let mut files = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => images.push(image.clone()),
                ClipboardEntry::ExternalPaths(ExternalPaths(paths)) => files.extend(paths.iter().cloned()),
                ClipboardEntry::String(_) => {}
            }
        }
        if !images.is_empty() {
            cx.emit(ComposerEditorEvent::Attach(PastedContent::Images(images)));
            return;
        }
        if !files.is_empty() {
            cx.emit(ComposerEditorEvent::Attach(PastedContent::Files(files)));
            return;
        }
        let Some(text) = item.text() else { return };
        let text = text.replace("\r\n", "\n");
        let would_exceed = self.text.chars().count() + text.chars().count() > MAX_PROMPT_CHARS;
        if !plain && (text.len() >= LARGE_PASTE_BYTES || would_exceed) {
            cx.emit(ComposerEditorEvent::Attach(PastedContent::LargeText(text.into())));
            return;
        }
        self.history.close_group();
        self.insert_text(&text, ComposerChangeKind::Other, cx);
        self.history.close_group();
    }

    // ---- Action handlers ----

    fn on_move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected.is_empty() { self.previous_boundary(self.cursor()) } else { self.selected.start };
        self.goal_x = None;
        self.move_to(target, cx);
    }

    fn on_move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        let target = if self.selected.is_empty() { self.next_boundary(self.cursor()) } else { self.selected.end };
        self.goal_x = None;
        self.move_to(target, cx);
    }

    fn on_select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    fn on_select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    fn on_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, false, cx);
    }

    fn on_move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(true, false, cx);
    }

    fn on_select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, true, cx);
    }

    fn on_select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(true, true, cx);
    }

    fn on_move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor()), cx);
    }

    fn on_move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor()), cx);
    }

    fn on_select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.cursor()), cx);
    }

    fn on_select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.cursor()), cx);
    }

    fn on_move_line_start(&mut self, _: &MoveLineStart, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.row_bounds() {
            self.move_to(row.start, cx);
        }
    }

    fn on_move_line_end(&mut self, _: &MoveLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.row_bounds() {
            self.move_to(row.end, cx);
        }
    }

    fn on_select_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.row_bounds() {
            self.select_to(row.start, cx);
        }
    }

    fn on_select_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.row_bounds() {
            self.select_to(row.end, cx);
        }
    }

    fn on_move_doc_start(&mut self, _: &MoveDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn on_move_doc_end(&mut self, _: &MoveDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    fn on_select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn on_select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.text.len(), cx);
    }

    fn on_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        self.reversed = false;
        cx.emit(ComposerEditorEvent::SelectionChanged);
        cx.notify();
    }

    fn on_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            self.delete_range(self.selected.clone(), cx);
            return;
        }
        let caret = self.cursor();
        let line = &self.doc.lines[self.doc.line_at(caret)];
        let marker = Self::hidden_block_marker(line);
        let list_marker = match line.kind {
            LineKind::ListItem { marker, .. } if self.rich_text => marker,
            _ => 0,
        };
        if marker > 0 && caret == line.range.start + marker {
            let start = line.range.start;
            self.apply_edit(start..caret, "", start..start, ComposerChangeKind::Other, cx);
            return;
        }
        if list_marker > 0 && caret == line.range.start + list_marker {
            let start = line.range.start;
            self.apply_edit(start..caret, "", start..start, ComposerChangeKind::Other, cx);
            return;
        }
        if let Some(LineKind::Code { block }) = Some(line.kind.clone())
            && caret == line.range.start
            && self.doc.code_blocks[block].lines.start + 1 == self.doc.line_at(caret)
        {
            self.unwrap_code_block(block, cx);
            return;
        }
        let previous = self.previous_boundary(caret);
        if previous < caret {
            self.delete_range(previous..caret, cx);
        }
    }

    /// Backspace at the start of a code block turns it back into plain lines.
    fn unwrap_code_block(&mut self, block: usize, cx: &mut Context<Self>) {
        let block = self.doc.code_blocks[block].clone();
        let open = self.doc.lines[block.lines.start].range.clone();
        let mut text = String::new();
        let body_end = if block.closed { self.doc.lines[block.lines.end - 1].range.start.saturating_sub(1) } else { self.text.len() };
        let range_end = if block.closed { self.doc.lines[block.lines.end - 1].range.end } else { self.text.len() };
        text.push_str(&self.text[(open.end + 1).min(body_end)..body_end]);
        let caret = open.start;
        self.apply_edit(open.start..range_end, &text, caret..caret, ComposerChangeKind::Other, cx);
    }

    fn on_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            self.delete_range(self.selected.clone(), cx);
            return;
        }
        let caret = self.cursor();
        let next = self.next_boundary(caret);
        if next > caret {
            let start = caret;
            self.apply_edit(caret..next, "", start..start, ComposerChangeKind::Delete, cx);
        }
    }

    fn on_delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() { self.previous_word(self.cursor())..self.cursor() } else { self.selected.clone() };
        self.delete_range(range, cx);
    }

    fn on_delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() { self.cursor()..self.next_word(self.cursor()) } else { self.selected.clone() };
        self.delete_range(range, cx);
    }

    fn on_delete_to_line_start(&mut self, _: &DeleteToLineStart, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.row_bounds() {
            let caret = self.cursor();
            let start = if row.start == caret { self.previous_boundary(caret) } else { row.start };
            self.delete_range(start..caret, cx);
        }
    }

    fn on_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.clipboard_text(self.selected.clone())));
        }
    }

    fn on_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.clipboard_text(self.selected.clone())));
            let range = self.selected.clone();
            let start = range.start;
            self.history.close_group();
            self.apply_edit(range, "", start..start, ComposerChangeKind::Other, cx);
        }
    }

    fn on_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            self.paste_item(item, false, cx);
        }
    }

    fn on_paste_plain(&mut self, _: &PastePlain, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            self.paste_item(item, true, cx);
        }
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(previous) = self.history.undo(self.snapshot()) {
            self.restore(previous, cx);
        }
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.history.redo(self.snapshot()) {
            self.restore(next, cx);
        }
    }

    fn key_event(window: &Window, shift: bool, alt: bool, modifier: bool) -> ComposerKeyEvent {
        let _ = window;
        ComposerKeyEvent { enter: true, shift, alt, modifier, is_composing: false, repeat: false }
    }

    fn on_enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        // Enter during IME composition belongs to the IME; it never sends.
        if self.marked.is_some() {
            return;
        }
        // Fences run before send, so a fence opens and newlines in code never send.
        if self.try_open_fence(cx) {
            return;
        }
        if self.code_block_at(self.cursor()).is_some() {
            self.insert_newline(false, cx);
            return;
        }
        // A closing fence just typed: Enter leaves the block.
        let line = &self.doc.lines[self.doc.line_at(self.cursor())];
        if matches!(line.kind, LineKind::FenceClose { .. }) && self.cursor() == line.range.end {
            self.history.close_group();
            self.insert_text("\n", ComposerChangeKind::Other, cx);
            return;
        }
        let event = Self::key_event(window, false, false, false);
        match composer_submission_intent_for_key(event, self.submit_context, &self.text) {
            Some(intent) => cx.emit(ComposerEditorEvent::Submit(intent)),
            None => self.insert_newline(true, cx),
        }
    }

    fn on_shift_enter(&mut self, _: &ShiftEnter, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        self.insert_newline(true, cx);
    }

    fn on_mod_enter(&mut self, _: &ModEnter, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let event = Self::key_event(window, false, false, true);
        if let Some(intent) = composer_submission_intent_for_key(event, self.submit_context, &self.text) {
            cx.emit(ComposerEditorEvent::Submit(intent));
        }
    }

    fn on_mod_alt_enter(&mut self, _: &ModAltEnter, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return;
        }
        let event = Self::key_event(window, false, true, true);
        if let Some(intent) = composer_submission_intent_for_key(event, self.submit_context, &self.text) {
            cx.emit(ComposerEditorEvent::Submit(intent));
        }
    }

    fn on_tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.indent(true, cx);
    }

    fn on_shift_tab(&mut self, _: &ShiftTab, _: &mut Window, cx: &mut Context<Self>) {
        self.indent(false, cx);
    }

    fn indent(&mut self, forward: bool, cx: &mut Context<Self>) {
        let selection = self.selected.clone();
        if let Some((_, body)) = self.code_block_at(selection.start)
            && let Some(edit) = indent_code_lines(&self.text, body, selection.clone(), forward)
        {
            self.history.close_group();
            self.apply_edit(edit.range, &edit.replacement, edit.selection_after, ComposerChangeKind::Other, cx);
            return;
        }
        let edit = if forward { list_indent_for_tab(&self.text, selection) } else { list_outdent_for_tab(&self.text, selection) };
        match edit {
            Some(edit) if self.rich_text => {
                self.history.close_group();
                self.apply_edit(edit.range, &edit.replacement, edit.cursor_after..edit.cursor_after, ComposerChangeKind::Other, cx);
            }
            _ if forward => cx.emit(ComposerEditorEvent::Tab),
            _ => {}
        }
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(ComposerEditorEvent::Escape);
    }

    fn on_page(&mut self, down: bool, cx: &mut Context<Self>) {
        let can_scroll = self.layout.borrow().as_ref().is_some_and(|l| {
            if down { self.scroll_y.get() + l.visible_height() < l.content_height } else { self.scroll_y.get() > px(0.) }
        });
        if !can_scroll {
            cx.emit(ComposerEditorEvent::PageScroll { down });
            return;
        }
        if let Some(target) = self.vertical_target(down, true) {
            self.move_to(target, cx);
        }
    }

    fn on_page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.on_page(false, cx);
    }

    fn on_page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.on_page(true, cx);
    }

    fn on_toggle_bold(&mut self, _: &ToggleBold, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_mark("**", cx);
    }

    fn on_toggle_italic(&mut self, _: &ToggleItalic, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_mark("*", cx);
    }

    fn on_character_palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, _: &mut Context<Self>) {
        window.show_character_palette();
    }

    // ---- Mouse ----

    pub(crate) fn on_mouse_down_at(&mut self, position: Point<Pixels>, click_count: usize, shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let Some(offset) = self.layout.borrow().as_ref().and_then(|l| l.offset_for_window_point(&self.doc, position, self.scroll_y.get())) else {
            return;
        };
        self.goal_x = None;
        match click_count {
            2 => {
                let word = self.word_range_at(offset);
                self.selecting = Some(SelectMode::Word(word.clone()));
                self.select_range(word, cx);
            }
            n if n >= 3 => {
                let line = self.line_range_at(offset);
                self.selecting = Some(SelectMode::Line(line.clone()));
                self.select_range(line, cx);
            }
            _ => {
                self.selecting = Some(SelectMode::Char);
                if shift {
                    self.select_to(offset, cx);
                } else {
                    self.move_to(offset, cx);
                }
            }
        }
    }

    pub(crate) fn on_mouse_drag_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mode) = self.selecting.clone() else { return };
        let Some(offset) = self.layout.borrow().as_ref().and_then(|l| l.offset_for_window_point(&self.doc, position, self.scroll_y.get())) else {
            return;
        };
        let anchor = match &mode {
            SelectMode::Char => {
                self.select_to(offset, cx);
                return;
            }
            SelectMode::Word(anchor) | SelectMode::Line(anchor) => anchor.clone(),
        };
        let unit = match mode {
            SelectMode::Word(_) => self.word_range_at(offset),
            _ => self.line_range_at(offset),
        };
        if unit.start < anchor.start {
            self.selected = unit.start..anchor.end;
            self.reversed = true;
        } else {
            self.selected = anchor.start..unit.end.max(anchor.end);
            self.reversed = false;
        }
        self.scroll_to_caret.set(true);
        cx.notify();
    }

    pub(crate) fn on_mouse_up(&mut self) {
        self.selecting = None;
    }

    pub(crate) fn scroll_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        let Some(layout) = self.layout.borrow().clone() else { return };
        let max = (layout.content_height - layout.visible_height()).max(px(0.));
        let next = (self.scroll_y.get() - delta).clamp(px(0.), max);
        if next != self.scroll_y.get() {
            self.scroll_y.set(next);
            cx.notify();
        }
    }

    /// Whether focus differs from what was last reported.
    pub(crate) fn focus_changed(&self, focused: bool) -> bool {
        focused != self.was_focused
    }

    /// Records and reports a focus change (deferred from the element).
    pub(crate) fn sync_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        if focused != self.was_focused {
            self.was_focused = focused;
            cx.emit(ComposerEditorEvent::FocusChanged(focused));
        }
    }

    /// Wraps the editor in its element and action handlers.
    pub fn element(editor: &gpui_kit::Entity<Self>, cx: &App) -> PromptEditorElement {
        PromptEditorElement::new(editor.clone(), cx)
    }

    /// Registers the editor's action handlers on a container element.
    pub fn actions<E: gpui_kit::InteractiveElement>(element: E, cx: &mut Context<Self>) -> E {
        element
            .key_context(EDITOR_CONTEXT)
            .on_action(cx.listener(Self::on_move_left))
            .on_action(cx.listener(Self::on_move_right))
            .on_action(cx.listener(Self::on_move_up))
            .on_action(cx.listener(Self::on_move_down))
            .on_action(cx.listener(Self::on_select_left))
            .on_action(cx.listener(Self::on_select_right))
            .on_action(cx.listener(Self::on_select_up))
            .on_action(cx.listener(Self::on_select_down))
            .on_action(cx.listener(Self::on_move_word_left))
            .on_action(cx.listener(Self::on_move_word_right))
            .on_action(cx.listener(Self::on_select_word_left))
            .on_action(cx.listener(Self::on_select_word_right))
            .on_action(cx.listener(Self::on_move_line_start))
            .on_action(cx.listener(Self::on_move_line_end))
            .on_action(cx.listener(Self::on_select_line_start))
            .on_action(cx.listener(Self::on_select_line_end))
            .on_action(cx.listener(Self::on_move_doc_start))
            .on_action(cx.listener(Self::on_move_doc_end))
            .on_action(cx.listener(Self::on_select_doc_start))
            .on_action(cx.listener(Self::on_select_doc_end))
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_backspace))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_delete_word_left))
            .on_action(cx.listener(Self::on_delete_word_right))
            .on_action(cx.listener(Self::on_delete_to_line_start))
            .on_action(cx.listener(Self::on_copy))
            .on_action(cx.listener(Self::on_cut))
            .on_action(cx.listener(Self::on_paste))
            .on_action(cx.listener(Self::on_paste_plain))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_enter))
            .on_action(cx.listener(Self::on_shift_enter))
            .on_action(cx.listener(Self::on_mod_enter))
            .on_action(cx.listener(Self::on_mod_alt_enter))
            .on_action(cx.listener(Self::on_tab))
            .on_action(cx.listener(Self::on_shift_tab))
            .on_action(cx.listener(Self::on_escape))
            .on_action(cx.listener(Self::on_page_up))
            .on_action(cx.listener(Self::on_page_down))
            .on_action(cx.listener(Self::on_toggle_bold))
            .on_action(cx.listener(Self::on_toggle_italic))
            .on_action(cx.listener(Self::on_character_palette))
    }

    // ---- UTF-16 conversion for the platform input handler ----

    fn offset_from_utf16(&self, utf16: usize) -> usize {
        let mut utf8 = 0;
        let mut count = 0;
        for c in self.text.chars() {
            if count >= utf16 {
                break;
            }
            count += c.len_utf16();
            utf8 += c.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())].encode_utf16().count()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for ComposerPromptEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.history.close_group();
        cx.notify();
    }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let range = self.snap(range.start, false)..self.snap(range.end, true);
        self.marked = None;
        let caret = range.start + text.len();
        let kind = ComposerChangeKind::of(&range, text.len());
        self.apply_edit(range, text, caret..caret, kind, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.disabled {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        // IME input replaces its own range on each keystroke; it is still typing.
        let caret = range.start + new_text.len();
        self.apply_edit(range.clone(), new_text, caret..caret, ComposerChangeKind::Insert, cx);
        self.marked = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let start16 = self.offset_to_utf16(range.start);
        let len16 = new_text.encode_utf16().count();
        if let Some(selected) = new_selected_range_utf16 {
            let sel = start16 + selected.start.min(len16)..start16 + selected.end.min(len16);
            self.selected = self.range_from_utf16(&sel);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: gpui_kit::Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<gpui_kit::Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let layout = self.layout.borrow().clone()?;
        layout.window_bounds_for_offset(&self.doc, range.start, self.scroll_y.get())
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let offset = self.layout.borrow().as_ref()?.offset_for_window_point(&self.doc, point, self.scroll_y.get())?;
        Some(self.offset_to_utf16(offset))
    }
}
