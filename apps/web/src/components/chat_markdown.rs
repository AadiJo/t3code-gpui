//! Draws parsed chat markdown with the typography of `index.css`
//! (`.chat-markdown`, lines 1903-2359) and the code block frame of
//! `MarkdownCodeBlock.tsx`. Ports the rendering half of `ChatMarkdown.tsx`.
//!
//! Block margins collapse the way CSS margins do: the gap between siblings is
//! the larger of the two margins, a container without vertical padding passes
//! its first and last child's margins outward, and the root drops its outer
//! margins (`.chat-markdown > :first-child`).

use std::{collections::HashSet, ops::Range, rc::Rc};

use gpui_kit::{
    AnyElement, App, Div, ElementId, FontStyle, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, StrikethroughStyle,
    Styled as _, StyledImage as _, TextRun, Window, div, img, px,
};

use super::chat::{
    chat_icons::{ChatIcon, icon},
    chat_style::{ChatColors, TEXT_2XS, TEXT_CODE, TEXT_SM_RELAXED, TEXT_XS, alpha, mono, sans},
    rich_text::{LinkHandler, RichText, TextBox, TextIcon},
    syntax_highlighting::{highlight, is_plain_language},
    syntax_theme::SyntaxTheme,
};
use crate::markdown_pipeline::{
    AlertKind, Block, CodeBlock, FILE_CHIP_PLACEHOLDER, Inlines, InlineStyle, List, ParsedBlocks, Table, TableAlign,
};

/// Block spacing for `p, ul, ol, blockquote, pre, table`: `margin: 0.65rem 0`.
pub const BLOCK_MARGIN: Pixels = px(10.4);
/// Heading margins `1.25rem 0 0.5rem`.
const HEADING_MARGIN_TOP: Pixels = px(20.);
const HEADING_MARGIN_BOTTOM: Pixels = px(8.);
/// `li + li { margin-top: 0.25rem }`.
const LIST_ITEM_GAP: Pixels = px(4.);
/// `ul { padding-inline-start: 1.25rem }`, also the minimum `ol` gutter.
const LIST_GUTTER: Pixels = px(20.);
/// Space between an outside list marker and the item text (one space).
const LIST_MARKER_GAP: Pixels = px(4.);
/// Width of a digit (`1ch`) at 14px, for widened ordered-list gutters.
const DIGIT_WIDTH: Pixels = px(8.6);
/// `blockquote { padding-inline-start: 0.8rem }`, border 2px.
const BLOCKQUOTE_PADDING: Pixels = px(12.8);
/// Code frame: radius 8px; header `pt-1.5 pr-1.5 pl-3`; `pre` padding `0.8rem 0.9rem`.
const CODE_RADIUS: Pixels = px(8.);
const CODE_HEADER_PAD_TOP: Pixels = px(6.);
const CODE_HEADER_PAD_RIGHT: Pixels = px(6.);
const CODE_HEADER_PAD_LEFT: Pixels = px(12.);
const CODE_PAD_Y: Pixels = px(12.8);
const CODE_PAD_X: Pixels = px(14.4);
/// `Button size=icon-xs`: 24px box, 12px icon.
const ICON_XS: Pixels = px(24.);
const ICON_XS_GLYPH: Pixels = px(12.);
/// Table cells `padding: 0.45rem 0.75rem`, header `padding-block: 0.55rem`, `max-width: 24rem`.
const CELL_PAD_Y: Pixels = px(7.2);
const HEADER_CELL_PAD_Y: Pixels = px(8.8);
const CELL_PAD_X: Pixels = px(12.);
const CELL_MAX_WIDTH: Pixels = px(384.);
/// Inline code: `radius 0.25rem; padding 0.1em 0.35em` at `0.85em`.
const INLINE_CODE_RADIUS: Pixels = px(4.);
const INLINE_CODE_PAD_X: Pixels = px(4.2);
/// Details: `my-2`, trigger `py-2`, content `pb-3 ps-6`, chevron 16px.
const DETAILS_MARGIN: Pixels = px(8.);
/// Footnotes: `margin-top 1.25rem; padding-top 0.75rem`.
const FOOTNOTES_MARGIN: Pixels = px(20.);
/// Images in markdown are capped at this height (sizes unverified upstream).
const IMAGE_MAX_HEIGHT: Pixels = px(240.);

