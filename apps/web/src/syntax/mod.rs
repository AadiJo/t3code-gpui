//! Shared syntax highlighting for the diff panel, the code view and, later,
//! chat code blocks. Ports `lib/syntaxHighlighting.ts` and the
//! `pierre-dark` / `t3-light` themes from `lib/diffRendering.ts`.

mod highlighter;
mod language;
mod palette;
mod text_lines;

pub use highlighter::{DocKey, HighlightStats, SyntaxStore, TokenSpan};
pub use language::{LanguageId, language_for_path};
/// For the file editor's incremental highlighter.
pub(crate) use language::{SYNTAXES, scope_token};
pub use palette::{SyntaxPalette, SyntaxToken, hex};
pub use text_lines::TextLines;
