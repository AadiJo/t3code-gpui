//! File path → syntax, and TextMate scope → [`SyntaxToken`].
//!
//! The web app highlights with `@pierre/highlights`, which names tokens like
//! Zed captures (`keyword`, `string.escape`). Syntect names them with TextMate
//! scopes, so each scope is mapped onto the capture the web would have used.

use std::sync::LazyLock;

use syntect::parsing::{Scope, SyntaxReference, SyntaxSet};

use super::SyntaxToken;

/// bat's syntax collection, deserialized once and shared with gpui-component.
pub(crate) static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// A syntax in [`SYNTAXES`], by index. `Copy` and cheap to hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LanguageId(u32);

impl LanguageId {
    pub(crate) fn syntax(self) -> &'static SyntaxReference {
        &SYNTAXES.syntaxes()[self.0 as usize]
    }

    /// The syntax's display name, such as "Rust".
    pub fn name(self) -> &'static str {
        &self.syntax().name
    }
}

/// The language for a file path, or `None` for plain text.
pub fn language_for_path(path: &str) -> Option<LanguageId> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let extension = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    // Highlights has no lexer for these dialects; the web maps them to their base language.
    let alias = match extension.as_deref() {
        Some("luau") => Some("lua"),
        Some("xsl" | "xslt") => Some("xml"),
        Some("mts" | "cts") => Some("ts"),
        Some("mjs" | "cjs") => Some("js"),
        Some("jsonc" | "json5") => Some("json"),
        _ => None,
    };
    let syntaxes = &*SYNTAXES;
    let found = alias
        .and_then(|token| syntaxes.find_syntax_by_extension(token))
        .or_else(|| extension.as_deref().and_then(|ext| syntaxes.find_syntax_by_extension(ext)))
        .or_else(|| syntaxes.find_syntax_by_extension(name))
        .or_else(|| syntaxes.find_syntax_by_token(name))?;
    if found.name == "Plain Text" {
        return None;
    }
    let index = syntaxes.syntaxes().iter().position(|syntax| std::ptr::eq(syntax, found))?;
    Some(LanguageId(index as u32))
}

/// Rules checked in order; the first prefix that matches a scope wins.
const SCOPE_RULES: &[(&str, SyntaxToken)] = {
    use SyntaxToken::*;
    &[
        ("comment.block.documentation", CommentDoc),
        ("punctuation.definition.comment", Comment),
        ("comment", Comment),
        ("string.regexp", StringRegex),
        ("constant.character.escape", StringEscape),
        ("punctuation.definition.string", String),
        ("string.other.link", LinkText),
        ("string", String),
        ("constant.numeric", Number),
        ("constant.language.boolean", Boolean),
        ("constant.language", ConstantBuiltin),
        ("constant.other.symbol", StringSpecialSymbol),
        ("constant.character", StringSpecial),
        ("constant", Constant),
        ("meta.preprocessor", Preproc),
        ("keyword.control.directive", Preproc),
        ("keyword.control.import", KeywordImport),
        ("keyword.control.export", KeywordImport),
        ("keyword.control.from", KeywordImport),
        ("keyword.control", KeywordControl),
        ("keyword.operator.new", Keyword),
        ("keyword.operator.expression", Keyword),
        ("keyword.operator.word", Keyword),
        ("keyword.operator", Operator),
        ("keyword.declaration", KeywordDeclaration),
        ("keyword", Keyword),
        ("storage.type.function.arrow", Operator),
        ("storage.type.primitive", TypeBuiltin),
        ("storage.type.built-in", TypeBuiltin),
        ("storage", Keyword),
        ("support.type.property-name", Property),
        ("support.type.primitive", TypeBuiltin),
        ("support.type.builtin", TypeBuiltin),
        ("support.type", Type),
        ("support.class", Type),
        ("support.function", Function),
        ("support.constant", ConstantBuiltin),
        ("support.variable.property", Property),
        ("support.variable", VariableSpecial),
        ("entity.name.function.method", FunctionMethod),
        ("entity.name.function", Function),
        ("entity.name.enum", Enum),
        ("entity.name.type", Type),
        ("entity.name.class", Type),
        ("entity.name.struct", Type),
        ("entity.name.trait", Type),
        ("entity.name.interface", Type),
        ("entity.other.inherited-class", Type),
        ("entity.name.namespace", Namespace),
        ("entity.name.module", Namespace),
        ("entity.name.tag", Tag),
        ("entity.name.label", Label),
        ("entity.name.constant", Constant),
        ("entity.name.section", Title),
        ("entity.other.attribute-name.class", SelectorClass),
        ("entity.other.attribute-name.id", SelectorId),
        ("entity.other.attribute-name.pseudo", SelectorPseudo),
        ("entity.other.attribute-name", Attribute),
        ("entity.name", Type),
        ("variable.parameter", VariableParameter),
        ("variable.language", VariableSpecial),
        ("variable.other.property", Property),
        ("variable.other.object.property", Property),
        ("variable.other.member", Property),
        ("variable.other.constant", Constant),
        ("variable.function", Function),
        ("variable", Variable),
        ("meta.object-literal.key", Property),
        ("meta.mapping.key", Property),
        ("punctuation.definition.tag", PunctuationBracket),
        ("punctuation.definition.list", PunctuationListMarker),
        ("punctuation.section.embedded", PunctuationSpecial),
        ("punctuation.definition.template-expression", PunctuationSpecial),
        ("punctuation.separator", PunctuationDelimiter),
        ("punctuation.terminator", PunctuationDelimiter),
        ("punctuation.accessor", PunctuationDelimiter),
        ("punctuation.section", PunctuationBracket),
        ("punctuation", Punctuation),
        ("markup.heading", Title),
        ("markup.bold", EmphasisStrong),
        ("markup.italic", Emphasis),
        ("markup.raw", TextLiteral),
        ("markup.inline.raw", TextLiteral),
        ("markup.underline.link", LinkUri),
        ("markup.list", PunctuationListMarker),
        ("markup.inserted", DiffPlus),
        ("markup.deleted", DiffMinus),
        ("markup.quote", Comment),
    ]
};

/// The token for one scope, or `None` when the scope names nothing the theme colours.
pub(crate) fn scope_token(scope: Scope) -> Option<SyntaxToken> {
    let name = scope.build_string();
    SCOPE_RULES
        .iter()
        .find(|(prefix, _)| {
            name.starts_with(prefix)
                && matches!(name.as_bytes().get(prefix.len()), None | Some(b'.'))
        })
        .map(|(_, token)| *token)
}
