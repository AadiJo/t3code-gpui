//! Rows and geometry of the code view. A file's body is a flat list of rows
//! (lines, hunk separators, notices, annotations) with cumulative tops, built
//! only for expanded files and rebuilt when the width, layout, expansion or
//! annotations change. Metrics follow Pierre with T3's item metrics: 20px
//! lines, 32px file headers, 24px hunk separators, 8px under each open file.

use std::{collections::HashMap, ops::Range};

use smallvec::SmallVec;
use unicode_width::UnicodeWidthChar as _;

use crate::{
    diff_panel_store::DiffLayout,
    diff_patch::{ChangeType, FileContents, FileDiff, HunkBlock},
    syntax::TextLines,
};

pub const LINE_HEIGHT: f32 = 20.;
pub const HEADER_HEIGHT: f32 = 32.;
pub const SEPARATOR_HEIGHT: f32 = 24.;
pub const FILE_BOTTOM_PADDING: f32 = 8.;
pub const NOTICE_HEIGHT: f32 = 32.;
/// Pierre's `tab-size` default.
pub const TAB_SIZE: usize = 2;
/// Lines revealed per click on a separator's expand control (`expansionLineCount`).
pub const EXPANSION_STEP: u32 = 100;
/// Pierre's gutter column border.
const GUTTER_BORDER: f32 = 2.;

/// What a code line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Addition,
    Deletion,
}

/// Which side of a change a line number belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Old,
    New,
}

/// Where a line's text lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineSource {
    HunkOld,
    HunkNew,
    FullOld,
    FullNew,
}

/// A line of text in one of a file's sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineRef {
    pub source: LineSource,
    pub index: u32,
}

/// One painted code cell: a line with its kind and number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub kind: LineKind,
    pub line: LineRef,
    pub number: u32,
}

impl Cell {
    /// The side whose line number this cell shows.
    pub fn side(&self) -> Side {
        if self.kind == LineKind::Deletion { Side::Old } else { Side::New }
    }
}

/// A full-width notice in place of a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Binary,
    ModeChange,
    RenameOnly,
    Empty,
}

/// One row of a file body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    /// A stacked (unified) line.
    Line(Cell),
    /// A split row: old on the left, new on the right; either may be a buffer.
    Split(Option<Cell>, Option<Cell>),
    /// Collapsed unchanged lines in gap `gap`.
    Separator { gap: u32, hidden: u32 },
    NoNewline { old: bool, new: bool },
    /// Comments or a draft under the line `(side, number)`.
    Annotation { side: Side, number: u32 },
    Notice(Notice),
}

/// A row with its offset inside the file body.
#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub kind: RowKind,
    pub top: f32,
    pub height: f32,
}

/// Lines revealed in one gap: `top` below the previous hunk, `bottom` above the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GapExpansion {
    pub top: u32,
    pub bottom: u32,
}

/// Font metrics the layout depends on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Advance of one monospace cell.
    pub ch: f32,
}

impl Metrics {
    /// Width of the number column for `digits`: `2ch` left pad, the digits
    /// (min 3), `1ch` right pad, then the 2px column border.
    pub fn gutter_width(&self, digits: u32) -> f32 {
        self.ch * (3 + digits.max(3)) as f32 + GUTTER_BORDER
    }

    /// Code columns that fit in `width` beside a gutter, with `1ch` padding each side.
    pub fn code_columns(&self, width: f32, digits: u32) -> usize {
        (((width - self.gutter_width(digits)) / self.ch) - 2.).floor().max(8.) as usize
    }
}

/// Everything a file body's rows depend on besides the file itself.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutInput {
    pub layout: DiffLayout,
    /// Code columns per side when wrapping, else `None`.
    pub wrap_columns: Option<usize>,
    pub gaps: Vec<GapExpansion>,
    /// Height of each annotation, by the line it follows.
    pub annotations: Vec<((Side, u32), f32)>,
    pub has_contents: bool,
    /// A plain file view: no separators or notices.
    pub plain: bool,
}

/// A laid-out file body.
#[derive(Clone, Debug, Default)]
pub struct FileLayout {
    pub rows: Vec<Row>,
    pub height: f32,
    /// Digits of the widest line number, for the gutter width.
    pub digits: u32,
    /// Index of each hunk's first line row, for hunk navigation.
    pub hunk_starts: Vec<u32>,
}

