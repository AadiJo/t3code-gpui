//! Drawing and hit testing for [`ComposerPromptEditor`]. Each source line
//! becomes a [`DisplayLine`]: display text with hidden or shown Markdown
//! markers, chip labels in place of chip source, and styled runs. Display lines
//! are cached by content, so a keystroke rebuilds only the line it touched;
//! GPUI's text system caches the shaped lines across frames the same way.

use crate::components::icons::IconName;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    App, AvailableSpace, Bounds, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent, SharedString,
    StrikethroughStyle, Style, TextAlign, TextRun, TransformationMatrix, Window, WrappedLine, fill,
    font, point, px, relative, size,
};

use crate::components::chat::composer_theme::{
    ComposerColors, EDITOR_MAX_HEIGHT, EDITOR_MIN_HEIGHT, MONO_FONT_FAMILY,
    PROMPT_FONT_SIZE, PROMPT_LINE_HEIGHT, UI_FONT_FAMILY,
};
use crate::components::composer_prompt_editor::{ComposerPromptEditor, Doc, DocLine, LineParse};
use crate::composer_code_languages::{CodeSpan, HighlightCache};
use crate::composer_editor_mentions::{ChipKind, ContextKind};
use crate::composer_rich_text::{Marks, RichTextMark};
use crate::composer_rich_text_doc::{ListKind, LineKind};
use crate::composer_text_direction::TextDirection;

/// Quote indent: `padding-inline-start: 0.8rem`.
const QUOTE_INDENT: Pixels = px(12.8);
/// Icon slot before a chip label: an em space and a six-per-em space (1em icon, 0.2em gap).
const CHIP_ICON_SLOT: &str = "\u{2003}\u{2006}";

/// A mapping from a source range of a line to its display range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub src: Range<usize>,
    pub disp: Range<usize>,
    /// A chip or a replaced marker: the caret never stops inside it.
    pub atom: bool,
}

/// An icon drawn in an icon slot of the display text.
#[derive(Clone, Debug, PartialEq)]
pub struct IconSlot {
    pub disp: usize,
    pub path: &'static str,
    pub color: Hsla,
}

/// One source line ready to shape.
#[derive(Debug)]
pub struct DisplayLine {
    pub parse: Arc<LineParse>,
    pub text: SharedString,
    pub runs: Vec<TextRun>,
    pub segs: Vec<Seg>,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub indent: Pixels,
    pub direction: TextDirection,
    pub icons: Vec<IconSlot>,
    /// Inline code backgrounds, as display ranges.
    pub code_boxes: Vec<Range<usize>>,
    /// Chips as (source range in line, display range).
    pub chips: Vec<(Range<usize>, Range<usize>)>,
    pub quote: bool,
    /// A rule drawn as a line instead of text.
    pub rule: bool,
    pub code_block: Option<usize>,
    /// Space kept free at the right edge (code block padding).
    pub right_inset: Pixels,
    /// A code block's language icon, drawn before the header label.
    pub header_icon: Option<crate::pierre_icons::PierreIcon>,
    /// A closing fence hidden while the caret is elsewhere: no height.
    pub collapsed: bool,
    /// The line shaped at its last wrap width, reused while the line and width are unchanged.
    pub shaped: RefCell<Option<(Pixels, Rc<WrappedLine>)>>,
}

impl DisplayLine {
    /// Display index for a source offset within the line.
    pub fn disp_for_src(&self, offset: usize) -> usize {
        let mut fallback = self.text.len();
        for seg in &self.segs {
            if offset < seg.src.start {
                return seg.disp.start;
            }
            if offset < seg.src.end || (offset == seg.src.end && seg.src.is_empty()) {
                return if seg.atom {
                    if offset == seg.src.start { seg.disp.start } else { seg.disp.end }
                } else if seg.disp.is_empty() {
                    seg.disp.start
                } else {
                    seg.disp.start + (offset - seg.src.start).min(seg.disp.len())
                };
            }
            fallback = seg.disp.end;
        }
        fallback
    }

    /// Source offset (within the line) for a display index.
    pub fn src_for_disp(&self, disp: usize) -> usize {
        let mut best = None;
        for seg in &self.segs {
            if disp < seg.disp.start || disp > seg.disp.end || seg.disp.is_empty() {
                continue;
            }
            let src = if seg.atom {
                if disp * 2 <= seg.disp.start + seg.disp.end { seg.src.start } else { seg.src.end }
            } else {
                seg.src.start + (disp - seg.disp.start)
            };
            // At a boundary prefer the later segment's start, which sits after hidden markers.
            best = Some(src);
            if disp < seg.disp.end {
                break;
            }
        }
        best.unwrap_or_else(|| self.segs.last().map_or(0, |s| s.src.end))
    }
}

/// Everything the display of a line depends on besides its text.
struct BuildCtx<'a> {
    colors: &'a ComposerColors,
    rich: bool,
    focused: bool,
    /// Selection relative to the line start, when it touches the line.
    active: Option<Range<usize>>,
    code: Option<&'a [CodeSpan]>,
}

#[derive(Clone, Copy, PartialEq)]
struct RunStyle {
    color: Hsla,
    weight: FontWeight,
    italic: bool,
    strike: bool,
    mono: bool,
}

struct Builder {
    text: String,
    runs: Vec<TextRun>,
    segs: Vec<Seg>,
    icons: Vec<IconSlot>,
    code_boxes: Vec<Range<usize>>,
    chips: Vec<(Range<usize>, Range<usize>)>,
}

