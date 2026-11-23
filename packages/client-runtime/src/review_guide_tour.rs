//! The review guide's tour: stepping through stops, the files a stop covers,
//! lettered notes, and cutting a stop's excerpt out of a file's diff. Ports
//! `reviewGuideTour.ts`.

use std::collections::BTreeMap;

use serde::Deserialize;
use t3code_contracts::{PullRequestReviewGuideExcerpt, PullRequestReviewGuideExcerptSide, PullRequestReviewGuideNote, PullRequestReviewGuideStop};

/// The next tour position, clamped to the stops.
pub fn step_tour_position(current: usize, forward: bool, stop_count: usize) -> usize {
    let next = if forward { current as i64 + 1 } else { current as i64 - 1 };
    next.min(stop_count as i64 - 1).max(0) as usize
}

/// The files a stop shows, excerpts first, without repeats.
pub fn stop_file_paths(stop: &PullRequestReviewGuideStop) -> Vec<&str> {
    let mut paths: Vec<&str> = Vec::new();
    for path in stop.excerpts.iter().map(|e| e.path.as_str()).chain(stop.files.iter().map(|f| f.path.as_str())) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// A note's marker: A to Z, then numbers.
pub fn note_marker(index: usize) -> String {
    if index < 26 { char::from(b'A' + index as u8).to_string() } else { (index + 1).to_string() }
}

/// One block of a hunk as `@pierre/diffs` describes it: shared context, or a change.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HunkContent {
    #[serde(rename_all = "camelCase")]
    Context { lines: usize, addition_line_index: usize },
    #[serde(rename_all = "camelCase")]
    Change { deletions: usize, additions: usize, deletion_line_index: usize, addition_line_index: usize },
}

/// One hunk: where each side starts and its blocks.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub deletion_start: usize,
    pub addition_start: usize,
    pub hunk_content: Vec<HunkContent>,
}

/// A file's hunks and the lines they index into, each line keeping its newline.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub hunks: Vec<DiffHunk>,
    pub addition_lines: Vec<String>,
    pub deletion_lines: Vec<String>,
}

/// What a diff row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffRowKind {
    Context,
    Addition,
    Deletion,
}

impl DiffRowKind {
    fn prefix(self) -> char {
        match self {
            Self::Context => ' ',
            Self::Addition => '+',
            Self::Deletion => '-',
        }
    }

    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Addition => "addition",
            Self::Deletion => "deletion",
        }
    }
}

/// One line of a file's diff, with the line numbers each side is at.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRow {
    pub kind: DiffRowKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub old_at: usize,
    pub new_at: usize,
    pub text: String,
    pub hunk: usize,
}

/// Every row of a file's diff in order.
pub fn file_diff_rows(file: &FileDiff) -> Vec<DiffRow> {
    let line = |lines: &[String], index: usize| lines.get(index).cloned().unwrap_or_default();
    let mut rows = Vec::new();
    for (hunk_index, hunk) in file.hunks.iter().enumerate() {
        let mut old_at = hunk.deletion_start;
        let mut new_at = hunk.addition_start;
        for content in &hunk.hunk_content {
            match *content {
                HunkContent::Context { lines, addition_line_index } => {
                    for offset in 0..lines {
                        rows.push(DiffRow {
                            kind: DiffRowKind::Context,
                            old_line: Some(old_at),
                            new_line: Some(new_at),
                            old_at,
                            new_at,
                            text: line(&file.addition_lines, addition_line_index + offset),
                            hunk: hunk_index,
                        });
                        old_at += 1;
                        new_at += 1;
                    }
                }
                HunkContent::Change { deletions, additions, deletion_line_index, addition_line_index } => {
                    for offset in 0..deletions {
                        rows.push(DiffRow {
                            kind: DiffRowKind::Deletion,
                            old_line: Some(old_at),
                            new_line: None,
                            old_at,
                            new_at,
                            text: line(&file.deletion_lines, deletion_line_index + offset),
                            hunk: hunk_index,
                        });
                        old_at += 1;
                    }
                    for offset in 0..additions {
                        rows.push(DiffRow {
                            kind: DiffRowKind::Addition,
                            old_line: None,
                            new_line: Some(new_at),
                            old_at,
                            new_at,
                            text: line(&file.addition_lines, addition_line_index + offset),
                            hunk: hunk_index,
                        });
                        new_at += 1;
                    }
                }
            }
        }
    }
    rows
}

