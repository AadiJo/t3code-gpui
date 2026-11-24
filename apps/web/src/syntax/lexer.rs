//! The web app's highlighter, `@pierre/highlights`: hand-written lexers compiled
//! to WebAssembly (`highlights.wasm`, Apache-2.0, see `HIGHLIGHTS_LICENSE.md`),
//! run here in the wasmi interpreter. Running the same lexers gives the web's
//! tokens exactly; they also need a few MB where TextMate grammars needed
//! hundreds. Protocol (`dist/highlighter.js`): input UTF-8 at `INPUT_PTR`,
//! a header at address 0, and line records out: `(end, token)` pairs in UTF-16
//! offsets, with `(next line start, u32::MAX)` between lines.

use std::sync::LazyLock;

use wasmi::{Caller, Config, Engine, Extern, Func, Linker, Memory, Module, Store, Val};

use super::{LanguageId, SyntaxToken, TokenSpan};

const WASM: &[u8] = include_bytes!("highlights.wasm");
const INPUT_PTR: usize = 81_920;
const PAGE: usize = 65_536;
/// Output mode 3: UTF-16 line records.
const MODE_LINE_RECORDS: u8 = 3;
/// Longer lines are left plain, as `DEFAULT_TOKENIZE_MAX_LENGTH` does on the web.
pub const MAX_LINE_BYTES: usize = 2_000;

static MODULE: LazyLock<(Engine, Module)> = LazyLock::new(|| {
    let mut config = Config::default();
    config.wasm_simd(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, WASM).expect("highlights.wasm is valid");
    (engine, module)
});

/// `is_id_start` / `is_id_continue` for the UTF-8 code point at `ptr`.
fn id_class(caller: Caller<'_, ()>, ptr: i32, len: i32, start: bool) -> i32 {
    let Some(memory) = caller.get_export("memory").and_then(Extern::into_memory) else { return 0 };
    let data = memory.data(&caller);
    let Some(bytes) = data.get(ptr as usize..ptr as usize + len as usize) else { return 0 };
    let Some(ch) = std::str::from_utf8(bytes).ok().and_then(|s| s.chars().next()) else { return 0 };
    let yes = if start {
        unicode_ident::is_xid_start(ch)
    } else {
        unicode_ident::is_xid_continue(ch) || matches!(ch, '\u{200C}' | '\u{200D}')
    };
    yes as i32
}

/// One lexer instance. Its memory grows to the largest input it has seen, so
/// make one per job and drop it after.
pub struct Lexer {
    store: Store<()>,
    memory: Memory,
    highlight: Func,
}