impl FileLayout {
    /// Index of the row at `y` (body coordinates), clamped.
    pub fn row_at(&self, y: f32) -> usize {
        self.rows.partition_point(|row| row.top + row.height <= y).min(self.rows.len().saturating_sub(1))
    }

    /// Rows intersecting `range` (body coordinates).
    pub fn rows_in(&self, range: Range<f32>) -> Range<usize> {
        let start = self.rows.partition_point(|row| row.top + row.height <= range.start);
        let end = self.rows.partition_point(|row| row.top < range.end);
        start..end.max(start)
    }
}

/// Text of a line reference.
pub fn line_text<'a>(file: &'a FileDiff, contents: Option<&'a FileContents>, line: LineRef) -> &'a str {
    let lines = source_lines(file, contents, line.source);
    if (line.index as usize) < lines.len() { lines.line(line.index as usize) } else { "" }
}

pub fn source_lines<'a>(file: &'a FileDiff, contents: Option<&'a FileContents>, source: LineSource) -> &'a TextLines {
    static EMPTY: std::sync::LazyLock<TextLines> = std::sync::LazyLock::new(TextLines::default);
    match source {
        LineSource::HunkOld => &file.old_lines,
        LineSource::HunkNew => &file.new_lines,
        LineSource::FullOld => contents.map_or(&*EMPTY, |c| &c.old),
        LineSource::FullNew => contents.map_or(&*EMPTY, |c| &c.new),
    }
}

/// Number of gaps a file has: one before each hunk, plus a trailing one when contents are known.
pub fn gap_count(file: &FileDiff, has_contents: bool) -> usize {
    file.hunks.len() + usize::from(has_contents && !file.hunks.is_empty())
}

/// The unchanged new-side lines of gap `gap` as a 1-based half-open range, and the
/// old-minus-new line offset across it.
pub fn gap_lines(file: &FileDiff, contents: Option<&FileContents>, gap: usize) -> (Range<u32>, i64) {
    if let Some(hunk) = file.hunks.get(gap) {
        let start = gap.checked_sub(1).map_or(1, |i| file.hunks[i].new_end());
        let offset = hunk.old_start as i64 - hunk.new_start as i64;
        if file.change == ChangeType::Deleted {
            let start = gap.checked_sub(1).map_or(1, |i| file.hunks[i].old_end());
            return (start..hunk.old_start.max(start), 0);
        }
        return (start..hunk.new_start.max(start), offset);
    }
    let Some(last) = file.hunks.last() else { return (1..1, 0) };
    let total = contents.map_or(0, |c| c.new.len() as u32);
    let start = last.new_end();
    (start..(total + 1).max(start), last.old_end() as i64 - last.new_end() as i64)
}

/// Builds the rows of one file body.
pub fn layout_file(file: &FileDiff, contents: Option<&FileContents>, input: &LayoutInput) -> FileLayout {
    let mut builder = Builder {
        file,
        contents,
        input,
        rows: Vec::new(),
        top: 0.,
        annotations: input.annotations.iter().copied().collect(),
        hunk_starts: Vec::new(),
    };
    let digits = number_digits(file, contents);

    if input.plain {
        for index in 0..file.new_lines.len() as u32 {
            let cell = Cell { kind: LineKind::Context, line: LineRef { source: LineSource::HunkNew, index }, number: index + 1 };
            builder.push_line(Some(cell), None);
        }
        return builder.finish(digits);
    }
    if file.binary {
        builder.push(RowKind::Notice(Notice::Binary), NOTICE_HEIGHT);
        return builder.finish(digits);
    }
    if file.hunks.is_empty() {
        let notice = if file.mode_change().is_some() {
            Notice::ModeChange
        } else if file.change.is_rename() {
            Notice::RenameOnly
        } else {
            Notice::Empty
        };
        builder.push(RowKind::Notice(notice), NOTICE_HEIGHT);
        return builder.finish(digits);
    }
    for (index, hunk) in file.hunks.iter().enumerate() {
        builder.push_gap(index);
        builder.hunk_starts.push(builder.rows.len() as u32);
        let (mut old, mut new) = (hunk.old_index, hunk.new_index);
        let (mut old_no, mut new_no) = (hunk.old_start, hunk.new_start);
        for block in &hunk.blocks {
            match *block {
                HunkBlock::Context { lines } => {
                    for _ in 0..lines {
                        let left = Cell { kind: LineKind::Context, line: LineRef { source: LineSource::HunkOld, index: old }, number: old_no };
                        let right = Cell { kind: LineKind::Context, line: LineRef { source: LineSource::HunkNew, index: new }, number: new_no };
                        builder.push_line(Some(left), Some(right));
                        (old, new, old_no, new_no) = (old + 1, new + 1, old_no + 1, new_no + 1);
                    }
                }
                HunkBlock::Change { deletions, additions } => {
                    let deleted: Vec<Cell> = (0..deletions)
                        .map(|i| Cell { kind: LineKind::Deletion, line: LineRef { source: LineSource::HunkOld, index: old + i }, number: old_no + i })
                        .collect();
                    let added: Vec<Cell> = (0..additions)
                        .map(|i| Cell { kind: LineKind::Addition, line: LineRef { source: LineSource::HunkNew, index: new + i }, number: new_no + i })
                        .collect();
                    match input.layout {
                        DiffLayout::Stacked => {
                            for cell in deleted.iter().chain(&added) {
                                builder.push_line(Some(*cell), None);
                            }
                        }
                        DiffLayout::Split => {
                            for i in 0..deletions.max(additions) as usize {
                                builder.push_line(deleted.get(i).copied(), added.get(i).copied());
                            }
                        }
                    }
                    (old, new, old_no, new_no) = (old + deletions, new + additions, old_no + deletions, new_no + additions);
                }
            }
        }
        if hunk.old_no_newline || hunk.new_no_newline {
            builder.push(RowKind::NoNewline { old: hunk.old_no_newline, new: hunk.new_no_newline }, LINE_HEIGHT);
        }
    }
    if input.has_contents {
        builder.push_gap(file.hunks.len());
    }
    builder.finish(digits)
}

