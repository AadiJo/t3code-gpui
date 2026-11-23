//! Literal Markdown list editing: Enter continues or ends a list item, Tab and
//! Shift+Tab nest and un-nest it, and ordered items renumber. Ports
//! `composer-list-continuation.ts`. Offsets are bytes into the source.

use std::ops::Range;

use crate::composer_editor_mentions::{ComposerPromptSegment, split_prompt_into_segments};

/// A replacement to apply to the draft, and where the caret lands after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerListEdit {
    pub range: Range<usize>,
    pub replacement: String,
    pub cursor_after: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Marker {
    Ordered { indent: String, number: String, delimiter: char, space: String },
    Task { indent: String, space: String },
    Bullet { indent: String, bullet: char, space: String },
}

impl Marker {
    fn indent(&self) -> &str {
        match self {
            Self::Ordered { indent, .. } | Self::Task { indent, .. } | Self::Bullet { indent, .. } => indent,
        }
    }

    fn space(&self) -> &str {
        match self {
            Self::Ordered { space, .. } | Self::Task { space, .. } | Self::Bullet { space, .. } => space,
        }
    }
}

struct Parsed {
    marker: Marker,
    marker_end: usize,
    content_column: usize,
}

/// Markdown tabs advance to the next four-column stop.
fn indent_width(text: &str, from_column: usize) -> usize {
    let mut column = from_column;
    for c in text.chars() {
        match c {
            ' ' => column += 1,
            '\t' => column += 4 - column % 4,
            _ => break,
        }
    }
    column - from_column
}

/// Content follows 1-4 columns of padding; wider padding is code.
fn content_column(indent: &str, marker_width: usize, padding: &str) -> usize {
    let marker_end = indent_width(indent, 0) + marker_width;
    let space = indent_width(padding, marker_end);
    marker_end + if (1..=4).contains(&space) { space } else { 1 }
}

/// Splits `rest` into leading spaces/tabs; returns it only when it is the whole
/// remainder or followed by more text (the `((?:[ \t]+)|\s*$)` group).
fn padding(rest: &str) -> Option<&str> {
    let len = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if len > 0 {
        Some(&rest[..len])
    } else if rest.trim().is_empty() {
        Some(rest)
    } else {
        None
    }
}

fn parse_list_marker(line: &str) -> Option<Parsed> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && let Some(delimiter) = rest[digits..].chars().next().filter(|c| *c == '.' || *c == ')')
        && let Some(space) = padding(&rest[digits + 1..])
    {
        return Some(Parsed {
            content_column: content_column(indent, digits + 1, space),
            marker_end: indent_len + digits + 1 + space.len(),
            marker: Marker::Ordered {
                indent: indent.to_owned(),
                number: rest[..digits].to_owned(),
                delimiter,
                space: space.to_owned(),
            },
        });
    }
    if let Some(after) = rest.strip_prefix('-')
        && let Some(gap) = after.chars().next().filter(|c| c.is_whitespace())
        && after[gap.len_utf8()..].len() >= 3
    {
        let boxed = &after[gap.len_utf8()..];
        let b = boxed.as_bytes();
        if b[0] == b'[' && matches!(b[1], b' ' | b'x' | b'X') && b[2] == b']'
            && let Some(space) = padding(&boxed[3..])
        {
            return Some(Parsed {
                content_column: content_column(indent, 1, &gap.to_string()),
                marker_end: indent_len + 1 + gap.len_utf8() + 3 + space.len(),
                marker: Marker::Task { indent: indent.to_owned(), space: space.to_owned() },
            });
        }
    }
    let bullet = rest.chars().next().filter(|c| matches!(c, '-' | '*' | '+'))?;
    let space = padding(&rest[1..])?;
    Some(Parsed {
        content_column: content_column(indent, 1, space),
        marker_end: indent_len + 1 + space.len(),
        marker: Marker::Bullet { indent: indent.to_owned(), bullet, space: space.to_owned() },
    })
}

/// `9.` → `10.`, keeping zero padding. Ports `nextOrderedMarkerText`.
pub fn next_ordered_marker_text(number: &str, delimiter: char) -> String {
    let next = number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_add(1))
        .map_or_else(|| number.to_owned(), |n| format!("{n:0>width$}", width = number.len()));
    format!("{next}{delimiter}")
}

fn next_marker_text(marker: &Marker) -> String {
    match marker {
        Marker::Ordered { indent, number, delimiter, .. } => {
            format!("{indent}{} ", next_ordered_marker_text(number, *delimiter))
        }
        Marker::Task { indent, .. } => format!("{indent}- [ ] "),
        Marker::Bullet { indent, bullet, .. } => format!("{indent}{bullet} "),
    }
}

fn inline_token_ranges(value: &str) -> Vec<Range<usize>> {
    split_prompt_into_segments(value)
        .into_iter()
        .filter_map(|segment| match segment {
            ComposerPromptSegment::Chip(token) => Some(token.range),
            ComposerPromptSegment::Text(_) => None,
        })
        .collect()
}

