//! Pull request and comment bodies before rendering: lifting attachments,
//! `<video>` embeds and media tables out of the markdown, carrying reference
//! definitions into every piece, resolving root-relative uploads, and GitHub's
//! `:alias:` emoji. Ports `pullRequestMarkdown.ts`.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use url::Url;

/// One piece of a body. `id` is positional: the same attachment can be embedded twice.
#[derive(Debug, Clone, PartialEq)]
pub enum PullRequestBodySegment {
    Markdown {
        id: String,
        text: String,
        /// `source_offset + i` is where `text[i]` sits in the body (UTF-16 units), so an edit can
        /// be written back. Holds past the reference definitions `text` may lead with.
        source_offset: i64,
    },
    Attachment { id: String, url: String, media: AttachmentMedia },
    Table { id: String, rows: Vec<Vec<TableCell>> },
}

/// What an attachment link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentMedia {
    Video,
    Image,
    Unknown,
}

impl AttachmentMedia {
    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Image => "image",
            Self::Unknown => "unknown",
        }
    }
}

/// One cell of a media table, split into segments of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub header: bool,
    pub segments: Vec<PullRequestBodySegment>,
}

macro_rules! re {
    ($name:ident, $pattern:literal) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
    };
}

// `.` stops at the `\r` of a CRLF body, which is how GitHub stores one written in its web editor.
re!(FENCE, r"^\s{0,3}(`{3,}|~{3,})([^\r\n]*)\r?$");
re!(BACKTICK_RUN, r"`+");
const VIDEO_TAG_MAX_LINES: usize = 8;
// A tab after up to three spaces still reaches the fourth column.
re!(INDENTED_CODE, r"^(?: {4}| {0,3}\t)");
re!(BARE_URL, r"^<?(https?://\S+?)>?$");
re!(IMAGE_EXTENSION, r"(?i)\.(?:png|jpe?g|gif|webp|avif|heic|heif|svg)(?:$|[?#])");
re!(MARKDOWN_LINK, r"^\[[^\]]*\]\((https?://[^\s)]+)\)$");
re!(VIDEO_EXTENSION, r"(?i)\.(?:mp4|webm|mov|m4v|ogv)(?:$|[?#])");
re!(VIDEO_TAG_SRC, r#"(?i)<(?:video|source)\b[^>]*\bsrc\s*=\s*["']([^"']+)["']"#);
re!(STANDALONE_VIDEO_TAG, r"(?i)^\s*<video\b");
re!(VIDEO_TAG_END, r"(?i)</video>\s*$");
const TABLE_MAX_LINES: usize = 64;
re!(TABLE_START, r"(?i)^\s*<table\b");
re!(TABLE_END, r"(?i)</table>\s*$");
re!(TABLE_OPEN, r"(?i)<table\b");
re!(TABLE_CLOSE, r"(?i)</table\s*>");
re!(TABLE_STRUCTURE_TAG, r#"(?i)<(/?)(table|thead|tbody|tfoot|tr|td|th)\b((?:[^<>"']|"[^"]*"|'[^']*')*)>"#);
re!(CELL_SPAN, r"(?i)\b(?:col|row)span\b");
re!(GITHUB_ATTACHMENT_PATH, r"^/user-attachments/assets/[A-Za-z0-9_-]+$");

/// UTF-16 length, the unit JavaScript string offsets count in.
fn u16_len(s: &str) -> i64 {
    s.encode_utf16().count() as i64
}

fn is_web_url(url: &str) -> bool {
    Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// GitHub serves an uploaded video at a bare `user-attachments` path with no extension.
fn is_host_video(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else { return false };
    parsed.scheme() == "https"
        && matches!(parsed.host_str().map(str::to_lowercase).as_deref(), Some("github.com" | "www.github.com"))
        && GITHUB_ATTACHMENT_PATH.is_match(parsed.path())
}

fn attachment_from_line(line: &str) -> Option<(String, AttachmentMedia)> {
    let trimmed = crate::pull_request_query::js_trim(line);
    let url = BARE_URL
        .captures(trimmed)
        .or_else(|| MARKDOWN_LINK.captures(trimmed))
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())?;
    if !is_web_url(&url) {
        return None;
    }
    if VIDEO_EXTENSION.is_match(&url) || is_host_video(&url) {
        return Some((url, AttachmentMedia::Video));
    }
    IMAGE_EXTENSION.is_match(&url).then_some((url, AttachmentMedia::Image))
}

fn trim_leading_newlines(s: &str) -> &str {
    let mut rest = s;
    loop {
        if let Some(r) = rest.strip_prefix("\r\n") {
            rest = r;
        } else if let Some(r) = rest.strip_prefix('\n') {
            rest = r;
        } else {
            return rest;
        }
    }
}

/// Splits a body into markdown, attachments and media tables.
pub fn split_pull_request_body(body: &str) -> Vec<PullRequestBodySegment> {
    let segments = split_body_at(body, 0);
    // One markdown segment is the whole body, which resolves its own references.
    let whole = segments.len() == 1 && matches!(segments[0], PullRequestBodySegment::Markdown { .. });
    if whole || !body.contains("]:") {
        return segments;
    }
    let definitions = reference_definitions(body);
    if definitions.is_empty() {
        return segments;
    }
    let leading = format!("{}\n\n", definitions.join("\n"));
    segments.into_iter().map(|s| with_definitions(s, &leading)).collect()
}

fn with_definitions(segment: PullRequestBodySegment, definitions: &str) -> PullRequestBodySegment {
    match segment {
        PullRequestBodySegment::Attachment { .. } => segment,
        PullRequestBodySegment::Table { id, rows } => PullRequestBodySegment::Table {
            id,
            rows: rows
                .into_iter()
                .map(|cells| {
                    cells
                        .into_iter()
                        .map(|cell| TableCell {
                            header: cell.header,
                            segments: cell.segments.into_iter().map(|s| with_definitions(s, definitions)).collect(),
                        })
                        .collect()
                })
                .collect(),
        },
        PullRequestBodySegment::Markdown { id, text, source_offset } => {
            if !text.contains('[') {
                return PullRequestBodySegment::Markdown { id, text, source_offset };
            }
            PullRequestBodySegment::Markdown { id, source_offset: source_offset - u16_len(definitions), text: format!("{definitions}{text}") }
        }
    }
}

/// `baseOffset` is where `body` starts in the whole body, so a table cell's edits write back.
fn split_body_at(body: &str, base_offset: i64) -> Vec<PullRequestBodySegment> {
    let mut segments: Vec<PullRequestBodySegment> = Vec::new();
    let mut markdown: Vec<&str> = Vec::new();
    let mut markdown_start = 0i64;
    let mut open_fence: Option<String> = None;

    // Blank lines around a run are dropped, but never leading spaces: four of them open an
    // indented code block, so trimming a run would silently turn code into prose.
    let flush = |markdown: &mut Vec<&str>, segments: &mut Vec<PullRequestBodySegment>, markdown_start: i64| {
        let joined = markdown.join("\n");
        let text = trim_leading_newlines(&joined);
        let source_offset = markdown_start + u16_len(&joined) - u16_len(text);
        markdown.clear();
        if !crate::pull_request_query::js_trim(text).is_empty() {
            segments.push(PullRequestBodySegment::Markdown {
                id: format!("markdown:{}", segments.len()),
                text: text.trim_end().to_owned(),
                source_offset,
            });
        }
    };

    let lines: Vec<&str> = body.split('\n').collect();
    let mut table_scanned_through: i64 = -1;
    let mut line_start = base_offset;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        if markdown.is_empty() {
            markdown_start = line_start;
        }
        let advance = |index: &mut usize, line_start: &mut i64| {
            *line_start += u16_len(lines[*index]) + 1;
            *index += 1;
        };
        let fence = FENCE.captures(line);
        // ```` ```npm test``` ```` is a code span, not an opener that would flip every later fence.
        let is_inline_code = open_fence.is_none()
            && fence.as_ref().is_some_and(|f| {
                let marker = f.get(1).unwrap().as_str();
                marker.starts_with('`')
                    && BACKTICK_RUN.find_iter(f.get(2).unwrap().as_str()).any(|run| run.as_str().len() == marker.len())
            });
        if let Some(f) = fence.as_ref().filter(|_| !is_inline_code) {
            let marker = f.get(1).unwrap().as_str();
            // A fence closes only on the same marker, at least as long as the one that opened it,
            // and with nothing after it.
            let closes = open_fence.as_ref().is_some_and(|open| {
                marker.as_bytes()[0] == open.as_bytes()[0]
                    && marker.len() >= open.len()
                    && crate::pull_request_query::js_trim(f.get(2).unwrap().as_str()).is_empty()
            });
            if open_fence.is_none() {
                open_fence = Some(marker.to_owned());
            } else if closes {
                open_fence = None;
            }
            markdown.push(line);
            advance(&mut index, &mut line_start);
            continue;
        }
        if open_fence.is_some() || INDENTED_CODE.is_match(line) {
            markdown.push(line);
            advance(&mut index, &mut line_start);
            continue;
        }

        if index as i64 > table_scanned_through && TABLE_START.is_match(line) {
            let (end, rows) = media_table_at(&lines, index, line_start);
            table_scanned_through = end as i64;
            if let Some(rows) = rows {
                flush(&mut markdown, &mut segments, markdown_start);
                segments.push(PullRequestBodySegment::Table { id: format!("table:{}", segments.len()), rows });
                while index < end {
                    line_start += u16_len(lines[index]) + 1;
                    index += 1;
                }
                advance(&mut index, &mut line_start);
                continue;
            }
        }

        if let Some((url, media)) = attachment_from_line(line) {
            flush(&mut markdown, &mut segments, markdown_start);
            segments.push(PullRequestBodySegment::Attachment { id: format!("attachment:{}", segments.len()), url, media });
            advance(&mut index, &mut line_start);
            continue;
        }

        if !STANDALONE_VIDEO_TAG.is_match(line) {
            markdown.push(line);
            advance(&mut index, &mut line_start);
            continue;
        }
        let last_candidate = (index + VIDEO_TAG_MAX_LINES).min(lines.len()) - 1;
        let mut cursor = index;
        while cursor < last_candidate && !VIDEO_TAG_END.is_match(lines[cursor]) {
            cursor += 1;
        }
        let source = VIDEO_TAG_END
            .is_match(lines[cursor])
            .then(|| VIDEO_TAG_SRC.captures(&lines[index..=cursor].join("\n")).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned()))
            .flatten();
        match source.filter(|s| is_web_url(s)) {
            Some(url) => {
                flush(&mut markdown, &mut segments, markdown_start);
                segments.push(PullRequestBodySegment::Attachment {
                    id: format!("attachment:{}", segments.len()),
                    url,
                    media: AttachmentMedia::Video,
                });
                while index < cursor {
                    line_start += u16_len(lines[index]) + 1;
                    index += 1;
                }
                advance(&mut index, &mut line_start);
            }
            None => {
                // Unclosed, or nothing linkable in it: prose. Only this line is consumed.
                markdown.push(line);
                advance(&mut index, &mut line_start);
            }
        }
    }
    flush(&mut markdown, &mut segments, markdown_start);
    segments
}

