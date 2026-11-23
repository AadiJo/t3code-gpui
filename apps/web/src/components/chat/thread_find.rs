//! In-thread find. Ports `threadFind.ts`, `threadSearch.ts` and the matching
//! half of `threadFindHighlights.ts`: case-folded, non-overlapping matches in
//! the text a reader sees (markdown as rendered, code included), counted per
//! row in render order so the n-th match of a row is the n-th one drawn.
//!
//! Rows are searched from their message text, not from what is laid out, so
//! rows outside the viewport count too. Each row's folded text is cached; a
//! keystroke only scans the cache.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{Hsla, SharedString, TextRun};

use super::markdown_stream_blocks::parse_message_blocks;
use super::messages_timeline_logic::{RowKind, TimelineRow};
use super::timeline_entries::MessageRole;
use crate::markdown_pipeline::{Block, MarkdownFlavor};

/// Lowercases `text` where that keeps each character's byte length, so
/// offsets in the result are offsets in `text` (`foldThreadSearchCase`).
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let folded = match c {
            'ς' => 'σ',
            '‘' => '’',
            c => {
                let mut lower = c.to_lowercase();
                match (lower.next(), lower.next()) {
                    (Some(l), None) if l.len_utf8() == c.len_utf8() => l,
                    _ => c,
                }
            }
        };
        out.push(folded);
    }
    out
}

/// Byte offsets of the non-overlapping matches of `query` in `text`, both folded.
pub fn occurrences<'a>(folded_text: &'a str, folded_query: &'a str) -> impl Iterator<Item = usize> + 'a {
    folded_text.match_indices(folded_query).map(|(offset, _)| offset).filter(move |_| !folded_query.is_empty())
}

/// Calls `visit` with each searchable text of `blocks`, in the order the
/// markdown renderer draws them. Closed details count, as find opens folds.
pub fn visit_block_texts<'a>(blocks: &'a [Block], visit: &mut impl FnMut(&'a str)) {
    for block in blocks {
        match block {
            Block::Paragraph(inlines) | Block::Heading { content: inlines, .. } => visit(&inlines.text),
            Block::List(list) => list.items.iter().for_each(|item| visit_block_texts(&item.blocks, visit)),
            Block::Blockquote(blocks) | Block::Alert { blocks, .. } | Block::Details { blocks, .. } => visit_block_texts(blocks, visit),
            Block::Code(code) => visit(&code.code),
            Block::Table(table) => {
                table.header.iter().chain(table.rows.iter().flatten()).for_each(|cell| visit(&cell.text));
            }
            Block::Math(text) | Block::Html(text) => visit(text),
            Block::Footnotes(notes) => notes.iter().for_each(|(_, blocks)| visit_block_texts(blocks, visit)),
            Block::Rule | Block::Images(_) => {}
        }
    }
}

/// The text a row shows that find searches, if it is a searchable row.
fn row_source(row: &TimelineRow) -> Option<(&SharedString, MarkdownFlavor)> {
    match &row.kind {
        RowKind::Message(message) => Some((
            &message.message.text,
            if message.message.role == MessageRole::User { MarkdownFlavor::USER } else { MarkdownFlavor::ASSISTANT },
        )),
        RowKind::SentMessage { text } => Some((text, MarkdownFlavor::ASSISTANT)),
        RowKind::ProposedPlan(plan) => Some((&plan.markdown, MarkdownFlavor::ASSISTANT)),
        _ => None,
    }
}

/// One row's searchable text, folded, one string per rendered text run.
struct RowText {
    source: SharedString,
    segments: Vec<String>,
}

/// One match: the row and which of the row's matches it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindMatch {
    pub row: usize,
    pub occurrence: usize,
}

/// The folded text of every searchable row, kept across keystrokes.
#[derive(Default)]
pub struct FindIndex {
    rows: HashMap<SharedString, RowText>,
}