fn inside_token(tokens: &[Range<usize>], cursor: usize) -> bool {
    tokens.iter().any(|r| cursor > r.start && cursor < r.end)
}

fn current_line(value: &str, cursor: usize) -> Range<usize> {
    let start = value[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let end = value[cursor..].find('\n').map_or(value.len(), |i| cursor + i);
    start..end
}

/// List markers on the lines above `line_start`, nearest first, stopping at a non-list line.
fn list_markers_above(value: &str, mut line_start: usize, tokens: &[Range<usize>]) -> Vec<Parsed> {
    let mut out = Vec::new();
    let mut token_index = tokens.len();
    while line_start > 0 {
        let line_end = line_start - 1;
        line_start = value[..line_end].rfind('\n').map_or(0, |i| i + 1);
        while token_index > 0 {
            let token = &tokens[token_index - 1];
            if token.start >= line_start {
                token_index -= 1;
            } else if token.end > line_start {
                line_start = value[..token.start].rfind('\n').map_or(0, |i| i + 1);
            } else {
                break;
            }
        }
        match parse_list_marker(&value[line_start..line_end]) {
            Some(parsed) => out.push(parsed),
            None => break,
        }
    }
    out
}

fn parent_marker(value: &str, line_start: usize, marker: &Marker, tokens: &[Range<usize>]) -> Option<Marker> {
    let width = indent_width(marker.indent(), 0);
    list_markers_above(value, line_start, tokens)
        .into_iter()
        .find(|above| indent_width(above.marker.indent(), 0) < width)
        .map(|above| above.marker)
}

fn renumber_following_items(value: &str, after_line_end: usize, marker: &Marker, tokens: &[Range<usize>]) -> Option<(usize, String)> {
    let Marker::Ordered { indent, number, delimiter, .. } = marker else { return None };
    let base_width = indent_width(indent, 0);
    let mut expected = number.parse::<u64>().ok()?.checked_add(2)?;
    let mut parts = String::new();
    let mut copied_until = after_line_end;
    let mut position = after_line_end;
    let mut token_index = 0;
    let mut changed = false;
    while position < value.len() {
        let line_start = position + 1;
        let line_end = value[line_start..].find('\n').map_or(value.len(), |i| line_start + i);
        position = line_end;
        while tokens.get(token_index).is_some_and(|t| t.end <= line_start) {
            token_index += 1;
        }
        if tokens.get(token_index).is_some_and(|t| t.start < line_start) {
            continue;
        }
        let line = &value[line_start..line_end];
        let width = indent_width(line, 0);
        if line.trim().is_empty() || width < base_width {
            break;
        }
        if width > base_width {
            continue;
        }
        let Some(Parsed { marker: Marker::Ordered { indent: current_indent, number: current, delimiter: d, .. }, .. }) =
            parse_list_marker(line)
        else {
            break;
        };
        if d != *delimiter {
            break;
        }
        let number_start = line_start + current_indent.len();
        let replacement = if current.starts_with('0') {
            format!("{expected:0>width$}", width = current.len())
        } else {
            expected.to_string()
        };
        if replacement != current {
            parts.push_str(&value[copied_until..number_start]);
            parts.push_str(&replacement);
            copied_until = number_start + current.len();
            changed = true;
        }
        expected += 1;
    }
    changed.then_some((copied_until, parts))
}

fn nested_number(marker: &Marker, previous_child: Option<&Marker>) -> (usize, String) {
    let Marker::Ordered { number, delimiter, .. } = marker else {
        return (0, String::new());
    };
    let text = match previous_child {
        Some(Marker::Ordered { number: prev, delimiter: d, .. }) if d == delimiter => {
            let next = next_ordered_marker_text(prev, *delimiter);
            next[..next.len() - 1].to_owned()
        }
        _ => "1".to_owned(),
    };
    (number.len(), text)
}

/// Enter inside a list item: continue the list, or end an empty item (lifting it
/// to its parent). Ports `listContinuationForEnter`.
pub fn list_continuation_for_enter(value: &str, cursor: usize) -> Option<ComposerListEdit> {
    if cursor > value.len() || !value.is_char_boundary(cursor) {
        return None;
    }
    let line = current_line(value, cursor);
    let parsed = parse_list_marker(&value[line.clone()])?;
    if parsed.marker.space().is_empty() {
        return None;
    }
    let marker_end = line.start + parsed.marker_end;
    if cursor < marker_end {
        return None;
    }
    let tokens = inline_token_ranges(value);
    if inside_token(&tokens, cursor) {
        return None;
    }
    if value[marker_end..line.end].trim().is_empty() {
        let parent = parent_marker(value, line.start, &parsed.marker, &tokens);
        let replacement = parent.as_ref().map(next_marker_text).unwrap_or_default();
        let cursor_after = line.start + replacement.len();
        let end = cursor.max(marker_end);
        if let Some(parent @ Marker::Ordered { .. }) = &parent
            && let Some((renumbered_end, text)) = renumber_following_items(value, line.end, parent, &tokens)
        {
            return Some(ComposerListEdit {
                range: line.start..renumbered_end,
                replacement: format!("{replacement}{}{text}", &value[end..line.end]),
                cursor_after,
            });
        }
        return Some(ComposerListEdit { range: line.start..end, replacement, cursor_after });
    }
    let insertion = format!("\n{}", next_marker_text(&parsed.marker));
    let cursor_after = cursor + insertion.len();
    if let Some((renumbered_end, text)) = renumber_following_items(value, line.end, &parsed.marker, &tokens) {
        return Some(ComposerListEdit {
            range: cursor..renumbered_end,
            replacement: format!("{insertion}{}{text}", &value[cursor..line.end]),
            cursor_after,
        });
    }
    Some(ComposerListEdit { range: cursor..cursor, replacement: insertion, cursor_after })
}

/// Tab on a list item: nest it under the item above. Ports `listIndentForTab`.
pub fn list_indent_for_tab(value: &str, selection: Range<usize>) -> Option<ComposerListEdit> {
    let start = selection.start;
    if start != selection.end || start > value.len() {
        return None;
    }
    let line = current_line(value, start);
    let parsed = parse_list_marker(&value[line.clone()])?;
    let tokens = inline_token_ranges(value);
    if inside_token(&tokens, start) {
        return None;
    }
    let marker = &parsed.marker;
    let width = indent_width(marker.indent(), 0);
    let mut previous_child: Option<Marker> = None;
    for above in list_markers_above(value, line.start, &tokens) {
        let above_width = indent_width(above.marker.indent(), 0);
        if above_width < width {
            break;
        }
        if above_width > width {
            if previous_child.as_ref().is_none_or(|p| above_width < indent_width(p.indent(), 0)) {
                previous_child = Some(above.marker);
            }
            continue;
        }
        let indent = previous_child.as_ref().map_or_else(
            || format!("{}{}", above.marker.indent(), " ".repeat(above.content_column - above_width)),
            |p| p.indent().to_owned(),
        );
        let (replaced_len, number) = nested_number(marker, previous_child.as_ref());
        let replaced_end = line.start + marker.indent().len() + replaced_len;
        let replacement = format!("{indent}{number}");
        let cursor_after = if start < replaced_end {
            line.start + indent.len()
        } else {
            start - replaced_end + line.start + replacement.len()
        };
        return Some(ComposerListEdit { range: line.start..replaced_end, replacement, cursor_after });
    }
    Some(ComposerListEdit { range: line.start..line.start, replacement: "  ".to_owned(), cursor_after: start + 2 })
}

/// Shift+Tab on a nested list item: move it out one level. Ports `listOutdentForTab`.
pub fn list_outdent_for_tab(value: &str, selection: Range<usize>) -> Option<ComposerListEdit> {
    let start = selection.start;
    if start != selection.end || start > value.len() {
        return None;
    }
    let line = current_line(value, start);
    let parsed = parse_list_marker(&value[line.clone()])?;
    if parsed.marker.indent().is_empty() {
        return None;
    }
    let tokens = inline_token_ranges(value);
    if inside_token(&tokens, start) {
        return None;
    }
    let marker = &parsed.marker;
    let parent = parent_marker(value, line.start, marker, &tokens);
    let indent = match &parent {
        Some(parent) => parent.indent().to_owned(),
        None => {
            let current = marker.indent();
            let cut = if current.starts_with('\t') {
                1
            } else {
                current.chars().take(2).take_while(|c| *c == ' ').count()
            };
            current[cut..].to_owned()
        }
    };
    let continues = matches!(
        (&parent, marker),
        (Some(Marker::Ordered { delimiter: a, .. }), Marker::Ordered { delimiter: b, .. }) if a == b
    );
    let (replaced_len, number) = if continues { nested_number(marker, parent.as_ref()) } else { (0, String::new()) };
    let replaced_end = line.start + marker.indent().len() + replaced_len;
    let replacement = format!("{indent}{number}");
    let cursor_after = if start < replaced_end {
        line.start + indent.len()
    } else {
        start - replaced_end + line.start + replacement.len()
    };
    if continues
        && let Some(parent) = &parent
        && let Some((renumbered_end, text)) = renumber_following_items(value, line.end, parent, &tokens)
    {
        return Some(ComposerListEdit {
            range: line.start..renumbered_end,
            replacement: format!("{replacement}{}{text}", &value[replaced_end..line.end]),
            cursor_after,
        });
    }
    Some(ComposerListEdit { range: line.start..replaced_end, replacement, cursor_after })
}
