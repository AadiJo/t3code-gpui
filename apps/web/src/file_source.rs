//! The one data interface the Files tab, the file viewer, go to file, content
//! search and the media viewers read through. Each method is one server call;
//! [`DirectoryFiles`] answers them from a real directory on disk the way the
//! server's `WorkspaceEntries` and `WorkspaceFileSystem` do, for scenes and
//! end-to-end checks.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use futures::future::BoxFuture;
use regex::RegexBuilder;
use t3code_contracts::{
    ProjectContentMatch, ProjectContentMatchRange, ProjectEntry, ProjectEntryKind, ProjectFileFailure,
    ProjectListEntriesResult, ProjectReadFileResult, ProjectSearchContentsResult, ProjectSearchEntriesResult,
    ProjectWriteFileResult,
};

use crate::file_preview::{FILE_TEXT_PREVIEW_MAX_BYTES, decode_file_preview_text};

/// A failed call, with the server's failure kind when it sent one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub message: String,
    pub failure: Option<ProjectFileFailure>,
}

impl FileError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), failure: None }
    }

    fn failure(failure: ProjectFileFailure, message: impl Into<String>) -> Self {
        Self { message: message.into(), failure: Some(failure) }
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type FileResult<T> = Result<T, FileError>;
pub type FileFuture<T> = BoxFuture<'static, FileResult<T>>;

/// Content search options, `ProjectSearchContentsInput` without `cwd`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentQuery {
    pub query: String,
    pub limit: u64,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
}

/// Bytes of a file fetched through a signed asset URL.
#[derive(Clone, Debug)]
pub struct AssetBytes {
    pub bytes: Arc<[u8]>,
    /// `fileRevision` when the server reports it: the cache key for the bytes.
    pub revision: Option<String>,
}

/// One workspace's files on one server. Paths are workspace-relative with `/`.
/// Futures run on GPUI's background executor.
pub trait WorkspaceFiles: Send + Sync + 'static {
    /// The workspace root, for display and absolute paths.
    fn root(&self) -> &str;

    /// `projects.listEntries`: a directory's direct children (`None` is the root).
    fn list_entries(&self, directory: Option<String>) -> FileFuture<ProjectListEntriesResult>;

    /// `projects.readFile`: up to 1 MB of UTF-8 text, `truncated` past that.
    fn read_file(&self, relative_path: String) -> FileFuture<ProjectReadFileResult>;

    /// `projects.writeFile`: replaces the file's contents.
    fn write_file(&self, relative_path: String, contents: String) -> FileFuture<ProjectWriteFileResult>;

    /// `projects.searchEntries`: ranked fuzzy path search.
    fn search_entries(&self, query: String, limit: u64) -> FileFuture<ProjectSearchEntriesResult>;

    /// `projects.searchContents`: matching lines, in path order.
    fn search_contents(&self, query: ContentQuery) -> FileFuture<ProjectSearchContentsResult>;

    /// `assets.createUrl` for a `workspace-file` resource, then `GET` of the signed URL.
    fn read_asset(&self, relative_path: String) -> FileFuture<AssetBytes>;

    /// A local path the OS can open: the file itself when it is on this machine,
    /// else the asset bytes written to a temporary file. `assets.createUrl` + `GET`.
    fn local_copy(&self, relative_path: String) -> FileFuture<PathBuf>;
}

/// Shared handle the views hold.
pub type SharedWorkspaceFiles = Arc<dyn WorkspaceFiles>;

/// Joins a workspace root and a relative path for display.
pub fn absolute_path(root: &str, relative: &str) -> String {
    if relative.is_empty() { root.to_owned() } else { format!("{}/{relative}", root.trim_end_matches('/')) }
}