struct Builder<'a> {
    file: &'a FileDiff,
    contents: Option<&'a FileContents>,
    input: &'a LayoutInput,
    rows: Vec<Row>,
    top: f32,
    annotations: HashMap<(Side, u32), f32>,
    hunk_starts: Vec<u32>,
}

/// Digits of the widest line number a file can show.
pub fn number_digits(file: &FileDiff, contents: Option<&FileContents>) -> u32 {
    let max_number = file
        .hunks
        .iter()
        .map(|h| h.old_end().max(h.new_end()))
        .max()
        .unwrap_or(file.new_lines.len() as u32)
        .max(contents.map_or(0, |c| c.old.len().max(c.new.len()) as u32));
    max_number.max(1).ilog10() + 1
}

impl Builder<'_> {
    fn push(&mut self, kind: RowKind, height: f32) {
        self.rows.push(Row { kind, top: self.top, height });
        self.top += height;
    }

    fn finish(self, digits: u32) -> FileLayout {
        let padding = if self.input.plain { 0. } else { FILE_BOTTOM_PADDING };
        FileLayout { height: self.top + padding, rows: self.rows, digits, hunk_starts: self.hunk_starts }
    }

    fn visual_lines(&self, cell: Option<Cell>) -> usize {
        match (cell, self.input.wrap_columns) {
            (Some(cell), Some(columns)) => wrap_count(line_text(self.file, self.contents, cell.line), columns),
            _ => 1,
        }
    }

    /// Pushes a code row. Stacked rows use `left` for every line kind, so a
    /// context line passes its old cell as `left` and new as `right`.
    fn push_line(&mut self, left: Option<Cell>, right: Option<Cell>) {
        let (kind, anchors): (RowKind, SmallVec<[(Side, u32); 2]>) = match self.input.layout {
            DiffLayout::Stacked if !self.input.plain => {
                let cell = match (left, right) {
                    (Some(l), Some(r)) if l.kind == LineKind::Context => r,
                    (Some(l), _) => l,
                    (None, Some(r)) => r,
                    (None, None) => return,
                };
                (RowKind::Line(cell), smallvec::smallvec![(cell.side(), cell.number)])
            }
            _ if self.input.plain => {
                let cell = left.expect("plain rows have a cell");
                (RowKind::Line(cell), smallvec::smallvec![(Side::New, cell.number)])
            }
            _ => {
                let mut anchors = SmallVec::new();
                if let Some(r) = right {
                    anchors.push((Side::New, r.number));
                }
                if let Some(l) = left.filter(|l| l.kind == LineKind::Deletion) {
                    anchors.push((Side::Old, l.number));
                }
                (RowKind::Split(left, right), anchors)
            }
        };
        let lines = match kind {
            RowKind::Line(cell) => self.visual_lines(Some(cell)),
            RowKind::Split(l, r) => self.visual_lines(l).max(self.visual_lines(r)),
            _ => 1,
        };
        self.push(kind, LINE_HEIGHT * lines as f32);
        for (side, number) in anchors {
            if let Some(height) = self.annotations.get(&(side, number)).copied() {
                self.push(RowKind::Annotation { side, number }, height);
            }
        }
    }

    /// Pushes gap `gap`: expanded lines below the previous hunk, the separator
    /// for what stays hidden, then expanded lines above the next hunk.
    fn push_gap(&mut self, gap: usize) {
        let (range, offset) = gap_lines(self.file, self.contents, gap);
        let total = range.end - range.start;
        if total == 0 {
            return;
        }
        let expansion = self.input.gaps.get(gap).copied().unwrap_or_default();
        let can_expand = self.input.has_contents;
        let top = if can_expand { expansion.top.min(total) } else { 0 };
        let bottom = if can_expand { expansion.bottom.min(total - top) } else { 0 };
        let hidden = total - top - bottom;
        let deleted = self.file.change == ChangeType::Deleted;
        let push_context = |builder: &mut Self, new_no: u32| {
            let old_no = (new_no as i64 + offset).max(1) as u32;
            let (old_source, new_source) = (LineSource::FullOld, LineSource::FullNew);
            let left = Cell { kind: LineKind::Context, line: LineRef { source: old_source, index: old_no - 1 }, number: old_no };
            let right = if deleted {
                left
            } else {
                Cell { kind: LineKind::Context, line: LineRef { source: new_source, index: new_no - 1 }, number: new_no }
            };
            builder.push_line(Some(left), Some(right));
        };
        for number in range.start..range.start + top {
            push_context(self, number);
        }
        if hidden > 0 {
            self.push(RowKind::Separator { gap: gap as u32, hidden }, SEPARATOR_HEIGHT);
        }
        for number in range.end - bottom..range.end {
            push_context(self, number);
        }
    }
}