/// The table opening at `start`, through its matching close, with its rows when it holds media.
fn media_table_at(lines: &[&str], start: usize, start_offset: i64) -> (usize, Option<Vec<Vec<TableCell>>>) {
    let last_candidate = (start + TABLE_MAX_LINES).min(lines.len()) - 1;
    let mut depth: i64 = 0;
    let mut end = start;
    loop {
        let line = lines[end];
        depth += TABLE_OPEN.find_iter(line).count() as i64 - TABLE_CLOSE.find_iter(line).count() as i64;
        if depth <= 0 || end == last_candidate {
            break;
        }
        end += 1;
    }
    if depth != 0 || !TABLE_END.is_match(lines[end]) {
        return (end, None);
    }
    // Lifting attachment links out of a table's cells would tear the table apart.
    let rows = table_rows(&lines[start..=end].join("\n"), start_offset);
    let has_media = rows.as_ref().is_some_and(|rows| {
        rows.iter().any(|cells| cells.iter().any(|c| c.segments.iter().any(|s| matches!(s, PullRequestBodySegment::Attachment { .. }))))
    });
    (end, if has_media { rows } else { None })
}

/// Cells of a table made only of rows and cells, or `None` when anything else is in it.
fn table_rows(html: &str, html_offset: i64) -> Option<Vec<Vec<TableCell>>> {
    let mut rows: Vec<Vec<TableCell>> = Vec::new();
    let mut row: Option<Vec<TableCell>> = None;
    let mut cell: Option<(String, usize)> = None;
    let (mut opened, mut closed) = (false, false);
    let mut cursor = 0;
    for m in TABLE_STRUCTURE_TAG.captures_iter(html) {
        let whole = m.get(0).unwrap();
        let closing = &m[1] == "/";
        let tag = m[2].to_lowercase();
        if let Some((cell_tag, content_start)) = cell.take() {
            if !closing || tag != cell_tag {
                return None;
            }
            let content = &html[content_start..whole.start()];
            // Indented HTML would render as a code block once the cell is markdown on its own.
            if content.split('\n').any(|l| INDENTED_CODE.is_match(l) && !crate::pull_request_query::js_trim(l).is_empty()) {
                return None;
            }
            let offset = html_offset + u16_len(&html[..content_start]);
            row.as_mut()?.push(TableCell { header: tag == "th", segments: split_body_at(content, offset) });
        } else {
            if !crate::pull_request_query::js_trim(&html[cursor..whole.start()]).is_empty() {
                return None;
            }
            let table_open = opened && !closed;
            match tag.as_str() {
                "table" => {
                    if if closing { !table_open || row.is_some() } else { opened } {
                        return None;
                    }
                    opened = true;
                    closed = closing;
                }
                "tr" => {
                    if !table_open || closing == row.is_none() {
                        return None;
                    }
                    if closing {
                        rows.push(row.take().unwrap());
                    } else {
                        row = Some(Vec::new());
                    }
                }
                "td" | "th" => {
                    if closing || row.is_none() || CELL_SPAN.is_match(&m[3]) {
                        return None;
                    }
                    cell = Some((tag, whole.end()));
                }
                _ => {
                    if !table_open || row.is_some() {
                        return None;
                    }
                }
            }
        }
        cursor = whole.end();
    }
    if !closed || !crate::pull_request_query::js_trim(&html[cursor..]).is_empty() {
        return None;
    }
    Some(rows.into_iter().filter(|c| !c.is_empty()).collect())
}