impl FindIndex {
    /// Every match of `query` in `rows`, in timeline order.
    pub fn search(&mut self, rows: &[TimelineRow], query: &str) -> Vec<FindMatch> {
        let query = fold(query);
        if query.is_empty() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let Some((source, flavor)) = row_source(row) else { continue };
            let text = self.row_text(&row.id, source, flavor);
            let count: usize = text.segments.iter().map(|segment| occurrences(segment, &query).count()).sum();
            matches.extend((0..count).map(|occurrence| FindMatch { row: index, occurrence }));
        }
        matches
    }

    fn row_text(&mut self, id: &SharedString, source: &SharedString, flavor: MarkdownFlavor) -> &RowText {
        let stale = self.rows.get(id).is_none_or(|cached| cached.source != *source);
        if stale {
            let pieces = parse_message_blocks(source, flavor, false);
            let mut segments = Vec::new();
            for piece in &pieces {
                visit_block_texts(piece, &mut |text| segments.push(fold(text)));
            }
            self.rows.insert(id.clone(), RowText { source: source.clone(), segments });
        }
        &self.rows[id]
    }

    /// Drops cached rows (another thread is shown).
    pub fn clear(&mut self) {
        self.rows.clear();
    }
}

/// `n/total`, clamped (`formatThreadFindCount`).
pub fn format_count(index: usize, total: usize) -> String {
    if total == 0 { "0/0".to_owned() } else { format!("{}/{}", index.min(total - 1) + 1, total) }
}

/// What the markdown renderer needs to highlight one row: the folded query,
/// which of the row's matches is active, and the search-match colours.
#[derive(Clone, Debug)]
pub struct FindHighlight {
    pub query: SharedString,
    pub active: Option<usize>,
    pub background: Hsla,
    pub foreground: Hsla,
    pub active_background: Hsla,
    pub active_foreground: Hsla,
}

impl FindHighlight {
    /// Highlights the matches in one text run of the row. `seen` counts the
    /// row's matches drawn so far and advances past this text's.
    pub fn apply(&self, text: &str, runs: Vec<TextRun>, seen: &mut usize) -> Vec<TextRun> {
        let folded = fold(text);
        let ranges: Vec<(Range<usize>, bool)> = occurrences(&folded, &self.query)
            .map(|offset| {
                let active = self.active == Some(*seen);
                *seen += 1;
                (offset..offset + self.query.len(), active)
            })
            .collect();
        if ranges.is_empty() {
            return runs;
        }
        split_runs(runs, &ranges, |run, active| {
            run.background_color = Some(if active { self.active_background } else { self.background });
            run.color = if active { self.active_foreground } else { self.foreground };
        })
    }

    /// Advances `seen` past the matches in text that is not drawn.
    pub fn skip(&self, text: &str, seen: &mut usize) {
        *seen += occurrences(&fold(text), &self.query).count();
    }
}

/// Splits `runs` at the range boundaries and styles the runs inside each range.
fn split_runs(runs: Vec<TextRun>, ranges: &[(Range<usize>, bool)], style: impl Fn(&mut TextRun, bool)) -> Vec<TextRun> {
    let mut out = Vec::with_capacity(runs.len() + ranges.len() * 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let mut cursor = start;
        while cursor < end {
            // The range covering `cursor`, or the next boundary.
            let inside = ranges.iter().find(|(r, _)| r.start <= cursor && cursor < r.end);
            let next = match inside {
                Some((r, _)) => r.end.min(end),
                None => ranges.iter().map(|(r, _)| r.start).filter(|&s| s > cursor).min().unwrap_or(end).min(end),
            };
            let mut piece = run.clone();
            piece.len = next - cursor;
            if let Some((_, active)) = inside {
                style(&mut piece, *active);
            }
            out.push(piece);
            cursor = next;
        }
        start = end;
    }
    out
}

/// Shared so every row's render reads the same matches.
pub type SharedMatches = Arc<[FindMatch]>;
