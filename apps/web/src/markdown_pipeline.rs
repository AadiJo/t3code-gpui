//! Chat markdown parsed into typed blocks the renderer draws. Ports the parts
//! of `packages/shared/src/markdownPipeline.ts` the chat view shows: GFM,
//! single-dollar math (only with the Render math setting, as the fork adds
//! `remark-math` only then), GitHub alerts, absolute file-path links, kept list
//! numbers, `<details>` blocks and remark-breaks for user messages.
//!
//! Inline content is flattened to one string per paragraph with styled
//! ranges, which is what a selectable text run lays out.

use std::{cell::Cell, ops::Range, sync::Arc};

use gpui_kit::SharedString;
use markdown::{
    Constructs, ParseOptions,
    mdast::{self, AlignKind, Node},
};

/// How a message's markdown is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MarkdownFlavor {
    /// Soft line breaks render as breaks (`remark-breaks`), as in user messages.
    pub line_breaks: bool,
    /// A single `~` stays literal (user messages).
    pub literal_single_tilde: bool,
}

impl MarkdownFlavor {
    pub const ASSISTANT: Self = Self { line_breaks: false, literal_single_tilde: false };
    pub const USER: Self = Self { line_breaks: true, literal_single_tilde: true };
}

/// A parsed block-level element.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Inlines),
    Heading { level: u8, content: Inlines },
    List(List),
    Blockquote(Vec<Block>),
    Alert { kind: AlertKind, blocks: Vec<Block> },
    Code(CodeBlock),
    Table(Table),
    Rule,
    /// A paragraph that holds only images.
    Images(Vec<Image>),
    Details { summary: Inlines, blocks: Vec<Block> },
    /// Display math (`$$ … $$`): its TeX.
    Math(SharedString),
    /// Footnote definitions collected at the end of the document.
    Footnotes(Vec<(SharedString, Vec<Block>)>),
    /// Raw HTML that is not a recognised `<details>` block, shown as text.
    Html(SharedString),
}

/// An ordered or bullet list.
#[derive(Clone, Debug, PartialEq)]
pub struct List {
    /// The first number for an ordered list.
    pub start: Option<u32>,
    pub items: Vec<ListItem>,
    /// Nesting depth, 0 for a top-level list; picks the marker style.
    pub depth: usize,
    /// A loose list wraps item text in paragraphs, which keep their margins.
    pub spread: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    /// The number the source wrote, for ordered lists (`remark` keeps it).
    pub number: Option<u32>,
    /// `Some` for a task list item.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
}

/// A fenced or indented code block.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeBlock {
    pub language: Option<SharedString>,
    /// A `title=`/`file=` meta value or a filename token from the fence.
    pub title: Option<SharedString>,
    pub code: SharedString,
    /// A fenced block whose closing fence was written (`isClosedCodeFence`):
    /// only those offer Run.
    pub closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAlign {
    Left,
    Center,
    Right,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub align: Vec<TableAlign>,
    pub header: Vec<Inlines>,
    pub rows: Vec<Vec<Inlines>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub url: SharedString,
    pub alt: SharedString,
}

/// `> [!NOTE]` and friends, plus Claude's `★ Insight` callout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
    Insight,
}

impl AlertKind {
    fn from_marker(marker: &str) -> Option<Self> {
        Some(match marker.to_ascii_lowercase().as_str() {
            "note" => Self::Note,
            "tip" => Self::Tip,
            "important" => Self::Important,
            "warning" => Self::Warning,
            "caution" => Self::Caution,
            "insight" => Self::Insight,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
            Self::Insight => "Insight",
        }
    }
}

/// Inline content: one string plus the ranges styled within it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inlines {
    pub text: SharedString,
    pub spans: Vec<InlineSpan>,
}

