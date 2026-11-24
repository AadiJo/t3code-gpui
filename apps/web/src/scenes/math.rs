//! Scene `math`: formulas drawn by the chat markdown renderer, one per block, the
//! way the fork's comparison page sets them: an inline formula in `Text $…$ after.`
//! and a display formula alone. `T3CODE_MATH_FORMULAS=<file>` names a JSON list of
//! `[name, tex, display]`; with `T3CODE_MATH_DUMP=<file>` each painted formula is
//! written out for comparison. Math renders when the Render math setting is on
//! (`T3CODE_CLIENT_SETTINGS`); the scene turns it on otherwise.

use std::rc::Rc;

use gpui_kit::{AnyView, App, AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div, px};

use crate::{
    components::chat::{chat_style::ChatColors, markdown_stream_blocks::parse_message_blocks},
    components::chat_markdown::{MarkdownUiState, MarkdownView, render_markdown},
    markdown_pipeline::{MarkdownFlavor, ParsedBlocks},
};

const SAMPLE: &str = r#"[["frac", "\\frac{a}{b}", false], ["quadratic", "x = \\frac{-b \\pm \\sqrt{b^2-4ac}}{2a}", true], ["sum", "\\sum_{i=1}^{n} i", false], ["matrix", "\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}", true], ["error", "\\foo{x}", false]]"#;

struct MathScene {
    blocks: Vec<(String, Vec<ParsedBlocks>)>,
    state: MarkdownUiState,
}

pub fn build(_: &mut Window, cx: &mut App) -> AnyView {
    crate::markdown_pipeline::set_math_enabled(true);
    let list = std::env::var("T3CODE_MATH_FORMULAS").ok().and_then(|path| std::fs::read_to_string(path).ok());
    let formulas: Vec<(String, String, bool)> = serde_json::from_str(list.as_deref().unwrap_or(SAMPLE)).unwrap_or_default();
    let blocks = formulas
        .into_iter()
        .map(|(name, tex, display)| {
            let source = if display { format!("$$\n{tex}\n$$") } else { format!("Text ${tex}$ after.") };
            (name, parse_message_blocks(&source, MarkdownFlavor::ASSISTANT, false))
        })
        .collect();
    cx.new(|_| MathScene { blocks, state: MarkdownUiState::default() }).into()
}

impl Render for MathScene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ChatColors::get(cx);
        let actions: crate::components::chat_markdown::MarkdownActions = Rc::new(|_, _, _| {});
        let items: Vec<_> = self
            .blocks
            .iter()
            .enumerate()
            .map(|(index, (_, pieces))| {
                let view = MarkdownView {
                    key: index as u64 + 1,
                    pieces,
                    colors: &colors,
                    user: false,
                    streaming: false,
                    piece_opacity: None,
                    state: &self.state,
                    actions: actions.clone(),
                    find: None,
                };
                // The comparison page's column: 683px wide with 16px of padding.
                div().w(px(683.)).child(render_markdown(&view, window, cx))
            })
            .collect();
        div().size_full().bg(colors.theme.background()).p(px(16.)).flex().flex_col().gap(px(16.)).children(items)
    }
}
