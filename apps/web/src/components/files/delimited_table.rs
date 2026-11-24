//! CSV and TSV as a table. Ports `DelimitedTablePreview.tsx`, which shows
//! the file through `LargeTextViewer`'s `VirtualTablePreview`: a summary bar
//! (`n rows · n columns · size`), a header row on `muted/60` that stays on
//! top, a 56px row number column that stays at the left edge, cells 128 to
//! 320px wide with `px-3 py-1.5`, virtualized zebra rows.

use std::{ops::Range, sync::Arc};

use gpui_kit::{
    AnyElement, Context, IntoElement, ListHorizontalSizingBehavior, ParentElement as _, Render, SharedString,
    Styled as _, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};

use crate::{ActiveAppTheme as _, components::ui::tooltip::tooltip, file_preview::format_file_size, i18n::t};

/// Row height: `text-xs` (16px line) plus `py-1.5` and the 1px bottom border.
const ROW_HEIGHT: f32 = 29.;
const NUMBER_WIDTH: f32 = 56.;
const MIN_COLUMN: f32 = 128.;
const MAX_COLUMN: f32 = 320.;
/// Columns past this are dropped, as the web viewer shows the first 1,024.
pub const MAX_COLUMNS: usize = 1024;
const MAX_CELL_CHARS: usize = 2000;

/// Parsed rows of a delimited file.
#[derive(Clone, Debug, Default)]
pub struct DelimitedRows {
    pub rows: Vec<Vec<SharedString>>,
    pub columns: usize,
}

/// `csvPreviewDelimiter`: semicolons win only when they are consistent across
/// the first two records and commas are not.
pub fn csv_delimiter(text: &str) -> u8 {
    let (mut commas, mut semicolons) = ([0usize; 2], [0usize; 2]);
    let (mut record, mut second, mut quoted, mut field_start) = (0usize, false, false, true);
    let bytes = text.strip_prefix('\u{feff}').unwrap_or(text).as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let c = bytes[index];
        if quoted {
            if c == b'"' {
                if bytes.get(index + 1) == Some(&b'"') {
                    index += 1;
                } else {
                    quoted = false;
                }
            }
            index += 1;
            continue;
        }
        if c == b'"' && field_start {
            quoted = true;
        } else if c == b'\n' || c == b'\r' {
            if record == 1 {
                break;
            }
            if c == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
            record = 1;
            field_start = true;
            index += 1;
            continue;
        } else if c == b',' {
            commas[record] += 1;
        } else if c == b';' {
            semicolons[record] += 1;
        }
        second |= record == 1;
        field_start = c == b',' || c == b';';
        index += 1;
    }
    if semicolons[0] <= commas[0] {
        return b',';
    }
    if !second {
        return b';';
    }
    let consistent_commas = commas[0] > 0 && commas[1] == commas[0];
    if semicolons[1] == semicolons[0] && !consistent_commas { b';' } else { b',' }
}

/// `parseDelimitedPreview` without a row limit: quoted fields, `""` escapes, CRLF.
pub fn parse_delimited(text: &str, delimiter: u8) -> DelimitedRows {
    let delimiter = if delimiter == b',' { csv_delimiter(text) } else { delimiter } as char;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows: Vec<Vec<SharedString>> = Vec::new();
    let mut row: Vec<SharedString> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut columns = 0;
    let mut chars = text.chars().peekable();
    let mut pending_row = false;
    let end_cell = |row: &mut Vec<SharedString>, cell: &mut String| {
        if row.len() < MAX_COLUMNS {
            row.push(std::mem::take(cell).into());
        } else {
            cell.clear();
        }
    };
    while let Some(c) = chars.next() {
        pending_row = true;
        if c == '"' {
            if quoted && chars.peek() == Some(&'"') {
                cell.push('"');
                chars.next();
                continue;
            }
            if quoted || cell.is_empty() {
                quoted = !quoted;
                continue;
            }
        }
        if !quoted && (c == delimiter || c == '\n' || c == '\r') {
            end_cell(&mut row, &mut cell);
            if c != delimiter {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                columns = columns.max(row.len());
                rows.push(std::mem::take(&mut row));
                pending_row = false;
            }
        } else if cell.chars().count() < MAX_CELL_CHARS {
            cell.push(c);
        }
    }
    if pending_row {
        end_cell(&mut row, &mut cell);
        columns = columns.max(row.len());
        rows.push(row);
    }
    DelimitedRows { rows, columns }
}

/// The table view.
pub struct DelimitedTable {
    rows: Arc<DelimitedRows>,
    widths: Arc<[f32]>,
    byte_length: u64,
    scroll: UniformListScrollHandle,
}

impl DelimitedTable {
    pub fn new(rows: DelimitedRows, byte_length: u64) -> Self {
        // Width from the widest of the first 200 rows, at ~7px per character plus padding.
        let mut widths = vec![MIN_COLUMN; rows.columns];
        for row in rows.rows.iter().take(200) {
            for (index, cell) in row.iter().enumerate() {
                let estimate = cell.chars().count() as f32 * 7. + 24.;
                widths[index] = widths[index].max(estimate.min(MAX_COLUMN));
            }
        }
        Self { rows: Arc::new(rows), widths: widths.into(), byte_length, scroll: UniformListScrollHandle::new() }
    }