impl Inlines {
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// A styled byte range of [`Inlines::text`].
#[derive(Clone, Debug, PartialEq)]
pub struct InlineSpan {
    pub range: Range<usize>,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InlineStyle {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    /// Inline math (`$ … $`); the range holds the TeX.
    Math(SharedString),
    Link(SharedString),
    /// A workspace file reference shown as a chip (`MarkdownFileLink`).
    FileLink(SharedString),
    FootnoteRef,
}

/// Parses a whole message. Use [`crate::components::chat::markdown_stream_blocks`]
/// to parse a streaming message block by block.
pub fn parse_markdown(source: &str, flavor: MarkdownFlavor) -> Vec<Block> {
    let options = parse_options(flavor);
    let Ok(Node::Root(root)) = markdown::to_mdast(source, &options) else {
        return vec![Block::Paragraph(Inlines { text: source.to_owned().into(), spans: vec![] })];
    };
    let mut converter = Converter { source, flavor, footnotes: Vec::new() };
    let mut blocks = converter.blocks(&root.children, 0);
    if !converter.footnotes.is_empty() {
        blocks.push(Block::Footnotes(std::mem::take(&mut converter.footnotes)));
    }
    blocks
}

thread_local! {
    static MATH: Cell<bool> = const { Cell::new(false) };
}

/// Turns math parsing on or off (`mathRenderingEnabled`); true when that changed it.
/// Parses cached before then were made with the old value.
pub fn set_math_enabled(enabled: bool) -> bool {
    MATH.replace(enabled) != enabled
}

/// Whether `$…$` and `$$…$$` parse as math.
pub fn math_enabled() -> bool {
    MATH.get()
}

fn parse_options(flavor: MarkdownFlavor) -> ParseOptions {
    let math = math_enabled();
    ParseOptions {
        constructs: Constructs { math_text: math, math_flow: math, ..Constructs::gfm() },
        math_text_single_dollar: true,
        gfm_strikethrough_single_tilde: !flavor.literal_single_tilde,
        ..ParseOptions::gfm()
    }
}

struct Converter<'a> {
    source: &'a str,
    flavor: MarkdownFlavor,
    footnotes: Vec<(SharedString, Vec<Block>)>,
}

impl Converter<'_> {
    /// The digits an ordered item starts with in the source.
    fn item_number(&self, item: &mdast::ListItem) -> Option<u32> {
        let start = item.position.as_ref()?.start.offset;
        let digits: String = self.source.get(start..)?.trim_start().chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    }

    fn blocks(&mut self, nodes: &[Node], depth: usize) -> Vec<Block> {
        let mut out = Vec::with_capacity(nodes.len());
        let mut index = 0;
        while index < nodes.len() {
            if let Node::Html(html) = &nodes[index]
                && let Some(summary) = details_summary(&html.value)
            {
                // `<details>` opens a block that runs to the matching `</details>`.
                let start = index + 1;
                let mut end = start;
                while end < nodes.len()
                    && !matches!(&nodes[end], Node::Html(h) if h.value.contains("</details>"))
                {
                    end += 1;
                }
                let blocks = self.blocks(&nodes[start..end], depth);
                out.push(Block::Details { summary: plain_inlines(summary), blocks });
                index = end + 1;
                continue;
            }
            if let Some(block) = self.block(&nodes[index], depth) {
                out.push(block);
            }
            index += 1;
        }
        out
    }