/// Something a reader did inside markdown that the owning view handles.
#[derive(Clone, Debug)]
pub enum MarkdownAction {
    Copy { key: u64, text: SharedString },
    ToggleWrap(u64),
    ToggleDetails(u64),
    OpenLink(SharedString),
}

/// Handles markdown actions for the view that rendered it.
pub type MarkdownActions = Rc<dyn Fn(MarkdownAction, &mut Window, &mut App)>;

/// UI state of markdown blocks, owned by the timeline so it survives
/// virtualization. Keys come from the message id and block position.
#[derive(Default)]
pub struct MarkdownUiState {
    /// The copy button showing its check mark.
    pub copied: Option<u64>,
    /// Code blocks the reader switched to horizontal scrolling.
    pub unwrapped: HashSet<u64>,
    pub open_details: HashSet<u64>,
}

/// Everything one render of a message's markdown needs.
pub struct MarkdownView<'a> {
    /// Seeds element ids and block-state keys; the message id hash.
    pub key: u64,
    pub pieces: &'a [ParsedBlocks],
    pub colors: &'a ChatColors,
    /// User-message flavour: decimal nested lists, inline code tinted with the bubble ink.
    pub user: bool,
    /// The fence still streaming has no Run action and highlights incrementally.
    pub streaming: bool,
    /// Opacity per piece for blocks fading in while streaming.
    pub piece_opacity: Option<&'a [f32]>,
    pub state: &'a MarkdownUiState,
    pub actions: MarkdownActions,
}

/// A rendered block and the margins it passes to its neighbours.
struct Rendered {
    element: AnyElement,
    top: Pixels,
    bottom: Pixels,
}

struct Renderer<'a> {
    view: &'a MarkdownView<'a>,
    theme: &'static SyntaxTheme,
    next_id: u64,
    text_color: Hsla,
    link_handler: LinkHandler,
}

/// Renders a message's markdown as one column.
pub fn render_markdown(view: &MarkdownView<'_>, _window: &mut Window, _cx: &mut App) -> Div {
    let actions = view.actions.clone();
    let mut renderer = Renderer {
        view,
        theme: SyntaxTheme::for_appearance(view.colors.appearance),
        next_id: 0,
        text_color: if view.user { view.colors.theme.message_foreground } else { view.colors.prose() },
        link_handler: Rc::new(move |target, window, cx| actions(MarkdownAction::OpenLink(target.clone()), window, cx)),
    };
    let mut rendered = Vec::new();
    for (piece_index, piece) in view.pieces.iter().enumerate() {
        let opacity = view.piece_opacity.and_then(|o| o.get(piece_index)).copied().unwrap_or(1.);
        for block in piece.iter() {
            let mut block = renderer.block(block, 0, true);
            if opacity < 1. {
                block.element = div().opacity(opacity).child(block.element).into_any_element();
            }
            rendered.push(block);
        }
    }
    let stacked = stack(rendered);
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .text_size(TEXT_SM_RELAXED.size)
        .line_height(TEXT_SM_RELAXED.line_height)
        .text_color(renderer.text_color)
        .children(stacked.0)
}

/// Lays out siblings with collapsed margins. Returns the children (each
/// carrying its top gap) and the outer margins of the first and last child.
fn stack(blocks: Vec<Rendered>) -> (Vec<AnyElement>, Pixels, Pixels) {
    let first_top = blocks.first().map_or(px(0.), |b| b.top);
    let last_bottom = blocks.last().map_or(px(0.), |b| b.bottom);
    let mut previous_bottom: Option<Pixels> = None;
    let children = blocks
        .into_iter()
        .map(|block| {
            let gap = previous_bottom.map_or(px(0.), |bottom| bottom.max(block.top));
            previous_bottom = Some(block.bottom);
            if gap > px(0.) {
                div().pt(gap).child(block.element).into_any_element()
            } else {
                block.element
            }
        })
        .collect();
    (children, first_top, last_bottom)
}

