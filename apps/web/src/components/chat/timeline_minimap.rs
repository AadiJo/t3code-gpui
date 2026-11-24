//! The timeline minimap (`TimelineMinimap.tsx`, `timelineMinimapItems.ts` and the minimap
//! helpers of `MessagesTimeline.logic.ts`): one strip per user message on a rail in the left
//! gutter, lit while its turn is on screen. Hovering the rail grows the strips around the
//! pointer and shows the turn's preview; a click scrolls to the turn, and the chevrons above
//! and below step to the previous or next turn.
//!
//! The strips are painted in one canvas that reads the list's viewport as it paints, so
//! scrolling relights them without re-rendering the timeline.

use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui_kit::{
    AnyElement, Bounds, Context, Corners, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, Styled as _, canvas, div, fill, point, prelude::FluentBuilder as _, px, rems,
    size,
};

use super::{
    chat_style::{TEXT_SM, alpha},
    messages_timeline::MessagesTimeline,
    messages_timeline_logic::{RowKind, TimelineRow},
    timeline_entries::MessageRole,
    virtual_list::VirtualListState,
};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::i18n::t;

/// Strips sit 8px apart while the rail fits (`TIMELINE_MINIMAP_ITEM_SPACING`).
const ITEM_SPACING: Pixels = px(8.);
/// The rail shows from two user messages (`TIMELINE_MINIMAP_MIN_ITEMS`).
const MIN_ITEMS: usize = 2;
/// A selected turn lands this far below the viewport top (`TIMELINE_MINIMAP_TARGET_OFFSET`).
pub const TARGET_OFFSET: Pixels = px(24.);
/// A side gutter at least this wide keeps the rail visible (`TIMELINE_MINIMAP_PERSISTENT_GUTTER`).
const PERSISTENT_GUTTER: Pixels = px(48.);
/// The hit strip starts 12px in and is at most 40px wide.
const HIT_STRIP_LEFT: f32 = 12.;
const HIT_STRIP_MAX_WIDTH: f32 = 40.;
/// The chevrons take clicks only when the strip reaches them (`TIMELINE_MINIMAP_NAVIGATION_REACH`).
const NAVIGATION_REACH: Pixels = px(14.);
/// The preview reads at most this much of a message.
const PREVIEW_SOURCE_CHARS: usize = 1_500;

/// One user message on the rail; its texts are read from the rows when it is previewed.
#[derive(Clone, Copy, Debug)]
pub struct MinimapItem {
    pub row_index: usize,
}

/// The minimap's state between frames, held by the timeline.
#[derive(Default)]
pub struct MinimapState {
    /// The rows the items were derived from, and the items.
    derived: Option<(Arc<Vec<TimelineRow>>, Arc<[MinimapItem]>)>,
    /// The strip under the pointer.
    active: Option<usize>,
    /// The rail's window bounds from the last paint, for pointer hit tests.
    rail: Rc<Cell<Bounds<Pixels>>>,
    /// The pointer is over the minimap: only then are its turn buttons built.
    hovered: bool,
}

/// `deriveTimelineMinimapItems`: each user message.
fn derive_items(rows: &[TimelineRow]) -> Arc<[MinimapItem]> {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| matches!(&row.kind, RowKind::Message(m) if m.message.role == MessageRole::User))
        .map(|(row_index, _)| MinimapItem { row_index })
        .collect()
}

/// The user message's text and the last assistant reply of its turn
/// (`resolveFinalAssistantTextForTurn`).
fn turn_texts(rows: &[TimelineRow], user_row: usize) -> (SharedString, Option<SharedString>) {
    let text = |row: &TimelineRow| match &row.kind {
        RowKind::Message(m) => Some((m.message.role, m.message.text.clone())),
        _ => None,
    };
    let user = rows.get(user_row).and_then(text).map(|(_, t)| t).unwrap_or_default();
    let mut assistant = None;
    for row in rows.iter().skip(user_row + 1) {
        match text(row) {
            Some((MessageRole::User, _)) => break,
            Some((MessageRole::Assistant, t)) => assistant = Some(t),
            None => {}
        }
    }
    (user, assistant)
}