    fn block(&mut self, node: &Node, depth: usize) -> Option<Block> {
        Some(match node {
            Node::Paragraph(p) => {
                if !p.children.is_empty() && p.children.iter().all(is_image_or_space) {
                    let images = p
                        .children
                        .iter()
                        .filter_map(|n| match n {
                            Node::Image(i) => Some(Image {
                                url: i.url.clone().into(),
                                alt: i.alt.clone().into(),
                            }),
                            _ => None,
                        })
                        .collect();
                    return Some(Block::Images(images));
                }
                Block::Paragraph(self.inlines(&p.children))
            }
            Node::Heading(h) => Block::Heading { level: h.depth, content: self.inlines(&h.children) },
            Node::List(list) => Block::List(List {
                start: list.ordered.then(|| list.start.unwrap_or(1)),
                depth,
                spread: list.spread
                    || list.children.iter().any(|item| matches!(item, Node::ListItem(item) if item.spread)),
                items: list
                    .children
                    .iter()
                    .filter_map(|item| match item {
                        Node::ListItem(item) => Some(ListItem {
                            number: list.ordered.then(|| self.item_number(item)).flatten(),
                            checked: item.checked,
                            blocks: self.blocks(&item.children, depth + 1),
                        }),
                        _ => None,
                    })
                    .collect(),
            }),
            Node::Blockquote(quote) => self.blockquote(quote, depth),
            Node::Code(code) => Block::Code(code_block(code, self.source)),
            Node::Math(math) => Block::Math(math.value.clone().into()),
            Node::ThematicBreak(_) => Block::Rule,
            Node::Table(table) => Block::Table(self.table(table)),
            Node::Html(html) => Block::Html(html.value.trim().to_owned().into()),
            Node::FootnoteDefinition(def) => {
                let blocks = self.blocks(&def.children, depth);
                self.footnotes.push((def.identifier.clone().into(), blocks));
                return None;
            }
            Node::Definition(_) | Node::Yaml(_) | Node::Toml(_) => return None,
            other => {
                let inlines = self.inlines(std::slice::from_ref(other));
                if inlines.is_empty() {
                    return None;
                }
                Block::Paragraph(inlines)
            }
        })
    }

    fn blockquote(&mut self, quote: &mdast::Blockquote, depth: usize) -> Block {
        // `> [!NOTE]` on the first line turns the quote into an alert.
        if let Some(Node::Paragraph(first)) = quote.children.first()
            && let Some(Node::Text(text)) = first.children.first()
            && let Some(rest) = text.value.strip_prefix("[!")
            && let Some((marker, after)) = rest.split_once(']')
            && let Some(kind) = AlertKind::from_marker(marker)
            && (after.is_empty() || after.starts_with('\n'))
        {
            let mut first = first.clone();
            match after.strip_prefix('\n') {
                Some(remaining) => {
                    if let Some(Node::Text(text)) = first.children.first_mut() {
                        text.value = remaining.to_owned();
                    }
                }
                None => {
                    first.children.remove(0);
                    if matches!(first.children.first(), Some(Node::Break(_))) {
                        first.children.remove(0);
                    }
                }
            }
            let mut children = quote.children.clone();
            if first.children.is_empty() {
                children.remove(0);
            } else {
                children[0] = Node::Paragraph(first);
            }
            return Block::Alert { kind, blocks: self.blocks(&children, depth) };
        }
        Block::Blockquote(self.blocks(&quote.children, depth))
    }