/// `scoreSubsequenceMatch` from `packages/shared/src/searchRanking.ts`, on
/// lowercased ASCII-compatible input. Lower is better; `None` is no match.
pub fn score_subsequence_match(value: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return Some(0);
    }
    let value = value.as_bytes();
    let query = query.as_bytes();
    let (mut qi, mut first, mut previous, mut gaps) = (0usize, usize::MAX, usize::MAX, 0usize);
    for (vi, &byte) in value.iter().enumerate() {
        if byte != query[qi] {
            continue;
        }
        if first == usize::MAX {
            first = vi;
        }
        if previous != usize::MAX {
            gaps += vi - previous - 1;
        }
        previous = vi;
        qi += 1;
        if qi == query.len() {
            let span = vi - first + 1 - query.len();
            let length = (value.len() - query.len()).min(64);
            return Some(first * 2 + gaps * 3 + span + length);
        }
    }
    None
}

/// `normalizeSearchQuery(raw, { trimLeadingPattern: /^[@./]+/ })` with
/// whitespace removed, as the file picker sends it.
pub fn normalize_path_query(raw: &str) -> String {
    raw.trim().trim_start_matches(['@', '.', '/']).to_lowercase().split_whitespace().collect()
}

const IGNORED_DIRECTORIES: &[&str] = &[".git", "node_modules", "target", "dist", ".turbo", ".next"];

/// A workspace backed by a directory on this machine. Directory listings and
/// file reads hit the disk on every call; the path index for searches is built
/// once and dropped on [`DirectoryFiles::refresh_index`].
pub struct DirectoryFiles {
    root: PathBuf,
    root_display: String,
    /// Shared with the handles searches run on, so the walk happens once.
    index: Arc<Mutex<Option<Arc<PathIndex>>>>,
}

/// Every file and directory under the root, with lowercased copies for matching.
struct PathIndex {
    entries: Vec<ProjectEntry>,
    lower: Vec<String>,
}

impl DirectoryFiles {
    pub fn new(root: impl Into<PathBuf>) -> Arc<Self> {
        let root = root.into();
        let root_display = root.to_string_lossy().into_owned();
        Arc::new(Self { root, root_display, index: Arc::default() })
    }

    /// Forgets the search index, as a workspace mutation does on the server.
    pub fn refresh_index(&self) {
        *self.index.lock().expect("index lock") = None;
    }