/// `markdownMinimapPreview`: the first stretch of a message as one line of rendered text.
fn preview(markdown: &str) -> Option<SharedString> {
    let end = markdown.char_indices().nth(PREVIEW_SOURCE_CHARS).map_or(markdown.len(), |(i, _)| i);
    let blocks = crate::markdown_pipeline::parse_markdown(&markdown[..end], crate::markdown_pipeline::MarkdownFlavor::ASSISTANT);
    let mut text = String::new();
    super::thread_find::visit_block_texts(&blocks, &mut |part| {
        text.push(' ');
        text.push_str(part);
    });
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!compact.is_empty()).then(|| compact.into())
}

/// `resolveTimelineMinimapTopPercent` as a fraction.
fn top_fraction(index: usize, count: usize) -> f32 {
    if count <= 1 { 0. } else { index.min(count - 1) as f32 / (count - 1) as f32 }
}

/// `resolveTimelineMinimapIndexFromPointer`.
fn index_at(count: usize, rail: Bounds<Pixels>, y: Pixels) -> Option<usize> {
    if count == 0 || rail.size.height <= px(0.) {
        return None;
    }
    let progress = ((y - rail.top()) / rail.size.height).clamp(0., 1.);
    Some(((progress * (count - 1) as f32).round() as usize).min(count - 1))
}

/// `isTimelineMinimapTurnVisible`: a turn spans its user message up to the next one.
fn turn_visible(items: &[MinimapItem], index: usize, visible: &std::ops::Range<usize>) -> bool {
    if visible.is_empty() {
        return false;
    }
    let next = items.get(index + 1).map(|item| item.row_index);
    items[index].row_index < visible.end && next.is_none_or(|next| next > visible.start)
}

/// `resolveTimelineMinimapCurrentIndex`: the first user message on screen, else the turn being read.
fn current_index(items: &[MinimapItem], visible: &std::ops::Range<usize>) -> Option<usize> {
    if visible.is_empty() {
        return None;
    }
    let mut preceding = None;
    for (index, item) in items.iter().enumerate() {
        if item.row_index >= visible.start {
            return if item.row_index < visible.end { Some(index) } else { preceding };
        }
        preceding = Some(index);
    }
    preceding
}

/// The strip's width scale for its distance from the hovered strip.
fn strip_scale(distance: Option<usize>) -> f32 {
    match distance {
        Some(0) => 1.,
        Some(1) => 0.667,
        Some(2) => 0.417,
        _ => 0.333,
    }
}

