//! Tokenizes code for chat code blocks and shell commands with the web's own
//! lexers (`@pierre/highlights`, through [`crate::syntax::Lexer`]), as
//! `lib/syntaxHighlighting.ts` and `lib/incrementalHighlighting.ts` do. Each
//! range carries the token's Zed highlight name, which the chat's
//! `SyntaxTheme` colours with the same `pierre-dark` / `t3-light` values the
//! diff panel's `SyntaxPalette` holds.
//!
//! Finished code is cached by content. Code that grows (a streaming fence) is
//! lexed again on each change; the lexers handle a few thousand lines in a
//! few milliseconds. Large finished code highlights on a background thread
//! the first time it shows, so scrolling onto it never waits for the lexer.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::{Arc, LazyLock, Mutex},
};

use gpui_kit::{App, SharedString};

use crate::syntax::{LanguageId, Lexer, language_for_name};

/// Highlighted ranges of a piece of code, by Zed highlight name.
pub type Highlights = Arc<[(Range<usize>, &'static str)]>;

/// Languages that render as plain text in the code colour.
pub fn is_plain_language(language: &str) -> bool {
    matches!(language, "" | "text" | "txt" | "plain" | "plaintext")
}

/// The lexer for a fence language (`highlighterLanguageOf`: gitignore reads as ini).
fn find_language(language: &str) -> Option<LanguageId> {
    let language = language.trim();
    if language.eq_ignore_ascii_case("gitignore") {
        return language_for_name("ini");
    }
    language_for_name(language)
}

thread_local! {
    /// The UI thread's lexer, reused across code blocks.
    static LEXER: RefCell<Option<Lexer>> = const { RefCell::new(None) };
}

/// Cap on cached highlights (the web client keeps 500 entries).
const FINISHED_CAPACITY: usize = 500;

/// Finished highlights by content, shared with the background highlighter.
static FINISHED: LazyLock<Mutex<HashMap<u64, Highlights>>> = LazyLock::new(Default::default);
/// Keys being highlighted on a background thread.
static PENDING: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(Default::default);
/// Finished code at least this long highlights off the UI thread.
const BACKGROUND_THRESHOLD: usize = 2_000;

fn content_key(language: LanguageId, code: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    language.hash(&mut hasher);
    code.hash(&mut hasher);
    hasher.finish()
}

fn finished(key: u64) -> Option<Highlights> {
    FINISHED.lock().ok()?.get(&key).cloned()
}

/// Lexes `code` as one document and returns its ranges in document bytes.
fn lex(lexer: &mut Lexer, language: LanguageId, code: &str) -> Highlights {
    // A CR before the newline is not part of the line the lexer sees.
    let lines = code.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line));
    let Some(spans) = lexer.lex(language, lines) else {
        return Arc::from([]);
    };
    let mut out = Vec::new();
    let mut offset = 0;
    for (line, spans) in code.split('\n').zip(spans) {
        out.extend(spans.iter().map(|span| {
            (offset + span.start as usize..offset + span.end as usize, span.token.name())
        }));
        offset += line.len() + 1;
    }
    out.into()
}

/// Lexes on the calling thread's lexer.
fn lex_here(language: LanguageId, code: &str) -> Highlights {
    LEXER.with_borrow_mut(|lexer| {
        if lexer.is_none() {
            *lexer = Lexer::new();
        }
        lexer.as_mut().map(|lexer| lex(lexer, language, code))
    })
    .unwrap_or_else(|| Arc::from([]))
}

fn store(key: u64, highlights: &Highlights) {
    if let Ok(mut map) = FINISHED.lock() {
        if map.len() >= FINISHED_CAPACITY {
            map.clear();
        }
        map.insert(key, highlights.clone());
    }
}

/// Highlights for `code` in `language`, computed now. Pass `growing` for code
/// that is still streaming in; it is not cached, since it changes next frame.
pub fn highlight(language: &str, code: &str, growing: bool) -> Option<Highlights> {
    let language = find_language(language)?;
    if growing {
        return Some(lex_here(language, code));
    }
    let key = content_key(language, code);
    Some(finished(key).unwrap_or_else(|| {
        let highlights = lex_here(language, code);
        store(key, &highlights);
        highlights
    }))
}

/// Highlights for finished code. Short code is highlighted now; long code not
/// yet cached returns `None` and is highlighted on a background thread, after
/// which windows redraw with the colours.
pub fn highlight_or_schedule(language: &str, code: &SharedString, cx: &App) -> Option<Highlights> {
    let language = find_language(language)?;
    let key = content_key(language, code);
    if let Some(found) = finished(key) {
        return Some(found);
    }
    if code.len() < BACKGROUND_THRESHOLD {
        let highlights = lex_here(language, code);
        store(key, &highlights);
        return Some(highlights);
    }
    if PENDING.lock().is_ok_and(|mut pending| pending.insert(key)) {
        let code = code.clone();
        let work = cx.background_executor().spawn(async move {
            // A lexer's memory grows to its largest input, so this one is dropped after.
            if let Some(mut lexer) = Lexer::new() {
                store(key, &lex(&mut lexer, language, &code));
            }
        });
        cx.spawn(async move |cx| {
            work.await;
            if let Ok(mut pending) = PENDING.lock() {
                pending.remove(&key);
            }
            cx.update(|cx| cx.refresh_windows());
        })
        .detach();
    }
    None
}