impl Lexer {
    pub fn new() -> Option<Self> {
        let (engine, module) = &*MODULE;
        let mut store = Store::new(engine, ());
        let mut linker = Linker::<()>::new(engine);
        linker.func_wrap("env", "is_id_start", |c: Caller<'_, ()>, p: i32, l: i32| id_class(c, p, l, true)).ok()?;
        linker.func_wrap("env", "is_id_continue", |c: Caller<'_, ()>, p: i32, l: i32| id_class(c, p, l, false)).ok()?;
        let instance = linker.instantiate_and_start(&mut store, module).ok()?;
        let memory = instance.get_memory(&store, "memory")?;
        let highlight = instance.get_func(&store, "highlight")?;
        Some(Self { store, memory, highlight })
    }

    /// Lexes `lines` as one document. Returns one span list per line, empty for
    /// plain lines; `None` if the lexer failed.
    pub fn lex<'a>(&mut self, language: LanguageId, lines: impl Iterator<Item = &'a str> + Clone) -> Option<Vec<Box<[TokenSpan]>>> {
        let length: usize = lines.clone().map(|line| line.len() + 1).sum::<usize>().saturating_sub(1);
        let needed = (INPUT_PTR + length + 96).div_ceil(PAGE);
        let pages = self.memory.size(&self.store) as usize;
        if needed > pages {
            self.memory.grow(&mut self.store, (needed - pages) as u64).ok()?;
        }
        let data = self.memory.data_mut(&mut self.store);
        let mut at = INPUT_PTR;
        for (index, line) in lines.clone().enumerate() {
            if index > 0 {
                data[at] = b'\n';
                at += 1;
            }
            data[at..at + line.len()].copy_from_slice(line.as_bytes());
            at += line.len();
        }
        data[at] = 0;
        data[0] = language.0;
        data[1] = MODE_LINE_RECORDS;
        data[2..6].copy_from_slice(&(length as u32).to_le_bytes());
        let mut results = vec![Val::I32(0); self.highlight.ty(&self.store).results().len()];
        self.highlight.call(&mut self.store, &[], &mut results).ok()?;
        let data = self.memory.data(&self.store);
        let start = u32::from_le_bytes(data.get(6..10)?.try_into().ok()?) as usize;
        let len = u32::from_le_bytes(data.get(10..14)?.try_into().ok()?) as usize;
        let records = data.get(start..start + len)?;
        Some(spans_from_records(records, lines))
    }
}

/// Converts line records to byte spans per line (`lineRecordsToTokens`).
fn spans_from_records<'a>(records: &[u8], lines: impl Iterator<Item = &'a str>) -> Vec<Box<[TokenSpan]>> {
    let mut records = records.chunks_exact(8).map(|pair| {
        let end = u32::from_le_bytes(pair[0..4].try_into().unwrap_or_default());
        let token = u32::from_le_bytes(pair[4..8].try_into().unwrap_or_default());
        (end as usize, token)
    });
    let mut out = Vec::new();
    // UTF-16 offset of the current line's start in the document.
    let mut base = 0usize;
    for line in lines {
        let mut spans: Vec<TokenSpan> = Vec::new();
        let ascii = line.is_ascii();
        // Walks the line once, mapping UTF-16 offsets to bytes for non-ASCII lines.
        let mut chars = line.char_indices().peekable();
        let mut unit = 0usize;
        let mut byte_at = |target: usize| -> usize {
            if ascii {
                return target.min(line.len());
            }
            while unit < target {
                let Some((_, ch)) = chars.next() else { break };
                unit += ch.len_utf16();
            }
            chars.peek().map_or(line.len(), |(at, _)| *at)
        };
        let mut cursor = 0usize;
        let mut next_base = None;
        for (end, token) in records.by_ref() {
            if token == u32::MAX {
                next_base = Some(end);
                break;
            }
            let end_byte = byte_at(end.saturating_sub(base));
            if end_byte > cursor {
                if let Some(token) = SyntaxToken::from_index(token).filter(|t| *t != SyntaxToken::Plain) {
                    match spans.last_mut() {
                        Some(last) if last.end as usize == cursor && last.token == token => last.end = end_byte as u32,
                        _ => spans.push(TokenSpan { start: cursor as u32, end: end_byte as u32, token }),
                    }
                }
                cursor = end_byte;
            }
        }
        if line.len() > MAX_LINE_BYTES {
            spans.clear();
        }
        out.push(spans.into_boxed_slice());
        match next_base {
            Some(next) => base = next,
            None => base += line.encode_utf16().count() + 1,
        }
    }
    out
}

thread_local! {
    /// A lexer kept for one-line jobs, so search results do not instantiate one per row.
    static LINE_LEXER: std::cell::RefCell<Option<Lexer>> = const { std::cell::RefCell::new(None) };
}

/// Lexes one line on its own, as search results do (no context from the lines above).
pub fn highlight_line(language: LanguageId, text: &str) -> Box<[TokenSpan]> {
    LINE_LEXER.with_borrow_mut(|lexer| {
        if lexer.is_none() {
            *lexer = Lexer::new();
        }
        lexer.as_mut()?.lex(language, std::iter::once(text))?.pop()
    })
    .unwrap_or_default()
}
