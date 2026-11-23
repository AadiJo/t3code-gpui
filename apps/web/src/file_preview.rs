//! Which viewer a file opens in, and how its bytes become text. Ports
//! `packages/shared/src/filePreview.ts`, `mediaPreview.ts`, `video.ts` and
//! `delimitedPreview.ts`'s delimiter rule, plus the text decode of
//! `packages/client-runtime/src/filePreview.ts`.
//!
//! The native client renders text, markdown, images and CSV/TSV itself. Every
//! other kind opens in the OS default app (spec 07, "Hard in native").

use crate::i18n::t;
use gpui_kit::SharedString;

/// `FILE_TEXT_PREVIEW_MAX_BYTES`: text previews read at most this many bytes,
/// as `projects.readFile` does on the server.
pub const FILE_TEXT_PREVIEW_MAX_BYTES: usize = 1024 * 1024;

/// Large text previews (read through the asset URL) stop here, as the web's
/// `LargeTextViewer` does ("Text previews support files up to 256 MB.").
pub const LARGE_TEXT_PREVIEW_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// `FilePreviewKind`, with rich documents on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilePreviewKind {
    Image,
    Video,
    Audio,
    Pdf,
    Html,
    Markdown,
    Mermaid,
    Font,
    Archive,
    Sqlite,
    Docx,
    Xlsx,
    Pptx,
    Odt,
    Ods,
    Odp,
    Rtf,
    Epub,
    Ipynb,
    Text,
    Unsupported,
}

impl FilePreviewKind {
    /// True for the kinds this client draws itself: text, markdown, images.
    /// Everything else gets "Open in default app".
    pub fn is_native(self) -> bool {
        matches!(self, Self::Text | Self::Markdown | Self::Image)
    }
}

/// `GENERIC_MIME_TYPES`: labels that say nothing about the contents.
const GENERIC_MIME_TYPES: &[&str] = &[
    "application/octet-stream",
    "binary/octet-stream",
    "application/unknown",
    "video/mp2t",
    "video/vnd.dlna.mpeg-tts",
];

fn is_generic(mime: &str) -> bool {
    mime.is_empty() || GENERIC_MIME_TYPES.contains(&mime) || mime == "text/plain"
}

fn normalize_mime(mime: Option<&str>) -> String {
    mime.and_then(|m| m.split(';').next()).unwrap_or("").trim().to_ascii_lowercase()
}

/// The lowercased extension with its dot, as `name.slice(name.lastIndexOf("."))`
/// (the whole name when there is no dot).
fn extension_of(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.rfind('.') {
        Some(index) => lower[index..].to_owned(),
        None => lower,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// `IMAGE_MIME_TYPE_BY_EXTENSION`.
fn image_mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        ".avif" => "image/avif",
        ".bmp" => "image/bmp",
        ".heic" => "image/heic",
        ".heif" => "image/heif",
        ".gif" => "image/gif",
        ".ico" => "image/x-icon",
        ".jpeg" | ".jpg" => "image/jpeg",
        ".png" => "image/png",
        ".svg" => "image/svg+xml",
        ".tif" | ".tiff" => "image/tiff",
        ".webp" => "image/webp",
        _ => return None,
    })
}

/// `VIDEO_MIME_TYPE_BY_EXTENSION`, keyed without the dot.
fn video_mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "avi" => "video/x-msvideo",
        "m4v" | "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "ogv" => "video/ogg",
        "webm" => "video/webm",
        _ => return None,
    })
}

/// The video extensions, for the file icon table.
pub const VIDEO_FILE_EXTENSIONS: &[&str] = &["avi", "m4v", "mkv", "mov", "mp4", "ogv", "webm"];

/// `AUDIO_MIME_TYPE_BY_EXTENSION`.
fn audio_mime(extension: &str) -> Option<&'static str> {
    Some(match extension {
        ".mp3" => "audio/mpeg",
        ".wav" => "audio/wav",
        ".ogg" | ".oga" | ".opus" => "audio/ogg",
        ".flac" => "audio/flac",
        ".aac" => "audio/aac",
        ".m4a" => "audio/mp4",
        ".aiff" => "audio/aiff",
        ".webm-audio" | ".weba" => "audio/webm",
        _ => return None,
    })
}

/// `mediaMimeTypeFromExtension`: an image or video MIME type for a literal extension.
pub fn media_mime_from_extension(extension: &str) -> Option<&'static str> {
    let valid = extension.len() > 1
        && extension.starts_with('.')
        && extension[1..].chars().all(|c| c.is_ascii_alphanumeric());
    if !valid {
        return None;
    }
    let lower = extension.to_ascii_lowercase();
    image_mime(&lower).or_else(|| video_mime(&lower[1..]))
}