impl Builder {
    fn push_run(&mut self, len: usize, style: RunStyle) {
        if len == 0 {
            return;
        }
        let family = if style.mono { MONO_FONT_FAMILY } else { UI_FONT_FAMILY };
        let mut f = font(family);
        f.weight = style.weight;
        f.style = if style.italic { FontStyle::Italic } else { FontStyle::Normal };
        let strikethrough = style.strike.then_some(StrikethroughStyle { thickness: px(1.), color: Some(style.color) });
        if let Some(last) = self.runs.last_mut()
            && last.font == f
            && last.color == style.color
            && last.strikethrough == strikethrough
        {
            last.len += len;
            return;
        }
        self.runs.push(TextRun { len, font: f, color: style.color, background_color: None, underline: None, strikethrough });
    }

    /// Source text shown as is.
    fn text(&mut self, src: Range<usize>, text: &str, style: RunStyle) {
        let start = self.text.len();
        self.text.push_str(text);
        self.push_run(text.len(), style);
        self.segs.push(Seg { src, disp: start..self.text.len(), atom: false });
    }

    /// Source with no display.
    fn hidden(&mut self, src: Range<usize>) {
        let at = self.text.len();
        self.segs.push(Seg { src, disp: at..at, atom: false });
    }

    /// Source replaced by a label the caret steps over.
    fn atom(&mut self, src: Range<usize>, label: &str, style: RunStyle, icon: Option<(&'static str, Hsla)>) -> Range<usize> {
        let start = self.text.len();
        if let Some((path, color)) = icon {
            self.icons.push(IconSlot { disp: start, path, color });
            self.text.push_str(CHIP_ICON_SLOT);
        }
        self.text.push_str(label);
        self.push_run(self.text.len() - start, style);
        let disp = start..self.text.len();
        self.segs.push(Seg { src, disp: disp.clone(), atom: true });
        disp
    }
}

fn chip_icon(kind: &ChipKind) -> &'static str {
    match kind {
        ChipKind::Mention { path } if path.ends_with('/') => IconName::Folder.path(),
        ChipKind::Mention { .. } => IconName::File.path(),
        ChipKind::Skill { .. } => IconName::Sparkles.path(),
        ChipKind::Model { .. } => IconName::Cpu.path(),
        ChipKind::Citation => IconName::MessagesSquare.path(),
        ChipKind::Context { kind, .. } => match kind {
            ContextKind::Image => IconName::Image.path(),
            ContextKind::Text => IconName::FileText.path(),
            ContextKind::Terminal => IconName::Terminal.path(),
            ContextKind::Thread => IconName::MessagesSquare.path(),
            ContextKind::PullRequest => IconName::GitPullRequest.path(),
            _ => IconName::File.path(),
        },
    }
}