    /// Scrolls the cells sideways, as a trackpad does; for checks.
    pub fn scroll_cells_to(&mut self, x: gpui_kit::Pixels, cx: &mut Context<Self>) {
        let handle = self.scroll.0.borrow().base_handle.clone();
        handle.set_offset(gpui_kit::point(-x, handle.offset().y));
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        self.rows.rows.len().saturating_sub(1)
    }

    /// The list's horizontal scroll, which the header row and the row numbers follow.
    fn scroll_x(&self) -> gpui_kit::Pixels {
        self.scroll.0.borrow().base_handle.offset().x
    }

    /// Body rows: data row `index` is file row `index + 1`, under the header.
    /// Row numbers stay at the left edge while the cells scroll sideways.
    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let scroll_x = self.scroll_x();
        range
            .filter_map(|index| self.rows.rows.get(index + 1).map(|row| (index, row)))
            .map(|(index, row)| {
                div()
                    .relative()
                    .flex()
                    .h(px(ROW_HEIGHT))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.6))
                    .when(index % 2 == 1, |el| el.bg(colors.muted.opacity(0.3)))
                    .text_size(px(12.))
                    .child(div().w(px(NUMBER_WIDTH)).flex_none())
                    .children(self.widths.iter().enumerate().map(|(column, width)| {
                        div()
                            .w(px(*width))
                            .flex_none()
                            .h_full()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(colors.border.opacity(0.4))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .children(row.get(column).cloned())
                    }))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(-scroll_x)
                            .w(px(NUMBER_WIDTH))
                            .h_full()
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .justify_end()
                            .border_r_1()
                            .border_color(colors.border)
                            .bg(colors.canvas)
                            .text_color(colors.muted_foreground)
                            .child((index + 1).to_string()),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// The header row, outside the list so it stays on top while rows scroll.
    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let header = self.rows.rows.first();
        div()
            .relative()
            .flex_none()
            .h(px(ROW_HEIGHT))
            .overflow_hidden()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.muted.opacity(0.6))
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .text_size(px(12.))
            .child(
                div().absolute().top_0().left(self.scroll_x()).h_full().flex().pl(px(NUMBER_WIDTH)).children(
                    self.widths.iter().enumerate().map(|(column, width)| {
                        let label: SharedString = header.and_then(|row| row.get(column).cloned()).unwrap_or_else(|| column_label(column).into());
                        div()
                            .id(("table-column", column))
                            .w(px(*width))
                            .flex_none()
                            .h_full()
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .border_r_1()
                            .border_color(colors.border.opacity(0.4))
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .tooltip(tooltip(label.clone()))
                            .child(label)
                    }),
                ),
            )
            .child(
                div().absolute().top_0().left_0().w(px(NUMBER_WIDTH)).h_full().border_r_1().border_color(colors.border).bg(colors.canvas),
            )
    }
}

/// `columnLabel`: A, B, … Z, AA, AB, …
fn column_label(index: usize) -> String {
    let mut index = index + 1;
    let mut label = Vec::new();
    while index > 0 {
        let rem = (index - 1) % 26;
        label.push(b'A' + rem as u8);
        index = (index - 1) / 26;
    }
    label.reverse();
    String::from_utf8(label).unwrap_or_default()
}

impl Render for DelimitedTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let colors = theme.colors;
        let rows = self.row_count();
        let columns = self.rows.columns;
        let summary = format!(
            "{} · {} · {}",
            t!("{count, plural, one {{formatted} row} other {{formatted} rows}}", count = rows, formatted = group_digits(rows)),
            t!("{count, plural, one {{formatted} column} other {{formatted} columns}}", count = columns, formatted = group_digits(columns)),
            if self.byte_length == 0 { t!("0 bytes").to_string() } else { format_file_size(self.byte_length) },
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .min_h(px(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(colors.border.opacity(0.6))
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(summary),
            )
            .when(columns >= MAX_COLUMNS, |el| {
                el.child(
                    div()
                        .px(px(12.))
                        .py(px(6.))
                        .border_b_1()
                        .border_color(colors.warning.opacity(0.2))
                        .bg(colors.warning_surface)
                        .text_size(px(11.))
                        .text_color(colors.warning_foreground)
                        .child(t!("Showing the first 1,024 columns. Source and Save retain every column.")),
                )
            })
            .child(self.render_header(cx))
            .child(
                div()
                    .id("delimited-table-body")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    // The header and row numbers read the scroll offset at render.
                    .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
                    .child(
                        uniform_list("delimited-table", self.row_count(), cx.processor(Self::render_rows))
                            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
                            .track_scroll(&self.scroll)
                            .flex_1(),
                    ),
            )
    }
}

/// `toLocaleString()` in the UI language.
pub fn group_digits(value: usize) -> String {
    crate::i18n::number_format::format_count(value as f64, None)
}