impl Renderer<'_> {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.view.key.wrapping_mul(31).wrapping_add(self.next_id)
    }

    fn element_id(&mut self, name: &'static str) -> (u64, ElementId) {
        let key = self.id();
        (key, ElementId::NamedInteger(name.into(), key))
    }

    /// `tight` drops paragraph margins, as in a tight list item.
    fn block(&mut self, block: &Block, depth: usize, loose: bool) -> Rendered {
        let colors = self.view.colors;
        match block {
            Block::Paragraph(inlines) => {
                let margin = if loose { BLOCK_MARGIN } else { px(0.) };
                Rendered { element: self.paragraph(inlines, None, None), top: margin, bottom: margin }
            }
            Block::Heading { level, content } => {
                let size = match level {
                    1 => px(20.),
                    2 => px(18.),
                    3 => px(16.),
                    _ => px(14.),
                };
                let color = if *level == 6 { colors.theme.muted_foreground } else { colors.theme.text };
                let text = self.paragraph(content, Some(FontWeight::SEMIBOLD), Some(color));
                Rendered {
                    element: div().text_size(size).line_height(size * 1.3).child(text).into_any_element(),
                    top: HEADING_MARGIN_TOP,
                    bottom: HEADING_MARGIN_BOTTOM,
                }
            }
            Block::List(list) => self.list(list, depth),
            Block::Blockquote(blocks) => {
                let (children, top, bottom) = stack(blocks.iter().map(|b| self.block(b, depth, true)).collect());
                let element = div()
                    .border_l_2()
                    .border_color(alpha(self.text_color, 0.35))
                    .pl(BLOCKQUOTE_PADDING)
                    .flex()
                    .flex_col()
                    .children(children);
                Rendered { element: element.into_any_element(), top: BLOCK_MARGIN.max(top), bottom: BLOCK_MARGIN.max(bottom) }
            }
            Block::Alert { kind, blocks } => self.alert(*kind, blocks, depth),
            Block::Code(code) => Rendered { element: self.code_block(code), top: BLOCK_MARGIN, bottom: BLOCK_MARGIN },
            Block::Table(table) => Rendered { element: self.table(table), top: BLOCK_MARGIN, bottom: BLOCK_MARGIN },
            Block::Rule => {
                let color = if colors.is_dark() { colors.theme.muted_foreground } else { colors.theme.border };
                Rendered { element: div().h(px(1.)).w_full().bg(color).into_any_element(), top: px(0.), bottom: px(0.) }
            }
            Block::Images(images) => {
                let element = div().flex().flex_wrap().gap_2().children(images.iter().map(|image| {
                    let alt = image.alt.clone();
                    let placeholder_color = colors.theme.muted_foreground;
                    img(image.url.clone())
                        .max_h(IMAGE_MAX_HEIGHT)
                        .max_w_full()
                        .rounded(px(8.))
                        .object_fit(ObjectFit::Contain)
                        .with_fallback(move || {
                            div().text_color(placeholder_color).child(alt.clone()).into_any_element()
                        })
                }));
                Rendered { element: element.into_any_element(), top: BLOCK_MARGIN, bottom: BLOCK_MARGIN }
            }
            Block::Details { summary, blocks } => self.details(summary, blocks, depth),
            Block::Math(tex) => {
                let (_, id) = self.element_id("math");
                let text = RichText::new(id, tex.clone(), vec![run(tex.len(), sans(FontWeight::NORMAL), self.text_color)])
                    .selection_color(colors.selection);
                let element = div().px(px(2.)).py(px(3.)).italic().child(text);
                Rendered { element: element.into_any_element(), top: BLOCK_MARGIN, bottom: BLOCK_MARGIN }
            }
            Block::Footnotes(notes) => {
                let items = notes
                    .iter()
                    .map(|(label, blocks)| {
                        let (children, ..) = stack(blocks.iter().map(|b| self.block(b, depth, false)).collect());
                        div()
                            .flex()
                            .gap_1()
                            .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(format!("{label}.")))
                            .child(div().flex_1().min_w_0().children(children))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();
                let element = div()
                    .mt(px(0.))
                    .border_t_1()
                    .border_color(colors.theme.border)
                    .pt(px(12.))
                    .text_color(colors.theme.muted_foreground)
                    .text_size(TEXT_XS.size)
                    .line_height(TEXT_XS.line_height)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(items);
                Rendered { element: element.into_any_element(), top: FOOTNOTES_MARGIN, bottom: px(0.) }
            }
            Block::Html(html) => {
                let (_, id) = self.element_id("html");
                let text = RichText::new(id, html.clone(), vec![run(html.len(), sans(FontWeight::NORMAL), colors.theme.muted_foreground)])
                    .selection_color(colors.selection);
                Rendered { element: text.into_any_element(), top: BLOCK_MARGIN, bottom: BLOCK_MARGIN }
            }
        }
    }

    /// A paragraph of inline content as one selectable text run.
    fn paragraph(&mut self, inlines: &Inlines, weight: Option<FontWeight>, color: Option<Hsla>) -> AnyElement {
        let (_, id) = self.element_id("p");
        let base = InlineBase {
            weight: weight.unwrap_or(FontWeight::NORMAL),
            color: color.unwrap_or(self.text_color),
        };
        let styled = style_inlines(inlines, base, self.view.colors, self.view.user);
        let mut text = RichText::new(id, inlines.text.clone(), styled.runs)
            .boxes(styled.boxes)
            .icons(styled.icons)
            .selection_color(self.view.colors.selection);
        if !styled.links.is_empty() {
            text = text.links(styled.links, self.link_handler.clone());
        }
        div().w_full().min_w_0().child(text).into_any_element()
    }

    fn list(&mut self, list: &List, depth: usize) -> Rendered {
        let colors = self.view.colors;
        let gutter = match list.start {
            Some(start) => {
                let largest = list
                    .items
                    .iter()
                    .filter_map(|item| item.number)
                    .max()
                    .unwrap_or(start)
                    .max(start + list.items.len().saturating_sub(1) as u32) as usize;
                let digits = largest.to_string().len();
                if digits > 1 { LIST_GUTTER.max(DIGIT_WIDTH * (digits as f32 + 2.)) } else { LIST_GUTTER }
            }
            None => LIST_GUTTER,
        };
        let items = list
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let marker: AnyElement = match item.checked {
                    Some(checked) => checkbox(checked, colors).into_any_element(),
                    None => {
                        let label = match list.start {
                            Some(start) => ordered_marker(
                                item.number.unwrap_or(start + index as u32) as usize,
                                depth,
                                self.view.user,
                            ),
                            None => bullet_marker(depth).to_string(),
                        };
                        div().child(label).into_any_element()
                    }
                };
                let (children, top, bottom) =
                    stack(item.blocks.iter().map(|b| self.block(b, depth + 1, list.spread)).collect());
                let row = div()
                    .flex()
                    .w_full()
                    .child(
                        div()
                            .flex_none()
                            .w(gutter)
                            .pr(LIST_MARKER_GAP)
                            .flex()
                            .justify_end()
                            .child(marker),
                    )
                    .child(div().flex_1().min_w_0().flex().flex_col().children(children));
                let item_top = if index == 0 { top } else { LIST_ITEM_GAP.max(top) };
                Rendered { element: row.into_any_element(), top: item_top, bottom }
            })
            .collect::<Vec<_>>();
        let (children, top, bottom) = stack(items);
        Rendered {
            element: div().flex().flex_col().children(children).into_any_element(),
            top: BLOCK_MARGIN.max(top),
            bottom: BLOCK_MARGIN.max(bottom),
        }
    }

    fn alert(&mut self, kind: AlertKind, blocks: &[Block], depth: usize) -> Rendered {
        let (border, title_color) = self.view.colors.alert(kind);
        let glyph = match kind {
            AlertKind::Insight => ChatIcon::Star,
            AlertKind::Note => ChatIcon::Info,
            AlertKind::Tip => ChatIcon::Lightbulb,
            AlertKind::Important => ChatIcon::MessageSquareWarning,
            AlertKind::Warning => ChatIcon::TriangleAlert,
            AlertKind::Caution => ChatIcon::OctagonAlert,
        };
        let title = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(title_color)
            .child(icon(glyph, px(14.), title_color))
            .child(kind.label());
        let mut children = vec![Rendered { element: title.into_any_element(), top: BLOCK_MARGIN, bottom: BLOCK_MARGIN }];
        children.extend(blocks.iter().map(|b| self.block(b, depth, true)));
        let (children, top, bottom) = stack(children);
        let element = div().border_l_2().border_color(border).pl(px(12.)).flex().flex_col().children(children);
        Rendered { element: element.into_any_element(), top: px(4.).max(top), bottom: px(4.).max(bottom) }
    }

    fn details(&mut self, summary: &Inlines, blocks: &[Block], depth: usize) -> Rendered {
        let colors = self.view.colors;
        let (key, id) = self.element_id("details");
        let open = self.view.state.open_details.contains(&key);
        let actions = self.view.actions.clone();
        let trigger = div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .py_2()
            .cursor_pointer()
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors.theme.text)
            .child(rotated_chevron(open, px(16.), colors.theme.muted_foreground))
            .child(summary.text.clone())
            .on_click(move |_, window, cx| actions(MarkdownAction::ToggleDetails(key), window, cx));
        let mut element = div().border_y_1().border_color(alpha(colors.theme.border, 0.6)).child(trigger);
        if open {
            let (children, ..) = stack(blocks.iter().map(|b| self.block(b, depth, true)).collect());
            element = element.child(
                div().pb_3().pl_6().text_color(alpha(colors.theme.text, 0.9)).flex().flex_col().children(children),
            );
        }
        Rendered { element: element.into_any_element(), top: DETAILS_MARGIN, bottom: DETAILS_MARGIN }
    }

    fn code_block(&mut self, code: &CodeBlock) -> AnyElement {
        let colors = self.view.colors;
        let (key, id) = self.element_id("code");
        let wrapped = !self.view.state.unwrapped.contains(&key);
        let copied = self.view.state.copied == Some(key);
        let language = code.language.clone().unwrap_or_default();
        let source = code.code.clone();
        let header_color = alpha(colors.theme.code_foreground, 0.72);
        let label = code.title.clone().or_else(|| code.language.clone());
        let actions = self.view.actions.clone();
        let copy_text: SharedString = source.trim_end_matches('\n').to_owned().into();
        let wrap_actions = self.view.actions.clone();
        let header = div()
            .flex()
            .justify_between()
            .items_center()
            .gap_2()
            .pt(CODE_HEADER_PAD_TOP)
            .pr(CODE_HEADER_PAD_RIGHT)
            .pl(CODE_HEADER_PAD_LEFT)
            .text_color(header_color)
            .child(
                div()
                    .font_family(mono().family)
                    .text_size(TEXT_2XS.size)
                    .line_height(TEXT_2XS.line_height)
                    .children(label),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(
                        icon_button(("wrap", key), ChatIcon::TextWrap, header_color, colors)
                            .when_active(wrapped, colors)
                            .on_click(move |_, window, cx| wrap_actions(MarkdownAction::ToggleWrap(key), window, cx)),
                    )
                    .child(
                        icon_button(
                            ("copy", key),
                            if copied { ChatIcon::Check } else { ChatIcon::Copy },
                            if copied { colors.theme.message_action } else { header_color },
                            colors,
                        )
                        .on_click(move |_, window, cx| {
                            actions(MarkdownAction::Copy { key, text: copy_text.clone() }, window, cx)
                        }),
                    ),
            );
        let highlights = if is_plain_language(&language) { None } else { highlight(&language, &source, self.view.streaming) };
        let runs = code_runs(&source, highlights.as_deref(), self.theme, colors.theme.code_foreground);
        let text = RichText::new(id, source, runs).selection_color(colors.selection);
        let body = div()
            .px(CODE_PAD_X)
            .py(CODE_PAD_Y)
            .font_family(mono().family)
            .text_size(TEXT_CODE.size)
            .line_height(TEXT_CODE.line_height);
        let body = if wrapped {
            body.child(text).into_any_element()
        } else {
            div()
                .id(("code-scroll", key))
                .overflow_x_scroll()
                .child(body.whitespace_nowrap().child(text))
                .into_any_element()
        };
        div()
            .w_full()
            .rounded(CODE_RADIUS)
            .overflow_hidden()
            .bg(colors.code_background())
            .border_1()
            .border_color(colors.theme.border)
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn table(&mut self, table: &Table) -> AnyElement {
        let colors = self.view.colors;
        let columns = table.header.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        // Auto table layout, approximated: columns grow in proportion to their longest cell.
        let weights: Vec<f32> = (0..columns)
            .map(|column| {
                let longest = std::iter::once(&table.header)
                    .chain(table.rows.iter())
                    .filter_map(|row| row.get(column))
                    .map(|cell| cell.text.chars().count())
                    .max()
                    .unwrap_or(1);
                (longest as f32 * 7.5 + 24.).min(f32::from(CELL_MAX_WIDTH))
            })
            .collect();
        let rule = alpha(colors.theme.border, 0.6);
        let render_row = |cells: &[Inlines], header: bool, this: &mut Self| {
            div()
                .flex()
                .w_full()
                .border_b_1()
                .border_color(rule)
                .children((0..columns).map(|column| {
                    let content = cells
                        .get(column)
                        .map(|cell| this.paragraph(cell, header.then_some(FontWeight::SEMIBOLD), None));
                    let align = table.align.get(column).copied().unwrap_or(TableAlign::None);
                    let pad_y = if header { HEADER_CELL_PAD_Y } else { CELL_PAD_Y };
                    let mut cell = div()
                        .flex_grow(1.)
                        .flex_basis(px(weights[column]))
                        .min_w(px(weights[column] * 0.5))
                        .max_w(CELL_MAX_WIDTH)
                        .py(pad_y)
                        .flex()
                        .children(content);
                    cell = if column == 0 { cell.pr(CELL_PAD_X) } else if column + 1 == columns { cell.pl(CELL_PAD_X) } else { cell.px(CELL_PAD_X) };
                    match align {
                        TableAlign::Center => cell.text_center(),
                        TableAlign::Right => cell.text_right(),
                        _ => cell,
                    }
                }))
        };
        let header = render_row(&table.header, true, self);
        let rows: Vec<_> = table.rows.iter().map(|row| render_row(row, false, self).into_any_element()).collect();
        let (key, _) = self.element_id("table");
        let copied = self.view.state.copied == Some(key);
        let markdown = table_markdown(table);
        let actions = self.view.actions.clone();
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("table-scroll", key))
                    .overflow_x_scroll()
                    .child(div().min_w_full().flex().flex_col().child(header).children(rows)),
            )
            .child(
                div().mt(px(2.)).mx(px(-4.)).flex().justify_end().child(
                    icon_button(
                        ("table-copy", key),
                        if copied { ChatIcon::Check } else { ChatIcon::Copy },
                        if copied { colors.theme.message_action } else { colors.theme.muted_foreground },
                        colors,
                    )
                    .on_click(move |_, window, cx| actions(MarkdownAction::Copy { key, text: markdown.clone() }, window, cx)),
                ),
            )
            .into_any_element()
    }
}