/// The first definition of each label in the body, as authored, each joined onto one line so it
/// stands outside the quote or list it was written in.
fn reference_definitions(body: &str) -> Vec<String> {
    let Ok(tree) = markdown::to_mdast(body, &markdown::ParseOptions::gfm()) else { return vec![] };
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    fn visit(node: &markdown::mdast::Node, quotes: usize, body: &str, seen: &mut Vec<String>, out: &mut Vec<String>) {
        if let markdown::mdast::Node::Definition(def) = node
            && !seen.contains(&def.identifier)
            && let Some(pos) = &def.position
        {
            seen.push(def.identifier.clone());
            let wrap = Regex::new(&format!(r"(?:\r\n|\r|\n)(?:[ \t]*>){{0,{quotes}}}[ \t]*")).unwrap();
            out.push(wrap.replace_all(&body[pos.start.offset..pos.end.offset], " ").into_owned());
        }
        let nested = quotes + usize::from(matches!(node, markdown::mdast::Node::Blockquote(_)));
        if let Some(children) = node.children() {
            for child in children {
                visit(child, nested, body, seen, out);
            }
        }
    }
    visit(&tree, 0, body, &mut seen, &mut out);
    out
}

/// Forges link uploads root-relative; in a change request body that means the forge's origin.
pub fn resolve_change_request_media_url(url: &str, change_request_url: &str) -> String {
    let root_relative = url.starts_with('/') && !url[1..].starts_with(['/', '\\']);
    if !root_relative {
        return url.to_owned();
    }
    Url::parse(change_request_url).and_then(|base| base.join(url)).map_or_else(|_| url.to_owned(), |u| u.to_string())
}

