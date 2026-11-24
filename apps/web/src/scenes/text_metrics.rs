//! Text metrics table: renders rows of text, each in one family, weight, size and line
//! height, at the positions a table file gives, and writes each row's advance width and
//! baseline to JSON, so a run can be compared with the same table laid out by a browser.
//!
//! `T3CODE_TEXT_TABLE=<table.json>` names the table (`styles` and placed `rows`), and
//! `T3CODE_TEXT_TABLE_OUT=<out.json>` where the measurements go. A style's family
//! `system` is the UI font, or `T3CODE_TEXT_TABLE_SANS` when set.

use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::appearance_fonts::ui_font_family;

pub fn build(_: &mut Window, cx: &mut App) -> AnyView {
    let table = std::env::var("T3CODE_TEXT_TABLE")
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    cx.new(|_| TextMetricsScene { table, written: false }).into()
}

#[derive(Default, Deserialize)]
struct Table {
    styles: Vec<Style>,
    rows: Vec<Row>,
}

#[derive(Deserialize)]
struct Style {
    family: String,
    weight: f32,
    size: f32,
    /// `None` is CSS `normal`; the scene then uses the rounded ascent plus descent.
    line_height: Option<f32>,
}

#[derive(Deserialize)]
struct Row {
    style: usize,
    text: SharedString,
    x: f32,
    y: f32,
}

#[derive(Serialize)]
struct Measured {
    width: f32,
    ascent: f32,
    descent: f32,
    line_height: f32,
    /// Baseline below the row's top, as painted.
    baseline: f32,
}

struct TextMetricsScene {
    table: Table,
    written: bool,
}

impl TextMetricsScene {
    fn font(style: &Style) -> Font {
        let family: SharedString = match style.family.as_str() {
            "system" => std::env::var("T3CODE_TEXT_TABLE_SANS").map(Into::into).unwrap_or_else(|_| ui_font_family()),
            name => SharedString::from(name.to_owned()),
        };
        Font { family, features: FontFeatures::default(), fallbacks: None, weight: FontWeight(style.weight), style: FontStyle::Normal }
    }

    fn line_height(style: &Style, ascent: Pixels, descent: Pixels) -> Pixels {
        style.line_height.map(px).unwrap_or_else(|| px(ascent.0.round() + descent.0.round()))
    }

    /// Shapes every row as the text element would and writes the measurements once.
    fn write_measurements(&self, window: &mut Window) {
        let Ok(out) = std::env::var("T3CODE_TEXT_TABLE_OUT") else { return };
        let measured: Vec<Measured> = self
            .table
            .rows
            .iter()
            .map(|row| {
                let style = &self.table.styles[row.style];
                let run = TextRun { len: row.text.len(), font: Self::font(style), color: white(), background_color: None, underline: None, strikethrough: None };
                let line = window.text_system().shape_line(row.text.clone(), px(style.size), &[run], None);
                let line_height = Self::line_height(style, line.ascent, line.descent);
                Measured {
                    width: line.width.0,
                    ascent: line.ascent.0,
                    descent: line.descent.0,
                    line_height: line_height.0,
                    baseline: line_baseline_offset(line_height, line.ascent, line.descent).0,
                }
            })
            .collect();
        if let Ok(json) = serde_json::to_vec_pretty(&measured) {
            let _ = std::fs::write(out, json);
        }
    }
}

impl Render for TextMetricsScene {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        if !self.written {
            self.written = true;
            self.write_measurements(window);
        }
        div().size_full().relative().bg(black()).text_color(white()).children(self.table.rows.iter().map(|row| {
            let style = &self.table.styles[row.style];
            let font = Self::font(style);
            let font_id = window.text_system().resolve_font(&font);
            let ascent = window.text_system().ascent(font_id, px(style.size));
            let descent = window.text_system().descent(font_id, px(style.size));
            div()
                .absolute()
                .left(px(row.x))
                .top(px(row.y))
                .whitespace_nowrap()
                .font_family(font.family)
                .font_weight(font.weight)
                .text_size(px(style.size))
                .line_height(Self::line_height(style, ascent, descent))
                .child(row.text.clone())
        }))
    }
}
