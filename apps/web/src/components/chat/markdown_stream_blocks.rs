//! Splits markdown at blank lines that close every open block, so each piece
//! parses exactly as it would inside the whole text. A streaming reply then
//! re-parses only its last block per delta; finished blocks come from a cache.
//! Ports `markdownStreamBlocks.ts`.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use crate::markdown_pipeline::{Block, MarkdownFlavor, ParsedBlocks, parse_markdown};

/// Splits `text` into independently parseable blocks, or returns `None` when
/// the text uses syntax that ties blocks together (definitions, footnotes, raw
/// HTML, `:::`, `$$`, setext headings, duplicate heading slugs).
pub fn split_markdown_blocks(text: &str) -> Option<Vec<&str>> {
    if text.starts_with('\u{feff}') || text.contains('\r') || has_document_wide_syntax(text) {
        return None;
    }
    let mut blocks = Vec::new();
    let mut block_start = 0;
    let mut line_start = 0;
    let mut fence: Option<(u8, usize)> = None;
    let mut fence_indent = 0;
    let mut after_blank = false;
    let mut after_content = false;
    let mut insight_indent: Option<usize> = None;
    let mut heading_slugs = HashSet::new();

    for line in text.split('\n') {
        let opener = fence_opener(line);
        let may_underline = after_content;
        after_content = false;
        if let Some((fence_char, fence_len)) = fence {
            if line.get(..fence_indent).is_some_and(|lead| lead.chars().any(|c| !c.is_whitespace())) {
                // A line left of an indented fence may close its list item; give up on splitting.
                for rest in text[line_start..].split('\n') {
                    if heading_needs_whole(rest, true, &mut heading_slugs) {
                        return None;
                    }
                }
                break;
            }
            if let Some((c, len)) = opener
                && c == fence_char
                && len >= fence_len
                && line.trim().len() == len
            {
                fence = None;
            }
        } else if insight_indent.is_some_and(|indent| is_insight_close(line, indent)) {
            insight_indent = None;
            after_blank = false;
        } else if line.trim().is_empty() {
            after_blank = true;
        } else {
            if heading_needs_whole(line, may_underline, &mut heading_slugs) {
                return None;
            }
            after_content = opener.is_none();
            if after_blank && insight_indent.is_none() && line_start > block_start && !is_continuation(line)
            {
                blocks.push(&text[block_start..line_start]);
                block_start = line_start;
            }
            after_blank = false;
            if insight_indent.is_none() {
                insight_indent = insight_open_indent(line);
            }
            fence = opener;
            if opener.is_some() {
                fence_indent = line.len() - line.trim_start().len();
            }
        }
        line_start += line.len() + 1;
    }
    blocks.push(&text[block_start.min(text.len())..]);
    Some(blocks)
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Definitions, footnotes, raw HTML, container directives and display math.
fn has_document_wide_syntax(text: &str) -> bool {
    if text.contains("[^") {
        return true;
    }
    text.split('\n').any(|line| {
        if leading_spaces(line) > 3 {
            return false;
        }
        let rest = line.trim_start_matches(' ');
        if rest.starts_with(":::") || rest.starts_with("$$") {
            return true;
        }
        if let Some(after) = rest.strip_prefix('<') {
            return after.starts_with(|c: char| c.is_ascii_alphabetic() || matches!(c, '!' | '?' | '/'));
        }
        if let Some(after) = rest.strip_prefix('[')
            && let Some(close) = after.find(']')
        {
            return close > 0 && !after[..close].contains('\n') && after[close + 1..].starts_with(':');
        }
        false
    })
}

/// A backtick or tilde fence opener: its character and length.
fn fence_opener(line: &str) -> Option<(u8, usize)> {
    if leading_spaces(line) > 3 {
        return None;
    }
    let rest = line.trim_start_matches(' ');
    let c = *rest.as_bytes().first()?;
    if c != b'`' && c != b'~' {
        return None;
    }
    let len = rest.bytes().take_while(|b| *b == c).count();
    if len < 3 {
        return None;
    }
    // A backtick fence's info string cannot contain a backtick.
    if c == b'`' && rest[len..].contains('`') {
        return None;
    }
    Some((c, len))
}

/// A list item or indented content still continues the block after a blank line.
fn is_continuation(line: &str) -> bool {
    if line.starts_with([' ', '\t']) {
        return true;
    }
    let bytes = line.as_bytes();
    if matches!(bytes.first(), Some(b'-' | b'*' | b'+')) {
        return matches!(bytes.get(1), None | Some(b' ' | b'\t'));
    }
    let digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    (1..=9).contains(&digits)
        && matches!(bytes.get(digits), Some(b'.' | b')'))
        && matches!(bytes.get(digits + 1), None | Some(b' ' | b'\t'))
}

fn insight_open_indent(line: &str) -> Option<usize> {
    let indent = leading_spaces(line);
    let rest = line[indent..].trim_matches('`').trim_end();
    let after = rest.strip_prefix("★ Insight")?;
    let rule = after.trim_start();
    (after.len() > rule.len() && rule.chars().count() >= 3 && rule.chars().all(|c| c == '─')).then_some(indent)
}

fn is_insight_close(line: &str, indent: usize) -> bool {
    leading_spaces(line) == indent && {
        let rule = line[indent..].trim_matches('`').trim_end();
        rule.chars().count() >= 3 && rule.chars().all(|c| c == '─')
    }
}

/// Strips list and quote markers in front of a heading or setext underline.
fn strip_container_markers(mut line: &str) -> &str {
    loop {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if let Some(rest) = trimmed.strip_prefix('>') {
            line = rest;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix(['-', '*', '+'])
            && rest.starts_with([' ', '\t'])
        {
            line = rest;
            continue;
        }
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=9).contains(&digits)
            && let Some(rest) = trimmed[digits..].strip_prefix(['.', ')'])
            && rest.starts_with([' ', '\t'])
        {
            line = rest;
            continue;
        }
        return trimmed;
    }
}

/// Whether a heading line needs the whole document to render (setext
/// underline, inline syntax in its text, or a slug already used).
fn heading_needs_whole(line: &str, may_underline: bool, slugs: &mut HashSet<String>) -> bool {
    let content = strip_container_markers(line);
    if may_underline {
        let underline = content.trim_end();
        if !underline.is_empty() && (underline.bytes().all(|b| b == b'=') || underline.bytes().all(|b| b == b'-')) {
            return true;
        }
    }
    let hashes = content.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return false;
    }
    let rest = &content[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return false;
    }
    let heading = rest.trim().trim_end_matches('#').trim();
    if heading.is_empty() {
        return false;
    }
    let mut chars = heading.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if "`*_~[]<&\\!$".contains(c) {
            return true;
        }
        if c == ':' && heading[i + 1..].chars().next().is_some_and(|n| !n.is_whitespace()) {
            return true;
        }
    }
    let slug = heading_slug(heading);
    !slug.is_empty() && !slugs.insert(slug)
}