/// `OFFICE_MIME_TYPES`.
const OFFICE_KINDS: &[(&str, FilePreviewKind, &str)] = &[
    ("docx", FilePreviewKind::Docx, "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("xlsx", FilePreviewKind::Xlsx, "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ("pptx", FilePreviewKind::Pptx, "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ("odt", FilePreviewKind::Odt, "application/vnd.oasis.opendocument.text"),
    ("ods", FilePreviewKind::Ods, "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", FilePreviewKind::Odp, "application/vnd.oasis.opendocument.presentation"),
    ("rtf", FilePreviewKind::Rtf, "application/rtf"),
    ("epub", FilePreviewKind::Epub, "application/epub+zip"),
    ("ipynb", FilePreviewKind::Ipynb, "application/x-ipynb+json"),
];

/// `officePreviewKind`.
pub fn office_preview_kind(name: &str, mime: Option<&str>) -> Option<FilePreviewKind> {
    let mime = normalize_mime(mime);
    if mime == "text/rtf" {
        return Some(FilePreviewKind::Rtf);
    }
    let lower = name.to_ascii_lowercase();
    OFFICE_KINDS.iter().find_map(|(ext, kind, office_mime)| {
        let by_name = (is_generic(&mime) || (*kind == FilePreviewKind::Ipynb && mime == "application/json"))
            && lower.ends_with(&format!(".{ext}"));
        (mime == *office_mime || by_name).then_some(*kind)
    })
}

/// `binaryPreviewKind`: archives and SQLite databases.
pub fn binary_preview_kind(name: &str, mime: Option<&str>) -> Option<FilePreviewKind> {
    let mime = normalize_mime(mime);
    if ["application/vnd.sqlite3", "application/x-sqlite3", "application/x-sqlite"].contains(&mime.as_str()) {
        return Some(FilePreviewKind::Sqlite);
    }
    if [
        "application/zip",
        "application/x-zip-compressed",
        "application/java-archive",
        "application/x-java-archive",
        "application/x-tar",
        "application/gzip",
        "application/x-gzip",
        "application/x-compressed-tar",
    ]
    .contains(&mime.as_str())
    {
        return Some(FilePreviewKind::Archive);
    }
    if !is_generic(&mime) {
        return None;
    }
    match extension_of(name).as_str() {
        ".sqlite" | ".sqlite3" | ".db" => Some(FilePreviewKind::Sqlite),
        ".zip" | ".jar" | ".tar" | ".gz" | ".tgz" => Some(FilePreviewKind::Archive),
        _ => None,
    }
}

/// `isWorkspaceFontPreviewPath`.
pub fn is_font_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".ttf", ".otf", ".woff", ".woff2"].iter().any(|ext| lower.ends_with(ext))
}

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "log", "json", "jsonc", "jsonl", "ndjson", "yaml", "yml", "toml", "ini", "conf", "config", "env", "csv",
    "tsv", "xml", "css", "scss", "sass", "less", "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "py", "pyi",
    "rb", "go", "rs", "swift", "kt", "kts", "java", "c", "h", "cc", "cpp", "hpp", "cs", "php", "sh", "bash", "zsh",
    "fish", "sql", "graphql", "gql", "vue", "svelte", "r", "lua", "ex", "exs", "erl", "hs", "clj", "dart", "diff",
    "patch", "lock", "properties", "gradle",
];

const TEXT_FILE_NAMES: &[&str] = &[
    "dockerfile", "makefile", "gemfile", "rakefile", "license", "readme", ".gitignore", ".gitattributes",
    ".editorconfig", ".env",
];

fn is_text_extension(extension: &str) -> bool {
    extension.strip_prefix('.').is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext))
}

