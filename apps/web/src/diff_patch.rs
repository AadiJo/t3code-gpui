//! Unified diffs as the server sends them (`orchestration.getTurnDiff`,
//! `getFullThreadDiff`, `review.getDiffPreview`), parsed into files, hunks and
//! lines. Ports what the panel uses of Pierre's `parsePatchFiles`: git headers
//! (renames, copies, modes, binary), plain `---`/`+++` patches, quoted paths,
//! "\ No newline at end of file", and the checkpoint truncation marker.

use std::{
    hash::{DefaultHasher, Hash as _, Hasher as _},
    sync::Arc,
};

use gpui_kit::SharedString;
use serde::{Deserialize, Serialize};

use crate::syntax::{LanguageId, TextLines, language_for_path};

/// Appended by the server when a checkpoint diff was cut at its size limit.
pub const CHECKPOINT_DIFF_TRUNCATED_MARKER: &str = "\n\n[truncated]";

/// How a file changed. Matches `ReviewDiffChangeType` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeType {
    Change,
    RenamePure,
    RenameChanged,
    New,
    Deleted,
}

impl ChangeType {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeType::Change => "change",
            ChangeType::RenamePure => "rename-pure",
            ChangeType::RenameChanged => "rename-changed",
            ChangeType::New => "new",
            ChangeType::Deleted => "deleted",
        }
    }

    pub fn is_rename(self) -> bool {
        matches!(self, ChangeType::RenamePure | ChangeType::RenameChanged)
    }
}

/// One run of lines inside a hunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkBlock {
    /// Unchanged lines, present on both sides.
    Context { lines: u32 },
    /// Deleted lines followed by added lines.
    Change { deletions: u32, additions: u32 },
}

/// One `@@` section.
#[derive(Clone, Debug)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The function context git prints after the second `@@`.
    pub context: SharedString,
    /// Index of the hunk's first line in [`FileDiff::old_lines`].
    pub old_index: u32,
    /// Index of the hunk's first line in [`FileDiff::new_lines`].
    pub new_index: u32,
    pub blocks: Vec<HunkBlock>,
    pub old_no_newline: bool,
    pub new_no_newline: bool,
}

impl Hunk {
    /// First new-side line after this hunk, 1-based.
    pub fn new_end(&self) -> u32 {
        self.new_start + self.new_count
    }

    pub fn old_end(&self) -> u32 {
        self.old_start + self.old_count
    }
}

/// One file of a patch.
#[derive(Clone, Debug)]
pub struct FileDiff {
    /// The file's path after the change (before it, for a deleted file).
    pub path: SharedString,
    /// The path before a rename or copy.
    pub prev_path: Option<SharedString>,
    pub change: ChangeType,
    pub old_mode: Option<SharedString>,
    pub new_mode: Option<SharedString>,
    pub binary: bool,
    /// Old-side lines the hunks show: context and deletions, in order.
    pub old_lines: TextLines,
    /// New-side lines the hunks show: context and additions, in order.
    pub new_lines: TextLines,
    pub hunks: Vec<Hunk>,
    pub additions: u32,
    pub deletions: u32,
    pub language: Option<LanguageId>,
    /// Hash of this file's section of the patch; changes when its content does.
    pub version: u64,
}

impl FileDiff {
    /// The path before the change: the rename source, else the path.
    pub fn previous_path(&self) -> &SharedString {
        self.prev_path.as_ref().unwrap_or(&self.path)
    }

    /// A mode change with nothing else, such as `chmod +x`.
    pub fn mode_change(&self) -> Option<(&SharedString, &SharedString)> {
        match (&self.old_mode, &self.new_mode) {
            (Some(old), Some(new)) if old != new => Some((old, new)),
            _ => None,
        }
    }

    /// Stable identity within a patch: previous path, path and change type.
    /// Ports `buildFileDiffIdentityKey`.
    pub fn identity_key(&self) -> String {
        format!("{}\0{}\0{}", self.previous_path(), self.path, self.change.as_str())
    }

    /// Unchanged new-side lines between the previous hunk (or the file start) and hunk `index`.
    pub fn lines_hidden_before(&self, index: usize) -> u32 {
        let hunk = &self.hunks[index];
        let (start, previous_end) = if self.change == ChangeType::Deleted {
            (hunk.old_start, index.checked_sub(1).map_or(1, |i| self.hunks[i].old_end()))
        } else {
            (hunk.new_start, index.checked_sub(1).map_or(1, |i| self.hunks[i].new_end()))
        };
        start.saturating_sub(previous_end.max(1))
    }
}

