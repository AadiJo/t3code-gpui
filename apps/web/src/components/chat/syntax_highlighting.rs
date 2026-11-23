//! Tokenizes code for chat code blocks and shell commands. Ports
//! `lib/syntaxHighlighting.ts` and `lib/incrementalHighlighting.ts`: syntect
//! TextMate grammars (the two-face set) stand in for Pierre's lexers, and each
//! TextMate scope maps to the Zed highlight name Pierre's themes are keyed by.
//!
//! Finished code is cached by content. Code that grows (a streaming fence)
//! resumes from the parser state at the last complete line it saw.

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
    sync::{Arc, LazyLock},
};

use syntect::{
    parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Highlighted ranges of a piece of code, by Zed highlight name.
pub type Highlights = Arc<[(Range<usize>, &'static str)]>;

/// Languages that render as plain text in the code colour.
pub fn is_plain_language(language: &str) -> bool {
    matches!(language, "" | "text" | "txt" | "plain" | "plaintext")
}

fn find_syntax(language: &str) -> Option<&'static SyntaxReference> {
    let language = language.trim().to_ascii_lowercase();
    if is_plain_language(&language) {
        return None;
    }
    let alias = match language.as_str() {
        "gitignore" => "ini",
        "luau" => "lua",
        "xsl" | "xslt" => "xml",
        "shell" | "zsh" | "fish" | "console" => "bash",
        "ts" | "tsx" | "typescript" => "ts",
        "js" | "jsx" | "javascript" | "mjs" | "cjs" => "js",
        "rs" | "rust" => "rs",
        "py" | "python" => "py",
        "yml" => "yaml",
        "jsonc" | "json5" => "json",
        "csharp" => "cs",
        "make" | "makefile" => "Makefile",
        other => other,
    };
    let syntaxes = &*SYNTAXES;
    [language.as_str(), alias]
        .into_iter()
        .find_map(|token| syntaxes.find_syntax_by_token(token))
        .filter(|syntax| syntax.name != "Plain Text")
}

/// Parser progress through code that may still grow.
struct Incremental {
    language: String,
    /// The complete lines parsed so far.
    parsed: String,
    state: ParseState,
    scopes: ScopeStack,
    highlights: Vec<(Range<usize>, &'static str)>,
}

#[derive(Default)]
struct HighlightCache {
    finished: HashMap<u64, Highlights>,
    growing: Vec<Incremental>,
    names: HashMap<Scope, Option<&'static str>>,
}

thread_local! {
    static CACHE: RefCell<HighlightCache> = RefCell::new(HighlightCache::default());
}

/// Cap on cached highlights (the web client keeps 500 entries).
const FINISHED_CAPACITY: usize = 500;
/// Growing documents tracked at once.
const GROWING_CAPACITY: usize = 8;

/// Highlights for `code` in `language`. Pass `growing` for code that is still
/// streaming in, so the next call can resume where this one stopped.
pub fn highlight(language: &str, code: &str, growing: bool) -> Option<Highlights> {
    let syntax = find_syntax(language)?;
    CACHE.with_borrow_mut(|cache| {
        if !growing {
            let mut hasher = DefaultHasher::new();
            language.hash(&mut hasher);
            code.hash(&mut hasher);
            let key = hasher.finish();
            if let Some(found) = cache.finished.get(&key) {
                return Some(found.clone());
            }
            let highlights: Highlights = cache.parse_from(syntax, language, code, false).into();
            if cache.finished.len() >= FINISHED_CAPACITY {
                cache.finished.clear();
            }
            cache.finished.insert(key, highlights.clone());
            return Some(highlights);
        }
        Some(cache.parse_from(syntax, language, code, true).into())
    })
}

impl HighlightCache {
    fn parse_from(
        &mut self,
        syntax: &'static SyntaxReference,
        language: &str,
        code: &str,
        growing: bool,
    ) -> Vec<(Range<usize>, &'static str)> {
        let resumed = self
            .growing
            .iter()
            .position(|g| g.language == language && code.starts_with(&g.parsed))
            .map(|index| self.growing.swap_remove(index));
        let mut doc = resumed.unwrap_or_else(|| Incremental {
            language: language.to_owned(),
            parsed: String::new(),
            state: ParseState::new(syntax),
            scopes: ScopeStack::new(),
            highlights: Vec::new(),
        });
        // Parse only complete lines into the resumable state; the trailing
        // partial line is parsed on a copy so the next delta can redo it.
        let rest = &code[doc.parsed.len()..];
        let complete_len = rest.rfind('\n').map_or(0, |i| i + 1);
        let (complete, partial) = rest.split_at(complete_len);
        let mut offset = doc.parsed.len();
        for line in LinesWithEndings::from(complete) {
            self.parse_line(&mut doc.state, &mut doc.scopes, line, offset, &mut doc.highlights);
            offset += line.len();
        }
        doc.parsed.push_str(complete);
        let mut highlights = doc.highlights.clone();
        if !partial.is_empty() {
            let mut state = doc.state.clone();
            let mut scopes = doc.scopes.clone();
            self.parse_line(&mut state, &mut scopes, partial, offset, &mut highlights);
        }
        if growing {
            if self.growing.len() >= GROWING_CAPACITY {
                self.growing.remove(0);
            }
            self.growing.push(doc);
        }
        highlights
    }

    fn parse_line(
        &mut self,
        state: &mut ParseState,
        scopes: &mut ScopeStack,
        line: &str,
        offset: usize,
        out: &mut Vec<(Range<usize>, &'static str)>,
    ) {
        let Ok(operations) = state.parse_line(line, &SYNTAXES) else {
            return;
        };
        let mut cursor = 0;
        for (index, operation) in operations {
            self.push(out, offset + cursor..offset + index, scopes);
            let _ = scopes.apply(&operation);
            cursor = index;
        }
        self.push(out, offset + cursor..offset + line.len(), scopes);
    }

    /// Records `range` under the innermost scope that names a highlight, so a
    /// string's quotes colour as the string rather than as punctuation.
    fn push(&mut self, out: &mut Vec<(Range<usize>, &'static str)>, range: Range<usize>, scopes: &ScopeStack) {
        if range.is_empty() {
            return;
        }
        let names = &mut self.names;
        let Some(name) = scopes
            .scopes
            .iter()
            .rev()
            .find_map(|scope| *names.entry(*scope).or_insert_with(|| highlight_name(*scope)))
        else {
            return;
        };
        match out.last_mut() {
            Some((last, last_name)) if last.end == range.start && *last_name == name => last.end = range.end,
            _ => out.push((range, name)),
        }
    }
}

/// The Zed highlight name for a TextMate scope, or `None` to let an
/// enclosing scope decide.
fn highlight_name(scope: Scope) -> Option<&'static str> {
    let scope = scope.build_string();
    let is = |prefix: &str| {
        scope == prefix || scope.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'))
    };
    if is("punctuation.definition.string")
        || is("punctuation.definition.comment")
        || is("invalid")
        || is("meta")
        || is("source")
        || is("text")
    {
        return None;
    }
    let name = if is("comment.block.documentation") || is("comment.line.documentation") {
        "comment.doc"
    } else if is("comment") {
        "comment"
    } else if is("constant.character.escape") {
        "string.escape"
    } else if is("string.regexp") {
        "string.regex"
    } else if is("string") {
        "string"
    } else if is("constant.numeric") {
        "number"
    } else if is("constant.language.boolean") {
        "boolean"
    } else if is("constant") || is("support.constant") || is("entity.name.constant") {
        "constant"
    } else if is("keyword.operator") {
        "keyword.operator"
    } else if is("keyword") || is("storage.modifier") {
        "keyword"
    } else if is("storage.type") {
        // `fn`, `class` and `let` are declarations; `int` and `bool` are types.
        const DECLARATIONS: [&str; 13] = [
            "class", "enum", "function", "impl", "interface", "module", "namespace", "struct", "trait",
            "type", "let", "var", "const",
        ];
        if DECLARATIONS.iter().any(|word| scope.contains(word)) { "keyword" } else { "type" }
    } else if is("entity.name.function") || is("support.function") || is("variable.function") || is("meta.function-call") {
        "function"
    } else if is("entity.name.type")
        || is("entity.name.class")
        || is("entity.name.struct")
        || is("entity.name.enum")
        || is("entity.name.trait")
        || is("entity.name.interface")
        || is("entity.other.inherited-class")
        || is("support.type.builtin")
        || is("support.class")
    {
        "type"
    } else if is("support.type.property-name") || is("meta.property-name") {
        "property"
    } else if is("support.type") {
        "type"
    } else if is("entity.name.tag") {
        "tag"
    } else if is("entity.other.attribute-name") {
        "attribute"
    } else if is("entity.name.label") {
        "label"
    } else if is("entity.name.namespace") || is("entity.name.module") {
        "namespace"
    } else if is("entity.name.section") || is("markup.heading") {
        "title"
    } else if is("variable.language") {
        "variable.special"
    } else if is("variable.parameter") {
        "variable.parameter"
    } else if is("variable.other.member") || is("variable.other.property") || is("support.variable.property") {
        "property"
    } else if is("variable") {
        "variable"
    } else if is("markup.bold") {
        "emphasis.strong"
    } else if is("markup.italic") {
        "emphasis"
    } else if is("markup.underline.link") {
        "link_uri"
    } else if is("markup.raw") {
        "text.literal"
    } else if is("markup.list") {
        "punctuation.list_marker"
    } else if is("punctuation.section") || is("punctuation.definition.block") {
        "punctuation.bracket"
    } else if is("punctuation.separator") || is("punctuation.terminator") {
        "punctuation.delimiter"
    } else if is("punctuation") {
        "punctuation"
    } else {
        return None;
    };
    Some(name)
}
