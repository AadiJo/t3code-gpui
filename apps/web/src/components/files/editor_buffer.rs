//! The file editor's text: a vector of lines, byte positions, edits and an
//! undo history. A keystroke touches one line's `String`; inserting or
//! removing lines shifts the vector, which is a pointer move per line.

use std::{ops::Range, time::Instant};

use unicode_segmentation::UnicodeSegmentation as _;

/// A position: line index and byte column within the line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// A selection: `head` is where the caret is, `anchor` where it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
}

impl Selection {
    pub fn caret(pos: Pos) -> Self {
        Self { anchor: pos, head: pos }
    }

    pub fn range(&self) -> Range<Pos> {
        if self.anchor <= self.head { self.anchor..self.head } else { self.head..self.anchor }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// How the file ended its lines, kept on save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

/// The text as lines without terminators.
#[derive(Clone, Debug)]
pub struct Buffer {
    lines: Vec<String>,
    ending: LineEnding,
    /// The file ended with a newline.
    trailing_newline: bool,
}

/// One applied edit, enough to undo and redo it.
#[derive(Clone, Debug)]
struct Edit {
    /// Where the edit started.
    start: Pos,
    removed: String,
    inserted: String,
    before: Selection,
    after: Selection,
}

/// What kind of change a typing group holds, so a word typed in one go undoes as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Insert,
    Delete,
    Other,
}

struct Group {
    edits: Vec<Edit>,
    kind: EditKind,
    at: Instant,
}

/// Undo and redo stacks with typing grouped by kind and a one-second pause.
#[derive(Default)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    open: bool,
}

impl History {
    fn record(&mut self, edit: Edit, kind: EditKind) {
        self.redo.clear();
        let now = Instant::now();
        if self.open
            && kind != EditKind::Other
            && let Some(group) = self.undo.last_mut()
            && group.kind == kind
            && now.duration_since(group.at).as_millis() < 1000
        {
            group.edits.push(edit);
            group.at = now;
            return;
        }
        self.undo.push(Group { edits: vec![edit], kind, at: now });
        self.open = kind != EditKind::Other;
    }

    /// Ends the current typing group (a caret move or a click does this).
    pub fn close_group(&mut self) {
        self.open = false;
    }
}

impl Buffer {
    /// Splits contents on `\n`, remembering CRLF and a final newline.
    pub fn new(contents: &str) -> Self {
        let ending = if contents.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        let trailing_newline = contents.ends_with('\n');
        let body = contents.strip_suffix('\n').unwrap_or(contents);
        let body = if ending == LineEnding::CrLf { body.strip_suffix('\r').unwrap_or(body) } else { body };
        let lines = body
            .split('\n')
            .map(|line| if ending == LineEnding::CrLf { line.strip_suffix('\r').unwrap_or(line) } else { line }.to_owned())
            .collect();
        Self { lines, ending, trailing_newline }
    }

    /// The contents with the original line endings.
    pub fn text(&self) -> String {
        let separator = match self.ending {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        };
        let mut text = self.lines.join(separator);
        if self.trailing_newline {
            text.push_str(separator);
        }
        text
    }