/// Builds the display of one line. `line` is the document line; offsets in the
/// result are relative to its start.
fn build_display_line(line: &DocLine, ctx: &BuildCtx<'_>) -> DisplayLine {
    let c = ctx.colors;
    let text = line.parse.text.as_str();
    let base = RunStyle { color: c.foreground, weight: FontWeight::NORMAL, italic: false, strike: false, mono: false };
    let mut b = Builder { text: String::with_capacity(text.len() + 8), runs: Vec::new(), segs: Vec::new(), icons: Vec::new(), code_boxes: Vec::new(), chips: Vec::new() };
    let mut font_size = PROMPT_FONT_SIZE;
    let mut line_height = PROMPT_LINE_HEIGHT;
    let mut quote = false;
    let mut code_block = None;
    let caret_on_line = ctx.focused && ctx.active.is_some();

    // Code lines: monospace, highlighted, no inline parsing. The fences are
    // hidden: the opening fence draws as the block's language header, the
    // closing one collapses, unless the caret is on it.
    if ctx.rich && line.kind.is_code() {
        code_block = line.kind.code_block();
        let mono = RunStyle { mono: true, color: c.code_foreground, ..base };
        let code_inset = px(1.) + c.rem * 0.75;
        let mut display = match (&line.kind, ctx.code) {
            (LineKind::FenceOpen { .. }, _) if caret_on_line => {
                b.text(0..text.len(), text, RunStyle { color: c.muted_foreground, ..mono });
                finish(b, line, c.rem * 0.75, c.rem * 1.5, false, false, code_block)
            }
            (LineKind::FenceOpen { .. }, _) => {
                let language = crate::composer_code_languages::language_of_info_string(text.trim_start_matches(['`', '~']));
                let label = crate::composer_code_languages::code_language_label(language);
                let style = RunStyle { color: c.muted_foreground, weight: FontWeight::MEDIUM, ..base };
                b.atom(0..text.len(), &label, style, None);
                let mut d = finish(b, line, c.rem * 0.75, c.rem * 1.5, false, false, code_block);
                let file = if language.is_empty() {
                    "file.txt".to_owned()
                } else {
                    crate::components::chat_markdown::synthetic_file_name(language)
                };
                d.header_icon = Some(crate::pierre_icons::resolve_file_icon(&file));
                d.indent = code_inset + px(16.);
                return d;
            }
            (LineKind::FenceClose { .. }, _) if !caret_on_line => {
                b.hidden(0..text.len());
                let mut d = finish(b, line, c.code_font_size, px(0.), false, false, code_block);
                d.collapsed = true;
                return d;
            }
            (LineKind::Code { .. }, Some(spans)) if !spans.is_empty() => {
                let mut at = 0;
                for span in spans {
                    if span.range.start > at {
                        b.text(at..span.range.start, &text[at..span.range.start], mono);
                    }
                    let style = RunStyle { color: span.color, italic: span.italic, ..mono };
                    b.text(span.range.clone(), &text[span.range.clone()], style);
                    at = span.range.end;
                }
                if at < text.len() {
                    b.text(at..text.len(), &text[at..], mono);
                }
                finish(b, line, c.code_font_size, c.code_font_size * 1.375, false, false, code_block)
            }
            (LineKind::Code { .. }, _) => {
                b.text(0..text.len(), text, mono);
                finish(b, line, c.code_font_size, c.code_font_size * 1.375, false, false, code_block)
            }
            _ => {
                b.text(0..text.len(), text, RunStyle { color: c.code_foreground.opacity(0.72), ..mono });
                finish(b, line, c.code_font_size, c.code_font_size * 1.375, false, false, code_block)
            }
        };
        if display.segs.is_empty() {
            display.segs.push(Seg { src: 0..0, disp: 0..0, atom: false });
        }
        display.indent = code_inset;
        display.right_inset = code_inset;
        return display;
    }

    let mut content_start = 0;
    if ctx.rich {
        match line.kind {
            LineKind::Heading { level, marker } => {
                font_size = match level {
                    1 => px(20.),
                    2 => px(18.),
                    3 => px(16.),
                    _ => px(14.),
                };
                line_height = font_size * 1.3;
                b.hidden(0..marker);
                content_start = marker;
            }
            LineKind::Quote { marker } => {
                quote = true;
                b.hidden(0..marker);
                content_start = marker;
            }
            LineKind::Rule if !caret_on_line => {
                b.hidden(0..text.len());
                return finish(b, line, font_size, line_height, false, true, None);
            }
            LineKind::Rule => {
                b.text(0..text.len(), text, RunStyle { color: c.muted_foreground, ..base });
                return finish(b, line, font_size, line_height, false, false, None);
            }
            LineKind::ListItem { kind, marker } => {
                let indent_len = text.len() - text.trim_start_matches([' ', '\t']).len();
                if indent_len > 0 {
                    b.text(0..indent_len, &text[..indent_len], base);
                }
                let muted = RunStyle { color: c.muted_foreground, ..base };
                match kind {
                    ListKind::Bullet => {
                        b.atom(indent_len..marker, "•\u{2002}", muted, None);
                    }
                    ListKind::Ordered => {
                        let digits = text[indent_len..marker].trim_end();
                        b.atom(indent_len..marker, &format!("{digits}\u{2002}"), muted, None);
                    }
                    ListKind::Task { checked } => {
                        let icon = if checked { IconName::SquareCheck.path() } else { IconName::Square.path() };
                        b.atom(indent_len..marker, "", muted, Some((icon, c.muted_foreground)));
                    }
                }
                content_start = marker;
            }
            _ => {}
        }
    }

    // Inline content: chips first, then marks on the rest.
    let tokens = &line.parse.tokens;
    let pieces = &line.parse.pieces;
    let content = content_start..text.len();
    let delimiter_visible = |range: &Range<usize>| -> bool {
        // A delimiter shows while the caret or selection touches its styled region.
        let Some(active) = ctx.active.as_ref().filter(|_| ctx.focused) else { return false };
        let mut start = range.start;
        let mut end = range.end;
        // Grow over adjacent marked pieces and delimiters.
        let covered = |at: usize| pieces.iter().find(|p| p.range.contains(&at));
        while start > content.start {
            match covered(start - 1) {
                Some(p) if p.marks.is_empty() => break,
                Some(p) => start = p.range.start,
                None => start -= 1,
            }
        }
        while end < content.end {
            match covered(end) {
                Some(p) if p.marks.is_empty() => break,
                Some(p) => end = p.range.end,
                None => end += 1,
            }
        }
        active.start <= end && active.end >= start
    };
    let style_for = |marks: Marks| -> RunStyle {
        let mut s = base;
        if marks.has(RichTextMark::Bold) {
            s.weight = FontWeight::BOLD;
        }
        if marks.has(RichTextMark::Italic) {
            s.italic = true;
        }
        if marks.has(RichTextMark::Strike) {
            s.strike = true;
        }
        if marks.has(RichTextMark::Code) {
            s.mono = true;
        }
        s
    };
    let marker_style = RunStyle { color: c.muted_foreground.opacity(0.75), ..base };
    let mut at = content.start;
    let mut token_iter = tokens.iter().filter(|t| t.range.start >= content.start).peekable();
    while at < content.end {
        if let Some(token) = token_iter.peek().filter(|t| t.range.start == at) {
            let ink = c.chip_ink(&token.kind);
            let style = RunStyle { color: ink, weight: FontWeight::MEDIUM, ..base };
            let disp = b.atom(token.range.clone(), &token.label, style, Some((chip_icon(&token.kind), ink)));
            b.chips.push((token.range.clone(), disp));
            at = token.range.end;
            token_iter.next();
            continue;
        }
        let next_token = token_iter.peek().map_or(content.end, |t| t.range.start);
        if !ctx.rich || pieces.is_empty() {
            b.text(at..next_token, &text[at..next_token], base);
            at = next_token;
            continue;
        }
        // Next piece boundary or delimiter gap.
        match pieces.iter().find(|p| p.range.contains(&at)) {
            Some(piece) => {
                let end = piece.range.end.min(next_token);
                let style = style_for(piece.marks);
                let start_disp = b.text.len();
                b.text(at..end, &text[at..end], style);
                if piece.marks.has(RichTextMark::Code) {
                    b.code_boxes.push(start_disp..b.text.len());
                }
                at = end;
            }
            None => {
                let gap_end = pieces.iter().map(|p| p.range.start).filter(|s| *s > at).min().unwrap_or(content.end).min(next_token);
                let gap = at..gap_end;
                if delimiter_visible(&gap) {
                    b.text(gap.clone(), &text[gap.clone()], marker_style);
                } else if !gap.is_empty() && text[gap.clone()].bytes().all(|c| c == b'`') {
                    // Inline code's 0.25em side padding: a thin space stands in for the hidden backticks.
                    b.atom(gap.clone(), "\u{2009}", base, None);
                } else {
                    b.hidden(gap.clone());
                }
                at = gap_end;
            }
        }
    }
    if b.segs.is_empty() || b.segs.last().is_some_and(|s| s.src.end < text.len()) {
        b.hidden(text.len()..text.len());
    }
    finish(b, line, font_size, line_height, quote, false, code_block)
}

