//! Port of `packages/shared/src/textDirection.ts` (re-exported by `apps/web/src/textDirection.ts`):
//! decides whether prose reads right-to-left by majority, since a renderer's own
//! first-strong-character rule misreads Arabic prose that opens with an English term.

use std::sync::LazyLock;

use fancy_regex::Regex;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

/// A paragraph direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDirection {
    Ltr,
    Rtl,
}

/// Hebrew through Arabic Extended-A, their presentation forms, and the
/// supplementary right-to-left blocks.
fn is_rtl_letter(c: char) -> bool {
    matches!(c,
        '\u{590}'..='\u{8FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}'
        | '\u{10800}'..='\u{10FFF}' | '\u{1E800}'..='\u{1EFFF}')
}

fn is_letter(c: char) -> bool {
    c.general_category_group() == GeneralCategoryGroup::Letter
}

/// JavaScript's `\s`.
fn is_js_space(c: char) -> bool {
    matches!(c,
        '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}'
        | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
}

/// True when the text holds any right-to-left letter.
pub fn has_right_to_left_letter(text: &str) -> bool {
    text.chars().any(is_rtl_letter)
}

fn word_direction(word: &str) -> Option<TextDirection> {
    let letter = word.chars().find(|&c| is_letter(c))?;
    Some(if is_rtl_letter(letter) { TextDirection::Rtl } else { TextDirection::Ltr })
}

/// Counts directions; a tie goes to `tie` when given, else to the first unit.
fn majority(
    units: impl Iterator<Item = Option<TextDirection>>,
    tie: Option<&dyn Fn() -> Option<TextDirection>>,
) -> Option<TextDirection> {
    let (mut rtl, mut ltr, mut first) = (0, 0, None);
    for direction in units.flatten() {
        first.get_or_insert(direction);
        match direction {
            TextDirection::Rtl => rtl += 1,
            TextDirection::Ltr => ltr += 1,
        }
    }
    match rtl.cmp(&ltr) {
        std::cmp::Ordering::Equal => tie.map_or(first, |tie| tie()),
        std::cmp::Ordering::Greater => Some(TextDirection::Rtl),
        std::cmp::Ordering::Less => Some(TextDirection::Ltr),
    }
}

fn words(text: &str) -> impl Iterator<Item = Option<TextDirection>> {
    text.split(is_js_space).map(word_direction)
}

/// Each sentence follows its main language by word count, and the text follows most
/// of its sentences, so one long English sentence cannot outvote the Arabic ones
/// around it. Sentence ties use the whole-text word majority, then the first word.
pub fn text_direction(text: &str) -> Option<TextDirection> {
    if !has_right_to_left_letter(text) {
        return text.chars().any(is_letter).then_some(TextDirection::Ltr);
    }
    let sentences = split_sentences(text).into_iter().map(|s| majority(words(s), None));
    majority(sentences, Some(&|| majority(words(text), None)))
}

/// `text.split(/(?<=[.!?؟])(?<!\b(?:e\.g|i\.e)\.)\s+|\n+/i)`.
fn split_sentences(text: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut piece_start = 0;
    let mut pos = 0;
    while pos < text.len() {
        let rest = &text[pos..];
        let after_stop = text[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| matches!(c, '.' | '!' | '?' | '؟'))
            && !ends_with_abbreviation(&text[..pos]);
        let space_len = rest.find(|c| !is_js_space(c)).unwrap_or(rest.len());
        let match_len = if after_stop && space_len > 0 {
            space_len
        } else {
            rest.find(|c| c != '\n').unwrap_or(rest.len())
        };
        if match_len > 0 {
            pieces.push(&text[piece_start..pos]);
            pos += match_len;
            piece_start = pos;
        } else {
            pos += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    pieces.push(&text[piece_start..]);
    pieces
}

/// `(?<!\b(?:e\.g|i\.e)\.)`, case-insensitive, with an ASCII word boundary.
fn ends_with_abbreviation(before: &str) -> bool {
    let bytes = before.as_bytes();
    if bytes.len() < 4 {
        return false;
    }
    let tail = &bytes[bytes.len() - 4..];
    let lower: Vec<u8> = tail.iter().map(u8::to_ascii_lowercase).collect();
    if lower != b"e.g." && lower != b"i.e." {
        return false;
    }
    bytes
        .len()
        .checked_sub(5)
        .map(|i| bytes[i])
        .is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
}

/// Fenced code, inline code and URLs, which do not count toward prose direction.
static MARKDOWN_CODE_OR_URL: LazyLock<Regex> = LazyLock::new(|| {
    const NOT_SPACE: &str = r"[^\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}]";
    Regex::new(&format!(
        r"(`{{3,}}|~{{3,}})(?s:.)*?(?:\1|$)|(`{{1,2}})(?!`)(?s:.)*?(?<!`)\2(?!`)|https?://{NOT_SPACE}+"
    ))
    .unwrap()
});

/// The direction of markdown prose, ignoring code and URLs.
pub fn markdown_prose_direction(markdown: &str) -> Option<TextDirection> {
    let mut prose = String::with_capacity(markdown.len());
    let mut last = 0;
    for found in MARKDOWN_CODE_OR_URL.find_iter(markdown).flatten() {
        prose.push_str(&markdown[last..found.start()]);
        prose.push(' ');
        last = found.end();
    }
    prose.push_str(&markdown[last..]);
    text_direction(&prose)
}
