//! Inline Markdown marks in composer text. Ports `composer-rich-text.ts`:
//! bold, italic, strike and code spans, with delimiters kept as source the
//! editor can show or hide. Offsets are bytes into one line of source.

use std::ops::Range;

/// A style mark on a run of source text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RichTextMark {
    Bold,
    Italic,
    Strike,
    Code,
}

/// A set of marks, as bits, so pieces compare and hash cheaply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Marks(u8);

impl Marks {
    pub const NONE: Self = Self(0);

    fn bit(mark: RichTextMark) -> u8 {
        match mark {
            RichTextMark::Bold => 1,
            RichTextMark::Italic => 2,
            RichTextMark::Strike => 4,
            RichTextMark::Code => 8,
        }
    }

    pub fn with(self, mark: RichTextMark) -> Self {
        Self(self.0 | Self::bit(mark))
    }

    pub fn has(self, mark: RichTextMark) -> bool {
        self.0 & Self::bit(mark) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// A run of source from `range` that parses with `marks`. Bytes not covered
/// by any piece are delimiters.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourcePiece {
    pub range: Range<usize>,
    pub marks: Marks,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_path_token_char(c: char) -> bool {
    c.is_alphanumeric() || "_/.:@%+=&?#-".contains(c)
}

fn is_file_extension(ext: &str) -> bool {
    // ^\.[a-z][a-z0-9]*(?::\d+){0,2}$ (case-insensitive)
    let Some(rest) = ext.strip_prefix('.') else { return false };
    let mut parts = rest.split(':');
    let name = parts.next().unwrap_or("");
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphabetic()) || !chars.all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let ports: Vec<_> = parts.collect();
    ports.len() <= 2 && ports.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// `_yes_/_no_`: each side of a slash is its own emphasis span.
fn is_emphasis_alternatives(body: &str) -> bool {
    let parts: Vec<_> = body.split('/').collect();
    parts.len() >= 2
        && parts.iter().all(|p| p.len() >= 3 && p.starts_with('_') && p.ends_with('_') && !p[1..p.len() - 1].is_empty())
}

/// Starts of underscore runs that belong to a URL, path, or filename, not emphasis.
fn literal_underscore_runs(text: &str) -> Vec<usize> {
    let mut literal = Vec::new();
    if !text.contains('_') {
        return literal;
    }
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < text.len() {
        // A path token: path chars, or a `~` not next to another `~`.
        let token_char = |at: usize| -> Option<char> {
            let c = text[at..].chars().next()?;
            if is_path_token_char(c) {
                return Some(c);
            }
            if c == '~' && bytes.get(at.wrapping_sub(1)) != Some(&b'~') && bytes.get(at + 1) != Some(&b'~') {
                return Some(c);
            }
            None
        };
        let Some(_) = token_char(i) else {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let start = i;
        while let Some(c) = token_char(i) {
            i += c.len_utf8();
        }
        let token = &text[start..i];
        let mut body_end = token.len();
        while body_end > 0 && ".:?".contains(&token[body_end - 1..body_end]) {
            body_end -= 1;
        }
        if is_emphasis_alternatives(&token[..body_end]) {
            continue;
        }
        while body_end > 0 && &token[body_end - 1..body_end] == "_" {
            body_end -= 1;
        }
        let body = &token[..body_end];
        let is_path = if body.contains('/') {
            body.starts_with('/') || body.starts_with("~/") || body.contains(['.', ':'])
        } else {
            body.rfind('.').is_some_and(|dot| is_file_extension(&body[dot..]))
        };
        if !is_path {
            continue;
        }
        let mut closes_inside = false;
        let mut search = body.find('_');
        while let Some(run_start) = search {
            let mut run_end = run_start;
            while body.as_bytes().get(run_end) == Some(&b'_') {
                run_end += 1;
            }
            if run_start > 0 {
                literal.push(start + run_start);
                closes_inside |= !body[run_end..].chars().next().is_some_and(is_word_char);
            }
            search = body[run_end..].find('_').map(|i| run_end + i);
        }
        if closes_inside && body.starts_with('_') {
            literal.push(start);
        }
    }
    literal.sort_unstable();
    literal
}

struct Frame {
    delimiter: &'static str,
    mark: RichTextMark,
    start: usize,
    pieces: Vec<SourcePiece>,
}

fn push(root: &mut Vec<SourcePiece>, stack: &mut [Frame], range: Range<usize>, marks: Marks) {
    if range.end <= range.start {
        return;
    }
    let pieces = match stack.last_mut() {
        Some(frame) => &mut frame.pieces,
        None => root,
    };
    if let Some(last) = pieces.last_mut()
        && last.range.end == range.start
        && last.marks == marks
    {
        last.range.end = range.end;
        return;
    }
    pieces.push(SourcePiece { range, marks });
}

/// Parses one line into marked pieces. Ports `parseInlineMarkdownPieces`.
pub fn parse_inline_markdown_pieces(text: &str) -> Vec<SourcePiece> {
    let mut root = Vec::new();
    if !text.contains(['*', '_', '~', '`', '\\']) {
        if !text.is_empty() {
            root.push(SourcePiece { range: 0..text.len(), marks: Marks::NONE });
        }
        return root;
    }
    let literal_runs = literal_underscore_runs(text);
    let bytes = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let c = text[index..].chars().next().unwrap_or('\0');
        if c == '\\' {
            let next = text[index + 1..].chars().next().map_or(0, char::len_utf8);
            push(&mut root, &mut stack, index..(index + 1 + next).min(text.len()), Marks::NONE);
            index += 1 + next;
            continue;
        }
        if c == '`' {
            let run = bytes[index..].iter().take_while(|b| **b == b'`').count();
            let close = text[index + run..].find('`').map(|i| index + run + i);
            match close {
                Some(close)
                    if run == 1
                        && close > index + 1
                        && !text[index..close].contains('\n')
                        && bytes.get(close + 1) != Some(&b'`') =>
                {
                    push(&mut root, &mut stack, index + 1..close, Marks::NONE.with(RichTextMark::Code));
                    index = close + 1;
                }
                _ => {
                    push(&mut root, &mut stack, index..index + run, Marks::NONE);
                    index += run;
                }
            }
            continue;
        }
        if !matches!(c, '*' | '_' | '~') {
            push(&mut root, &mut stack, index..index + c.len_utf8(), Marks::NONE);
            index += c.len_utf8();
            continue;
        }
        let ch = c as u8;
        let mut end = index;
        while bytes.get(end) == Some(&ch) {
            end += 1;
        }
        if literal_runs.binary_search(&index).is_ok() {
            push(&mut root, &mut stack, index..end, Marks::NONE);
            index = end;
            continue;
        }
        let before = text[..index].chars().next_back();
        let after = text[end..].chars().next();
        let word_after = after.is_some_and(is_word_char);
        let word_before = before.is_some_and(is_word_char);
        let can_close = before.is_some_and(|b| !b.is_whitespace()) && (c != '_' || !word_after);
        let can_open = after.is_some_and(|a| !a.is_whitespace()) && (c != '_' || !word_before);
        while index < end {
            let top_mark = stack.last().map(|f| f.mark);
            let opens_nested_bold = top_mark == Some(RichTextMark::Italic)
                && end - index == 2
                && can_open
                && (c == '*' || c == '_')
                && !stack.iter().any(|f| f.mark == RichTextMark::Bold);
            let closes = !opens_nested_bold
                && can_close
                && stack.last().is_some_and(|top| {
                    text[index..].starts_with(top.delimiter) && index + top.delimiter.len() <= end
                });
            if closes {
                let top = stack.pop().unwrap_or_else(|| unreachable!());
                for piece in &top.pieces {
                    push(&mut root, &mut stack, piece.range.clone(), piece.marks.with(top.mark));
                }
                index += top.delimiter.len();
            } else if can_open && (c != '~' || end - index >= 2) {
                let length = if c == '~' || end - index >= 2 { 2 } else { 1 };
                let mark = match (c, length) {
                    ('~', _) => RichTextMark::Strike,
                    (_, 2) => RichTextMark::Bold,
                    _ => RichTextMark::Italic,
                };
                if stack.iter().any(|f| f.mark == mark) {
                    push(&mut root, &mut stack, index..end, Marks::NONE);
                    index = end;
                    continue;
                }
                let delimiter = match (c, length) {
                    ('*', 2) => "**",
                    ('*', _) => "*",
                    ('_', 2) => "__",
                    ('_', _) => "_",
                    _ => "~~",
                };
                stack.push(Frame { delimiter, mark, start: index, pieces: Vec::new() });
                index += length;
            } else {
                push(&mut root, &mut stack, index..end, Marks::NONE);
                index = end;
            }
        }
    }
    while let Some(frame) = stack.pop() {
        push(&mut root, &mut stack, frame.start..frame.start + frame.delimiter.len(), Marks::NONE);
        for piece in frame.pieces {
            push(&mut root, &mut stack, piece.range, piece.marks);
        }
    }
    root
}