fn finish(
    b: Builder,
    line: &DocLine,
    font_size: Pixels,
    line_height: Pixels,
    quote: bool,
    rule: bool,
    code_block: Option<usize>,
) -> DisplayLine {
    DisplayLine {
        parse: line.parse.clone(),
        text: b.text.into(),
        runs: b.runs,
        segs: b.segs,
        font_size,
        line_height,
        indent: if quote { QUOTE_INDENT } else { px(0.) },
        direction: line.direction,
        icons: b.icons,
        code_boxes: b.code_boxes,
        chips: b.chips,
        quote,
        rule,
        code_block,
        right_inset: px(0.),
        header_icon: None,
        collapsed: false,
        shaped: RefCell::new(None),
    }
}

/// Display lines kept across frames, keyed by everything a line's display depends on.
#[derive(Default)]
pub struct DisplayCache {
    entries: HashMap<u64, Rc<DisplayLine>>,
    highlight: HighlightCache,
}

/// A line placed in the editor's content.
pub struct LaidLine {
    /// Top of the line's text, below its spacing.
    pub y: Pixels,
    pub height: Pixels,
    /// Spacing after the text, before the next line's spacing.
    pub below: Pixels,
    pub wrapped: Rc<WrappedLine>,
    pub display: Rc<DisplayLine>,
    pub source_start: usize,
}

/// The last frame's layout, kept on the editor for hit testing and caret movement.
pub struct EditorLayout {
    pub lines: Vec<LaidLine>,
    pub bounds: Bounds<Pixels>,
    pub content_height: Pixels,
    pub wrap_width: Pixels,
}

impl LaidLine {
    /// Display indices where each visual row starts.
    fn row_starts(&self) -> Vec<usize> {
        let layout = &self.wrapped.unwrapped_layout;
        let mut starts = vec![0];
        for boundary in self.wrapped.wrap_boundaries() {
            starts.push(layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index);
        }
        starts
    }

    fn row_count(&self) -> usize {
        self.wrapped.wrap_boundaries().len() + 1
    }

    /// X offset of a row from right alignment.
    fn row_align_x(&self, row: usize, wrap_width: Pixels) -> Pixels {
        if self.display.direction != TextDirection::Rtl {
            return px(0.);
        }
        let starts = self.row_starts();
        let layout = &self.wrapped.unwrapped_layout;
        let start = starts[row];
        let end = starts.get(row + 1).copied().unwrap_or(layout.len);
        let width = layout.x_for_index(end) - layout.x_for_index(start);
        (wrap_width - self.display.indent - width).max(px(0.))
    }

    /// (row, x) of a display index.
    fn locate(&self, disp: usize, wrap_width: Pixels) -> (usize, Pixels) {
        let lh = self.display.line_height;
        let p = self.wrapped.position_for_index(disp, lh).unwrap_or_default();
        let row = (p.y / lh).floor() as usize;
        (row, self.display.indent + self.row_align_x(row, wrap_width) + p.x)
    }

    fn disp_at(&self, local: Point<Pixels>, wrap_width: Pixels) -> usize {
        let lh = self.display.line_height;
        let row = ((local.y / lh).floor().max(0.) as usize).min(self.row_count() - 1);
        let x = local.x - self.display.indent - self.row_align_x(row, wrap_width);
        let p = point(x.max(px(0.)), lh * row as f32 + lh / 2.);
        match self.wrapped.closest_index_for_position(p, lh) {
            Ok(i) | Err(i) => i,
        }
    }
}

impl EditorLayout {
    fn line_for(&self, doc: &Doc, offset: usize) -> Option<(usize, &LaidLine)> {
        let index = doc.line_at(offset);
        self.lines.get(index).map(|l| (index, l))
    }

    /// The visual row of an offset among all rows, and the row count.
    pub fn row_of(&self, doc: &Doc, offset: usize) -> (usize, usize) {
        let total = self.lines.iter().map(LaidLine::row_count).sum();
        let Some((index, line)) = self.line_for(doc, offset) else { return (0, total) };
        let before: usize = self.lines[..index].iter().map(LaidLine::row_count).sum();
        let disp = line.display.disp_for_src(offset - line.source_start);
        (before + line.locate(disp, self.wrap_width).0, total)
    }

    /// Caret position in content coordinates (top of its row).
    pub fn position_for_offset(&self, doc: &Doc, offset: usize) -> Option<Point<Pixels>> {
        let (_, line) = self.line_for(doc, offset)?;
        let disp = line.display.disp_for_src(offset - line.source_start);
        let (row, x) = line.locate(disp, self.wrap_width);
        Some(point(x, line.y + line.display.line_height * row as f32))
    }

    pub fn row_height_at(&self, doc: &Doc, offset: usize) -> Pixels {
        self.line_for(doc, offset).map_or(PROMPT_LINE_HEIGHT, |(_, l)| l.display.line_height)
    }

    pub fn visible_height(&self) -> Pixels {
        self.bounds.size.height
    }

    /// The source offset nearest a point in content coordinates.
    pub fn offset_for_content_point(&self, _doc: &Doc, p: Point<Pixels>) -> Option<usize> {
        let line = self
            .lines
            .iter()
            .filter(|l| !l.display.collapsed)
            .find(|l| p.y < l.y + l.height + l.below)
            .or(self.lines.last())?;
        let disp = line.disp_at(point(p.x, (p.y - line.y).max(px(0.))), self.wrap_width);
        Some(line.source_start + line.display.src_for_disp(disp))
    }

    pub fn offset_for_window_point(&self, doc: &Doc, p: Point<Pixels>, scroll_y: Pixels) -> Option<usize> {
        let local = p - self.bounds.origin;
        self.offset_for_content_point(doc, point(local.x, local.y + scroll_y))
    }