    fn table(&mut self, table: &mdast::Table) -> Table {
        let mut rows = table.children.iter().filter_map(|row| match row {
            Node::TableRow(row) => Some(
                row.children
                    .iter()
                    .map(|cell| match cell {
                        Node::TableCell(cell) => self.inlines(&cell.children),
                        _ => Inlines::default(),
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
        let header = rows.next().unwrap_or_default();
        let rows = rows.collect();
        Table {
            align: table
                .align
                .iter()
                .map(|a| match a {
                    AlignKind::Left => TableAlign::Left,
                    AlignKind::Center => TableAlign::Center,
                    AlignKind::Right => TableAlign::Right,
                    AlignKind::None => TableAlign::None,
                })
                .collect(),
            header,
            rows,
        }
    }

    fn inlines(&self, nodes: &[Node]) -> Inlines {
        let mut builder = InlineBuilder::default();
        for node in nodes {
            builder.push(node, self.flavor);
        }
        builder.finish()
    }
}

#[derive(Default)]
struct InlineBuilder {
    text: String,
    spans: Vec<InlineSpan>,
}

impl InlineBuilder {
    fn push(&mut self, node: &Node, flavor: MarkdownFlavor) {
        let start = self.text.len();
        let style = match node {
            Node::Text(text) => {
                self.push_text(&text.value, flavor);
                return;
            }
            Node::Break(_) => {
                self.text.push('\n');
                return;
            }
            Node::InlineCode(code) => {
                if looks_like_file_path(&code.value) {
                    self.push_file_chip(code.value.trim(), None);
                    return;
                }
                self.text.push_str(&code.value);
                InlineStyle::Code
            }
            Node::InlineMath(math) => {
                self.text.push_str(&math.value);
                InlineStyle::Math(math.value.clone().into())
            }
            Node::Strong(n) => {
                n.children.iter().for_each(|c| self.push(c, flavor));
                InlineStyle::Strong
            }
            Node::Emphasis(n) => {
                n.children.iter().for_each(|c| self.push(c, flavor));
                InlineStyle::Emphasis
            }
            Node::Delete(n) => {
                n.children.iter().for_each(|c| self.push(c, flavor));
                InlineStyle::Strikethrough
            }
            Node::Link(link) if is_file_target(&link.url) => {
                let label: String = link.children.iter().map(plain_text).collect();
                let label = (label.trim() != link.url && !label.trim().is_empty()).then_some(label);
                self.push_file_chip(&link.url, label.as_deref());
                return;
            }
            Node::Link(link) => {
                link.children.iter().for_each(|c| self.push(c, flavor));
                InlineStyle::Link(link.url.clone().into())
            }
            Node::LinkReference(n) => {
                n.children.iter().for_each(|c| self.push(c, flavor));
                return;
            }
            Node::Image(image) => {
                self.text.push_str(&image.alt);
                InlineStyle::Link(image.url.clone().into())
            }
            Node::FootnoteReference(note) => {
                self.text.push_str(&format!("[{}]", note.label.as_deref().unwrap_or(&note.identifier)));
                InlineStyle::FootnoteRef
            }
            Node::Html(html) => {
                self.text.push_str(&html.value);
                return;
            }
            other => {
                if let Some(children) = other.children() {
                    children.iter().for_each(|c| self.push(c, flavor));
                }
                return;
            }
        };
        if self.text.len() > start {
            self.spans.push(InlineSpan { range: start..self.text.len(), style });
        }
    }

    /// Plain text; soft breaks follow the flavor and bare absolute paths become file links.
    fn push_text(&mut self, value: &str, flavor: MarkdownFlavor) {
        let value = if flavor.line_breaks { value.to_owned() } else { value.replace('\n', " ") };
        let mut rest = value.as_str();
        while !rest.is_empty() {
            let next_path = find_absolute_path(rest);
            let Some(path) = next_path else {
                self.text.push_str(rest);
                break;
            };
            self.text.push_str(&rest[..path.start]);
            self.push_file_chip(&rest[path.clone()], None);
            rest = &rest[path.end..];
        }
    }

    /// A file chip: an icon placeholder, then the label or the file's basename.
    fn push_file_chip(&mut self, target: &str, label: Option<&str>) {
        let start = self.text.len();
        self.text.push_str(FILE_CHIP_PLACEHOLDER);
        match label {
            Some(label) => self.text.push_str(label),
            None => self.text.push_str(file_display_name(target)),
        }
        self.spans.push(InlineSpan {
            range: start..self.text.len(),
            style: InlineStyle::FileLink(target.to_owned().into()),
        });
    }

    fn finish(mut self) -> Inlines {
        // Nested styles push the inner span first; renderers want outer-first order.
        self.spans.sort_by_key(|span| (span.range.start, std::cmp::Reverse(span.range.end)));
        Inlines { text: self.text.into(), spans: self.spans }
    }
}

/// Space the file icon is painted over: an em for the 1em icon, then a thin gap.
pub const FILE_CHIP_PLACEHOLDER: &str = "\u{2003}\u{2009}";

/// `src/app/main.rs:12` → `main.rs:12`.
fn file_display_name(target: &str) -> &str {
    let trimmed = target.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn plain_text(node: &Node) -> String {
    match node {
        Node::Text(t) => t.value.clone(),
        Node::InlineCode(c) => c.value.clone(),
        other => other.children().map(|c| c.iter().map(plain_text).collect()).unwrap_or_default(),
    }
}

fn plain_inlines(text: &str) -> Inlines {
    Inlines { text: text.to_owned().into(), spans: vec![] }
}

fn is_image_or_space(node: &Node) -> bool {
    match node {
        Node::Image(_) | Node::Break(_) => true,
        Node::Text(text) => text.value.trim().is_empty(),
        _ => false,
    }
}

fn code_block(code: &mdast::Code, source: &str) -> CodeBlock {
    let meta = code.meta.as_deref().unwrap_or("");
    let title = meta.split_whitespace().find_map(|token| {
        token
            .strip_prefix("title=")
            .or_else(|| token.strip_prefix("file="))
            .map(|v| v.trim_matches(|c| c == '"' || c == '\''))
            .or_else(|| token.contains('.').then_some(token))
    });
    CodeBlock {
        language: code.lang.clone().filter(|l| !l.is_empty()).map(Into::into),
        title: title.map(|t| SharedString::from(t.to_owned())),
        code: code.value.clone().into(),
        closed: code.position.as_ref().and_then(|p| source.get(p.start.offset..p.end.offset)).is_some_and(fence_is_closed),
    }
}

/// Whether a fenced block's source ends with a closing fence at least as
/// long as its opening one (indented blocks are never "closed").
fn fence_is_closed(block: &str) -> bool {
    let opening = block.trim_start();
    let Some(marker) = opening.chars().next().filter(|c| *c == '`' || *c == '~') else { return false };
    let length = opening.chars().take_while(|c| *c == marker).count();
    if length < 3 {
        return false;
    }
    let mut lines = block.trim_end().lines();
    lines.next();
    lines.next_back().is_some_and(|last| {
        let last = last.trim();
        last.chars().take_while(|c| *c == marker).count() >= length && last.chars().all(|c| c == marker)
    })
}

/// The `<summary>` text when `html` opens a `<details>` block.
fn details_summary(html: &str) -> Option<&str> {
    let html = html.trim_start();
    if !html.starts_with("<details") {
        return None;
    }
    let summary = html
        .split_once("<summary>")
        .and_then(|(_, rest)| rest.split_once("</summary>"))
        .map(|(summary, _)| summary.trim())
        .unwrap_or("Details");
    Some(summary)
}

/// A link target that names a file rather than a web page.
fn is_file_target(url: &str) -> bool {
    !url.contains("://") && !url.starts_with("mailto:") && !url.starts_with('#') && looks_like_file_path(url)
}

/// `src/app.rs`, `/abs/path.ts:12`, `./x.md`: a path with a separator or an extension.
pub fn looks_like_file_path(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.contains(char::is_whitespace) || text.contains("://") {
        return false;
    }
    let path = text.split(':').next().unwrap_or(text);
    let base = path.rsplit('/').next().unwrap_or(path);
    let has_extension = base
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()));
    (path.contains('/') && has_extension) || path.starts_with("./") || path.starts_with('/') && path.len() > 1 && path[1..].contains('/')
}

/// The next bare absolute path (`/a/b.ext`) in plain text, as a byte range.
fn find_absolute_path(text: &str) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while let Some(offset) = text[index..].find('/') {
        let start = index + offset;
        let boundary = start == 0 || matches!(bytes[start - 1], b' ' | b'(' | b'\n' | b'"' | b'\'');
        if boundary {
            let end = text[start..]
                .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | ',' | ';'))
                .map_or(text.len(), |e| start + e);
            let end = if text[start..end].ends_with('.') { end - 1 } else { end };
            let candidate = &text[start..end];
            if candidate.matches('/').count() >= 2 && looks_like_file_path(candidate) {
                return Some(start..end);
            }
            index = end.max(start + 1);
        } else {
            index = start + 1;
        }
    }
    None
}

/// Parsed blocks shared between the cache and renderers.
pub type ParsedBlocks = Arc<[Block]>;
