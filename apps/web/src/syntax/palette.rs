//! The syntax palettes the web app highlights with: `pierre-dark` and
//! `t3-light` (`pierre-light` darkened until it reads on light code,
//! `lib/readableSyntaxTheme.ts`). `pierre_palettes.json` holds both, resolved
//! from `@pierre/highlights`' Zed-style themes with the same prefix fallback
//! the web resolver uses, so every token here has its final colour.

use std::{collections::HashMap, sync::LazyLock};

use gpui_kit::{FontStyle, FontWeight, HighlightStyle, Hsla, Rgba};
use serde::Deserialize;

use crate::theme_palette::Appearance;

/// A highlight class, named like the Zed capture the web highlighter emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SyntaxToken {
    Attribute,
    Boolean,
    Comment,
    CommentDoc,
    Constant,
    ConstantBuiltin,
    Constructor,
    DiffMinus,
    DiffPlus,
    Emphasis,
    EmphasisStrong,
    Enum,
    Function,
    FunctionMethod,
    Keyword,
    KeywordControl,
    KeywordDeclaration,
    KeywordImport,
    KeywordOperator,
    Label,
    LinkText,
    LinkUri,
    Namespace,
    Number,
    Operator,
    Preproc,
    Property,
    PropertyJsonKey,
    Punctuation,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationListMarker,
    PunctuationSpecial,
    Selector,
    SelectorClass,
    SelectorId,
    SelectorPseudo,
    String,
    StringEscape,
    StringRegex,
    StringSpecial,
    StringSpecialSymbol,
    Tag,
    TagDoctype,
    TextLiteral,
    Title,
    Type,
    TypeBuiltin,
    TypeClass,
    Variable,
    VariableParameter,
    VariableSpecial,
    Variant,
}

impl SyntaxToken {
    /// Every token, in discriminant order.
    pub const ALL: [SyntaxToken; 53] = {
        use SyntaxToken::*;
        [
            Attribute,
            Boolean,
            Comment,
            CommentDoc,
            Constant,
            ConstantBuiltin,
            Constructor,
            DiffMinus,
            DiffPlus,
            Emphasis,
            EmphasisStrong,
            Enum,
            Function,
            FunctionMethod,
            Keyword,
            KeywordControl,
            KeywordDeclaration,
            KeywordImport,
            KeywordOperator,
            Label,
            LinkText,
            LinkUri,
            Namespace,
            Number,
            Operator,
            Preproc,
            Property,
            PropertyJsonKey,
            Punctuation,
            PunctuationBracket,
            PunctuationDelimiter,
            PunctuationListMarker,
            PunctuationSpecial,
            Selector,
            SelectorClass,
            SelectorId,
            SelectorPseudo,
            String,
            StringEscape,
            StringRegex,
            StringSpecial,
            StringSpecialSymbol,
            Tag,
            TagDoctype,
            TextLiteral,
            Title,
            Type,
            TypeBuiltin,
            TypeClass,
            Variable,
            VariableParameter,
            VariableSpecial,
            Variant,
        ]
    };

    /// The Zed capture name the palette JSON keys its styles by.
    pub fn name(self) -> &'static str {
        use SyntaxToken::*;
        match self {
            Attribute => "attribute",
            Boolean => "boolean",
            Comment => "comment",
            CommentDoc => "comment.doc",
            Constant => "constant",
            ConstantBuiltin => "constant.builtin",
            Constructor => "constructor",
            DiffMinus => "diff.minus",
            DiffPlus => "diff.plus",
            Emphasis => "emphasis",
            EmphasisStrong => "emphasis.strong",
            Enum => "enum",
            Function => "function",
            FunctionMethod => "function.method",
            Keyword => "keyword",
            KeywordControl => "keyword.control",
            KeywordDeclaration => "keyword.declaration",
            KeywordImport => "keyword.import",
            KeywordOperator => "keyword.operator",
            Label => "label",
            LinkText => "link_text",
            LinkUri => "link_uri",
            Namespace => "namespace",
            Number => "number",
            Operator => "operator",
            Preproc => "preproc",
            Property => "property",
            PropertyJsonKey => "property.json_key",
            Punctuation => "punctuation",
            PunctuationBracket => "punctuation.bracket",
            PunctuationDelimiter => "punctuation.delimiter",
            PunctuationListMarker => "punctuation.list_marker",
            PunctuationSpecial => "punctuation.special",
            Selector => "selector",
            SelectorClass => "selector.class",
            SelectorId => "selector.id",
            SelectorPseudo => "selector.pseudo",
            String => "string",
            StringEscape => "string.escape",
            StringRegex => "string.regex",
            StringSpecial => "string.special",
            StringSpecialSymbol => "string.special.symbol",
            Tag => "tag",
            TagDoctype => "tag.doctype",
            TextLiteral => "text.literal",
            Title => "title",
            Type => "type",
            TypeBuiltin => "type.builtin",
            TypeClass => "type.class",
            Variable => "variable",
            VariableParameter => "variable.parameter",
            VariableSpecial => "variable.special",
            Variant => "variant",
        }
    }
}