    /// Source range of the visual row holding `offset`.
    pub fn row_range_at(&self, doc: &Doc, offset: usize) -> Option<Range<usize>> {
        let (index, line) = self.line_for(doc, offset)?;
        let disp = line.display.disp_for_src(offset - line.source_start);
        let (row, _) = line.locate(disp, self.wrap_width);
        let starts = line.row_starts();
        let start = starts[row];
        let end = starts.get(row + 1).copied().unwrap_or(line.display.text.len());
        let doc_line = &doc.lines[index];
        let content_start = doc_line.range.start + ComposerPromptEditor::hidden_block_marker(doc_line);
        let s = (line.source_start + line.display.src_for_disp(start)).max(content_start);
        let e = line.source_start + line.display.src_for_disp(end);
        Some(s..e)
    }

    /// Caret bounds in window coordinates, for IME candidate windows.
    pub fn window_bounds_for_offset(&self, doc: &Doc, offset: usize, scroll_y: Pixels) -> Option<Bounds<Pixels>> {
        let p = self.position_for_offset(doc, offset)?;
        let h = self.row_height_at(doc, offset);
        Some(Bounds::new(self.bounds.origin + point(p.x, p.y - scroll_y), size(px(2.), h)))
    }
}

/// The editor's element. Build it with [`ComposerPromptEditor::element`].
pub struct PromptEditorElement {
    editor: Entity<ComposerPromptEditor>,
    colors: ComposerColors,
}

impl PromptEditorElement {
    pub fn new(editor: Entity<ComposerPromptEditor>, cx: &App) -> Self {
        Self { editor, colors: ComposerColors::new(cx) }
    }
}

thread_local! {
    static DISPLAY_CACHES: RefCell<HashMap<gpui_kit::EntityId, DisplayCache>> = RefCell::new(HashMap::new());
}

fn display_lines(editor: &ComposerPromptEditor, colors: &ComposerColors, focused: bool, cache: &mut DisplayCache) -> Vec<Rc<DisplayLine>> {
    let doc = &editor.doc;
    let selection = editor.selection();
    let rich = editor.rich_text;
    let text = editor.text();
    let mut used = Vec::with_capacity(doc.lines.len());
    let mut highlighted: HashMap<usize, Arc<Vec<Vec<CodeSpan>>>> = HashMap::new();
    let lines = doc
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let touches = selection.start <= line.range.end && selection.end >= line.range.start;
            let active = (focused && touches).then(|| {
                let start = selection.start.max(line.range.start) - line.range.start;
                let end = selection.end.min(line.range.end) - line.range.start;
                start..end.max(start)
            });
            let code = match (&line.kind, rich) {
                (LineKind::Code { block }, true) => {
                    let spans = highlighted.entry(*block).or_insert_with(|| {
                        let b = &doc.code_blocks[*block];
                        let body: Vec<&str> = (b.lines.start + 1..b.lines.end)
                            .filter(|i| matches!(doc.lines[*i].kind, LineKind::Code { .. }))
                            .map(|i| &text[doc.lines[i].range.clone()])
                            .collect();
                        cache.highlight.get(&body, &b.language, colors.appearance)
                    });
                    spans.get(index - doc.code_blocks[*block].lines.start - 1).cloned()
                }
                _ => None,
            };
            // Only lines with markers, chips or a rule care where the caret is.
            let caret_matters = rich
                && (line.parse.pieces.iter().any(|p| !p.marks.is_empty())
                    || matches!(line.kind, LineKind::Rule | LineKind::FenceOpen { .. } | LineKind::FenceClose { .. }));
            let mut h = std::collections::hash_map::DefaultHasher::new();
            Arc::as_ptr(&line.parse).hash(&mut h);
            line.kind.hash(&mut h);
            line.direction.hash(&mut h);
            rich.hash(&mut h);
            colors.is_dark().hash(&mut h);
            f32::from(colors.rem).to_bits().hash(&mut h);
            f32::from(colors.code_font_size).to_bits().hash(&mut h);
            if caret_matters {
                active.hash(&mut h);
            }
            if let Some(code) = &code {
                for span in code {
                    span.range.hash(&mut h);
                    span.color.to_rgb().r.to_bits().hash(&mut h);
                    span.color.to_rgb().g.to_bits().hash(&mut h);
                    span.color.to_rgb().b.to_bits().hash(&mut h);
                }
            }
            let key = h.finish();
            used.push(key);
            cache
                .entries
                .entry(key)
                .or_insert_with(|| {
                    Rc::new(build_display_line(line, &BuildCtx { colors, rich, focused, active: active.clone(), code: code.as_deref() }))
                })
                .clone()
        })
        .collect();
    if cache.entries.len() > used.len() * 2 + 32 {
        let used: std::collections::HashSet<u64> = used.into_iter().collect();
        cache.entries.retain(|k, _| used.contains(k));
    }
    cache.highlight.end_frame();
    lines
}

/// Space above and below a line's text.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineSpace {
    pub above: Pixels,
    pub below: Pixels,
}

/// Which margin group a line's block belongs to; neighbours in one group share margins.
#[derive(PartialEq)]
enum MarginGroup {
    None,
    Heading,
    List,
    Quote,
    Rule,
    Code(usize),
}

