//! Text direction per composer block. Ports `composer-text-direction.ts` and
//! `packages/shared/src/textDirection.ts`: each sentence follows its main
//! language by word count, a block follows most of its sentences, and a block
//! with no letters yet continues the one above. Code stays left to right.

use crate::composer_rich_text_doc::LineKind;

/// Left to right or right to left.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextDirection {
    Ltr,
    Rtl,
}

/// Hebrew through Arabic Extended-A, their presentation forms, and the
/// supplementary right-to-left blocks.
pub fn is_rtl_letter(c: char) -> bool {
    matches!(c as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

fn word_direction(word: &str) -> Option<TextDirection> {
    let letter = word.chars().find(|c| c.is_alphabetic())?;
    Some(if is_rtl_letter(letter) { TextDirection::Rtl } else { TextDirection::Ltr })
}

fn majority(
    units: impl Iterator<Item = Option<TextDirection>>,
    tie: impl FnOnce() -> Option<TextDirection>,
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
        std::cmp::Ordering::Equal => tie().or(first),
        std::cmp::Ordering::Greater => Some(TextDirection::Rtl),
        std::cmp::Ordering::Less => Some(TextDirection::Ltr),
    }
}

/// Splits after `.!?؟` followed by whitespace (not after "e.g."/"i.e."), and at newlines.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end_of_sentence = matches!(c, '.' | '!' | '?' | '؟')
            && chars.peek().is_some_and(|(_, n)| n.is_whitespace())
            && !text[..=i].to_lowercase().ends_with("e.g.")
            && !text[..=i].to_lowercase().ends_with("i.e.");
        if end_of_sentence || c == '\n' {
            out.push(&text[start..i + c.len_utf8()]);
            while chars.peek().is_some_and(|(_, n)| n.is_whitespace()) {
                chars.next();
            }
            start = chars.peek().map_or(text.len(), |(j, _)| *j);
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// The direction of a text, or `None` when it has no letters. Ports `textDirection`.
pub fn text_direction(text: &str) -> Option<TextDirection> {
    if !text.chars().any(is_rtl_letter) {
        return text.chars().any(char::is_alphabetic).then_some(TextDirection::Ltr);
    }
    majority(
        sentences(text).into_iter().map(|s| majority(s.split_whitespace().map(word_direction), || None)),
        || majority(text.split_whitespace().map(word_direction), || None),
    )
}

/// Directions for every source line. Code lines are always left to right; a line
/// with no letters follows the line above. Ports `composerTextDirections`.
pub fn line_directions<'a>(lines: impl Iterator<Item = (&'a str, &'a LineKind)>) -> Vec<TextDirection> {
    let mut previous = None;
    lines
        .map(|(text, kind)| {
            if kind.is_code() {
                return TextDirection::Ltr;
            }
            let direction = text_direction(text).or(previous);
            previous = direction;
            direction.unwrap_or(TextDirection::Ltr)
        })
        .collect()
}
