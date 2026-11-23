//! Composer triggers, cursor coordinates and the send key. Ports `composer-logic.ts`
//! and the trigger helpers of `packages/shared/src/composerTrigger.ts`.
//!
//! Offsets are byte offsets into the Markdown source ("expanded" coordinates).
//! "Collapsed" coordinates count each chip as one unit, as the web store does.

use std::ops::Range;

use crate::composer_editor_mentions::{ComposerPromptSegment, is_currency_symbol, split_prompt_into_segments};

/// What a menu trigger before the caret is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComposerTriggerKind {
    /// `@query`: files, folders, threads, models, environments.
    Path,
    /// `#query`: pull requests.
    PullRequest,
    /// `/query`: commands and skills.
    SlashCommand,
    /// `$query`: skills.
    Skill,
}

/// A trigger token ending at the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerTrigger {
    pub kind: ComposerTriggerKind,
    pub query: String,
    pub range: Range<usize>,
}

/// The user's send key setting (`sendShortcut`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SendShortcut {
    /// Enter sends, Shift+Enter adds a line. The owner's setting.
    #[default]
    Enter,
    /// Cmd/Ctrl+Enter sends.
    ModEnter,
    /// Enter sends a single line; once the draft has a newline, Cmd/Ctrl+Enter sends.
    ModEnterMultiline,
}

/// How a send should be dispatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerSubmissionIntent {
    Foreground,
    Background,
    /// The opposite of the follow-up default while a turn runs (queue ↔ steer).
    Alternate,
}

/// The parts of a key event the send decision reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComposerKeyEvent {
    pub enter: bool,
    pub shift: bool,
    pub alt: bool,
    /// Cmd on macOS, Ctrl elsewhere.
    pub modifier: bool,
    pub is_composing: bool,
    pub repeat: bool,
}

/// Context the send decision depends on.
#[derive(Clone, Copy, Debug, Default)]
pub struct ComposerSubmitContext {
    pub is_draft_thread: bool,
    pub is_running: bool,
    pub software_keyboard_open: bool,
    pub send_shortcut: SendShortcut,
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\n' | '\t' | '\r')
}

fn clamp_cursor(text: &str, cursor: usize) -> usize {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    cursor
}

/// Whether the send key needs Cmd/Ctrl for this prompt. Ports `composerRequiresModifier`.
pub fn composer_requires_modifier(send_shortcut: SendShortcut, prompt: &str) -> bool {
    match send_shortcut {
        SendShortcut::Enter => false,
        SendShortcut::ModEnter => true,
        SendShortcut::ModEnterMultiline => prompt.contains(['\r', '\n']),
    }
}

/// Resolves a key press to a send, or `None` for an ordinary key. IME composition
/// and key repeat never send. Ports `composerSubmissionIntentForKey` with the
/// default composer keybindings (`packages/shared/src/keybindings.ts:94-111`).
pub fn composer_submission_intent_for_key(
    event: ComposerKeyEvent,
    context: ComposerSubmitContext,
    prompt: &str,
) -> Option<ComposerSubmissionIntent> {
    if event.is_composing || event.repeat || !event.enter {
        return None;
    }
    if context.software_keyboard_open && !event.modifier {
        return None;
    }
    if event.modifier && !event.shift {
        if event.alt {
            return Some(ComposerSubmissionIntent::Background);
        }
        if context.is_running {
            return Some(ComposerSubmissionIntent::Alternate);
        }
        if context.is_draft_thread {
            return Some(ComposerSubmissionIntent::Background);
        }
    }
    if event.shift || event.alt {
        return None;
    }
    if composer_requires_modifier(context.send_shortcut, prompt) && !event.modifier {
        return None;
    }
    Some(ComposerSubmissionIntent::Foreground)
}

/// A second slash or backslash makes the whole word a path. Ports `isPathWord`.
fn is_path_word(text: &str, word_start: usize, cursor: usize) -> bool {
    let word_end = text[cursor..].find(is_ws).map_or(text.len(), |i| cursor + i);
    let after_first = text[word_start..].chars().next().map_or(word_start, |c| word_start + c.len_utf8());
    text[after_first.min(word_end)..word_end].contains(['/', '\\'])
}

/// `$5`, `€9.99`, `$10k.`: money, not skills. Ports `isCurrencyAmountQuery`.
pub fn is_currency_amount_query(query: &str) -> bool {
    let body = query.strip_prefix('.').unwrap_or(query);
    let mut chars = body.char_indices().peekable();
    if !chars.next().is_some_and(|(_, c)| c.is_ascii_digit()) {
        return false;
    }
    let mut rest = "";
    for (i, c) in chars {
        if !(c.is_ascii_digit() || matches!(c, ',' | '.' | '_')) {
            rest = &body[i..];
            break;
        }
    }
    let rest = if let Some(r) = rest.strip_prefix(|c: char| "kKmMbBtT".contains(c)) {
        r
    } else if let Some(r) = rest.strip_prefix(['e', 'E']) {
        let digits = r.len() - r.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 { rest } else { &r[digits..] }
    } else {
        rest
    };
    rest.chars().all(|c| ".,;:!?)".contains(c))
}

fn is_open_punctuation(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '«' | '‹' | '「' | '『' | '（' | '［' | '｛' | '〈' | '《' | '【')
}