fn is_text_file_name(name: &str) -> bool {
    let lower = basename(name).to_ascii_lowercase();
    TEXT_FILE_NAMES.iter().any(|known| {
        lower.strip_prefix(known).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

/// `filePreviewKind(file, { richDocuments: true })`.
pub fn file_preview_kind(name: &str, mime: Option<&str>) -> FilePreviewKind {
    use FilePreviewKind as K;
    let mime = normalize_mime(mime);
    let lower = name.to_ascii_lowercase();
    let extension = extension_of(name);
    let generic = is_generic(&mime);
    if let Some(kind) = office_preview_kind(name, Some(&mime)) {
        return kind;
    }
    if let Some(kind) = binary_preview_kind(name, Some(&mime)) {
        return kind;
    }
    match mime.as_str() {
        "application/pdf" => return K::Pdf,
        "text/html" => return K::Html,
        "text/markdown" | "text/x-markdown" => return K::Markdown,
        "text/vnd.mermaid" => return K::Mermaid,
        _ => {}
    }
    if mime.starts_with("font/") {
        return K::Font;
    }
    if extension == ".otf" && mime == "application/vnd.oasis.opendocument.formula-template" {
        return K::Font;
    }
    if mime.starts_with("image/") {
        return K::Image;
    }
    if mime.starts_with("video/") && !generic {
        return K::Video;
    }
    if mime.starts_with("audio/") {
        return K::Audio;
    }
    if generic {
        match extension.as_str() {
            ".pdf" => return K::Pdf,
            ".htm" | ".html" => return K::Html,
            ".md" | ".markdown" | ".mdown" | ".mkd" | ".mdx" => return K::Markdown,
            ".mmd" | ".mermaid" => return K::Mermaid,
            ".rtf" => return K::Text,
            _ => {}
        }
        if is_font_path(&lower) {
            return K::Font;
        }
        match media_mime_from_extension(&extension) {
            Some(media) if media.starts_with("image/") => return K::Image,
            Some(media) if media.starts_with("video/") => return K::Video,
            _ => {}
        }
        if audio_mime(&extension).is_some() {
            return K::Audio;
        }
        if is_text_extension(&extension) || is_text_file_name(name) {
            return K::Text;
        }
    }
    let structured_text = matches!(
        mime.as_str(),
        "application/json"
            | "application/javascript"
            | "application/x-javascript"
            | "application/yaml"
            | "application/x-yaml"
            | "application/toml"
            | "application/sql"
            | "application/xml"
    ) || (mime.starts_with("application/") && (mime.ends_with("+json") || mime.ends_with("+xml")));
    if mime.starts_with("text/") || structured_text {
        return K::Text;
    }
    K::Unsupported
}

/// How a workspace file opens in the Files tab. The web asks the server for a
/// MIME type only for some kinds; with none, the name decides. A name with no
/// known kind is tried as text, and the read's binary check catches the rest.
pub fn workspace_preview_kind(path: &str) -> FilePreviewKind {
    match file_preview_kind(path, None) {
        FilePreviewKind::Unsupported => FilePreviewKind::Text,
        kind => kind,
    }
}

/// `isMarkdownPreviewFile`.
pub fn is_markdown_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".mdx", ".markdown", ".mdown", ".mkd"].iter().any(|ext| lower.ends_with(ext))
}

/// `filePreviewDelimiter`, by name: `,` for CSV and a tab for TSV.
pub fn file_preview_delimiter(path: &str) -> Option<u8> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".csv") {
        Some(b',')
    } else if lower.ends_with(".tsv") {
        Some(b'\t')
    } else {
        None
    }
}

/// Why bytes could not become preview text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextDecodeError {
    Binary,
    NotUtf8,
}

impl TextDecodeError {
    /// The web's message for the failure.
    pub fn message(&self) -> SharedString {
        match self {
            Self::Binary => t!("This file contains binary data and cannot be shown as text."),
            Self::NotUtf8 => t!("This file is not UTF-8 text. Open it in another app to view its contents."),
        }
    }
}

/// `decodeFilePreviewText`: rejects NUL bytes and invalid UTF-8 within the
/// first [`FILE_TEXT_PREVIEW_MAX_BYTES`]; a character the limit splits is dropped.
pub fn decode_file_preview_text(bytes: &[u8], limit: usize) -> Result<(String, bool), TextDecodeError> {
    let truncated = bytes.len() > limit;
    let bounded = &bytes[..bytes.len().min(limit)];
    if memchr_zero(bounded) {
        return Err(TextDecodeError::Binary);
    }
    match std::str::from_utf8(bounded) {
        Ok(text) => Ok((text.to_owned(), truncated)),
        // A streaming decoder keeps an incomplete trailing sequence for the next chunk.
        Err(error) if truncated && error.error_len().is_none() => {
            Ok((String::from_utf8_lossy(&bounded[..error.valid_up_to()]).into_owned(), true))
        }
        Err(_) => Err(TextDecodeError::NotUtf8),
    }
}

fn memchr_zero(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Human file size like `formatAttachmentSize`: bytes, KB, MB, GB with one decimal.
pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{value:.1} {}", UNITS[unit]) }
}
