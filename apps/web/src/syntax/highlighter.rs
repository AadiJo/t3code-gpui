//! Background syntax highlighting with a shared, per-version cache.
//!
//! A document is a [`TextLines`] plus a language, keyed by its content hash,
//! so a file viewed in the diff panel and in the file viewer is lexed once.
//! The first request lexes the whole document on the background executor
//! with the web's own lexers ([`Lexer`]), as the web's highlight worker does;
//! a 2,000-line file takes a few milliseconds. Observers of the store repaint
//! when it lands.

use std::{collections::HashMap, time::Instant};

use gpui_kit::{App, AppContext as _, Context, Entity, Global};

use super::{LanguageId, SyntaxToken, TextLines, lexer::Lexer};

/// Cached highlighted lines across all documents before old documents are dropped.
const LINE_BUDGET: usize = 600_000;

/// One coloured range of a line, in bytes.
#[derive(Clone, Copy, Debug)]
pub struct TokenSpan {
    pub start: u32,
    pub end: u32,
    pub token: SyntaxToken,
}

/// A document's identity: its content and language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocKey {
    pub content: u64,
    pub language: LanguageId,
}

impl DocKey {
    pub fn new(lines: &TextLines, language: LanguageId) -> Self {
        Self { content: lines.content_hash(), language }
    }
}

struct DocState {
    source: TextLines,
    lines: Vec<Box<[TokenSpan]>>,
    /// True while the background lex runs.
    lexing: bool,
    last_used: u64,
}

/// Timing of the most recent highlight work, for performance reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct HighlightStats {
    pub lines_parsed: usize,
    pub busy_micros: u128,
}

/// The app-wide highlight cache. Views observe it to repaint as chunks land.
pub struct SyntaxStore {
    docs: HashMap<DocKey, DocState>,
    clock: u64,
    pub stats: HighlightStats,
}

struct GlobalSyntaxStore(Entity<SyntaxStore>);

impl Global for GlobalSyntaxStore {}

impl SyntaxStore {
    /// The shared store, created on first use.
    pub fn global(cx: &mut App) -> Entity<SyntaxStore> {
        if let Some(store) = cx.try_global::<GlobalSyntaxStore>() {
            return store.0.clone();
        }
        let store = cx.new(|_| SyntaxStore { docs: HashMap::new(), clock: 0, stats: Default::default() });
        cx.set_global(GlobalSyntaxStore(store.clone()));
        store
    }

    /// Spans for one line, if that line has been highlighted.
    pub fn line(&self, key: DocKey, line: usize) -> Option<&[TokenSpan]> {
        self.docs.get(&key)?.lines.get(line).map(|spans| &**spans)
    }

    /// Asks for a document's colours, starting the background lex on first use.
    /// `through` is kept for callers; the whole document is lexed at once.
    pub fn request(&mut self, key: DocKey, source: &TextLines, _through: usize, cx: &mut Context<Self>) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(doc) = self.docs.get_mut(&key) {
            doc.last_used = clock;
            return;
        }
        self.docs.insert(key, DocState { source: source.clone(), lines: Vec::new(), lexing: true, last_used: clock });
        let source = source.clone();
        let work = cx.background_spawn(async move {
            let started = Instant::now();
            let spans = Lexer::new()
                .and_then(|mut lexer| lexer.lex(key.language, (0..source.len()).map(|index| source.line(index))))
                .unwrap_or_else(|| vec![Box::default(); source.len()]);
            (spans, started.elapsed().as_micros())
        });
        cx.spawn(async move |this, cx| {
            let (spans, micros) = work.await;
            this.update(cx, |store, cx| {
                store.stats.lines_parsed += spans.len();
                store.stats.busy_micros += micros;
                if let Some(doc) = store.docs.get_mut(&key) {
                    doc.lines = spans;
                    doc.lexing = false;
                }
                cx.notify();
                store.evict();
            })
            .ok();
        })
        .detach();
    }

    /// True when every line of the document up to `through` has colours.
    pub fn is_ready(&self, key: DocKey, through: usize) -> bool {
        self.docs.get(&key).is_some_and(|doc| doc.lines.len() > through.min(doc.source.len().saturating_sub(1)))
    }

    fn evict(&mut self) {
        let mut total: usize = self.docs.values().map(|doc| doc.lines.len()).sum();
        while total > LINE_BUDGET {
            let Some((&oldest, _)) = self
                .docs
                .iter()
                .filter(|(_, doc)| !doc.lexing)
                .min_by_key(|(_, doc)| doc.last_used)
            else {
                return;
            };
            total -= self.docs.remove(&oldest).map_or(0, |doc| doc.lines.len());
        }
    }
}
