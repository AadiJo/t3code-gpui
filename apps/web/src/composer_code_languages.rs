//! Code block languages and highlighting for the composer. Ports
//! `composer-code-languages.ts` (names and aliases) and applies the chat's
//! syntax theme (pierre-dark, and t3-light: pierre-light darkened until it
//! reads on light code) through syntect.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use gpui_kit::{Hsla, rgb};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
use syntect::parsing::SyntaxSet;

use crate::theme_palette::Appearance;

/// Blocks larger than this are drawn without highlighting.
pub const MAX_HIGHLIGHT_BYTES: usize = 24 * 1024;

/// Every language the picker offers, as `(id, label)`.
pub const CODE_BLOCK_LANGUAGES: &[(&str, &str)] = &[
    ("", "Plain text"), ("asm", "Assembly"), ("bash", "Bash"), ("c", "C"), ("cpp", "C++"),
    ("csharp", "C#"), ("css", "CSS"), ("dart", "Dart"), ("diff", "Diff"), ("dockerfile", "Dockerfile"),
    ("elixir", "Elixir"), ("go", "Go"), ("graphql", "GraphQL"), ("haskell", "Haskell"), ("html", "HTML"),
    ("java", "Java"), ("javascript", "JavaScript"), ("json", "JSON"), ("jsx", "JSX"), ("kotlin", "Kotlin"),
    ("lua", "Lua"), ("markdown", "Markdown"), ("php", "PHP"), ("powershell", "PowerShell"),
    ("python", "Python"), ("r", "R"), ("ruby", "Ruby"), ("rust", "Rust"), ("scala", "Scala"),
    ("scss", "SCSS"), ("sql", "SQL"), ("svelte", "Svelte"), ("swift", "Swift"), ("toml", "TOML"),
    ("tsx", "TSX"), ("typescript", "TypeScript"), ("vue", "Vue"), ("xml", "XML"), ("yaml", "YAML"), ("zig", "Zig"),
];

const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("assembly", "asm"), ("cs", "csharp"), ("c++", "cpp"), ("js", "javascript"), ("md", "markdown"),
    ("plaintext", ""), ("py", "python"), ("rb", "ruby"), ("rs", "rust"), ("s", "asm"), ("sh", "bash"),
    ("shell", "bash"), ("text", ""), ("ts", "typescript"), ("txt", ""), ("yml", "yaml"),
];

/// The first word of a fence's info string.
pub fn language_of_info_string(info: &str) -> &str {
    info.split_whitespace().next().unwrap_or("")
}

/// The canonical language id for a name or alias. Ports `codeLanguageEntry`.
pub fn canonical_language(language: &str) -> String {
    let lower = language.to_lowercase();
    LANGUAGE_ALIASES.iter().find(|(alias, _)| *alias == lower).map_or(lower, |(_, id)| (*id).to_owned())
}

/// The display label for a language. Ports `codeLanguageLabel`.
pub fn code_language_label(language: &str) -> String {
    let id = canonical_language(language);
    CODE_BLOCK_LANGUAGES.iter().find(|(lid, _)| *lid == id).map_or_else(|| language.to_owned(), |(_, label)| (*label).to_owned())
}

use crate::composer_code_theme::CODE_THEME_SCOPES;

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn build_theme(appearance: Appearance) -> Theme {
    let color = |hex: u32| Color { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8, a: 0xff };
    let scopes = CODE_THEME_SCOPES
        .iter()
        .filter_map(|(scope, dark, light, italic)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(scope).ok()?,
                style: StyleModifier {
                    foreground: Some(color(if appearance == Appearance::Dark { *dark } else { *light })),
                    background: None,
                    font_style: italic.then_some(FontStyle::ITALIC),
                },
            })
        })
        .collect();
    let foreground = if appearance == Appearance::Dark { 0xfafafa } else { 0x0a0a0a };
    Theme {
        settings: ThemeSettings { foreground: Some(color(foreground)), ..Default::default() },
        scopes,
        ..Default::default()
    }
}

fn theme(appearance: Appearance) -> &'static Theme {
    static DARK: OnceLock<Theme> = OnceLock::new();
    static LIGHT: OnceLock<Theme> = OnceLock::new();
    match appearance {
        Appearance::Dark => DARK.get_or_init(|| build_theme(Appearance::Dark)),
        Appearance::Light => LIGHT.get_or_init(|| build_theme(Appearance::Light)),
    }
}

/// Loads the syntax set and themes on a background thread, so the first code
/// block typed does not pay for it.
pub fn warm_highlighter() {
    std::thread::spawn(|| {
        syntax_set();
        theme(Appearance::Dark);
        theme(Appearance::Light);
    });
}

/// A coloured span in one code line.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeSpan {
    pub range: Range<usize>,
    pub color: Hsla,
    pub italic: bool,
}

/// Highlighted lines of one block: spans per line, by byte offset in the line.
pub type HighlightedBlock = Arc<Vec<Vec<CodeSpan>>>;

/// Highlights a block's lines. Unknown languages and very large blocks get no spans.
pub fn highlight_block(lines: &[&str], language: &str, appearance: Appearance) -> HighlightedBlock {
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    let set = syntax_set();
    let id = canonical_language(language_of_info_string(language));
    let syntax = (!id.is_empty() && total <= MAX_HIGHLIGHT_BYTES)
        .then(|| set.find_syntax_by_token(&id).or_else(|| set.find_syntax_by_extension(&id)))
        .flatten();
    let Some(syntax) = syntax else {
        return Arc::new(vec![Vec::new(); lines.len()]);
    };
    let mut highlighter = HighlightLines::new(syntax, theme(appearance));
    let mut out = Vec::with_capacity(lines.len());
    let mut buffer = String::new();
    for line in lines {
        buffer.clear();
        buffer.push_str(line);
        buffer.push('\n');
        let mut spans = Vec::new();
        let mut offset = 0;
        if let Ok(styled) = highlighter.highlight_line(&buffer, set) {
            for (style, piece) in styled {
                let end = (offset + piece.len()).min(line.len());
                if end > offset {
                    let c = style.foreground;
                    let hex = (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b);
                    spans.push(CodeSpan { range: offset..end, color: rgb(hex).into(), italic: style.font_style.contains(FontStyle::ITALIC) });
                }
                offset += piece.len();
            }
        }
        out.push(spans);
    }
    Arc::new(out)
}

/// Highlight results keyed by block content, language and appearance, so a
/// keystroke only re-highlights the block it changed.
#[derive(Default)]
pub struct HighlightCache {
    entries: HashMap<u64, HighlightedBlock>,
    used: Vec<u64>,
}

impl HighlightCache {
    /// Returns the cached highlight for a block, computing it on a miss.
    pub fn get(&mut self, lines: &[&str], language: &str, appearance: Appearance) -> HighlightedBlock {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        lines.hash(&mut hasher);
        language.hash(&mut hasher);
        (appearance == Appearance::Dark).hash(&mut hasher);
        let key = hasher.finish();
        self.used.push(key);
        self.entries.entry(key).or_insert_with(|| highlight_block(lines, language, appearance)).clone()
    }

    /// Drops entries no block used since the last call.
    pub fn end_frame(&mut self) {
        let used = std::mem::take(&mut self.used);
        if self.entries.len() > used.len() {
            self.entries.retain(|key, _| used.contains(key));
        }
    }
}