/// The web composer's block spacing (`index.css` `.composer-tiptap`): lists,
/// quotes, rules and code blocks have 0.65rem margins, headings 1.25rem above
/// and 0.5rem below, adjacent margins collapse, and the draft's first and last
/// blocks have none. A code block adds its border and padding: 1px + 0.375rem
/// above the header, 0.25rem above the code, 0.5rem + 1px below it.
fn line_spacing(doc: &Doc, lines: &[Rc<DisplayLine>], rich: bool, rem: Pixels) -> Vec<LineSpace> {
    let mut space = vec![LineSpace::default(); lines.len()];
    if !rich {
        return space;
    }
    let group = |i: usize| match doc.lines.get(i).map(|l| &l.kind) {
        Some(LineKind::Heading { .. }) => MarginGroup::Heading,
        Some(LineKind::ListItem { .. }) => MarginGroup::List,
        Some(LineKind::Quote { .. }) => MarginGroup::Quote,
        Some(LineKind::Rule) => MarginGroup::Rule,
        Some(kind) => kind.code_block().map_or(MarginGroup::None, MarginGroup::Code),
        None => MarginGroup::None,
    };
    let margins = |g: &MarginGroup| match g {
        MarginGroup::None => (px(0.), px(0.)),
        MarginGroup::Heading => (rem * 1.25, rem * 0.5),
        _ => (rem * 0.65, rem * 0.65),
    };
    let visible = |i: usize| lines.get(i).is_some_and(|l| !l.collapsed);
    for i in 0..lines.len() {
        let g = group(i);
        if i > 0 {
            let prev = group(i - 1);
            let joined = g == prev && !matches!(g, MarginGroup::Heading | MarginGroup::Rule | MarginGroup::None);
            if !joined {
                space[i].above = margins(&prev).1.max(margins(&g).0);
            }
        }
        if let MarginGroup::Code(block) = g {
            let kind = &doc.lines[i].kind;
            if matches!(kind, LineKind::FenceOpen { .. }) {
                space[i].above += px(1.) + rem * 0.375;
            } else if i > 0 && matches!(doc.lines[i - 1].kind, LineKind::FenceOpen { .. }) {
                space[i].above += rem * 0.25;
            }
            let next_visible = (i + 1..lines.len()).find(|j| visible(*j));
            let last = visible(i) && next_visible.is_none_or(|j| group(j) != MarginGroup::Code(block));
            if last {
                space[i].below += rem * 0.5 + px(1.);
            }
        }
    }
    space
}

/// Shapes every line at a wrap width. A line keeps its shaped result across
/// frames, so a keystroke shapes only the line it changed.
fn shape(lines: &[Rc<DisplayLine>], space: &[LineSpace], width: Pixels, window: &mut Window) -> (Vec<Rc<WrappedLine>>, Pixels) {
    let mut total = space.iter().fold(px(0.), |a, s| a + s.above + s.below);
    let shaped = lines
        .iter()
        .map(|line| {
            let wrap = (width - line.indent - line.right_inset).max(px(1.));
            let cached = line.shaped.borrow().as_ref().filter(|(w, _)| *w == wrap).map(|(_, l)| l.clone());
            let wrapped = cached.unwrap_or_else(|| {
                let wrapped = Rc::new(if line.text.is_empty() {
                    WrappedLine::default()
                } else {
                    window
                        .text_system()
                        .shape_text(line.text.clone(), line.font_size, &line.runs, Some(wrap), None)
                        .ok()
                        .and_then(|mut v| v.pop())
                        .unwrap_or_default()
                });
                *line.shaped.borrow_mut() = Some((wrap, wrapped.clone()));
                wrapped
            });
            let rows = wrapped.wrap_boundaries().len() + 1;
            total += line.line_height * rows as f32;
            wrapped
        })
        .collect();
    (shaped, total)
}

/// Measured state shared between layout and prepaint.
#[derive(Default)]
pub struct MeasureState {
    lines: Vec<Rc<DisplayLine>>,
    space: Vec<LineSpace>,
    shaped: Option<(Pixels, Vec<Rc<WrappedLine>>, Pixels)>,
    placeholder: Option<(Pixels, WrappedLine)>,
}

pub struct Prepaint {
    hitbox: Hitbox,
    layout: Rc<EditorLayout>,
    placeholder: Option<WrappedLine>,
    scroll_y: Pixels,
    focused: bool,
}

