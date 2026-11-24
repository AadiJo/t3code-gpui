//! Code block languages and highlighting for the composer. Ports
//! `composer-code-languages.ts` (names and aliases) and colours code with
//! the chat's lexers and syntax theme (pierre-dark, and t3-light).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{FontStyle, Hsla};

use crate::components::chat::syntax_highlighting;
use crate::components::chat::syntax_theme::SyntaxTheme;
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

/// Builds both syntax themes ahead of the first code block typed.
pub fn warm_highlighter() {
    SyntaxTheme::for_appearance(Appearance::Dark);
    SyntaxTheme::for_appearance(Appearance::Light);
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

/// Highlights a block's lines with the chat's lexers and syntax theme.
/// Unknown languages and very large blocks get no spans.
pub fn highlight_block(lines: &[&str], language: &str, appearance: Appearance) -> HighlightedBlock {
    let total: usize = lines.iter().map(|l| l.len() + 1).sum();
    let id = canonical_language(language_of_info_string(language));
    let mut out = vec![Vec::new(); lines.len()];
    if id.is_empty() || total > MAX_HIGHLIGHT_BYTES {
        return Arc::new(out);
    }
    let code = lines.join("\n");
    // The cache below keeps the result, so the chat's content cache is skipped.
    let Some(highlights) = syntax_highlighting::highlight(&id, &code, true) else {
        return Arc::new(out);
    };
    let theme = SyntaxTheme::for_appearance(appearance);
    let mut starts = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for line in lines {
        starts.push(offset);
        offset += line.len() + 1;
    }
    for (range, name) in highlights.iter() {
        let Some(style) = theme.style(name) else { continue };
        let Some(color) = style.color else { continue };
        let italic = style.font_style == Some(FontStyle::Italic);
        // A token can span lines (block comments, template strings).
        let mut line = starts.partition_point(|start| *start <= range.start).saturating_sub(1);
        while line < lines.len() && starts[line] < range.end {
            let line_start = starts[line];
            let start = range.start.max(line_start);
            let end = range.end.min(line_start + lines[line].len());
            if end > start {
                out[line].push(CodeSpan { range: start - line_start..end - line_start, color, italic });
            }
            line += 1;
        }
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