    pub fn len_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, index: usize) -> &str {
        &self.lines[index]
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn end(&self) -> Pos {
        let last = self.lines.len() - 1;
        Pos::new(last, self.lines[last].len())
    }

    /// Clamps a position into the buffer and onto a char boundary.
    pub fn clamp(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.lines.len() - 1);
        let text = &self.lines[line];
        let mut col = pos.col.min(text.len());
        while !text.is_char_boundary(col) {
            col -= 1;
        }
        Pos::new(line, col)
    }

    /// The text in a range, lines joined with `\n`.
    pub fn slice(&self, range: Range<Pos>) -> String {
        if range.start.line == range.end.line {
            return self.lines[range.start.line][range.start.col..range.end.col].to_owned();
        }
        let mut out = String::new();
        out.push_str(&self.lines[range.start.line][range.start.col..]);
        for line in &self.lines[range.start.line + 1..range.end.line] {
            out.push('\n');
            out.push_str(line);
        }
        out.push('\n');
        out.push_str(&self.lines[range.end.line][..range.end.col]);
        out
    }

    /// Replaces a range and returns the position after the inserted text.
    fn splice(&mut self, range: Range<Pos>, text: &str) -> Pos {
        let start = range.start;
        let removed_lines = range.end.line - start.line;
        let tail = self.lines[range.end.line].split_off(range.end.col);
        self.lines[start.line].truncate(start.col);
        let mut parts = text.split('\n');
        self.lines[start.line].push_str(parts.next().unwrap_or(""));
        let mut new_lines: Vec<String> = parts.map(str::to_owned).collect();
        let count = new_lines.len();
        let end = match new_lines.last_mut() {
            Some(last) => {
                let end = Pos::new(start.line + count, last.len());
                last.push_str(&tail);
                end
            }
            None => {
                let end = Pos::new(start.line, self.lines[start.line].len());
                self.lines[start.line].push_str(&tail);
                end
            }
        };
        self.lines.splice(start.line + 1..start.line + 1 + removed_lines, new_lines);
        end
    }

    /// Applies an edit, records it in `history`, and returns the new caret.
    pub fn edit(&mut self, history: &mut History, before: Selection, range: Range<Pos>, text: &str, kind: EditKind) -> Pos {
        let removed = self.slice(range.clone());
        let end = self.splice(range.clone(), text);
        let after = Selection::caret(end);
        history.record(Edit { start: range.start, removed, inserted: text.to_owned(), before, after }, kind);
        end
    }

    /// Undoes the last group; returns the selection to restore and the first line touched.
    pub fn undo(&mut self, history: &mut History) -> Option<(Selection, usize)> {
        let group = history.undo.pop()?;
        history.open = false;
        let mut first = usize::MAX;
        for edit in group.edits.iter().rev() {
            let end = self.position_after(edit.start, &edit.inserted);
            self.splice(edit.start..end, &edit.removed);
            first = first.min(edit.start.line);
        }
        let selection = group.edits[0].before;
        history.redo.push(group);
        Some((selection, first))
    }

    /// Redoes the last undone group.
    pub fn redo(&mut self, history: &mut History) -> Option<(Selection, usize)> {
        let group = history.redo.pop()?;
        let mut first = usize::MAX;
        for edit in &group.edits {
            let end = self.position_after(edit.start, &edit.removed);
            self.splice(edit.start..end, &edit.inserted);
            first = first.min(edit.start.line);
        }
        let selection = group.edits.last().map(|e| e.after).unwrap_or_default();
        history.undo.push(group);
        Some((selection, first))
    }

    fn position_after(&self, start: Pos, text: &str) -> Pos {
        let mut parts = text.split('\n');
        let first = parts.next().unwrap_or("");
        match parts.next_back() {
            None => Pos::new(start.line, start.col + first.len()),
            Some(last) => Pos::new(start.line + text.matches('\n').count(), last.len()),
        }
    }

    /// The previous grapheme boundary, crossing to the end of the line above.
    pub fn prev_boundary(&self, pos: Pos) -> Pos {
        if pos.col == 0 {
            return if pos.line == 0 { pos } else { Pos::new(pos.line - 1, self.lines[pos.line - 1].len()) };
        }
        let text = &self.lines[pos.line][..pos.col];
        let col = text.grapheme_indices(true).next_back().map_or(0, |(i, _)| i);
        Pos::new(pos.line, col)
    }

    /// The next grapheme boundary, crossing to the start of the line below.
    pub fn next_boundary(&self, pos: Pos) -> Pos {
        let text = &self.lines[pos.line];
        if pos.col >= text.len() {
            return if pos.line + 1 >= self.lines.len() { pos } else { Pos::new(pos.line + 1, 0) };
        }
        let rest = &text[pos.col..];
        let step = rest.graphemes(true).next().map_or(rest.len(), str::len);
        Pos::new(pos.line, pos.col + step)
    }

    /// The start of the word before `pos` (word = alphanumeric or `_` run).
    pub fn prev_word(&self, pos: Pos) -> Pos {
        if pos.col == 0 {
            return self.prev_boundary(pos);
        }
        let text = &self.lines[pos.line][..pos.col];
        let trimmed = text.trim_end_matches(|c: char| !is_word_char(c));
        let start = trimmed.trim_end_matches(is_word_char).len();
        Pos::new(pos.line, if trimmed.is_empty() { 0 } else { start })
    }

    /// The end of the word after `pos`.
    pub fn next_word(&self, pos: Pos) -> Pos {
        let text = &self.lines[pos.line];
        if pos.col >= text.len() {
            return self.next_boundary(pos);
        }
        let rest = &text[pos.col..];
        let skipped = rest.len() - rest.trim_start_matches(|c: char| !is_word_char(c)).len();
        let word = rest[skipped..].len() - rest[skipped..].trim_start_matches(is_word_char).len();
        Pos::new(pos.line, pos.col + skipped + word)
    }

    /// The word around `pos`, for double click.
    pub fn word_at(&self, pos: Pos) -> Range<Pos> {
        let text = &self.lines[pos.line];
        let before = text[..pos.col].trim_end_matches(is_word_char).len();
        let after = pos.col + (text[pos.col..].len() - text[pos.col..].trim_start_matches(is_word_char).len());
        Pos::new(pos.line, before)..Pos::new(pos.line, after)
    }

    /// The leading whitespace of a line, copied onto a new line.
    pub fn indentation(&self, line: usize) -> &str {
        let text = &self.lines[line];
        &text[..text.len() - text.trim_start_matches([' ', '\t']).len()]
    }

    /// The indent unit for Tab: a tab when the file indents with tabs, else two spaces.
    pub fn indent_unit(&self) -> &'static str {
        if self.lines.iter().take(2000).any(|line| line.starts_with('\t')) { "\t" } else { "  " }
    }

    /// Bytes held, for memory reports.
    pub fn heap_size(&self) -> usize {
        self.lines.iter().map(|line| line.capacity() + std::mem::size_of::<String>()).sum()
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
