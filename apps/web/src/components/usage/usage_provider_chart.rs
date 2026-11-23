//! The per-provider usage chart. Ports `UsageProviderChart.tsx`: one series
//! per provider over the days or hours of the window, drawn as GPUI paths in
//! the web chart's 960×260 view box stretched to the plot, with the same nice
//! scale, ticks, monotone curves and colours. Hovering shows a rule and a tooltip.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::{
    Bounds, Context, Hsla, IntoElement, MouseMoveEvent, PathBuilder, Pixels, Point, Render, SharedString, Window,
    canvas, div, point, prelude::*, px,
};
use jiff::tz::TimeZone;
use t3code_contracts::UsageProviderKind;
use t3code_shared::usage_merge::PeriodTotals;

use super::usage_format::{format_day_short, format_hour_short, format_relative_hour_short};
use super::usage_providers::{PROVIDER_ORDER, provider_color, provider_label, provider_mark};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::popover::glass_surface;
use crate::i18n::locales::Locale;
use crate::i18n::number_format::{format_tokens, format_usd};
use crate::i18n::t;
use crate::theme_color::with_alpha;

const VIEW_WIDTH: f32 = 960.;
const VIEW_HEIGHT: f32 = 260.;
const TICK_COUNT: usize = 4;
const PLOT_TOP: f32 = 8.;

/// Days or hours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartResolution {
    Day,
    Hour,
}

/// What the chart plots.
#[derive(Clone)]
pub struct ChartInput {
    pub providers: Vec<UsageProviderKind>,
    pub loading: Vec<UsageProviderKind>,
    /// Day keys (`YYYY-MM-DD`) or hour starts (ISO), oldest first.
    pub periods: Vec<String>,
    pub totals: Rc<[PeriodTotals]>,
    pub by_cost: bool,
    pub resolution: ChartResolution,
    pub zone: TimeZone,
    pub hour12: bool,
    /// The window's end, for "3 PM today" tooltips.
    pub reference_time: Option<String>,
    pub locale: Locale,
}

/// One period: a value per provider in [`PROVIDER_ORDER`] and their sum.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub bands: [f64; 8],
    pub total: f64,
}

/// `buildPeriodColumns`.
pub fn build_period_columns(periods: &[String], totals: &[PeriodTotals], by_cost: bool) -> Vec<Column> {
    periods
        .iter()
        .map(|period| {
            let entry = totals.iter().find(|t| t.key() == period);
            let bands = PROVIDER_ORDER.map(|provider| {
                entry.and_then(|e| e.by_provider.get(&provider)).map_or(0.0, |cell| if by_cost { cell.cost_usd } else { cell.total_tokens })
            });
            Column { bands, total: bands.iter().sum() }
        })
        .collect()
}

/// A scale whose ticks are 1, 2 or 5 times a power of ten (`niceScale`).
pub fn nice_scale(peak: f64, count: usize) -> (f64, Vec<f64>) {
    if peak <= 0.0 {
        return (0.0, vec![0.0]);
    }
    let raw = peak / count as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let step = magnitude
        * if normalized > 5.0 {
            10.0
        } else if normalized > 2.0 {
            5.0
        } else if normalized > 1.0 {
            2.0
        } else {
            1.0
        };
    let max = (peak / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut value = 0.0;
    while value <= max + step * 1e-6 {
        ticks.push(value);
        value += step;
    }
    (max, ticks)
}

/// The scale and whether its ticks get labels (a loading chart with no data
/// shows unlabelled placeholder lines).
#[derive(Clone, Debug, PartialEq)]
pub struct ChartScale {
    pub max: f64,
    pub ticks: Vec<f64>,
    pub labeled: bool,
}

fn chart_scale(columns: &[Column], loading: bool) -> ChartScale {
    // Layered series each start at zero; summing peaks would waste plot height.
    let peak = columns.iter().flat_map(|c| c.bands.iter()).fold(0.0f64, |a, b| a.max(*b));
    if peak == 0.0 && loading {
        return ChartScale { max: TICK_COUNT as f64, ticks: (0..=TICK_COUNT).map(|i| i as f64).collect(), labeled: false };
    }
    let (max, ticks) = nice_scale(peak, TICK_COUNT);
    ChartScale { max, ticks, labeled: true }
}

fn value_to_y(value: f64, max: f64) -> f32 {
    if max == 0.0 { VIEW_HEIGHT } else { VIEW_HEIGHT - (value / max) as f32 * (VIEW_HEIGHT - PLOT_TOP) }
}

/// Fritsch–Carlson tangents for a monotone cubic through `points`.
fn monotone_tangents(points: &[(f32, f32)]) -> Vec<f32> {
    let count = points.len();
    if count < 2 {
        return vec![0.0];
    }
    let slopes: Vec<f32> = points
        .windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            if dx == 0.0 { 0.0 } else { (w[1].1 - w[0].1) / dx }
        })
        .collect();
    let mut tangents = vec![0.0; count];
    tangents[0] = slopes[0];
    tangents[count - 1] = slopes[count - 2];
    for index in 1..count - 1 {
        let (previous, next) = (slopes[index - 1], slopes[index]);
        tangents[index] = if previous * next <= 0.0 { 0.0 } else { (previous + next) / 2.0 };
    }
    for index in 0..count - 1 {
        let slope = slopes[index];
        if slope == 0.0 {
            tangents[index] = 0.0;
            tangents[index + 1] = 0.0;
            continue;
        }
        let a = tangents[index] / slope;
        let b = tangents[index + 1] / slope;
        let magnitude = a * a + b * b;
        if magnitude > 9.0 {
            let scale = 3.0 / magnitude.sqrt();
            tangents[index] = scale * a * slope;
            tangents[index + 1] = scale * b * slope;
        }
    }
    tangents
}