    fn resolve(&self, relative: &str) -> FileResult<PathBuf> {
        let relative = Path::new(relative);
        if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(FileError::failure(
                ProjectFileFailure::WorkspacePathOutsideRoot,
                "The requested path is outside the workspace.",
            ));
        }
        Ok(self.root.join(relative))
    }

    fn index(&self) -> Arc<PathIndex> {
        let mut slot = self.index.lock().expect("index lock");
        if let Some(index) = slot.as_ref() {
            return index.clone();
        }
        let mut entries = Vec::new();
        walk(&self.root, "", &mut entries);
        let lower = entries.iter().map(|e| e.path.to_lowercase()).collect();
        let index = Arc::new(PathIndex { entries, lower });
        *slot = Some(index.clone());
        index
    }

    /// `projects.searchEntries` over the index: subsequence-ranked like the
    /// server's symlink merge (`WorkspaceSearchIndex.ts:494`), files only when
    /// the picker asks. The real server ranks with its fff index; this stands in.
    pub fn search_entries_now(&self, query: &str, limit: usize) -> ProjectSearchEntriesResult {
        let index = self.index();
        let query = normalize_path_query(query);
        let mut ranked: Vec<(usize, usize)> = Vec::new();
        let mut matched = 0usize;
        for (i, lower) in index.lower.iter().enumerate() {
            if index.entries[i].kind != ProjectEntryKind::File {
                continue;
            }
            let Some(score) = score_subsequence_match(lower, &query) else { continue };
            matched += 1;
            // Keep the best `limit`, ties by path as `localeCompare` would for ASCII.
            let position = ranked.partition_point(|&(s, j)| (s, &index.lower[j]) <= (score, lower));
            if position < limit {
                ranked.insert(position, (score, i));
                ranked.truncate(limit);
            }
        }
        ProjectSearchEntriesResult {
            entries: ranked.into_iter().map(|(_, i)| index.entries[i].clone()).collect(),
            truncated: matched > limit,
        }
    }

    /// `projects.searchContents` over the index: git-grep semantics (case,
    /// whole word, regex with literal fallback), ranges in UTF-16 units.
    pub fn search_contents_now(&self, query: &ContentQuery) -> ProjectSearchContentsResult {
        let index = self.index();
        let mut regex_fallback_error = None;
        let pattern = if query.use_regex {
            query.query.clone()
        } else {
            regex::escape(&query.query)
        };
        let pattern = if query.whole_word { format!(r"\b(?:{pattern})\b") } else { pattern };
        let build = |pattern: &str| RegexBuilder::new(pattern).case_insensitive(!query.case_sensitive).build();
        let regex = match build(&pattern) {
            Ok(regex) => regex,
            Err(error) => {
                regex_fallback_error = Some(error.to_string().lines().last().unwrap_or("Invalid regex").to_owned());
                let literal = regex::escape(&query.query);
                build(&if query.whole_word { format!(r"\b{literal}\b") } else { literal }).expect("escaped pattern")
            }
        };
        let mut matches = Vec::new();
        let mut truncated = false;
        'files: for entry in index.entries.iter().filter(|e| e.kind == ProjectEntryKind::File) {
            let Ok(bytes) = std::fs::read(self.root.join(&entry.path)) else { continue };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else { continue };
            for (line_index, line) in text.lines().enumerate() {
                let ranges: Vec<_> = regex
                    .find_iter(line)
                    .filter(|m| !m.is_empty())
                    .map(|m| ProjectContentMatchRange {
                        start: utf16_len(&line[..m.start()]),
                        end: utf16_len(&line[..m.end()]),
                    })
                    .collect();
                if ranges.is_empty() {
                    continue;
                }
                if matches.len() as u64 >= query.limit {
                    truncated = true;
                    break 'files;
                }
                matches.push(ProjectContentMatch {
                    path: entry.path.clone(),
                    line_number: (line_index + 1) as u64,
                    line_content: line.to_owned(),
                    match_ranges: ranges,
                });
            }
        }
        ProjectSearchContentsResult { matches, truncated, regex_fallback_error }
    }
}

fn utf16_len(text: &str) -> u64 {
    text.encode_utf16().count() as u64
}

fn walk(root: &Path, prefix: &str, out: &mut Vec<ProjectEntry>) {
    let Ok(read) = std::fs::read_dir(root.join(prefix)) else { return };
    let mut children: Vec<_> = read.flatten().collect();
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let Ok(kind) = child.file_type() else { continue };
        if kind.is_dir() {
            if IGNORED_DIRECTORIES.contains(&name.as_str()) {
                continue;
            }
            out.push(ProjectEntry { path: path.clone(), kind: ProjectEntryKind::Directory, ignored: None });
            walk(root, &path, out);
        } else {
            out.push(ProjectEntry { path, kind: ProjectEntryKind::File, ignored: None });
        }
    }
}

/// A directory's children: folders first, then files, each by name, as the
/// tree sorts them. Ignored folders are listed and flagged, not walked.
fn list_directory(root: &Path, directory: Option<&str>) -> FileResult<ProjectListEntriesResult> {
    let prefix = directory.unwrap_or("").trim_matches('/');
    let read = std::fs::read_dir(root.join(prefix)).map_err(|error| FileError::new(error.to_string()))?;
    let mut entries: Vec<ProjectEntry> = read
        .flatten()
        .filter_map(|child| {
            let name = child.file_name().to_string_lossy().into_owned();
            let kind = if child.file_type().ok()?.is_dir() { ProjectEntryKind::Directory } else { ProjectEntryKind::File };
            let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let ignored = (kind == ProjectEntryKind::Directory && IGNORED_DIRECTORIES.contains(&name.as_str())).then_some(true);
            Some(ProjectEntry { path, kind, ignored })
        })
        .collect();
    entries.sort_by(|a, b| {
        (a.kind != ProjectEntryKind::Directory, a.path.to_lowercase()).cmp(&(b.kind != ProjectEntryKind::Directory, b.path.to_lowercase()))
    });
    Ok(ProjectListEntriesResult { entries, truncated: false })
}

