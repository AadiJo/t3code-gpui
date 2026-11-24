//! Shared syntax highlighting for the diff panel, the code view, the files
//! panel and chat code blocks. Ports `lib/syntaxHighlighting.ts` and the
//! `pierre-dark` / `t3-light` themes from `lib/diffRendering.ts`.

mod highlighter;
mod language;
mod lexer;
mod palette;
mod pierre_tables;
mod text_lines;

pub use highlighter::{DocKey, HighlightStats, SyntaxStore, TokenSpan};
pub use language::{LanguageId, language_for_name, language_for_path};
pub use lexer::{Lexer, MAX_LINE_BYTES, highlight_line};
pub use palette::{SyntaxPalette, hex};
pub use pierre_tables::SyntaxToken;
pub use text_lines::TextLines;
