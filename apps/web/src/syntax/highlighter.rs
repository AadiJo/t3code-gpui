//! Background syntax highlighting with a shared, per-version cache.
//!
//! A document is a [`TextLines`] plus a language, keyed by its content hash,
//! so a file viewed in the diff panel and in the file viewer is parsed once.
//! Syntect is stateful line to line, so a document is parsed from the top in
//! chunks on the background executor, only as far as some view has asked for
//! (its visible rows plus a lookahead). Each finished chunk notifies observers
//! of the store, which repaint the rows that now have colours.

use std::{collections::HashMap, time::Instant};

use gpui_kit::{App, AppContext as _, Context, Entity, Global};
use syntect::parsing::{ParseState, Scope, ScopeStack, ScopeStackOp};

use super::{LanguageId, SyntaxToken, TextLines, language::SYNTAXES, language::scope_token};

/// Lines parsed per background step.
const CHUNK_LINES: usize = 400;
/// Lines past the last requested row that are parsed ahead of time.
const LOOKAHEAD_LINES: usize = 400;
/// Longer lines are left plain, as `DEFAULT_TOKENIZE_MAX_LENGTH` does on the web;
/// regex grammars backtrack badly on minified code.
const MAX_LINE_BYTES: usize = 2_000;
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

struct Resume {
    parser: ParseState,
    scopes: ScopeStack,
}

struct DocState {
    source: TextLines,
    lines: Vec<Box<[TokenSpan]>>,
    /// Parser state after `lines`; `None` while a background step holds it.
    resume: Option<Resume>,
    wanted: usize,
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

    /// Asks for lines up to `through` of a document, starting background work if needed.
    pub fn request(&mut self, key: DocKey, source: &TextLines, through: usize, cx: &mut Context<Self>) {
        self.clock += 1;
        let clock = self.clock;
        let doc = self.docs.entry(key).or_insert_with(|| DocState {
            source: source.clone(),
            lines: Vec::new(),
            resume: Some(Resume { parser: ParseState::new(key.language.syntax()), scopes: ScopeStack::new() }),
            wanted: 0,
            last_used: clock,
        });
        doc.last_used = clock;
        let wanted = (through + LOOKAHEAD_LINES).min(doc.source.len());
        if wanted <= doc.wanted {
            return;
        }
        doc.wanted = wanted;
        if doc.resume.is_some() {
            self.step(key, cx);
        }
    }

    /// True when every line of the document up to `through` has colours.
    pub fn is_ready(&self, key: DocKey, through: usize) -> bool {
        self.docs.get(&key).is_some_and(|doc| doc.lines.len() > through.min(doc.source.len().saturating_sub(1)))
    }

    fn step(&mut self, key: DocKey, cx: &mut Context<Self>) {
        let Some(doc) = self.docs.get_mut(&key) else { return };
        if doc.lines.len() >= doc.wanted {
            return;
        }
        let Some(resume) = doc.resume.take() else { return };
        let source = doc.source.clone();
        let range = doc.lines.len()..(doc.lines.len() + CHUNK_LINES).min(doc.wanted);
        let work = cx.background_spawn(async move {
            let started = Instant::now();
            let mut resume = resume;
            let spans = parse_lines(&source, range, &mut resume);
            (spans, resume, started.elapsed().as_micros())
        });
        cx.spawn(async move |this, cx| {
            let (spans, resume, micros) = work.await;
            this.update(cx, |store, cx| {
                store.stats.lines_parsed += spans.len();
                store.stats.busy_micros += micros;
                if let Some(doc) = store.docs.get_mut(&key) {
                    doc.lines.extend(spans);
                    doc.resume = Some(resume);
                }
                cx.notify();
                store.evict();
                store.step(key, cx);
            })
            .ok();
        })
        .detach();
    }

    fn evict(&mut self) {
        let mut total: usize = self.docs.values().map(|doc| doc.lines.len()).sum();
        while total > LINE_BUDGET {
            let Some((&oldest, _)) = self
                .docs
                .iter()
                .filter(|(_, doc)| doc.resume.is_some())
                .min_by_key(|(_, doc)| doc.last_used)
            else {
                return;
            };
            total -= self.docs.remove(&oldest).map_or(0, |doc| doc.lines.len());
        }
    }
}

fn parse_lines(source: &TextLines, range: std::ops::Range<usize>, resume: &mut Resume) -> Vec<Box<[TokenSpan]>> {
    let mut tokens_by_scope: HashMap<Scope, Option<SyntaxToken>> = HashMap::new();
    let mut buffer = String::new();
    let mut out = Vec::with_capacity(range.len());
    for index in range {
        let text = source.line(index);
        if text.len() > MAX_LINE_BYTES {
            out.push(Box::default());
            continue;
        }
        buffer.clear();
        buffer.push_str(text);
        buffer.push('\n');
        let Ok(ops) = resume.parser.parse_line(&buffer, &SYNTAXES) else {
            out.push(Box::default());
            continue;
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
                .find_map(|scope| *tokens_by_scope.entry(*scope).or_insert_with(|| scope_token(*scope)));
            if let Some(token) = token {
                match spans.last_mut() {
                    Some(last) if last.end as usize == cursor && last.token == token => last.end = end as u32,
                    _ => spans.push(TokenSpan { start: cursor as u32, end: end as u32, token }),
                }
            }
            cursor = end;
        };
        for (offset, op) in ops {
            push(offset, &resume.scopes, &mut spans);
            if !matches!(op, ScopeStackOp::Noop) {
                resume.scopes.apply(&op).ok();
            }
        }
        push(text.len(), &resume.scopes, &mut spans);
        out.push(spans.into_boxed_slice());
    }
    out
}
