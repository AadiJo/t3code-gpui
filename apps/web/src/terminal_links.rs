//! Finds clickable URLs and file paths in terminal text. Ports
//! `terminal-links.ts`; offsets here are UTF-8 byte offsets into the line.

use std::sync::LazyLock;

use fancy_regex::Regex;

/// Whether a link opens in a browser or an editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalLinkKind {
    Url,
    Path,
}

/// A link found in a line: its text and byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLinkMatch {
    pub kind: TerminalLinkKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("terminal link pattern compiles")
}

static URL: LazyLock<Regex> = LazyLock::new(|| regex(r#"(?i)https?://[^\s"'`<>]+"#));
// The default PowerShell prompt delimits its unquoted path with `PS ` and `>`.
static POWERSHELL_PROMPT_PATH: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^PS ((?:[A-Za-z]:[\\/]|\\\\|/)[^>\r\n]*)>"#));
// Bare paths start only at a segment boundary. A bare `name.ext` needs
// `:line:col` or a TS location after it, so prose and `host:port` stay text.
static FILE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    regex(concat!(
        r#"(?:~[\\/]|\.{1,2}[\\/]|/|[A-Za-z]:[\\/]|\\\\)[^\s"'`<>]+"#,
        r#"|(?<![\p{L}\p{M}\p{N}._$+@-])[\p{L}\p{M}\p{N}._$+@-]+"#,
        r#"(?:(?:[\\/](?:[\p{L}\p{M}\p{N}._$+@-]|\[{1,2}[\p{L}\p{M}\p{N}._-]+\]{1,2}|\([\p{L}\p{M}\p{N}._-]+\)(?=[\\/]))+)+"#,
        r#"|\.\p{L}[\p{L}\p{M}\p{N}_-]*(?=:\d+:\d|\(\d+,\d+\):\s+(?:error|warning) TS\d+:))"#,
        r#"(?:(?::\d+){1,2}|\(\d+,\d+\)(?=:\s+(?:error|warning) TS\d+:))?"#,
    ))
});
// fancy-regex refuses a numbered backreference next to named groups, so the quote is named.
static QUOTED_FILE_PATH: LazyLock<Regex> = LazyLock::new(|| {
    regex(concat!(
        r#"(?<quote>["'`])(?<path>(?:~[\\/]|\.{1,2}[\\/]|/|[A-Za-z]:[\\/]|\\\\|[\p{L}\p{M}\p{N}._ -]+[\\/])[^\r\n]*?"#,
        r#"|[\p{L}\p{M}\p{N}._ -]+\.[\p{L}\p{M}\p{N}_-]+(?::\d+){0,2})\k<quote>(?<position>(?::\d+){0,2})"#,
    ))
});
// A question mark is legal in filenames; only exclude query-bearing request targets.
static REQUEST_ROUTE: LazyLock<Regex> = LazyLock::new(|| {
    regex(r#"(?:^|[\s"'`])(?:GET|HEAD|POST|PUT|PATCH|DELETE|OPTIONS|CONNECT|TRACE|proxying)\s+["'`]?(?:/|[A-Za-z0-9_-]+/)[^\s?]*\?\S*"#)
});
// The tail of `redis://host/0` or `file:///a.mjs` is not a path.
static SCHEME_URL: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"(?<![A-Za-z\d+.-])[+-]?[A-Za-z][A-Za-z\d+.-]+://[^\s"'`<>]+"#));
// Nor is the tail of an SCP-style Git remote such as `git@github.com:acme/widgets.git`.
static SCP_REMOTE: LazyLock<Regex> = LazyLock::new(|| regex(r#"(?<![\w.@-])[\w.-]+@[\w.-]+:[^\s"'`<>]+"#));
static TS_LOCATION_AFTER: LazyLock<Regex> = LazyLock::new(|| regex(r#"^:\s+(?:error|warning) TS\d+:"#));
static TS_POSITION: LazyLock<Regex> = LazyLock::new(|| regex(r#"\((\d+),(\d+)\)$"#));

/// Whether text is an http(s) URL.
pub fn is_terminal_url(value: &str) -> bool {
    let lower = value.get(..8).unwrap_or(value).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn trim_closing_delimiters(value: &str, kind: TerminalLinkKind) -> &str {
    let punctuation: &[char] = match kind {
        TerminalLinkKind::Path => &['.', ',', ';', ':', '!', '?'],
        TerminalLinkKind::Url => &['.', ',', ';', '!', '?'],
    };
    let output = value.trim_end_matches(punctuation);
    let mut end = output.len();
    for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
        if !output[..end].ends_with(close) {
            continue;
        }
        let mut excess = output.matches(close).count() as isize - output.matches(open).count() as isize;
        while excess > 0 && output[..end].ends_with(close) {
            end -= 1;
            excess -= 1;
        }
    }
    &output[..end]
}

fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

fn collect_matches(line: &str, kind: TerminalLinkKind, pattern: &Regex, existing: &[TerminalLinkMatch]) -> Vec<TerminalLinkMatch> {
    let mut matches: Vec<TerminalLinkMatch> = Vec::new();
    for captures in pattern.captures_iter(line).flatten() {
        let whole = captures.get(0).expect("group 0 exists");
        let quoted = captures.name("path");
        let position = captures.name("position").map_or("", |m| m.as_str());
        let (raw, start) = match quoted {
            Some(path) => (format!("{}{position}", path.as_str()), path.start()),
            None => (whole.as_str().to_string(), whole.start()),
        };
        if raw.is_empty() {
            continue;
        }
        let trimmed = if quoted.is_some() { raw.as_str() } else { trim_closing_delimiters(&raw, kind) };
        if trimmed.is_empty() || (kind == TerminalLinkKind::Path && is_terminal_url(trimmed)) {
            continue;
        }
        let after = &line[(start + trimmed.len()).min(line.len())..];
        let text = if kind == TerminalLinkKind::Path && TS_LOCATION_AFTER.is_match(after).unwrap_or(false) {
            TS_POSITION.replace(trimmed, ":$1:$2").into_owned()
        } else {
            trimmed.to_string()
        };
        // A position outside quotes spans the closing quote in the terminal buffer.
        let end = start + trimmed.len() + usize::from(!position.is_empty());
        let span = (start, end);
        if existing.iter().chain(&matches).any(|other| overlaps(span, (other.start, other.end))) {
            continue;
        }
        matches.push(TerminalLinkMatch { kind, text, start, end });
    }
    matches
}

/// Every link in a line, sorted by start.
pub fn extract_terminal_links(line: &str) -> Vec<TerminalLinkMatch> {
    let mut found: Vec<TerminalLinkMatch> = Vec::new();
    if let Ok(Some(captures)) = POWERSHELL_PROMPT_PATH.captures(line) {
        if let Some(path) = captures.get(1) {
            found.push(TerminalLinkMatch {
                kind: TerminalLinkKind::Path,
                text: path.as_str().to_string(),
                start: path.start(),
                end: path.end(),
            });
        }
    }
    let ranges = |pattern: &Regex| -> Vec<(usize, usize)> {
        pattern.find_iter(line).flatten().map(|m| (m.start(), m.end())).collect()
    };
    let mut non_path = ranges(&REQUEST_ROUTE);
    non_path.extend(ranges(&SCHEME_URL));
    let remote_tails: Vec<(usize, usize)> = SCP_REMOTE
        .find_iter(line)
        .flatten()
        .map(|m| (m.start() + m.as_str().find(':').map_or(0, |i| i + 1), m.end()))
        .collect();

    let urls = collect_matches(line, TerminalLinkKind::Url, &URL, &found);
    found.extend(urls);
    let quoted = collect_matches(line, TerminalLinkKind::Path, &QUOTED_FILE_PATH, &found);
    found.extend(quoted);
    let paths = collect_matches(line, TerminalLinkKind::Path, &FILE_PATH, &found);
    found.extend(paths);

    found.retain(|link| {
        link.kind == TerminalLinkKind::Url
            || (!non_path.iter().any(|range| overlaps((link.start, link.end), *range))
                && !remote_tails.iter().any(|tail| link.start >= tail.0 && link.end <= tail.1))
    });
    found.sort_by_key(|link| link.start);
    found
}

/// The link covering a byte offset, if any.
pub fn terminal_link_at(line: &str, offset: usize) -> Option<TerminalLinkMatch> {
    extract_terminal_links(line).into_iter().find(|link| link.start <= offset && offset < link.end)
}

/// Resolves a path link against the terminal's working directory. `file://`
/// targets (OSC 8, GNU `ls --hyperlink`) keep their URI form.
pub fn resolve_terminal_link_target(text: &str, cwd: &str) -> String {
    if text.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://")) {
        return text.to_string();
    }
    let windows = cwd.contains('\\') || cwd.get(1..2) == Some(":");
    let separator = if windows { '\\' } else { '/' };
    let is_absolute = text.starts_with('/') || text.starts_with("\\\\") || text.get(1..3).is_some_and(|s| s == ":\\" || s == ":/");
    if is_absolute {
        return text.to_string();
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        if let Some(home) = infer_home(cwd) {
            return format!("{}{separator}{rest}", home.trim_end_matches(['/', '\\']));
        }
        return text.to_string();
    }
    let mut relative = text;
    while let Some(rest) = relative.strip_prefix("./").or_else(|| relative.strip_prefix(".\\")) {
        relative = rest;
    }
    format!("{}{separator}{relative}", cwd.trim_end_matches(['/', '\\']))
}

fn infer_home(cwd: &str) -> Option<String> {
    let normalized = cwd.replace('\\', "/");
    let mut parts = normalized.split('/');
    let first = parts.next()?;
    let second = parts.next()?;
    let third = parts.next()?;
    match (first, second) {
        ("", "home" | "Users") => Some(format!("/{second}/{third}")),
        (drive, "Users") if drive.ends_with(':') => Some(format!("{drive}\\Users\\{third}")),
        _ => None,
    }
}