/// GitHub's heading slug: lowercase, spaces to dashes, punctuation dropped.
fn heading_slug(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// Parsed blocks for finished markdown, keyed by source hash and flavor.
/// Entries are never mutated, so renderers share them through `Arc`.
#[derive(Default)]
pub struct MarkdownBlockCache {
    entries: HashMap<u64, ParsedBlocks>,
}

thread_local! {
    static CACHE: RefCell<MarkdownBlockCache> = RefCell::new(MarkdownBlockCache::default());
}

/// Upper bound on cached block parses before the cache starts over.
const CACHE_CAPACITY: usize = 20_000;

fn cache_key(source: &str, flavor: MarkdownFlavor) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    flavor.hash(&mut hasher);
    hasher.finish()
}

/// Parses `source` (or fetches it from the cache) as one list of blocks.
/// While `streaming`, every block but the last is cached and the last is
/// parsed fresh, so a delta costs one block parse.
pub fn parse_message_blocks(source: &str, flavor: MarkdownFlavor, streaming: bool) -> Vec<ParsedBlocks> {
    let pieces = split_markdown_blocks(source).unwrap_or_else(|| vec![source]);
    let last = pieces.len().saturating_sub(1);
    pieces
        .into_iter()
        .enumerate()
        .map(|(index, piece)| {
            if streaming && index == last {
                return ParsedBlocks::from(parse_markdown(piece, flavor));
            }
            let key = cache_key(piece, flavor);
            CACHE.with_borrow_mut(|cache| {
                if cache.entries.len() >= CACHE_CAPACITY {
                    cache.entries.clear();
                }
                cache
                    .entries
                    .entry(key)
                    .or_insert_with(|| Arc::from(parse_markdown(piece, flavor)))
                    .clone()
            })
        })
        .collect()
}

/// Flattens parsed pieces into one block list for layout.
pub fn flatten(pieces: &[ParsedBlocks]) -> impl Iterator<Item = &Block> {
    pieces.iter().flat_map(|piece| piece.iter())
}