/// A whole parsed patch.
#[derive(Clone, Debug, Default)]
pub struct ParsedPatch {
    pub files: Vec<Arc<FileDiff>>,
    /// The server cut the patch at its size limit.
    pub truncated: bool,
}

impl ParsedPatch {
    /// Total added and deleted lines. Ports `getDiffLineStat`.
    pub fn line_stat(&self) -> (u64, u64) {
        self.files.iter().fold((0, 0), |(a, d), file| (a + file.additions as u64, d + file.deletions as u64))
    }
}

/// What the panel shows for a patch: parsed files, or the raw text with a reason.
/// Ports `RenderablePatch`.
#[derive(Clone, Debug)]
pub enum RenderablePatch {
    Files(ParsedPatch),
    Raw { text: SharedString, reason: SharedString },
}

/// Parses a patch for display. `None` for an empty patch. Ports `getRenderablePatch`.
pub fn renderable_patch(patch: &str) -> Option<RenderablePatch> {
    if patch.trim().is_empty() {
        return None;
    }
    let parsed = parse_patch(patch);
    if parsed.files.is_empty() {
        return Some(RenderablePatch::Raw {
            text: patch.to_owned().into(),
            reason: "Unsupported diff format. Showing raw patch.".into(),
        });
    }
    Some(RenderablePatch::Files(parsed))
}

/// Parses every file in a unified or git diff.
pub fn parse_patch(patch: &str) -> ParsedPatch {
    let (patch, truncated) = match patch.strip_suffix(CHECKPOINT_DIFF_TRUNCATED_MARKER) {
        Some(rest) => (rest, true),
        None => (patch, false),
    };
    let mut parser = Parser::default();
    for line in patch.split('\n') {
        parser.line(line.strip_suffix('\r').unwrap_or(line));
    }
    parser.finish_file();
    ParsedPatch { files: parser.files, truncated }
}

#[derive(Default)]
struct FileBuilder {
    git_old: Option<String>,
    git_new: Option<String>,
    minus: Option<String>,
    plus: Option<String>,
    rename_from: Option<String>,
    rename_to: Option<String>,
    old_mode: Option<String>,
    new_mode: Option<String>,
    new_file: bool,
    deleted_file: bool,
    binary: bool,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    hunks: Vec<Hunk>,
    additions: u32,
    deletions: u32,
    hasher: DefaultHasher,
}

#[derive(Default)]
struct Parser {
    files: Vec<Arc<FileDiff>>,
    file: Option<FileBuilder>,
    /// Old and new lines still expected in the open hunk.
    remaining: (u32, u32),
    /// Which side the last hunk line was on, for "\ No newline".
    last_side: Option<LineSide>,
}

#[derive(Clone, Copy)]
enum LineSide {
    Context,
    Old,
    New,
}

impl Parser {
    fn in_hunk(&self) -> bool {
        self.remaining.0 > 0 || self.remaining.1 > 0
    }

