//! One matched line with syntax colours and its matches marked. Ports
//! `HighlightedSearchLine.tsx`: the line is highlighted on its own, match
//! ranges are clamped, sorted and merged, and matched text gets the search
//! `bg-primary/25` mark.

use std::ops::Range;

use gpui_kit::{App, HighlightStyle, Hsla, SharedString, StyledText};
use t3code_contracts::ProjectContentMatch;

use crate::{
    ActiveAppTheme as _,
    components::files::editor_highlight::highlight_line,
    syntax::{SyntaxPalette, language_for_path},
};

/// Longest line drawn; past this the line is cut, as the row truncates anyway.
const MAX_LINE_CHARS: usize = 400;

/// Converts a UTF-16 offset (the wire's unit) to a byte offset in `text`.
pub fn utf16_to_byte(text: &str, offset: u64) -> usize {
    let mut units = 0u64;
    for (index, c) in text.char_indices() {
        if units >= offset {
            return index;
        }
        units += c.len_utf16() as u64;
    }
    text.len()
}

/// `normalizeRanges`: clamped, sorted, merged byte ranges.
pub fn match_ranges(item: &ProjectContentMatch) -> Vec<Range<usize>> {
    let text = &item.line_content;
    let mut ranges: Vec<Range<usize>> = item
        .match_ranges
        .iter()
        .map(|r| utf16_to_byte(text, r.start)..utf16_to_byte(text, r.end))
        .filter(|r| r.end > r.start)
        .collect();
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(previous) if range.start <= previous.end => previous.end = previous.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// The styled line for a result row.
pub fn highlighted_search_line(item: &ProjectContentMatch, path: &str, cx: &App) -> StyledText {
    let theme = cx.app_theme();
    let text = &item.line_content;
    let cut = text.char_indices().nth(MAX_LINE_CHARS).map_or(text.len(), |(i, _)| i);
    let shown: SharedString = text[..cut].to_owned().into();
    let palette = SyntaxPalette::for_appearance(theme.appearance);
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    if let Some(language) = language_for_path(path) {
        for span in highlight_line(language, &shown).iter() {
            highlights.push((span.start as usize..span.end as usize, palette.style(span.token)));
        }
    }
    let matches: Vec<Range<usize>> = match_ranges(item).into_iter().filter(|r| r.start < cut).map(|r| r.start..r.end.min(cut)).collect();
    StyledText::new(shown).with_highlights(overlay(highlights, &matches, theme.colors.primary().opacity(0.25)))
}

/// Splits syntax spans at match edges and gives matched pieces the match colours.
fn overlay(
    syntax: Vec<(Range<usize>, HighlightStyle)>,
    matches: &[Range<usize>],
    background: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut edges: Vec<usize> = syntax.iter().flat_map(|(r, _)| [r.start, r.end]).chain(matches.iter().flat_map(|r| [r.start, r.end])).collect();
    edges.sort_unstable();
    edges.dedup();
    let mut out = Vec::new();
    for pair in edges.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let base = syntax.iter().find(|(r, _)| r.start <= start && end <= r.end).map(|(_, s)| *s);
        let matched = matches.iter().any(|r| r.start <= start && end <= r.end);
        let style = match (base, matched) {
            // `<mark className="bg-primary/25 text-inherit">`: the token keeps its colour.
            (_, true) => HighlightStyle { background_color: Some(background), ..base.unwrap_or_default() },
            (Some(style), false) => style,
            (None, false) => continue,
        };
        out.push((start..end, style));
    }
    out
}