impl MessagesTimeline {
    /// The minimap over the timeline's left gutter, or nothing below two user messages.
    pub(super) fn render_minimap(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let items = match &self.minimap.derived {
            Some((rows, items)) if Arc::ptr_eq(rows, &self.rows) => items.clone(),
            _ => {
                let items = derive_items(&self.rows);
                self.minimap.derived = Some((self.rows.clone(), items.clone()));
                items
            }
        };
        if items.len() < MIN_ITEMS {
            self.minimap.active = None;
            return None;
        }
        let colors = self.colors;
        let (bounds, content_height, scroll_top, visible) = self.list.viewport_metrics();
        let width = bounds.size.width;
        if width <= px(0.) {
            return None;
        }
        let content = self.lane.content.map_or(width, |lane| lane.min(width));
        let gutter = ((width - content) / 2.).max(px(0.));
        let persistent = gutter >= PERSISTENT_GUTTER;
        let hit_width = px((f32::from(gutter).floor() - HIT_STRIP_LEFT).clamp(0., HIT_STRIP_MAX_WIDTH));
        let navigable = hit_width >= NAVIGATION_REACH;
        // `min((n - 1) * 8px, max(4rem, 100% - 14.75rem))`.
        let rail_height = (ITEM_SPACING * (items.len() - 1) as f32)
            .max(px(1.))
            .min(colors.r(px(64.)).max(bounds.size.height - colors.r(px(236.))));
        let active = self.minimap.active.filter(|index| *index < items.len());
        let current = current_index(&items, &visible);
        let at_end = content_height - scroll_top - bounds.size.height <= px(1.);
        let previous = current.and_then(|c| c.checked_sub(1));
        let next = current.filter(|_| !at_end).map(|c| c + 1).filter(|n| *n < items.len());
        let rail_cell = self.minimap.rail.clone();
        let list = self.list.clone();
        let strips = {
            let items = items.clone();
            canvas(
                move |bounds, _, _| {
                    rail_cell.set(bounds);
                    bounds
                },
                move |_, bounds, window, _| paint_strips(&items, &list, bounds, active, &colors, window),
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };
        let preview_card = active.and_then(|index| items.get(index)).map(|item| {
            let (user_text, assistant_text) = turn_texts(&self.rows, item.row_index);
            let user = preview(&user_text).unwrap_or_else(|| t!("User message"));
            let assistant = assistant_text.as_deref().and_then(preview);
            let theme = crate::ActiveAppTheme::app_theme(&**cx);
            div()
                .absolute()
                .left(rems(2.))
                .top(rail_height * top_fraction(active.unwrap_or(0), items.len()))
                .h(px(0.))
                .flex()
                .items_center()
                .child(
                    crate::components::ui::popover::glass_surface(theme)
                        .w(rems(20.))
                        .rounded(rems(0.75))
                        .p_3()
                        .text_size(TEXT_SM.size)
                        .line_height(rems(1.25))
                        .child(div().w_full().truncate().font_weight(FontWeight::MEDIUM).child(user))
                        .children(assistant.map(|text| {
                            div().mt_1().w_full().line_clamp(3).text_color(colors.theme.muted_foreground).child(text)
                        })),
                )
        });
        let nav = |previous_turn: bool, target: Option<usize>, cx: &mut Context<Self>| {
            let label = if previous_turn { t!("Previous turn") } else { t!("Next turn") };
            let row = target.and_then(|index| items.get(index)).map(|item| item.row_index);
            let group: SharedString = if previous_turn { "minimap-previous".into() } else { "minimap-next".into() };
            div()
                .id(group.clone())
                .group(group.clone())
                .absolute()
                .left(rems(0.25) - rems(0.625))
                .when(previous_turn, |d| d.bottom(rail_height + px(6.)))
                .when(!previous_turn, |d| d.top(rail_height + px(6.)))
                .size(rems(1.25))
                .flex()
                .items_center()
                .justify_center()
                .rounded(rems(0.25))
                .opacity(0.)
                .when(navigable, |d| {
                    d.hover(|s| s.opacity(1.).bg(colors.theme.accent_surface))
                        .when(row.is_some(), |d| d.cursor_pointer())
                        .tooltip(crate::components::ui::tooltip::tooltip(label))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(row) = row {
                                this.select_minimap_row(row, window, cx);
                            }
                        }))
                })
                .child(
                    Icon::new(if previous_turn { IconName::ChevronUp } else { IconName::ChevronDown })
                        .size(IconSize::Custom(rems(1.).to_pixels(px(colors.rem * 16.))))
                        .color(alpha(colors.theme.text, if row.is_some() { 0.9 } else { 0.45 })),
                )
        };
        let rail_items = items.clone();
        let rail = div()
            .id("timeline-minimap-rail")
            .relative()
            .ml(rems(0.75))
            .w(hit_width)
            .h(rail_height)
            .when(hit_width > px(0.), |d| d.cursor_pointer())
            .on_mouse_move(cx.listener(move |this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                let rail = this.minimap.rail.get();
                let index = if rail.contains(&event.position) { index_at(rail_items.len(), rail, event.position.y) } else { None };
                if this.minimap.active != index {
                    this.minimap.active = index;
                    cx.notify();
                }
            }))
            .on_click({
                let items = items.clone();
                cx.listener(move |this, event: &gpui_kit::ClickEvent, window, cx| {
                    let rail = this.minimap.rail.get();
                    if let Some(item) = index_at(items.len(), rail, event.position().y).and_then(|index| items.get(index)) {
                        this.select_minimap_row(item.row_index, window, cx);
                    }
                })
            })
            .child(strips)
            .when(self.minimap.hovered, |d| d.child(nav(true, previous, cx)).child(nav(false, next, cx)))
            .children(preview_card);
        Some(
            div()
                .id("timeline-minimap")
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(rems(4.5))
                .flex()
                .items_center()
                .when(!persistent, |d| d.opacity(0.).hover(|s| s.opacity(1.)))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.minimap.hovered = *hovered;
                    if !hovered {
                        this.minimap.active = None;
                    }
                    cx.notify();
                }))
                .child(rail)
                .into_any_element(),
        )
    }

    /// Scrolls the chosen turn's user message to [`TARGET_OFFSET`] below the top.
    fn select_minimap_row(&mut self, row: usize, _window: &mut gpui_kit::Window, cx: &mut Context<Self>) {
        self.minimap.active = None;
        self.list.scroll_to_row(row, TARGET_OFFSET);
        cx.notify();
    }
}

