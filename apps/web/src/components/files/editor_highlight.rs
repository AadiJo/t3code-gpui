//! Syntax highlighting for the file editor. The shared
//! [`SyntaxStore`](crate::syntax::SyntaxStore) keys documents by content, so
//! here the editor's own document is re-lexed after each edit instead. The
//! web's lexers take a few microseconds a line, so a file up to
//! [`SYNC_LINES`] lines is lexed in the frame; a longer one is lexed on a
//! worker thread while the previous colours stay on screen.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use crate::syntax::{LanguageId, Lexer, TokenSpan};

pub use crate::syntax::highlight_line;

/// Files up to this many lines are re-lexed in the frame (~7ms at worst).
const SYNC_LINES: usize = 2_000;

thread_local! {
    /// The UI thread's lexer for in-frame jobs.
    static LEXER: std::cell::RefCell<Option<Lexer>> = const { std::cell::RefCell::new(None) };
}

/// Highlight spans for an editable document.
pub struct EditorHighlighter {
    language: LanguageId,
    spans: Vec<Box<[TokenSpan]>>,
    /// True when `spans` predate the latest edit.
    stale: bool,
    /// Bumped on every edit; a worker result for an older edit is dropped.
    generation: u64,
    job: Option<(u64, Receiver<Vec<Box<[TokenSpan]>>>)>,
}

impl EditorHighlighter {
    pub fn new(language: LanguageId) -> Self {
        Self { language, spans: Vec::new(), stale: true, generation: 0, job: None }
    }

    pub fn language(&self) -> LanguageId {
        self.language
    }

    /// Spans for a line. During a re-lex these are the previous colours.
    pub fn line(&self, index: usize) -> Option<&[TokenSpan]> {
        self.spans.get(index).map(|spans| &**spans)
    }

    /// Records that lines `start..start + removed` became `inserted` lines.
    pub fn edited(&mut self, start: usize, removed: usize, inserted: usize) {
        let start = start.min(self.spans.len());
        let end = (start + removed).min(self.spans.len());
        self.spans.splice(start..end, std::iter::repeat_with(Box::default).take(inserted));
        self.stale = true;
        self.generation += 1;
    }

    /// Brings the spans up to date with `lines`. Returns false while a worker
    /// is still lexing, so the caller draws again next frame.
    pub fn ensure(&mut self, lines: &[String], _through: usize, _budget_ms: f32) -> bool {
        if let Some((generation, receiver)) = &self.job {
            match receiver.try_recv() {
                Ok(spans) => {
                    if *generation == self.generation {
                        self.spans = spans;
                        self.stale = false;
                    }
                    self.job = None;
                }
                Err(TryRecvError::Empty) => return false,
                Err(TryRecvError::Disconnected) => self.job = None,
            }
        }
        if !self.stale {
            return true;
        }
        let language = self.language;
        if lines.len() <= SYNC_LINES {
            let spans = LEXER.with_borrow_mut(|lexer| {
                if lexer.is_none() {
                    *lexer = Lexer::new();
                }
                lexer.as_mut()?.lex(language, lines.iter().map(String::as_str))
            });
            self.spans = spans.unwrap_or_else(|| vec![Box::default(); lines.len()]);
            self.stale = false;
            return true;
        }
        let (sender, receiver) = channel();
        let text = lines.to_vec();
        std::thread::spawn(move || {
            if let Some(spans) = Lexer::new().and_then(|mut lexer| lexer.lex(language, text.iter().map(String::as_str))) {
                sender.send(spans).ok();
            }
        });
        self.job = Some((self.generation, receiver));
        false
    }

    /// Bytes held, for memory reports.
    pub fn heap_size(&self) -> usize {
        self.spans.iter().map(|s| s.len() * std::mem::size_of::<TokenSpan>() + 16).sum()
    }
}