    fn line(&mut self, line: &str) {
        if let Some(file) = self.file.as_mut() {
            line.hash(&mut file.hasher);
        }
        if self.in_hunk() {
            self.hunk_line(line);
            return;
        }
        if line.starts_with('\\') {
            self.no_newline();
            return;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            self.finish_file();
            let mut file = FileBuilder::default();
            if let Some((old, new)) = split_git_header(rest) {
                file.git_old = Some(old);
                file.git_new = Some(new);
            }
            line.hash(&mut file.hasher);
            self.file = Some(file);
            return;
        }
        if let Some(rest) = line.strip_prefix("--- ") {
            // A plain unified diff, or the next file of one, starts at `---`.
            let starts_new = self.file.as_ref().is_none_or(|file| !file.hunks.is_empty() || file.minus.is_some());
            if starts_new {
                self.finish_file();
                self.file = Some(FileBuilder::default());
            }
            let file = self.file.as_mut().expect("file started");
            file.minus = Some(header_path(rest));
            return;
        }
        let Some(file) = self.file.as_mut() else { return };
        if let Some(rest) = line.strip_prefix("+++ ") {
            file.plus = Some(header_path(rest));
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            self.start_hunk(rest);
        } else if let Some(mode) = line.strip_prefix("new file mode ") {
            file.new_file = true;
            file.new_mode = Some(mode.trim().to_owned());
        } else if let Some(mode) = line.strip_prefix("deleted file mode ") {
            file.deleted_file = true;
            file.old_mode = Some(mode.trim().to_owned());
        } else if let Some(mode) = line.strip_prefix("old mode ") {
            file.old_mode = Some(mode.trim().to_owned());
        } else if let Some(mode) = line.strip_prefix("new mode ") {
            file.new_mode = Some(mode.trim().to_owned());
        } else if let Some(path) = line.strip_prefix("rename from ").or_else(|| line.strip_prefix("copy from ")) {
            file.rename_from = Some(unquote(path));
        } else if let Some(path) = line.strip_prefix("rename to ").or_else(|| line.strip_prefix("copy to ")) {
            file.rename_to = Some(unquote(path));
        } else if let Some(rest) = line.strip_prefix("index ") {
            if let Some((_, mode)) = rest.split_once(' ') {
                file.old_mode.get_or_insert_with(|| mode.trim().to_owned());
                file.new_mode.get_or_insert_with(|| mode.trim().to_owned());
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            file.binary = true;
        }
    }

    fn start_hunk(&mut self, header: &str) {
        let Some(file) = self.file.as_mut() else { return };
        let Some((ranges, context)) = header.split_once(" @@") else { return };
        let mut parts = ranges.split(' ');
        let (Some(old), Some(new)) = (parts.next(), parts.next()) else { return };
        let parse_range = |range: &str| -> Option<(u32, u32)> {
            let range = &range[1..];
            Some(match range.split_once(',') {
                Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
                None => (range.parse().ok()?, 1),
            })
        };
        let (Some((old_start, old_count)), Some((new_start, new_count))) = (parse_range(old), parse_range(new)) else {
            return;
        };
        file.hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            context: context.trim().to_owned().into(),
            old_index: file.old_lines.len() as u32,
            new_index: file.new_lines.len() as u32,
            blocks: Vec::new(),
            old_no_newline: false,
            new_no_newline: false,
        });
        self.remaining = (old_count, new_count);
        self.last_side = None;
    }

    fn hunk_line(&mut self, line: &str) {
        let Some(file) = self.file.as_mut() else { return };
        let Some(hunk) = file.hunks.last_mut() else { return };
        let (marker, text) = match line.as_bytes().first() {
            Some(b'+') => (LineSide::New, &line[1..]),
            Some(b'-') => (LineSide::Old, &line[1..]),
            Some(b' ') => (LineSide::Context, &line[1..]),
            Some(b'\\') => {
                self.no_newline();
                return;
            }
            // Some tools strip the space of an empty context line.
            None => (LineSide::Context, ""),
            Some(_) => {
                self.remaining = (0, 0);
                return;
            }
        };
        match marker {
            LineSide::Context => {
                file.old_lines.push(text.to_owned());
                file.new_lines.push(text.to_owned());
                self.remaining.0 = self.remaining.0.saturating_sub(1);
                self.remaining.1 = self.remaining.1.saturating_sub(1);
                match hunk.blocks.last_mut() {
                    Some(HunkBlock::Context { lines }) => *lines += 1,
                    _ => hunk.blocks.push(HunkBlock::Context { lines: 1 }),
                }
            }
            LineSide::Old => {
                file.old_lines.push(text.to_owned());
                file.deletions += 1;
                self.remaining.0 = self.remaining.0.saturating_sub(1);
                match hunk.blocks.last_mut() {
                    Some(HunkBlock::Change { deletions, additions: 0 }) => *deletions += 1,
                    _ => hunk.blocks.push(HunkBlock::Change { deletions: 1, additions: 0 }),
                }
            }
            LineSide::New => {
                file.new_lines.push(text.to_owned());
                file.additions += 1;
                self.remaining.1 = self.remaining.1.saturating_sub(1);
                match hunk.blocks.last_mut() {
                    Some(HunkBlock::Change { additions, .. }) => *additions += 1,
                    _ => hunk.blocks.push(HunkBlock::Change { deletions: 0, additions: 1 }),
                }
            }
        }
        self.last_side = Some(marker);
    }

    fn no_newline(&mut self) {
        let Some(hunk) = self.file.as_mut().and_then(|file| file.hunks.last_mut()) else { return };
        match self.last_side {
            Some(LineSide::Old) => hunk.old_no_newline = true,
            Some(LineSide::New) => hunk.new_no_newline = true,
            Some(LineSide::Context) => {
                hunk.old_no_newline = true;
                hunk.new_no_newline = true;
            }
            None => {}
        }
    }

    fn finish_file(&mut self) {
        self.remaining = (0, 0);
        let Some(file) = self.file.take() else { return };
        let old_path = file.rename_from.clone().or(file.minus.clone()).or(file.git_old.clone());
        let new_path = file.rename_to.clone().or(file.plus.clone()).or(file.git_new.clone());
        let old_path = old_path.filter(|path| path != DEV_NULL);
        let new_path = new_path.filter(|path| path != DEV_NULL);
        let new_file = file.new_file || (file.minus.as_deref() == Some(DEV_NULL));
        let deleted = file.deleted_file || (file.plus.as_deref() == Some(DEV_NULL));
        let Some(path) = (if deleted { old_path.clone() } else { new_path.clone() }).or(old_path.clone()) else {
            return;
        };
        let renamed = !new_file && !deleted && old_path.as_ref().is_some_and(|old| *old != path);
        let change = if new_file {
            ChangeType::New
        } else if deleted {
            ChangeType::Deleted
        } else if renamed && file.hunks.is_empty() {
            ChangeType::RenamePure
        } else if renamed {
            ChangeType::RenameChanged
        } else {
            ChangeType::Change
        };
        let has_content = !file.hunks.is_empty()
            || file.binary
            || renamed
            || new_file
            || deleted
            || file.old_mode.as_ref().zip(file.new_mode.as_ref()).is_some_and(|(a, b)| a != b);
        if !has_content {
            return;
        }
        let language = language_for_path(&path);
        let mut hasher = file.hasher;
        change.as_str().hash(&mut hasher);
        self.files.push(Arc::new(FileDiff {
            prev_path: renamed.then(|| old_path.unwrap_or_default().into()),
            path: path.into(),
            change,
            old_mode: file.old_mode.map(Into::into),
            new_mode: file.new_mode.map(Into::into),
            binary: file.binary,
            old_lines: TextLines::from_lines(file.old_lines.iter().map(String::as_str)),
            new_lines: TextLines::from_lines(file.new_lines.iter().map(String::as_str)),
            hunks: file.hunks,
            additions: file.additions,
            deletions: file.deletions,
            language,
            version: hasher.finish(),
        }));
    }
}

