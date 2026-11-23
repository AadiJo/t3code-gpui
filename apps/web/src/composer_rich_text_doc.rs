//! Block structure of composer Markdown. Ports the block grammar of
//! `composer-rich-text-doc.ts` (`buildTiptapContent`, `parseOpeningFence`,
//! `isClosingFence`, `parseListPrefix`). The web editor turns these into
//! ProseMirror nodes; here each source line keeps its text and gets a kind the
//! editor styles, so the stored Markdown is always the document.

use std::ops::Range;

/// A list item's marker kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Bullet,
    Ordered,
    Task { checked: bool },
}

/// What one source line is, for styling.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LineKind {
    Paragraph,
    /// `#`..`######` then space; `marker` covers the hashes and the space.
    Heading { level: u8, marker: usize },
    /// `>` and its spaces.
    Quote { marker: usize },
    /// `---`, `***`, `___` (spaces allowed between).
    Rule,
    /// A list item; `marker` covers indent, marker and padding (and a task box).
    ListItem { kind: ListKind, marker: usize },
    /// An opening fence, with the info string's language.
    FenceOpen { block: usize },
    /// A line inside a fenced block.
    Code { block: usize },
    /// The closing fence of a block.
    FenceClose { block: usize },
}

impl LineKind {
    /// Whether the line belongs to a fenced code block (fences included).
    pub fn is_code(&self) -> bool {
        matches!(self, Self::FenceOpen { .. } | Self::Code { .. } | Self::FenceClose { .. })
    }

    /// The code block the line belongs to.
    pub fn code_block(&self) -> Option<usize> {
        match self {
            Self::FenceOpen { block } | Self::Code { block } | Self::FenceClose { block } => Some(*block),
            _ => None,
        }
    }
}

/// A fenced code block: its fence, language and the source lines it spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlock {
    pub fence: String,
    pub language: String,
    /// Line indices of the opening fence through the closing fence (or the last line).
    pub lines: Range<usize>,
    pub closed: bool,
}

/// An opening fence: ```` ``` ```` or `~~~`, three or more, then an info string.
pub fn parse_opening_fence(line: &str) -> Option<(String, String)> {
    let first = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let run = line.chars().take_while(|c| *c == first).count();
    if run < 3 {
        return None;
    }
    let language = &line[run..];
    if first == '`' && language.contains('`') {
        return None;
    }
    Some((line[..run].to_owned(), language.to_owned()))
}

/// Whether `line` closes a block opened with `fence`.
pub fn is_closing_fence(line: &str, fence: &str) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let Some(first) = line.chars().next().filter(|c| *c == '`' || *c == '~') else {
        return false;
    };
    let run = line.chars().take_while(|c| *c == first).count();
    run >= 3
        && line[run..].chars().all(|c| c == ' ' || c == '\t')
        && fence.starts_with(first)
        && run >= fence.len()
}

/// `- [ ]`, `1.`, `-`: the list marker at the start of a line and its byte length.
pub fn parse_list_prefix(line: &str) -> Option<(ListKind, usize)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let spaces = |s: &str| s.len() - s.trim_start_matches([' ', '\t']).len();
    if let Some(after_dash) = rest.strip_prefix('-') {
        let gap = spaces(after_dash);
        let boxed = &after_dash[gap..];
        if gap > 0 && boxed.len() >= 3 && boxed.starts_with('[') && boxed.as_bytes()[2] == b']' {
            let mark = boxed.as_bytes()[1];
            if matches!(mark, b' ' | b'x' | b'X') {
                let tail = &boxed[3..];
                let pad = spaces(tail);
                if tail.len() == pad || pad > 0 {
                    let checked = mark != b' ';
                    return Some((ListKind::Task { checked }, indent + 1 + gap + 3 + pad));
                }
            }
        }
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && matches!(rest.as_bytes().get(digits), Some(b'.' | b')')) {
        let tail = &rest[digits + 1..];
        let pad = spaces(tail);
        if pad > 0 || tail.is_empty() {
            return Some((ListKind::Ordered, indent + digits + 1 + pad));
        }
    }
    if let Some(c) = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '+')) {
        let tail = &rest[c.len_utf8()..];
        let pad = spaces(tail);
        if pad > 0 || tail.is_empty() {
            return Some((ListKind::Bullet, indent + 1 + pad));
        }
    }
    None
}

fn is_rule(line: &str) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let Some(first) = line.chars().next().filter(|c| matches!(c, '-' | '*' | '_')) else {
        return false;
    };
    let mut count = 0;
    for c in line.chars() {
        if c == first {
            count += 1;
        } else if c != ' ' && c != '\t' {
            return false;
        }
    }
    count >= 3
}

fn heading(line: &str) -> Option<LineKind> {
    let level = line.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    let space = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    (space > 0).then_some(LineKind::Heading { level: level as u8, marker: level + space })
}

/// Classifies every source line. `blocks` is false in plain mode (rich text off),
/// where every line is a paragraph.
pub fn classify_lines<'a>(lines: impl IntoIterator<Item = &'a str>, blocks: bool) -> (Vec<LineKind>, Vec<CodeBlock>) {
    let lines: Vec<&str> = lines.into_iter().collect();
    let mut kinds = Vec::with_capacity(lines.len());
    let mut code_blocks = Vec::new();
    if !blocks {
        kinds.resize(lines.len(), LineKind::Paragraph);
        return (kinds, code_blocks);
    }
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let Some((fence, language)) = parse_opening_fence(line) else {
            let kind = if is_rule(line) {
                LineKind::Rule
            } else if let Some(h) = heading(line) {
                h
            } else if let Some(rest) = line.strip_prefix('>') {
                LineKind::Quote { marker: 1 + rest.len() - rest.trim_start_matches([' ', '\t']).len() }
            } else if let Some((kind, marker)) = parse_list_prefix(line) {
                LineKind::ListItem { kind, marker }
            } else {
                LineKind::Paragraph
            };
            kinds.push(kind);
            index += 1;
            continue;
        };
        let block = code_blocks.len();
        let start = index;
        kinds.push(LineKind::FenceOpen { block });
        index += 1;
        let mut closed = false;
        while index < lines.len() {
            if is_closing_fence(lines[index], &fence) {
                kinds.push(LineKind::FenceClose { block });
                index += 1;
                closed = true;
                break;
            }
            kinds.push(LineKind::Code { block });
            index += 1;
        }
        code_blocks.push(CodeBlock { fence, language: language.trim().to_owned(), lines: start..index, closed });
    }
    (kinds, code_blocks)
}
