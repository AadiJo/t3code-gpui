//! Review comments on diff lines. Ports `buildDiffReviewComment` and its
//! helpers from `reviewCommentContext.ts`: a comment carries the selected
//! lines as a small diff that the composer sends with the next message.

use gpui_kit::SharedString;
use serde::Serialize;

use super::layout::{LineKind, Side};
use crate::diff_patch::{FileDiff, HunkBlock};

/// Pierre's `AnnotationSide`, on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationSide {
    Deletions,
    Additions,
}

impl From<Side> for AnnotationSide {
    fn from(side: Side) -> Self {
        match side {
            Side::Old => AnnotationSide::Deletions,
            Side::New => AnnotationSide::Additions,
        }
    }
}

/// The selected range a comment was made on. Ports `ReviewCommentSelection`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCommentSelection {
    pub start: u32,
    pub side: AnnotationSide,
    pub end: u32,
    pub end_side: AnnotationSide,
}

/// A comment ready for the composer draft. Ports `ReviewCommentContext`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComment {
    pub id: SharedString,
    pub section_id: SharedString,
    pub section_title: SharedString,
    pub file_path: SharedString,
    pub start_index: u32,
    pub end_index: u32,
    pub range_label: SharedString,
    pub text: SharedString,
    pub diff: SharedString,
    pub fence_language: SharedString,
    pub selection: ReviewCommentSelection,
}

/// One line of a file's hunks in stacked order, as `buildDiffReviewLines` lists them.
#[derive(Clone, Copy, Debug)]
struct ReviewLine {
    kind: LineKind,
    old: Option<u32>,
    new: Option<u32>,
    source_index: u32,
}

fn review_lines(file: &FileDiff) -> Vec<ReviewLine> {
    let mut lines = Vec::new();
    for hunk in &file.hunks {
        let (mut old, mut new) = (hunk.old_index, hunk.new_index);
        let (mut old_no, mut new_no) = (hunk.old_start, hunk.new_start);
        for block in &hunk.blocks {
            match *block {
                HunkBlock::Context { lines: count } => {
                    for _ in 0..count {
                        lines.push(ReviewLine { kind: LineKind::Context, old: Some(old_no), new: Some(new_no), source_index: new });
                        (old, new, old_no, new_no) = (old + 1, new + 1, old_no + 1, new_no + 1);
                    }
                }
                HunkBlock::Change { deletions, additions } => {
                    for i in 0..deletions {
                        lines.push(ReviewLine { kind: LineKind::Deletion, old: Some(old_no + i), new: None, source_index: old + i });
                    }
                    for i in 0..additions {
                        lines.push(ReviewLine { kind: LineKind::Addition, old: None, new: Some(new_no + i), source_index: new + i });
                    }
                    (old, new, old_no, new_no) = (old + deletions, new + additions, old_no + deletions, new_no + additions);
                }
            }
        }
    }
    lines
}

fn find_line(lines: &[ReviewLine], side: Side, number: u32) -> Option<usize> {
    lines.iter().position(|line| match side {
        Side::Old => line.kind != LineKind::Addition && line.old == Some(number),
        Side::New => line.kind != LineKind::Deletion && line.new == Some(number),
    })
}

fn marker(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Addition => "+",
        LineKind::Deletion => "-",
        LineKind::Context => " ",
    }
}

/// `+12`, `4 to 9`, `-3 to -5`, or `line` when numbers are missing.
fn range_label(lines: &[ReviewLine]) -> String {
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else { return "line".into() };
    let (Some(first_number), Some(last_number)) = (first.new.or(first.old), last.new.or(last.old)) else {
        return if lines.len() == 1 { "line".into() } else { format!("{} lines", lines.len()) };
    };
    let first_marker = marker(first.kind).trim();
    let marker = if !first_marker.is_empty() && lines.iter().all(|line| line.kind == first.kind) { first_marker } else { "" };
    if first_number == last_number {
        format!("{marker}{first_number}")
    } else {
        format!("{marker}{first_number} to {marker}{last_number}")
    }
}

/// Input to [`build_review_comment`].
pub struct ReviewCommentInput<'a> {
    pub id: SharedString,
    pub section_id: SharedString,
    pub section_title: SharedString,
    pub file: &'a FileDiff,
    pub start: (Side, u32),
    pub end: (Side, u32),
    pub text: &'a str,
}

/// Builds the comment for a line range, or `None` when the range is not in the hunks.
pub fn build_review_comment(input: ReviewCommentInput<'_>) -> Option<ReviewComment> {
    let lines = review_lines(input.file);
    let start = find_line(&lines, input.start.0, input.start.1)?;
    let end = find_line(&lines, input.end.0, input.end.1)?;
    let (start_index, end_index) = (start.min(end), start.max(end));
    let selected = &lines[start_index..=end_index];
    let range = |pick: fn(&ReviewLine) -> Option<u32>| {
        let numbered: Vec<u32> = selected.iter().filter_map(pick).collect();
        (numbered.first().copied().unwrap_or(0), numbered.len())
    };
    let (old_start, old_count) = range(|line| line.old);
    let (new_start, new_count) = range(|line| line.new);
    let mut diff = format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@");
    for line in selected {
        let text = match line.kind {
            LineKind::Deletion => input.file.old_lines.line(line.source_index as usize),
            _ => input.file.new_lines.line(line.source_index as usize),
        };
        diff.push('\n');
        diff.push_str(marker(line.kind));
        diff.push_str(text);
    }
    Some(ReviewComment {
        id: input.id,
        section_id: input.section_id,
        section_title: input.section_title,
        file_path: input.file.path.clone(),
        start_index: start_index as u32,
        end_index: end_index as u32,
        range_label: range_label(selected).into(),
        text: input.text.trim().to_owned().into(),
        diff: diff.into(),
        fence_language: "diff".into(),
        selection: ReviewCommentSelection {
            start: input.start.1,
            side: input.start.0.into(),
            end: input.end.1,
            end_side: input.end.0.into(),
        },
    })
}