/// Display columns of `text` with tabs expanded.
pub fn display_columns(text: &str) -> usize {
    let mut columns = 0;
    for ch in text.chars() {
        columns += char_columns(ch, columns);
    }
    columns
}

/// Display columns of `ch` when it starts at display column `at`.
pub fn char_columns(ch: char, at: usize) -> usize {
    if ch == '\t' { TAB_SIZE - at % TAB_SIZE } else { ch.width().unwrap_or(0) }
}

/// Visual lines `text` takes when wrapped at `columns`.
pub fn wrap_count(text: &str, columns: usize) -> usize {
    if text.len() <= columns && text.is_ascii() && !text.contains('\t') {
        return 1;
    }
    wrap_segments(text, columns).len()
}

/// Byte ranges of each visual line of `text` wrapped at `columns`, breaking
/// after whitespace where it can and anywhere inside longer words
/// (`white-space: pre-wrap; word-break: break-word`).
pub fn wrap_segments(text: &str, columns: usize) -> SmallVec<[Range<usize>; 4]> {
    let columns = columns.max(1);
    let mut segments = SmallVec::new();
    let mut start = 0;
    let mut width = 0;
    let mut last_break: Option<(usize, usize)> = None;
    for (offset, ch) in text.char_indices() {
        let w = char_columns(ch, width);
        if width + w > columns && width > 0 && !ch.is_whitespace() {
            let (cut, carried) = match last_break {
                Some((cut, at)) if cut > start => (cut, width - at),
                _ => (offset, 0),
            };
            segments.push(start..cut);
            start = cut;
            width = carried;
            last_break = None;
        }
        width += char_columns(ch, width);
        if ch.is_whitespace() {
            last_break = Some((offset + ch.len_utf8(), width));
        }
    }
    segments.push(start..text.len());
    segments
}

/// Expands tabs in `text` to spaces at `TAB_SIZE` stops, starting at display column `column`.
/// Returns the text and, when it changed, the display offset of each source byte.
pub fn expand_tabs(text: &str, mut column: usize) -> (std::borrow::Cow<'_, str>, Option<Vec<u32>>) {
    if !text.contains('\t') {
        return (text.into(), None);
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut map = Vec::with_capacity(text.len() + 1);
    for ch in text.chars() {
        for _ in 0..ch.len_utf8() {
            map.push(out.len() as u32);
        }
        if ch == '\t' {
            let spaces = TAB_SIZE - column % TAB_SIZE;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(ch);
            column += ch.width().unwrap_or(0);
        }
    }
    map.push(out.len() as u32);
    (out.into(), Some(map))
}