impl Element for PromptEditorElement {
    type RequestLayoutState = Rc<RefCell<MeasureState>>;
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        Some("composer-prompt-editor".into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let editor = self.editor.read(cx);
        let focused = editor.focus_handle.is_focused(window);
        let colors = self.colors;
        let id = self.editor.entity_id();
        let lines = DISPLAY_CACHES.with(|caches| {
            let mut caches = caches.borrow_mut();
            display_lines(editor, &colors, focused, caches.entry(id).or_default())
        });
        let last_width = editor.layout.borrow().as_ref().map_or(px(600.), |l| l.wrap_width);
        let show_placeholder = editor.text().is_empty();
        let placeholder: SharedString = editor.placeholder.clone();
        let space = line_spacing(&editor.doc, &lines, editor.rich_text, colors.rem);
        let state = Rc::new(RefCell::new(MeasureState { lines, space, shaped: None, placeholder: None }));
        let measure_state = state.clone();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let layout_id = window.request_measured_layout(style, move |known, available, window, _cx| {
            // Intrinsic-size passes give no definite width; use the last laid-out
            // width so they hit the same shaped lines instead of reshaping.
            let width = known.width.unwrap_or(match available.width {
                AvailableSpace::Definite(w) => w,
                _ => last_width,
            });
            let mut state = measure_state.borrow_mut();
            let (shaped, height) = shape(&state.lines, &state.space, width, window);
            let mut height = height;
            if show_placeholder {
                let run = TextRun { len: placeholder.len(), font: font(UI_FONT_FAMILY), color: colors.placeholder, background_color: None, underline: None, strikethrough: None };
                let wrapped = window
                    .text_system()
                    .shape_text(placeholder.clone(), PROMPT_FONT_SIZE, &[run], Some(width), None)
                    .ok()
                    .and_then(|mut v| v.pop())
                    .unwrap_or_default();
                height = height.max(PROMPT_LINE_HEIGHT * (wrapped.wrap_boundaries().len() + 1) as f32);
                state.placeholder = Some((width, wrapped));
            }
            state.shaped = Some((width, shaped, height));
            size(width, height.clamp(EDITOR_MIN_HEIGHT, EDITOR_MAX_HEIGHT))
        });
        (layout_id, state)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let mut measured = state.borrow_mut();
        let width = bounds.size.width;
        if measured.shaped.as_ref().is_none_or(|(w, ..)| *w != width) {
            let (shaped, height) = shape(&measured.lines, &measured.space, width, window);
            measured.shaped = Some((width, shaped, height));
        }
        let (_, shaped, content_height) = measured.shaped.take().unwrap_or_default();
        let placeholder = measured.placeholder.take().map(|(_, w)| w);
        let mut y = px(0.);
        let mut source_lines = self.editor.read(cx).doc.lines.iter().map(|l| l.range.start);
        let lines: Vec<LaidLine> = measured
            .lines
            .iter()
            .zip(shaped)
            .zip(measured.space.iter())
            .map(|((display, wrapped), space)| {
                let height = display.line_height * (wrapped.wrap_boundaries().len() + 1) as f32;
                y += space.above;
                let laid = LaidLine { y, height, below: space.below, wrapped, display: display.clone(), source_start: source_lines.next().unwrap_or(0) };
                y += height + space.below;
                laid
            })
            .collect();
        let layout = Rc::new(EditorLayout { lines, bounds, content_height, wrap_width: width });
        let editor = self.editor.read(cx);
        let focused = editor.focus_handle.is_focused(window);
        let focus_changed = editor.focus_changed(focused);
        let max = (content_height - bounds.size.height).max(px(0.));
        let mut scroll_y = editor.scroll_y.get();
        if editor.scroll_to_caret.replace(false)
            && let Some(p) = layout.position_for_offset(&editor.doc, editor.cursor())
        {
            let h = layout.row_height_at(&editor.doc, editor.cursor());
            if p.y < scroll_y {
                scroll_y = p.y;
            } else if p.y + h > scroll_y + bounds.size.height {
                scroll_y = p.y + h - bounds.size.height;
            }
        }
        let scroll_y = scroll_y.clamp(px(0.), max);
        editor.scroll_y.set(scroll_y);
        *editor.layout.borrow_mut() = Some(layout.clone());
        if focus_changed {
            let entity = self.editor.clone();
            window.defer(cx, move |_, cx| entity.update(cx, |e, cx| e.sync_focus(focused, cx)));
        }
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Prepaint { hitbox, layout, placeholder, scroll_y, focused }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let editor_entity = self.editor.clone();
        let editor = self.editor.read(cx);
        let focus = editor.focus_handle.clone();
        let disabled = editor.disabled;
        window.handle_input(&focus, ElementInputHandler::new(bounds, editor_entity.clone()), cx);
        if !disabled {
            window.set_cursor_style(CursorStyle::IBeam, &prepaint.hitbox);
        }
        self.register_mouse(&prepaint.hitbox, window);

        let c = self.colors;
        let layout = prepaint.layout.clone();
        let scroll_y = prepaint.scroll_y;
        let origin = bounds.origin - point(px(0.), scroll_y);
        let selection = editor.selection();
        let marked = editor.marked_range_for_paint();
        let cursor = editor.cursor();
        let doc = editor.doc.clone();
        let doc = &*doc;
        let visible = scroll_y..scroll_y + bounds.size.height;

        window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
            // Code blocks: a rounded, bordered box from the header to the last
            // visible line (`MarkdownCodeBlockFrame`: rounded-lg, 1px border,
            // header pt-1.5, pre pt-1 pb-2).
            let mut block_spans: HashMap<usize, (Pixels, Pixels)> = HashMap::new();
            for line in layout.lines.iter().filter(|l| !l.display.collapsed) {
                if let Some(block) = line.display.code_block {
                    let top = line.y - px(1.) - c.rem * 0.375;
                    let bottom = line.y + line.height + c.rem * 0.5 + px(1.);
                    let entry = block_spans.entry(block).or_insert((top, bottom));
                    entry.1 = bottom;
                }
            }
            for (top, bottom) in block_spans.values() {
                let rect = Bounds::from_corners(origin + point(px(0.), *top), origin + point(layout.wrap_width, *bottom));
                window.paint_quad(
                    fill(rect, c.code_background).corner_radii(px(8.)).border_widths(px(1.)).border_color(c.border),
                );
            }
            for (index, line) in layout.lines.iter().enumerate() {
                if line.y + line.height < visible.start || line.y > visible.end {
                    continue;
                }
                let line_origin = origin + point(line.display.indent, line.y);
                let lh = line.display.line_height;
                let doc_line = &doc.lines[index];
                if line.display.quote {
                    let rule = Bounds::new(origin + point(px(0.), line.y), size(px(2.), line.height));
                    window.paint_quad(fill(rule, c.foreground.opacity(0.35)));
                }
                if line.display.rule {
                    let y = line.y + line.height / 2.;
                    let rule = Bounds::new(origin + point(px(0.), y), size(layout.wrap_width, px(1.)));
                    window.paint_quad(fill(rule, c.muted_foreground));
                }
                // Inline code boxes.
                // `.composer-tiptap :not(pre) > code`: 0.92em, padding 0.05em 0.25em,
                // radius 0.3em; the chip is the code font's content box plus padding.
                let em = line.display.font_size * 0.92;
                for code in &line.display.code_boxes {
                    paint_range_boxes(line, code.clone(), origin, layout.wrap_width, window, |b| {
                        let h = em * 1.11;
                        let top = b.top() + (b.size.height - h) / 2. + px(1.);
                        let chip = Bounds::new(point(b.left() - em * 0.25, top), size(b.size.width + em * 0.5, h));
                        fill(chip, c.muted_foreground.opacity(0.12)).corner_radii(em * 0.3)
                    });
                }
                // Selection.
                if !selection.is_empty() {
                    let start = selection.start.max(doc_line.range.start);
                    let end = selection.end.min(doc_line.range.end);
                    let covers_newline = selection.end > doc_line.range.end && selection.start <= doc_line.range.end;
                    if start <= end && (start < end || covers_newline) {
                        let ds = line.display.disp_for_src(start - doc_line.range.start);
                        let de = line.display.disp_for_src(end - doc_line.range.start);
                        paint_range_boxes(line, ds..de, origin, layout.wrap_width, window, |b| fill(b, c.selection));
                        if covers_newline {
                            let (row, x) = line.locate(de, layout.wrap_width);
                            let b = Bounds::new(origin + point(x, line.y + lh * row as f32), size(px(6.), lh));
                            window.paint_quad(fill(b, c.selection));
                        }
                    }
                }
                let align = if line.display.direction == TextDirection::Rtl { TextAlign::Right } else { TextAlign::Left };
                let align_bounds = Bounds::new(line_origin, size(layout.wrap_width - line.display.indent, line.height));
                line.wrapped.paint(line_origin, lh, align, Some(align_bounds), window, cx).ok();
                if let Some(icon) = line.display.header_icon {
                    let s = px(14.);
                    let top = line.y + (lh - s) / 2.;
                    let icon_box = Bounds::new(origin + point(line.display.indent - px(16.), top), size(s, s));
                    for layer in icon.layers() {
                        let color = layer.tint.color(c.appearance, c.muted_foreground);
                        window.paint_svg(icon_box, layer.asset.into(), None, TransformationMatrix::unit(), color, cx).ok();
                    }
                    let label_end = line.wrapped.unwrapped_layout.width;
                    let chevron = Bounds::new(origin + point(line.display.indent + label_end + px(2.), top), size(s, s));
                    let ink = c.muted_foreground.opacity(0.6);
                    window.paint_svg(chevron, IconName::ChevronDown.path().into(), None, TransformationMatrix::unit(), ink, cx).ok();
                }
                for icon in &line.display.icons {
                    let (row, x) = line.locate(icon.disp, layout.wrap_width);
                    let s = line.display.font_size;
                    let top = line.y + lh * row as f32 + (lh - s) / 2.;
                    let b = Bounds::new(origin + point(x, top), size(s, s));
                    window.paint_svg(b, icon.path.into(), None, TransformationMatrix::unit(), icon.color, cx).ok();
                }
                // Chips: a ring when selected alone, a tint inside a wider selection.
                for (src, disp) in &line.display.chips {
                    let abs = doc_line.range.start + src.start..doc_line.range.start + src.end;
                    if selection == abs {
                        paint_range_boxes(line, disp.clone(), origin, layout.wrap_width, window, |b| {
                            gpui_kit::outline(b, c.ring, gpui_kit::BorderStyle::Solid).corner_radii(px(6.))
                        });
                    } else if selection.start <= abs.start && selection.end >= abs.end && !selection.is_empty() {
                        paint_range_boxes(line, disp.clone(), origin, layout.wrap_width, window, |b| {
                            fill(b, c.ring.opacity(0.3)).corner_radii(px(6.))
                        });
                    }
                }
                if let Some(marked) = marked.clone()
                    && marked.start >= doc_line.range.start
                    && marked.end <= doc_line.range.end
                {
                    let ds = line.display.disp_for_src(marked.start - doc_line.range.start);
                    let de = line.display.disp_for_src(marked.end - doc_line.range.start);
                    paint_range_boxes(line, ds..de, origin, layout.wrap_width, window, |b| {
                        let underline = Bounds::new(point(b.left(), b.bottom() - px(3.)), size(b.size.width, px(1.)));
                        fill(underline, c.foreground)
                    });
                }
            }
            if let Some(placeholder) = prepaint.placeholder.as_ref() {
                placeholder.paint(origin, PROMPT_LINE_HEIGHT, TextAlign::Left, None, window, cx).ok();
            }
            if prepaint.focused && selection.is_empty() && !disabled {
                if let Some(p) = layout.position_for_offset(doc, cursor) {
                    let lh = layout.row_height_at(doc, cursor);
                    let font_size = layout.lines.get(doc.line_at(cursor)).map_or(PROMPT_FONT_SIZE, |l| l.display.font_size);
                    let h = font_size * 1.25;
                    let caret = Bounds::new(origin + point(p.x, p.y + (lh - h) / 2.), size(px(1.5), h));
                    window.paint_quad(fill(caret, c.foreground));
                }
            }
        });
    }
}

