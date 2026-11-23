//! Incremental syntax highlighting for the file editor. The shared
//! [`SyntaxStore`](crate::syntax::SyntaxStore) keys documents by content, so
//! every keystroke would parse the file from the top. Here the parser state is
//! kept at every [`CHECKPOINT`]th line: an edit re-parses from the checkpoint
//! above it, only as far as the viewport needs, and stops early when the
//! parser state at a later checkpoint matches what it was before the edit.

use std::{collections::HashMap, time::Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp};

use crate::syntax::{LanguageId, SyntaxToken, TokenSpan};

/// Lines between stored parser states.
const CHECKPOINT: usize = 32;
/// Longer lines stay plain, as the shared highlighter leaves them.
const MAX_LINE_BYTES: usize = 2_000;

#[derive(Clone, PartialEq, Eq)]
struct State {
    parser: ParseState,
    scopes: ScopeStack,
}

/// Highlight spans for an editable document.
pub struct EditorHighlighter {
    language: LanguageId,
    spans: Vec<Box<[TokenSpan]>>,
    /// Parser state at the start of line `i * CHECKPOINT`.
    checkpoints: Vec<State>,
    /// Lines `< valid` have spans that match the text.
    valid: usize,
    /// Checkpoints from before the last edit that may still match, for resyncing.
    stale_from: usize,
    stale_valid: usize,
    tokens: HashMap<Scope, Option<SyntaxToken>>,
}

impl EditorHighlighter {
    pub fn new(language: LanguageId) -> Self {
        let start = State { parser: ParseState::new(language.syntax()), scopes: ScopeStack::new() };
        Self {
            language,
            spans: Vec::new(),
            checkpoints: vec![start],
            valid: 0,
            stale_from: usize::MAX,
            stale_valid: 0,
            tokens: HashMap::new(),
        }
    }

    pub fn language(&self) -> LanguageId {
        self.language
    }

    /// Spans for a line, when it has been parsed since its last change.
    pub fn line(&self, index: usize) -> Option<&[TokenSpan]> {
        (index < self.valid).then(|| &*self.spans[index])
    }

    /// Records that lines `start..start + removed` became `inserted` lines.
    pub fn edited(&mut self, start: usize, removed: usize, inserted: usize) {
        let checkpoint = start / CHECKPOINT;
        let old_valid = self.valid;
        self.valid = self.valid.min(checkpoint * CHECKPOINT);
        if removed == inserted {
            // Same line count: later checkpoints keep their lines and may still match.
            self.stale_from = checkpoint + 1;
            self.stale_valid = old_valid;
        } else {
            self.checkpoints.truncate(checkpoint + 1);
            self.stale_from = usize::MAX;
            let keep = (checkpoint * CHECKPOINT).min(self.spans.len());
            self.spans.truncate(keep);
        }
    }

    /// Parses until line `through` has spans or `budget_ms` passes. Returns
    /// true when `through` is covered.
    pub fn ensure(&mut self, lines: &[String], through: usize, budget_ms: f32) -> bool {
        let through = through.min(lines.len().saturating_sub(1));
        if lines.is_empty() || self.valid > through {
            return true;
        }
        let started = Instant::now();
        let start_checkpoint = (self.valid / CHECKPOINT).min(self.checkpoints.len() - 1);
        let mut line = start_checkpoint * CHECKPOINT;
        let mut state = self.checkpoints[start_checkpoint].clone();
        let mut buffer = String::new();
        while line <= through {
            if line % CHECKPOINT == 0 && line > 0 {
                let index = line / CHECKPOINT;
                if index >= self.stale_from && index < self.checkpoints.len() && self.checkpoints[index] == state {
                    // Same state as before the edit: everything up to the old valid line still holds.
                    self.valid = self.stale_valid.max(line);
                    self.stale_from = usize::MAX;
                    if self.valid > through {
                        return true;
                    }
                    line = self.valid - self.valid % CHECKPOINT;
                    state = self.checkpoints[line / CHECKPOINT].clone();
                    if line > through {
                        return true;
                    }
                    continue;
                }
                if index < self.checkpoints.len() {
                    self.checkpoints[index] = state.clone();
                } else {
                    self.checkpoints.push(state.clone());
                }
            }
            let spans = parse_line(&lines[line], &mut state, &mut buffer, &mut self.tokens);
            if line < self.spans.len() {
                self.spans[line] = spans;
            } else {
                self.spans.push(spans);
            }
            line += 1;
            self.valid = self.valid.max(line);
            if line % 64 == 0 && started.elapsed().as_secs_f32() * 1000. > budget_ms {
                return line > through;
            }
        }
        true
    }

    /// Bytes held, for memory reports (spans only; parser states are small).
    pub fn heap_size(&self) -> usize {
        self.spans.iter().map(|s| s.len() * std::mem::size_of::<TokenSpan>() + 16).sum()
    }
}

fn parse_line(
    text: &str,
    state: &mut State,
    buffer: &mut String,
    tokens: &mut HashMap<Scope, Option<SyntaxToken>>,
) -> Box<[TokenSpan]> {
    if text.len() > MAX_LINE_BYTES {
        return Box::default();
    }
    buffer.clear();
    buffer.push_str(text);
    buffer.push('\n');
    let Ok(ops) = state.parser.parse_line(buffer, &crate::syntax::SYNTAXES) else {
        return Box::default();
    };
    let mut spans: Vec<TokenSpan> = Vec::new();
    let mut cursor = 0usize;
    let mut push = |end: usize, scopes: &ScopeStack, spans: &mut Vec<TokenSpan>| {
        let end = end.min(text.len());
        if end <= cursor {
            return;
        }
        let token = scopes
            .scopes
            .iter()
            .rev()
            .find_map(|scope| *tokens.entry(*scope).or_insert_with(|| crate::syntax::scope_token(*scope)));
        if let Some(token) = token {
            match spans.last_mut() {
                Some(last) if last.end as usize == cursor && last.token == token => last.end = end as u32,
                _ => spans.push(TokenSpan { start: cursor as u32, end: end as u32, token }),
            }
        }
        cursor = end;
    };
    for (offset, op) in ops {
        push(offset, &state.scopes, &mut spans);
        if !matches!(op, ScopeStackOp::Noop) {
            state.scopes.apply(&op).ok();
        }
    }
    push(text.len(), &state.scopes, &mut spans);
    spans.into_boxed_slice()
}