#[derive(Deserialize)]
struct TokenStyleJson {
    color: Option<String>,
    #[serde(default)]
    italic: bool,
    weight: Option<u16>,
}

#[derive(Deserialize)]
struct PaletteJson {
    background: String,
    foreground: String,
    created: String,
    deleted: String,
    modified: String,
    syntax: HashMap<String, TokenStyleJson>,
}

#[derive(Deserialize)]
struct PalettesJson {
    #[serde(rename = "pierre-dark")]
    dark: PaletteJson,
    #[serde(rename = "t3-light")]
    light: PaletteJson,
}

/// One appearance's syntax colours, indexed by [`SyntaxToken`].
#[derive(Debug)]
pub struct SyntaxPalette {
    pub background: Hsla,
    pub foreground: Hsla,
    pub created: Hsla,
    pub deleted: Hsla,
    pub modified: Hsla,
    styles: [HighlightStyle; SyntaxToken::ALL.len()],
}

impl SyntaxPalette {
    /// The palette for the app's appearance: `pierre-dark` or `t3-light`.
    pub fn for_appearance(appearance: Appearance) -> &'static SyntaxPalette {
        static PALETTES: LazyLock<(SyntaxPalette, SyntaxPalette)> = LazyLock::new(|| {
            let json: PalettesJson = serde_json::from_str(include_str!("pierre_palettes.json"))
                .expect("embedded syntax palettes parse");
            (SyntaxPalette::from_json(json.dark), SyntaxPalette::from_json(json.light))
        });
        match appearance {
            Appearance::Dark => &PALETTES.0,
            Appearance::Light => &PALETTES.1,
        }
    }

    /// The style for a token. Tokens the theme leaves unset draw in the foreground.
    pub fn style(&self, token: SyntaxToken) -> HighlightStyle {
        self.styles[token as usize]
    }

    fn from_json(json: PaletteJson) -> Self {
        let styles = SyntaxToken::ALL.map(|token| {
            let Some(style) = json.syntax.get(token.name()) else {
                return HighlightStyle::default();
            };
            HighlightStyle {
                color: style.color.as_deref().and_then(hex),
                font_style: style.italic.then_some(FontStyle::Italic),
                font_weight: style.weight.map(|w| FontWeight(w as f32)),
                ..Default::default()
            }
        });
        Self {
            background: hex(&json.background).unwrap_or_default(),
            foreground: hex(&json.foreground).unwrap_or_default(),
            created: hex(&json.created).unwrap_or_default(),
            deleted: hex(&json.deleted).unwrap_or_default(),
            modified: hex(&json.modified).unwrap_or_default(),
            styles,
        }
    }
}

/// Parses `#rgb`, `#rrggbb` or `#rrggbbaa`.
pub fn hex(value: &str) -> Option<Hsla> {
    let digits = value.strip_prefix('#')?;
    let expanded: String = match digits.len() {
        3 | 4 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 | 8 => digits.to_owned(),
        _ => return None,
    };
    let number = u32::from_str_radix(&expanded, 16).ok()?;
    let (rgb, alpha) = if expanded.len() == 8 {
        (number >> 8, (number & 0xff) as f32 / 255.)
    } else {
        (number, 1.)
    };
    let rgba = Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.,
        g: ((rgb >> 8) & 0xff) as f32 / 255.,
        b: (rgb & 0xff) as f32 / 255.,
        a: alpha,
    };
    Some(rgba.into())
}