/// The base look of a paragraph's text before inline styles apply.
#[derive(Clone, Copy)]
struct InlineBase {
    weight: FontWeight,
    color: Hsla,
}

/// Text runs and decorations for one paragraph.
struct StyledInlines {
    runs: Vec<TextRun>,
    boxes: Vec<TextBox>,
    icons: Vec<TextIcon>,
    links: Vec<(Range<usize>, SharedString)>,
}

fn run(len: usize, font: gpui_kit::Font, color: Hsla) -> TextRun {
    TextRun { len, font, color, background_color: None, underline: None, strikethrough: None }
}

/// Splits a paragraph into runs at every span boundary and applies each
/// covering span's style, so nested styles (bold inside a link) combine.
fn style_inlines(inlines: &Inlines, base: InlineBase, colors: &ChatColors, user: bool) -> StyledInlines {
    let text = inlines.text.as_ref();
    let mut boundaries: Vec<usize> = vec![0, text.len()];
    for span in &inlines.spans {
        boundaries.push(span.range.start);
        boundaries.push(span.range.end);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let code_ink = if user { colors.theme.message_foreground } else { colors.theme.text };
    let mut styled = StyledInlines { runs: Vec::new(), boxes: Vec::new(), icons: Vec::new(), links: Vec::new() };
    for window in boundaries.windows(2) {
        let (start, end) = (window[0], window[1]);
        if start >= end {
            continue;
        }
        let mut font = sans(base.weight);
        let mut color = base.color;
        let mut strikethrough = None;
        for span in inlines.spans.iter().filter(|s| s.range.start <= start && s.range.end >= end) {
            match &span.style {
                InlineStyle::Strong => font.weight = FontWeight::BOLD,
                InlineStyle::Emphasis | InlineStyle::Math => font.style = FontStyle::Italic,
                InlineStyle::Strikethrough => {
                    strikethrough = Some(StrikethroughStyle { thickness: px(1.), color: None });
                }
                InlineStyle::Code => {
                    font = gpui_kit::Font { weight: font.weight, ..mono() };
                    color = code_ink;
                }
                InlineStyle::Link(_) => color = colors.info_foreground,
                InlineStyle::FileLink(_) => font.weight = FontWeight::MEDIUM,
                InlineStyle::FootnoteRef => {
                    font.weight = FontWeight::SEMIBOLD;
                    color = colors.theme.muted_foreground;
                }
            }
        }
        styled.runs.push(TextRun { strikethrough, ..run(end - start, font, color) });
    }
    for span in &inlines.spans {
        match &span.style {
            InlineStyle::Code => styled.boxes.push(TextBox {
                range: span.range.clone(),
                color: alpha(code_ink, 0.1),
                pad_x: INLINE_CODE_PAD_X,
                radius: INLINE_CODE_RADIUS,
            }),
            InlineStyle::Link(target) => styled.links.push((span.range.clone(), target.clone())),
            InlineStyle::FileLink(target) => {
                if text[span.range.clone()].starts_with(FILE_CHIP_PLACEHOLDER) {
                    styled.icons.push(TextIcon {
                        index: span.range.start,
                        path: ChatIcon::File.path().into(),
                        size: px(14.),
                        color: base.color,
                    });
                }
                styled.links.push((span.range.clone(), target.clone()));
            }
            _ => {}
        }
    }
    styled
}

/// Runs for a code block: highlighted ranges in theme colours, the rest in `foreground`.
fn code_runs(
    source: &str,
    highlights: Option<&[(Range<usize>, &'static str)]>,
    theme: &SyntaxTheme,
    foreground: Hsla,
) -> Vec<TextRun> {
    let base = mono();
    let mut runs = Vec::new();
    let mut cursor = 0;
    let push = |range: Range<usize>, name: Option<&str>, runs: &mut Vec<TextRun>| {
        if range.is_empty() {
            return;
        }
        let style = name.and_then(|n| theme.style(n));
        let mut font = base.clone();
        if let Some(style) = style {
            if let Some(weight) = style.font_weight {
                font.weight = weight;
            }
            if let Some(slant) = style.font_style {
                font.style = slant;
            }
        }
        let color = style.and_then(|s| s.color).unwrap_or(foreground);
        match runs.last_mut() {
            Some(last) if last.color == color && last.font == font => last.len += range.len(),
            _ => runs.push(run(range.len(), font, color)),
        }
    };
    for (range, name) in highlights.unwrap_or(&[]) {
        if range.start > cursor {
            push(cursor..range.start, None, &mut runs);
        }
        push(range.clone(), Some(name), &mut runs);
        cursor = range.end;
    }
    push(cursor..source.len(), None, &mut runs);
    if runs.is_empty() {
        runs.push(run(0, base, foreground));
    }
    runs
}

fn bullet_marker(depth: usize) -> &'static str {
    match depth {
        0 => "•",
        1 => "◦",
        _ => "▪",
    }
}

/// `1.`, then `a.` and `i.` for nested assistant lists; user messages stay decimal.
fn ordered_marker(number: usize, depth: usize, user: bool) -> String {
    match (depth, user) {
        (_, true) | (0, _) => format!("{number}."),
        (1, _) => format!("{}.", alpha_marker(number)),
        _ => format!("{}.", roman_marker(number)),
    }
}

fn alpha_marker(mut number: usize) -> String {
    let mut out = Vec::new();
    while number > 0 {
        number -= 1;
        out.push((b'a' + (number % 26) as u8) as char);
        number /= 26;
    }
    out.iter().rev().collect()
}

fn roman_marker(mut number: usize) -> String {
    const NUMERALS: [(usize, &str); 13] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"),
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while number >= value {
            out.push_str(numeral);
            number -= value;
        }
    }
    out
}

fn checkbox(checked: bool, colors: &ChatColors) -> Div {
    let side = px(13.);
    let base = div().mt(px(5.)).size(side).rounded(px(3.)).flex().items_center().justify_center();
    if checked {
        base.bg(colors.info).child(icon(ChatIcon::Check, px(10.), gpui_kit::white()))
    } else {
        base.border_1().border_color(colors.theme.muted_foreground)
    }
}

/// `ChevronRight` turned 90° when open, as the fork's disclosures draw it.
pub fn rotated_chevron(open: bool, size: Pixels, color: Hsla) -> AnyElement {
    if open {
        icon(ChatIcon::ChevronDown, size, color).into_any_element()
    } else {
        icon(ChatIcon::ChevronRight, size, color).into_any_element()
    }
}

/// A 24px square ghost button with a 12px icon (`Button size=icon-xs`).
pub fn icon_button(
    id: impl Into<ElementId>,
    glyph: ChatIcon,
    color: Hsla,
    colors: &ChatColors,
) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .size(ICON_XS)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(|style| style.bg(colors.theme.accent_surface))
        .child(icon(glyph, ICON_XS_GLYPH, color))
}

trait CodeButtonExt {
    fn when_active(self, active: bool, colors: &ChatColors) -> Self;
}

impl CodeButtonExt for gpui_kit::Stateful<Div> {
    /// `variant=secondary` while on.
    fn when_active(self, active: bool, colors: &ChatColors) -> Self {
        if active { self.bg(colors.theme.secondary) } else { self }
    }
}

/// A GFM table written back as markdown, for its copy button.
fn table_markdown(table: &Table) -> SharedString {
    let line = |cells: &[Inlines]| format!("| {} |", cells.iter().map(|c| c.text.as_ref()).collect::<Vec<_>>().join(" | "));
    let mut out = vec![line(&table.header)];
    out.push(format!("|{}|", vec![" --- "; table.header.len()].join("|")));
    out.extend(table.rows.iter().map(|row| line(row)));
    out.join("\n").into()
}
