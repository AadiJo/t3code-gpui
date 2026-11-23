//! Editing inside fenced code blocks: Enter keeps indentation, two blank lines
//! or a closing fence exit, Tab and Shift+Tab indent lines, and Enter after an
//! opening fence opens a closed block. Ports `composer-code-block.ts` onto the
//! Markdown source, where the fences are ordinary lines.

use std::ops::Range;

use crate::composer_rich_text_doc::{CodeBlock, is_closing_fence, parse_opening_fence};

/// The two-space step Tab adds inside code.
pub const CODE_BLOCK_INDENT: &str = "  ";

/// A source edit and the selection after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlockEdit {
    pub range: Range<usize>,
    pub replacement: String,
    pub selection_after: Range<usize>,
}

/// Byte range of the code between the fences (excluding the fence lines' newlines).
pub fn code_body_range(text: &str, line_starts: &[usize], block: &CodeBlock) -> Range<usize> {
    let line_end = |line: usize| line_starts.get(line + 1).map_or(text.len(), |next| next - 1);
    let open_end = line_end(block.lines.start);
    let body_start = (open_end + 1).min(text.len());
    let body_end = if block.closed {
        line_starts[block.lines.end - 1].saturating_sub(1).max(body_start)
    } else {
        text.len()
    };
    body_start..body_end.max(body_start)
}

fn leading_whitespace(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Enter in a code line that starts indented: a newline at the same indent.
/// Ports `indentedNewlineInCodeBlock`.
pub fn indented_newline(text: &str, body: Range<usize>, selection: Range<usize>) -> Option<CodeBlockEdit> {
    if selection.start < body.start || selection.end > body.end {
        return None;
    }
    let line_start = text[body.start..selection.start].rfind('\n').map_or(body.start, |i| body.start + i + 1);
    let indent = leading_whitespace(&text[line_start..selection.start]);
    if indent.is_empty() {
        return None;
    }
    let inserted = format!("\n{indent}");
    let caret = selection.start + inserted.len();
    Some(CodeBlockEdit { range: selection, replacement: inserted, selection_after: caret..caret })
}

/// Enter at the end of a block whose last two lines are blank leaves the block,
/// closing its fence. Ports `exitCodeBlockOnTrailingBlankLines`.
pub fn exit_on_trailing_blank_lines(text: &str, body: Range<usize>, block: &CodeBlock, caret: usize) -> Option<CodeBlockEdit> {
    if caret != body.end {
        return None;
    }
    // /\n[ \t]*\n[ \t]*$/ on the block's text.
    let content = &text[body.clone()];
    let last = content.trim_end_matches([' ', '\t']).strip_suffix('\n')?;
    let before = last.trim_end_matches([' ', '\t']).strip_suffix('\n')?;
    let cut = body.start + before.len();
    let (end, fence_line) = if block.closed {
        let fence_start = body.end + 1;
        let fence_end = text[fence_start..].find('\n').map_or(text.len(), |i| fence_start + i);
        (fence_end, text[fence_start..fence_end].to_owned())
    } else {
        (text.len(), block.fence.clone())
    };
    let replacement = format!("\n{fence_line}\n");
    let caret = cut + replacement.len();
    Some(CodeBlockEdit { range: cut..end, replacement, selection_after: caret..caret })
}

/// Enter at the end of a top-level line that is an opening fence opens a closed
/// block with the caret inside. Ports `convertCodeFenceOnEnter`.
pub fn convert_fence_on_enter(text: &str, line: Range<usize>, caret: usize) -> Option<CodeBlockEdit> {
    if caret != line.end {
        return None;
    }
    let (fence, _) = parse_opening_fence(&text[line.clone()])?;
    let rest = &text[line.end..];
    // An existing closing fence below already makes this a closed block.
    if rest.lines().skip(1).any(|l| is_closing_fence(l, &fence)) {
        return None;
    }
    let replacement = format!("\n\n{fence}");
    let caret = line.end + 1;
    Some(CodeBlockEdit { range: line.end..line.end, replacement, selection_after: caret..caret })
}

fn outdent_line(line: &str) -> &str {
    line.strip_prefix(CODE_BLOCK_INDENT)
        .or_else(|| line.strip_prefix('\t'))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line)
}

/// Tab or Shift+Tab with the selection in one block: a collapsed Tab inserts two
/// spaces, otherwise every touched line moves. Ports `indentCodeBlock`.
pub fn indent_code_lines(text: &str, body: Range<usize>, selection: Range<usize>, indent: bool) -> Option<CodeBlockEdit> {
    if selection.start < body.start || selection.end > body.end {
        return None;
    }
    if selection.is_empty() && indent {
        let caret = selection.start + CODE_BLOCK_INDENT.len();
        return Some(CodeBlockEdit { range: selection, replacement: CODE_BLOCK_INDENT.to_owned(), selection_after: caret..caret });
    }
    let line_start = text[body.start..selection.start].rfind('\n').map_or(body.start, |i| body.start + i + 1);
    let probe = selection.start.max(selection.end.saturating_sub(1)).max(body.start);
    let line_end = text[probe..body.end].find('\n').map_or(body.end, |i| probe + i);
    let original = &text[line_start..line_end];
    let next: Vec<String> = original
        .split('\n')
        .map(|line| match (indent, line.is_empty()) {
            (true, true) => String::new(),
            (true, false) => format!("{CODE_BLOCK_INDENT}{line}"),
            (false, _) => outdent_line(line).to_owned(),
        })
        .collect();
    let next = next.join("\n");
    if next == original {
        return Some(CodeBlockEdit { range: selection.clone(), replacement: text[selection.clone()].to_owned(), selection_after: selection });
    }
    let first_original = original.split('\n').next().map_or(0, str::len) as isize;
    let first_next = next.split('\n').next().map_or(0, str::len) as isize;
    let first_delta = first_next - first_original;
    let total_delta = next.len() as isize - original.len() as isize;
    let from = (selection.start as isize + first_delta).max(line_start as isize) as usize;
    let to = ((selection.end as isize + total_delta) as usize).max(from);
    Some(CodeBlockEdit { range: line_start..line_end, replacement: next, selection_after: from..to })
}