/// Paints one quad per visual row a display range covers.
fn paint_range_boxes(
    line: &LaidLine,
    range: Range<usize>,
    origin: Point<Pixels>,
    wrap_width: Pixels,
    window: &mut Window,
    quad: impl Fn(Bounds<Pixels>) -> gpui_kit::PaintQuad,
) {
    let lh = line.display.line_height;
    let (start_row, start_x) = line.locate(range.start, wrap_width);
    let (end_row, end_x) = line.locate(range.end, wrap_width);
    for row in start_row..=end_row {
        let left = if row == start_row { start_x } else { line.display.indent };
        let right = if row == end_row { end_x } else { wrap_width };
        if right <= left {
            continue;
        }
        let top = line.y + lh * row as f32;
        let b = Bounds::from_corners(origin + point(left, top), origin + point(right, top + lh));
        window.paint_quad(quad(b));
    }
}

impl PromptEditorElement {
    fn register_mouse(&self, hitbox: &Hitbox, window: &mut Window) {
        let editor = self.editor.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && hb.is_hovered(window) {
                editor.update(cx, |editor, cx| {
                    editor.on_mouse_down_at(event.position, event.click_count, event.modifiers.shift, window, cx)
                });
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left) {
                editor.update(cx, |editor, cx| editor.on_mouse_drag_to(event.position, cx));
            }
        });
        let editor = self.editor.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                editor.update(cx, |editor, _| editor.on_mouse_up());
            }
        });
        let editor = self.editor.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                let delta = event.delta.pixel_delta(PROMPT_LINE_HEIGHT).y;
                editor.update(cx, |editor, cx| editor.scroll_by(delta, cx));
            }
        });
    }
}