impl WorkspaceFiles for DirectoryFiles {
    fn root(&self) -> &str {
        &self.root_display
    }

    fn list_entries(&self, directory: Option<String>) -> FileFuture<ProjectListEntriesResult> {
        let root = self.root.clone();
        Box::pin(async move { list_directory(&root, directory.as_deref()) })
    }

    fn read_file(&self, relative_path: String) -> FileFuture<ProjectReadFileResult> {
        let path = self.resolve(&relative_path);
        Box::pin(async move {
            let path = path?;
            let metadata = std::fs::metadata(&path).map_err(|_| {
                FileError::failure(ProjectFileFailure::OperationFailed, "The file could not be accessed or read.")
            })?;
            if !metadata.is_file() {
                return Err(FileError::failure(ProjectFileFailure::PathNotFile, "The path is not a regular file."));
            }
            let bytes = read_prefix(&path, FILE_TEXT_PREVIEW_MAX_BYTES)?;
            let truncated = metadata.len() as usize > FILE_TEXT_PREVIEW_MAX_BYTES;
            // The server reads at most the limit, so a split character at the end is dropped.
            let decoded = if truncated {
                let mut padded = bytes.clone();
                padded.push(b' ');
                decode_file_preview_text(&padded, bytes.len()).map(|(text, _)| text)
            } else {
                decode_file_preview_text(&bytes, usize::MAX).map(|(text, _)| text)
            };
            let contents = decoded.map_err(|_| {
                FileError::failure(ProjectFileFailure::BinaryFile, "The file is binary and cannot be displayed as text.")
            })?;
            Ok(ProjectReadFileResult {
                relative_path,
                contents,
                byte_length: metadata.len(),
                truncated,
                exists: Some(true),
            })
        })
    }

    fn write_file(&self, relative_path: String, contents: String) -> FileFuture<ProjectWriteFileResult> {
        let path = self.resolve(&relative_path);
        Box::pin(async move {
            let path = path?;
            std::fs::write(&path, contents.as_bytes())
                .map_err(|error| FileError::failure(ProjectFileFailure::OperationFailed, error.to_string()))?;
            Ok(ProjectWriteFileResult { relative_path })
        })
    }

    fn search_entries(&self, query: String, limit: u64) -> FileFuture<ProjectSearchEntriesResult> {
        let this = self.clone_handle();
        Box::pin(async move { Ok(this.search_entries_now(&query, limit as usize)) })
    }

    fn search_contents(&self, query: ContentQuery) -> FileFuture<ProjectSearchContentsResult> {
        let this = self.clone_handle();
        Box::pin(async move { Ok(this.search_contents_now(&query)) })
    }

    fn read_asset(&self, relative_path: String) -> FileFuture<AssetBytes> {
        let path = self.resolve(&relative_path);
        Box::pin(async move {
            let path = path?;
            let bytes = std::fs::read(&path).map_err(|error| FileError::new(error.to_string()))?;
            let revision = std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos().to_string());
            Ok(AssetBytes { bytes: bytes.into(), revision })
        })
    }

    fn local_copy(&self, relative_path: String) -> FileFuture<PathBuf> {
        let path = self.resolve(&relative_path);
        Box::pin(async move { path })
    }
}

impl DirectoryFiles {
    /// A handle for futures: the fields that searches need, behind an `Arc`.
    fn clone_handle(&self) -> Arc<Self> {
        Arc::new(Self { root: self.root.clone(), root_display: self.root_display.clone(), index: self.index.clone() })
    }
}

fn read_prefix(path: &Path, limit: usize) -> FileResult<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path)
        .map_err(|error| FileError::failure(ProjectFileFailure::OperationFailed, error.to_string()))?;
    let mut bytes = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| FileError::failure(ProjectFileFailure::OperationFailed, error.to_string()))?;
    Ok(bytes)
}