/// `#123` or `(#feat` at the end of the token. Ports `detectComposerPullRequestTrigger`.
fn detect_pull_request_trigger(token: &str, cursor: usize) -> Option<ComposerTrigger> {
    if token.starts_with('@') || token.chars().next().is_some_and(is_currency_symbol) {
        return None;
    }
    let hash = token.rfind('#')?;
    let query = &token[hash + 1..];
    let before_ok = hash == 0 || token[..hash].chars().next_back().is_some_and(is_open_punctuation);
    let mut chars = query.chars();
    let query_ok = match chars.next() {
        None => true,
        Some(first) => first.is_alphanumeric() && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '-'),
    };
    (before_ok && query_ok).then(|| ComposerTrigger {
        kind: ComposerTriggerKind::PullRequest,
        query: query.to_owned(),
        range: cursor - query.len() - 1..cursor,
    })
}

/// The trigger token ending at `cursor`, if any. Ports `detectComposerTrigger`.
pub fn detect_composer_trigger(text: &str, cursor: usize) -> Option<ComposerTrigger> {
    let cursor = clamp_cursor(text, cursor);
    let token_start = text[..cursor].rfind(is_ws).map_or(0, |i| i + 1);
    let token = &text[token_start..cursor];
    let trigger = |kind, prefix_len: usize| ComposerTrigger {
        kind,
        query: token[prefix_len..].to_owned(),
        range: token_start..cursor,
    };
    if token.starts_with('/') {
        if is_path_word(text, token_start, cursor) {
            return None;
        }
        return Some(trigger(ComposerTriggerKind::SlashCommand, 1));
    }
    if let Some(pr) = detect_pull_request_trigger(token, cursor) {
        return Some(pr);
    }
    if let Some(symbol) = token.chars().next().filter(|c| is_currency_symbol(*c)) {
        if is_currency_amount_query(&token[symbol.len_utf8()..]) {
            return None;
        }
        return Some(trigger(ComposerTriggerKind::Skill, symbol.len_utf8()));
    }
    token.starts_with('@').then(|| trigger(ComposerTriggerKind::Path, 1))
}

/// Replaces `range` with `replacement` and returns the new text and caret. Ports `replaceTextRange`.
pub fn replace_text_range(text: &str, range: Range<usize>, replacement: &str) -> (String, usize) {
    let start = clamp_cursor(text, range.start);
    let end = clamp_cursor(text, range.end.max(start));
    let mut next = String::with_capacity(text.len() + replacement.len());
    next.push_str(&text[..start]);
    next.push_str(replacement);
    next.push_str(&text[end..]);
    (next, start + replacement.len())
}

/// A replacement ending in a space swallows one space after the range. Ports
/// `extendReplacementRangeForTrailingSpace`.
pub fn extend_replacement_range_for_trailing_space(text: &str, range_end: usize, replacement: &str) -> usize {
    if replacement.ends_with(' ') && text.as_bytes().get(range_end) == Some(&b' ') {
        range_end + 1
    } else {
        range_end
    }
}

/// How much of a paste continues an `@` query already typed. Ports `pastedPathQueryLength`.
pub fn pasted_path_query_length(line_before: &str, pasted: &str) -> usize {
    match detect_composer_trigger(line_before, line_before.len()) {
        Some(ComposerTrigger { kind: ComposerTriggerKind::Path, .. }) => {
            pasted.find(char::is_whitespace).unwrap_or(pasted.len())
        }
        _ => 0,
    }
}

fn collapsed_len(segment: &ComposerPromptSegment<'_>) -> usize {
    match segment {
        ComposerPromptSegment::Text(text) => text.len(),
        ComposerPromptSegment::Chip(_) => 1,
    }
}

/// Converts a collapsed cursor (chips count as one) to a source offset.
/// Ports `expandCollapsedComposerCursor`.
pub fn expand_collapsed_cursor(text: &str, collapsed: usize) -> usize {
    let mut remaining = collapsed;
    let mut expanded = 0;
    for segment in split_prompt_into_segments(text) {
        match &segment {
            ComposerPromptSegment::Chip(token) => {
                let len = token.range.len();
                if remaining <= 1 {
                    return expanded + if remaining == 0 { 0 } else { len };
                }
                remaining -= 1;
                expanded += len;
            }
            ComposerPromptSegment::Text(run) => {
                if remaining <= run.len() {
                    return expanded + remaining;
                }
                remaining -= run.len();
                expanded += run.len();
            }
        }
    }
    expanded
}

/// Converts a source offset to a collapsed cursor; offsets inside a chip land after it.
/// Ports `collapseExpandedComposerCursor`.
pub fn collapse_expanded_cursor(text: &str, expanded: usize) -> usize {
    let mut remaining = clamp_cursor(text, expanded);
    let mut collapsed = 0;
    for segment in split_prompt_into_segments(text) {
        match &segment {
            ComposerPromptSegment::Chip(token) => {
                if remaining == 0 {
                    return collapsed;
                }
                if remaining <= token.range.len() {
                    return collapsed + 1;
                }
                remaining -= token.range.len();
                collapsed += 1;
            }
            ComposerPromptSegment::Text(run) => {
                if remaining <= run.len() {
                    return collapsed + remaining;
                }
                remaining -= run.len();
                collapsed += run.len();
            }
        }
    }
    collapsed
}

/// The length of a prompt in collapsed units. Ports `clampCollapsedComposerCursor`'s total.
pub fn collapsed_length(text: &str) -> usize {
    split_prompt_into_segments(text).iter().map(collapsed_len).sum()
}

/// `/clear` as the whole message arms a fresh session instead of sending.
pub fn is_clear_command(text: &str) -> bool {
    let t = text.trim();
    t.len() >= 6 && t[..6].eq_ignore_ascii_case("/clear") && t[6..].trim().is_empty()
}