/// Paints every strip: `h-0.5 w-6` at its scale, `muted-foreground/25` (`/60` hovered), with
/// `foreground/55` over it while its turn is on screen. On a rail denser than a strip is tall the
/// strips overlap into a band: a run of overlapping strips that look alike is painted as one quad
/// in the colour that many translucent strips stacked give, so a thread with hundreds of turns
/// paints a handful of quads instead of one per turn.
fn paint_strips(
    items: &[MinimapItem],
    list: &VirtualListState,
    bounds: Bounds<Pixels>,
    active: Option<usize>,
    colors: &super::chat_style::ChatColors,
    window: &mut gpui_kit::Window,
) {
    let visible = list.viewport_metrics().3;
    let height = colors.r(px(2.));
    let full = colors.r(px(24.));
    let count = items.len();
    let spacing = if count > 1 { bounds.size.height / (count - 1) as f32 } else { height };
    // How many strips cover one point of an overlapping run.
    let depth = (f32::from(height) / f32::from(spacing).max(0.001)).ceil().max(1.);
    let mut paint = |look: (usize, bool), first: usize, last: usize, window: &mut gpui_kit::Window| {
        let (distance, in_view) = look;
        let stacked = if last > first { depth.min((last - first + 1) as f32) } else { 1. };
        let layered = |alpha: f32| 1. - (1. - alpha).powf(stacked);
        let top = bounds.top() + bounds.size.height * top_fraction(first, count) - height / 2.;
        let bottom = bounds.top() + bounds.size.height * top_fraction(last, count) + height / 2.;
        let strip = Bounds::new(point(bounds.left(), top), size(full * strip_scale(Some(distance).filter(|d| *d < 3)), bottom - top));
        let base = alpha(colors.theme.muted_foreground, layered(if distance == 0 { 0.6 } else { 0.25 }));
        window.paint_quad(fill(strip, base).corner_radii(Corners::all(height / 2.)));
        if in_view {
            window.paint_quad(fill(strip, alpha(colors.theme.text, layered(0.55))).corner_radii(Corners::all(height / 2.)));
        }
    };
    let overlapping = spacing < height;
    let mut run: Option<((usize, bool), usize, usize)> = None;
    for index in 0..count {
        let distance = active.map_or(usize::MAX, |active| active.abs_diff(index)).min(3);
        let look = (distance, turn_visible(items, index, &visible));
        match run {
            Some((current, first, _)) if overlapping && current == look => run = Some((current, first, index)),
            _ => {
                if let Some((current, first, last)) = run.take() {
                    paint(current, first, last, window);
                }
                run = Some((look, index, index));
            }
        }
    }
    if let Some((current, first, last)) = run {
        paint(current, first, last, window);
    }
}