/// One cubic segment in view-box units: from, control 1, control 2, to.
type Segment = [(f32, f32); 4];

fn smooth_curve(points: &[(f32, f32)]) -> Vec<Segment> {
    if points.len() < 2 {
        return Vec::new();
    }
    let tangents = monotone_tangents(points);
    points
        .windows(2)
        .enumerate()
        .map(|(index, w)| {
            let (from, to) = (w[0], w[1]);
            let dx = to.0 - from.0;
            [from, (from.0 + dx / 3.0, from.1 + tangents[index] * dx / 3.0), (to.0 - dx / 3.0, to.1 - tangents[index + 1] * dx / 3.0), to]
        })
        .collect()
}

/// One provider's curve, ready to paint.
#[derive(Clone)]
struct Series {
    provider: UsageProviderKind,
    loading: bool,
    segments: Vec<Segment>,
}

/// Everything derived from a [`ChartInput`], computed once per data change.
struct Prepared {
    input: ChartInput,
    columns: Vec<Column>,
    scale: ChartScale,
    step_x: f32,
    /// Biggest total first, so small series paint on top.
    series: Rc<[Series]>,
    tick_labels: Vec<(f32, SharedString)>,
    axis: [SharedString; 3],
}

impl Prepared {
    fn new(input: ChartInput) -> Self {
        let columns = build_period_columns(&input.periods, &input.totals, input.by_cost);
        let loading_shown = input.providers.iter().any(|p| input.loading.contains(p));
        let scale = chart_scale(&columns, loading_shown);
        let step_x = if input.periods.len() < 2 { 0.0 } else { VIEW_WIDTH / (input.periods.len() - 1) as f32 };
        let mut series: Vec<(f64, Series)> = input
            .providers
            .iter()
            .map(|provider| {
                let slot = PROVIDER_ORDER.iter().position(|p| p == provider).unwrap_or(0);
                let points: Vec<(f32, f32)> =
                    columns.iter().enumerate().map(|(i, c)| (i as f32 * step_x, value_to_y(c.bands[slot], scale.max))).collect();
                let total = columns.iter().map(|c| c.bands[slot]).sum();
                (total, Series { provider: *provider, loading: input.loading.contains(provider), segments: smooth_curve(&points) })
            })
            .collect();
        series.sort_by(|a, b| b.0.total_cmp(&a.0));
        let format = |value: f64| format_value(value, input.by_cost, input.locale);
        let tick_labels = if scale.labeled {
            scale
                .ticks
                .iter()
                .map(|tick| (value_to_y(*tick, scale.max) / VIEW_HEIGHT, if *tick == 0.0 { "0".into() } else { format(*tick).into() }))
                .collect()
        } else {
            Vec::new()
        };
        let period = |index: usize| input.periods.get(index).map(|p| format_period(&input, p).to_uppercase().into()).unwrap_or_default();
        let count = input.periods.len();
        let axis = [period(0), period(count / 2), if count == 0 { SharedString::default() } else { period(count - 1) }];
        Self { columns, scale, step_x, series: series.into_iter().map(|(_, s)| s).collect::<Vec<_>>().into(), tick_labels, axis, input }
    }
}

fn format_value(value: f64, by_cost: bool, locale: Locale) -> String {
    if by_cost { format_usd(value, Some(locale.tag())) } else { format_tokens(value) }
}