/// The rows an excerpt covers, widened to keep a change's other half in the same hunk.
pub fn excerpt_rows<'a>(rows: &'a [DiffRow], excerpt: &PullRequestReviewGuideExcerpt) -> &'a [DiffRow] {
    let new_side = excerpt.side == PullRequestReviewGuideExcerptSide::New;
    let in_range = |row: &DiffRow| {
        let line = if new_side { row.new_line } else { row.old_line };
        line.is_some_and(|l| l as u64 >= excerpt.start && l as u64 <= excerpt.end)
    };
    let Some(mut first) = rows.iter().position(in_range) else { return &[] };
    let mut last = rows.iter().rposition(in_range).unwrap_or(first);
    if new_side && rows[first].kind == DiffRowKind::Addition {
        while first > 0 && rows[first - 1].kind == DiffRowKind::Deletion && rows[first - 1].hunk == rows[first].hunk {
            first -= 1;
        }
    }
    if !new_side && rows[last].kind == DiffRowKind::Deletion {
        while last + 1 < rows.len() && rows[last + 1].kind == DiffRowKind::Addition && rows[last + 1].hunk == rows[last].hunk {
            last += 1;
        }
    }
    &rows[first..=last]
}

/// A unified patch of `rows`, one hunk per source hunk.
pub fn rows_to_patch(path: &str, previous_path: &str, rows: &[DiffRow]) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        format!("diff --git a/{previous_path} b/{path}"),
        format!("--- a/{previous_path}"),
        format!("+++ b/{path}"),
    ];
    for group in rows.chunk_by(|a, b| a.hunk == b.hunk) {
        let head = &group[0];
        let old_count = group.iter().filter(|r| r.old_line.is_some()).count();
        let new_count = group.iter().filter(|r| r.new_line.is_some()).count();
        // Git names the line before an empty side, so a pure insertion after line 9 is `-9,0`.
        let old_start = if old_count == 0 { head.old_at.saturating_sub(1) } else { head.old_at };
        let new_start = if new_count == 0 { head.new_at.saturating_sub(1) } else { head.new_at };
        lines.push(format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"));
        for row in group {
            lines.push(format!("{}{}", row.kind.prefix(), row.text.strip_suffix('\n').unwrap_or(&row.text)));
            if !row.text.ends_with('\n') {
                lines.push("\\ No newline at end of file".into());
            }
        }
    }
    format!("{}\n", lines.join("\n"))
}

/// Splits text into lines that keep their `\n`, as `/[^\n]*\n|[^\n]+$/g` does.
fn split_keep_newlines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// An old file that agrees with the new one outside the excerpt's single hunk, so the renderer
/// can fill the code around it. `None` when the rows span several hunks.
pub fn excerpt_expansion_old_contents(new_contents: &str, rows: &[DiffRow]) -> Option<String> {
    let head = rows.first()?;
    if rows.iter().any(|r| r.hunk != head.hunk) {
        return None;
    }
    let new_lines = split_keep_newlines(new_contents);
    let before = head.new_at.saturating_sub(1).min(new_lines.len());
    let new_count = rows.iter().filter(|r| r.new_line.is_some()).count();
    let old_before = head.old_at.saturating_sub(1);
    let prefix = &new_lines[..before];
    let mut out = String::new();
    if old_before >= prefix.len() {
        out.extend(std::iter::repeat_n("\n", old_before - prefix.len()));
        out.extend(prefix.iter().copied());
    } else {
        out.extend(prefix[prefix.len() - old_before..].iter().copied());
    }
    let old_rows: Vec<&str> = rows.iter().filter(|r| r.old_line.is_some()).map(|r| r.text.as_str()).collect();
    let suffix = new_lines.get((before + new_count).min(new_lines.len())..).unwrap_or(&[]);
    out.extend(old_rows.iter().copied());
    // Synthetic context still needs its own line after an old EOF row.
    if !suffix.is_empty() && old_rows.last().is_some_and(|t| !t.ends_with('\n')) {
        out.push('\n');
    }
    out.extend(suffix.iter().copied());
    Some(out)
}

/// A stop's notes with their markers, and which markers sit on which line of each excerpt.
#[derive(Debug, Clone, PartialEq)]
pub struct LetteredNotes<'a> {
    pub lettered: Vec<(String, &'a PullRequestReviewGuideNote)>,
    /// Per excerpt, in excerpt order: each line with its markers and note titles, in first-seen order.
    pub by_excerpt: Vec<Vec<(u64, Vec<(String, &'a str)>)>>,
}

/// Letters a stop's notes and places them on the excerpts they fall inside.
pub fn letter_stop_notes(stop: &PullRequestReviewGuideStop) -> LetteredNotes<'_> {
    let lettered: Vec<_> = stop.notes.iter().enumerate().map(|(i, note)| (note_marker(i), note)).collect();
    let by_excerpt = stop
        .excerpts
        .iter()
        .map(|excerpt| {
            let mut order: Vec<u64> = Vec::new();
            let mut lines: BTreeMap<u64, Vec<(String, &str)>> = BTreeMap::new();
            for (marker, note) in &lettered {
                if note.path != excerpt.path || note.line < excerpt.start || note.line > excerpt.end {
                    continue;
                }
                if !lines.contains_key(&note.line) {
                    order.push(note.line);
                }
                lines.entry(note.line).or_default().push((marker.clone(), note.title.as_str()));
            }
            order.into_iter().map(|line| (line, lines.remove(&line).unwrap_or_default())).collect()
        })
        .collect();
    LetteredNotes { lettered, by_excerpt }
}