static EMOJI_BY_ALIAS: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    include_str!("github_emoji_aliases.txt").lines().filter_map(|line| line.split_once(' ')).collect()
});
re!(EMOJI_ALIAS, r":([a-z0-9_+-]+):");

/// GitHub's `:alias:` emoji in `text`, as `(byte index, matched text, emoji)`; a colon closes one
/// alias only, so `:tada:tada:` has one.
pub fn github_emoji_aliases(text: &str) -> Vec<(usize, &str, &'static str)> {
    let mut found = Vec::new();
    let mut end = 0;
    let mut at = 0;
    // The pattern ends on a lookahead in JavaScript, so a match may start on the colon that
    // closed the previous candidate.
    while let Some(m) = EMOJI_ALIAS.find_at(text, at) {
        let alias = &text[m.start() + 1..m.end() - 1];
        at = m.end() - 1;
        match EMOJI_BY_ALIAS.get(alias) {
            Some(emoji) if m.start() >= end => {
                end = m.end();
                found.push((m.start(), &text[m.start()..m.end()], *emoji));
            }
            _ => {}
        }
    }
    found
}

/// `text` with every `:alias:` replaced by its emoji.
pub fn expand_github_emoji_aliases(text: &str) -> String {
    let aliases = github_emoji_aliases(text);
    if aliases.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut start = 0;
    for (index, matched, emoji) in aliases {
        out.push_str(&text[start..index]);
        out.push_str(emoji);
        start = index + matched.len();
    }
    out.push_str(&text[start..]);
    out
}