fn format_period(input: &ChartInput, period: &str) -> String {
    match input.resolution {
        ChartResolution::Hour => format_hour_short(period, &input.zone, input.hour12, input.locale),
        ChartResolution::Day => format_day_short(period, input.locale),
    }
}

fn format_tooltip_period(input: &ChartInput, period: &str) -> String {
    match (input.resolution, &input.reference_time) {
        (ChartResolution::Hour, Some(reference)) => format_relative_hour_short(period, reference, &input.zone, input.hour12, input.locale),
        _ => format_period(input, period),
    }
}

/// The chart view. Owns its hover state, so hovering repaints only the chart.
pub struct UsageProviderChart {
    prepared: Option<Prepared>,
    hover: Option<(usize, Point<Pixels>)>,
    plot_bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl UsageProviderChart {
    pub fn new() -> Self {
        Self { prepared: None, hover: None, plot_bounds: Rc::default() }
    }

    /// Replaces the plotted data.
    pub fn set_input(&mut self, input: ChartInput, cx: &mut Context<Self>) {
        self.prepared = Some(Prepared::new(input));
        self.hover = None;
        cx.notify();
    }

    /// The scale in use, for checks against the web chart.
    pub fn scale(&self) -> Option<&ChartScale> {
        self.prepared.as_ref().map(|p| &p.scale)
    }

    fn on_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(prepared) = &self.prepared else { return };
        let bounds = self.plot_bounds.get();
        let count = prepared.input.periods.len();
        if count == 0 || bounds.size.width <= px(0.) {
            return;
        }
        let local = event.position - bounds.origin;
        let x = local.x.clamp(px(0.), bounds.size.width);
        let y = local.y.clamp(px(0.), bounds.size.height);
        let index = ((x / bounds.size.width) * (count - 1) as f32).round() as usize;
        let next = Some((index.min(count - 1), point(x, y)));
        if next != self.hover {
            self.hover = next;
            cx.notify();
        }
    }
}

impl Default for UsageProviderChart {
    fn default() -> Self {
        Self::new()
    }
}

fn paint_chart(bounds: Bounds<Pixels>, scale: &ChartScale, series: &[Series], step_x: f32, hover: Option<usize>, colors: &ChartColors, window: &mut Window) {
    let (w, h) = (bounds.size.width / VIEW_WIDTH, bounds.size.height / VIEW_HEIGHT);
    let at = |(x, y): (f32, f32)| point(bounds.origin.x + w * x, bounds.origin.y + h * y);
    for tick in &scale.ticks {
        let y = value_to_y(*tick, scale.max);
        let mut line = PathBuilder::stroke(px(1.));
        line.move_to(at((0.0, y)));
        line.line_to(at((VIEW_WIDTH, y)));
        if let Ok(path) = line.build() {
            window.paint_path(path, colors.grid);
        }
    }
    let curve = |builder: &mut PathBuilder, segments: &[Segment]| {
        for segment in segments {
            builder.cubic_bezier_to(at(segment[3]), at(segment[1]), at(segment[2]));
        }
    };
    for series in series {
        let Some(first) = series.segments.first() else { continue };
        let color = colors.series[PROVIDER_ORDER.iter().position(|p| *p == series.provider).unwrap_or(0)];
        let alpha = if series.loading { 0.4 } else { 1.0 };
        let mut area = PathBuilder::fill();
        area.move_to(at(first[0]));
        curve(&mut area, &series.segments);
        area.line_to(at((VIEW_WIDTH, VIEW_HEIGHT)));
        area.line_to(at((0.0, VIEW_HEIGHT)));
        area.close();
        if let Ok(path) = area.build() {
            window.paint_path(path, with_alpha(color, 0.12 * alpha));
        }
    }
    for series in series {
        let Some(first) = series.segments.first() else { continue };
        let color = colors.series[PROVIDER_ORDER.iter().position(|p| *p == series.provider).unwrap_or(0)];
        let mut line = PathBuilder::stroke(px(2.));
        line.move_to(at(first[0]));
        curve(&mut line, &series.segments);
        if let Ok(path) = line.build() {
            window.paint_path(path, with_alpha(color, if series.loading { 0.4 } else { 1.0 }));
        }
    }
    if let Some(index) = hover {
        let x = index as f32 * step_x;
        let mut rule = PathBuilder::stroke(px(1.));
        rule.move_to(at((x, PLOT_TOP)));
        rule.line_to(at((x, VIEW_HEIGHT)));
        if let Ok(path) = rule.build() {
            window.paint_path(path, colors.rule);
        }
    }
}