const DEV_NULL: &str = "/dev/null";

/// The path from a `---` or `+++` line: unquoted, without its `a/` or `b/`
/// prefix or a trailing tab and timestamp.
fn header_path(rest: &str) -> String {
    let rest = if rest.starts_with('"') { rest } else { rest.split('\t').next().unwrap_or(rest) };
    let path = unquote(rest.trim_end());
    if path == DEV_NULL {
        return path;
    }
    strip_prefix(&path).to_owned()
}

fn strip_prefix(path: &str) -> &str {
    path.strip_prefix("a/").or_else(|| path.strip_prefix("b/")).unwrap_or(path)
}

/// Splits `a/x b/y` from a `diff --git` line. Unquoted names with spaces are
/// split where both halves name the same file, as git does for non-renames.
fn split_git_header(rest: &str) -> Option<(String, String)> {
    if rest.starts_with('"') {
        let end = quoted_end(rest)?;
        let old = unquote(&rest[..end]);
        let new = unquote(rest[end..].trim_start());
        return Some((strip_prefix(&old).to_owned(), strip_prefix(&new).to_owned()));
    }
    if let Some(index) = rest.find(" \"") {
        return Some((strip_prefix(&rest[..index]).to_owned(), strip_prefix(&unquote(&rest[index + 1..])).to_owned()));
    }
    let candidates: Vec<usize> = rest.match_indices(" b/").map(|(i, _)| i).collect();
    let split = candidates
        .iter()
        .copied()
        .find(|&i| strip_prefix(&rest[..i]) == &rest[i + 3..])
        .or(candidates.last().copied())?;
    Some((strip_prefix(&rest[..split]).to_owned(), rest[split + 3..].to_owned()))
}

fn quoted_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Decodes git's C-style quoting (`"a/caf\303\251.txt"`); other text is returned as is.
fn unquote(text: &str) -> String {
    let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) else {
        return text.to_owned();
    };
    let mut bytes = Vec::with_capacity(inner.len());
    let mut chars = inner.bytes().peekable();
    while let Some(byte) = chars.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match chars.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'a') => bytes.push(7),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(digit @ b'0'..=b'7') => {
                let mut value = (digit - b'0') as u32;
                for _ in 0..2 {
                    match chars.peek() {
                        Some(&next @ b'0'..=b'7') => {
                            value = value * 8 + (next - b'0') as u32;
                            chars.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Full old and new contents of a file, loaded so collapsed context can expand.
/// Comes from `review.getDiffFileContents` (`ReviewDiffFileContentsResult`).
#[derive(Clone, Debug)]
pub struct FileContents {
    pub old: TextLines,
    pub new: TextLines,
}

impl FileContents {
    pub fn new(old: &str, new: &str) -> Self {
        Self { old: TextLines::from_contents(old), new: TextLines::from_contents(new) }
    }
}