/// Colours read once per render and moved into the paint closure.
struct ChartColors {
    grid: Hsla,
    rule: Hsla,
    series: [Hsla; 8],
}

impl Render for UsageProviderChart {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let Some(prepared) = &self.prepared else { return div().into_any_element() };
        let plot_height = theme.sp(56.);
        let colors = ChartColors { grid: c.border, rule: c.muted_foreground, series: PROVIDER_ORDER.map(|p| provider_color(p, theme)) };
        let hover_index = self.hover.map(|(index, _)| index);
        let bounds_cell = self.plot_bounds.clone();
        let paint_data = (prepared.scale.clone(), prepared.series.clone(), prepared.step_x);
        let label_size = TextSize::Xs3.line_height(theme.fonts.interface);
        let y_axis = div().relative().w(theme.sp(14.)).h(plot_height).flex_none().children(prepared.tick_labels.iter().map(|(fraction, label)| {
            div()
                .absolute()
                .right_0()
                .top(plot_height * *fraction - px(label_size / 2.))
                .ui_text(theme, TextSize::Xs3, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(label.clone())
        }));
        let plot = div()
            .id("usage-chart-plot")
            .relative()
            .flex_1()
            .min_w_0()
            .h(plot_height)
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.on_move(event, cx)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered && this.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(bounds),
                    move |bounds, _, window, _| {
                        let (scale, series, step_x) = paint_data;
                        paint_chart(bounds, &scale, &series, step_x, hover_index, &colors, window);
                    },
                )
                .size_full(),
            )
            .when_some(self.hover.and_then(|(index, at)| self.tooltip(index, at, cx)), |el, tooltip| el.child(tooltip));
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(1.))
            .child(div().flex().gap(theme.sp(2.)).child(y_axis).child(plot))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .pl(theme.sp(16.))
                    .ui_text(theme, TextSize::Xs3, weight::NORMAL)
                    .text_color(c.muted_foreground)
                    .children(prepared.axis.iter().cloned()),
            )
            .into_any_element()
    }
}

impl UsageProviderChart {
    fn tooltip(&self, index: usize, at: Point<Pixels>, cx: &gpui_kit::App) -> Option<gpui_kit::AnyElement> {
        let prepared = self.prepared.as_ref()?;
        let theme = cx.app_theme();
        let c = &theme.colors;
        let column = prepared.columns.get(index)?;
        let input = &prepared.input;
        let format = |value: f64| format_value(value, input.by_cost, input.locale);
        let partial = input.providers.iter().any(|p| input.loading.contains(p));
        let bounds = self.plot_bounds.get().size;
        let (gap, width, height) = (px(12.), theme.sp(36.), theme.sp(6.) * (input.providers.len() as f32 + 2.5));
        let left = if at.x + gap + width <= bounds.width { at.x + gap } else { at.x - gap - width };
        let top = if at.y + gap + height <= bounds.height { at.y + gap } else { at.y - gap - height };
        let left = left.clamp(px(0.), (bounds.width - width).max(px(0.)));
        let top = top.clamp(px(0.), (bounds.height - height).max(px(0.)));
        let row = |label: gpui_kit::AnyElement, value: String, loading: bool| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(theme.sp(3.))
                .child(label)
                .child(div().text_color(if loading { c.muted_foreground } else { c.foreground() }).child(value))
        };
        Some(
            glass_surface(theme)
                .absolute()
                .left(left)
                .top(top)
                .min_w(width)
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(with_alpha(c.border, 0.5))
                .px(theme.sp(2.5))
                .py(theme.sp(2.))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .child(div().mb(theme.sp(1.)).text_color(c.muted_foreground).child(format_tooltip_period(input, &input.periods[index])))
                .children(input.providers.iter().map(|provider| {
                    let slot = PROVIDER_ORDER.iter().position(|p| p == provider).unwrap_or(0);
                    row(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.sp(1.5))
                            .text_color(c.muted_foreground)
                            .child(provider_mark(*provider, theme.sp(3.), cx))
                            .child(provider_label(*provider))
                            .into_any_element(),
                        format(column.bands[slot]),
                        input.loading.contains(provider),
                    )
                }))
                .child(
                    div().mt(theme.sp(1.)).pt(theme.sp(1.)).border_t_1().border_color(c.border).child(row(
                        div().text_color(c.muted_foreground).child(t!("Total")).into_any_element(),
                        format(column.total),
                        partial,
                    )),
                )
                .into_any_element(),
        )
    }
}

