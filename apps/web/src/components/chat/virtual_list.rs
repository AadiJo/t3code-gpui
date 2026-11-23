//! A virtualized list of variable-height rows for the chat timeline. Ports
//! the parts of LegendList that `MessagesTimeline.tsx` relies on: per-kind size
//! estimates for rows not yet measured, a scroll position anchored to a row so
//! growth above it never moves what is on screen, the follow state machine of
//! `timelineViewport.ts`, a new turn anchored near the top, and insets for the
//! header fade and the composer overlay.
//!
//! Only rows inside the viewport plus an overdraw band are rendered and laid
//! out each frame. Every other row keeps its last measured height, or its
//! estimate, in a prefix-sum table.

use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Corners, DispatchPhase, Element,
    ElementId, EntityId, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ScrollEvent, Size, Style, Window, point, px, size,
};

use super::timeline_viewport::{
    TimelineFollow, TimelineFollowEvent, TimelineNavigation, ViewportReach,
    navigation_releases_follow,
};

/// Distance from the end that still counts as "at the end" (`TIMELINE_FOLLOW_REARM_THRESHOLD_PX`).
pub const AT_END_THRESHOLD: Pixels = px(40.);
/// Rows rendered beyond each viewport edge so scrolling does not pop rows in.
const OVERDRAW: Pixels = px(240.);
/// Scrollbar thumb width (`index.css:92`) and its radius.
const SCROLLBAR_WIDTH: Pixels = px(6.);
/// The trailing strip where a press counts as a scrollbar press (`timelineViewport.ts`).
const SCROLLBAR_HIT_WIDTH: Pixels = px(16.);
const SCROLLBAR_MIN_THUMB: Pixels = px(24.);
/// Lines-based wheel deltas scroll this far per line.
const WHEEL_LINE_HEIGHT: Pixels = px(20.);

/// A stable identity for a row, usually a hash of its id. Measured heights and
/// the scroll anchor follow keys, so inserting or removing rows above keeps
/// the visible content where it is.
pub type RowKey = u64;

/// A row the list knows about before it has been rendered.
#[derive(Clone, Copy, Debug)]
pub struct RowSlot {
    pub key: RowKey,
    pub estimate: Pixels,
}

/// The remembered scroll position of one list, so switching threads can come
/// back to the same place (`useTimelinePositionRestore`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollMemory {
    pub anchor: Option<(RowKey, Pixels)>,
    pub follow: TimelineFollow,
}

/// Where the list scrolls.
#[derive(Clone, Copy, Debug)]
enum Position {
    /// The end of the content, while following.
    End,
    /// `offset` pixels below the top of row `index`.
    Row { index: usize, offset: Pixels },
    /// A new user message held `offset` below the viewport top, with end
    /// space added beneath the content when it is too short to allow that.
    Pinned { index: usize, offset: Pixels },
}

/// Visible geometry from the last layout, for overlays like the scroll-to-end button.
#[derive(Clone, Debug, Default)]
pub struct ListViewport {
    pub bounds: Bounds<Pixels>,
    pub content_height: Pixels,
    pub scroll_top: Pixels,
    pub visible: Range<usize>,
    /// Window-space bounds of each rendered row, by index.
    pub row_bounds: Vec<(usize, Bounds<Pixels>)>,
}

impl ListViewport {
    /// Distance between the viewport bottom and the content end.
    pub fn distance_from_end(&self) -> Pixels {
        (self.content_height - self.scroll_top - self.bounds.size.height).max(px(0.))
    }
}

/// The list's state, held by the view that renders it.
#[derive(Clone)]
pub struct VirtualListState(Rc<RefCell<Inner>>);

struct Inner {
    keys: Vec<RowKey>,
    heights: Vec<Pixels>,
    measured: Vec<bool>,
    /// `prefix[i]` = sum of heights before row `i`; valid below `prefix_valid`.
    prefix: Vec<Pixels>,
    prefix_valid: usize,
    /// Heights measured at the current width, by key, so a rebuilt row list keeps them.
    measured_by_key: HashMap<RowKey, Pixels>,
    position: Position,
    follow: TimelineFollow,
    top_inset: Pixels,
    bottom_inset: Pixels,
    last_width: Option<Pixels>,
    viewport: ListViewport,
    scrollbar_drag: Option<Pixels>,
    scrollbar_hovered: bool,
    on_scroll: Option<Box<dyn FnMut(&ListViewport, TimelineFollow, &mut Window, &mut App)>>,
}

impl VirtualListState {
    /// An empty list that starts out following its end.
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(Inner {
            keys: Vec::new(),
            heights: Vec::new(),
            measured: Vec::new(),
            prefix: vec![px(0.)],
            prefix_valid: 0,
            measured_by_key: HashMap::new(),
            position: Position::End,
            follow: TimelineFollow::Following,
            top_inset: px(0.),
            bottom_inset: px(0.),
            last_width: None,
            viewport: ListViewport::default(),
            scrollbar_drag: None,
            scrollbar_hovered: false,
            on_scroll: None,
        })))
    }

    /// Replaces the rows. Rows whose key was measured before keep that height,
    /// and the scroll anchor stays on the same row.
    pub fn set_rows(&self, rows: &[RowSlot]) {
        let mut inner = self.0.borrow_mut();
        let anchor_key = match inner.position {
            Position::Row { index, offset } => inner.keys.get(index).map(|k| (*k, offset, false)),
            Position::Pinned { index, offset } => inner.keys.get(index).map(|k| (*k, offset, true)),
            Position::End => None,
        };
        inner.keys = rows.iter().map(|row| row.key).collect();
        inner.heights.clear();
        inner.measured.clear();
        for row in rows {
            let known = inner.measured_by_key.get(&row.key).copied();
            inner.heights.push(known.unwrap_or(row.estimate));
            inner.measured.push(known.is_some());
        }
        inner.prefix_valid = 0;
        if let Some((key, offset, pinned)) = anchor_key {
            inner.position = match inner.keys.iter().position(|k| *k == key) {
                Some(index) if pinned => Position::Pinned { index, offset },
                Some(index) => Position::Row { index, offset },
                None if inner.follow.is_following() => Position::End,
                None => Position::Row { index: 0, offset: px(0.) },
            };
        }
    }

    /// Space reserved above the first row and below the last (header fade, composer).
    pub fn set_insets(&self, top: Pixels, bottom: Pixels) {
        let mut inner = self.0.borrow_mut();
        inner.top_inset = top;
        inner.bottom_inset = bottom;
    }

    pub fn follow(&self) -> TimelineFollow {
        self.0.borrow().follow
    }

    pub fn viewport(&self) -> ListViewport {
        self.0.borrow().viewport.clone()
    }

    /// Called after every scroll and layout that moved the viewport.
    pub fn on_scroll(
        &self,
        handler: impl FnMut(&ListViewport, TimelineFollow, &mut Window, &mut App) + 'static,
    ) {
        self.0.borrow_mut().on_scroll = Some(Box::new(handler));
    }

    /// Jumps to the end and follows it (the scroll-to-end button, sending).
    pub fn follow_end(&self) {
        let mut inner = self.0.borrow_mut();
        inner.follow = inner.follow.transition(TimelineFollowEvent::FollowEnd);
        inner.position = Position::End;
    }

    /// Holds row `index` `offset` below the viewport top, as a newly sent
    /// message is (`CHAT_TIMELINE_ANCHOR_OFFSET`). Follow stays armed, so
    /// [`Self::follow_end`] releases the pin once the turn shows activity.
    pub fn pin_row_to_top(&self, index: usize, offset: Pixels) {
        let mut inner = self.0.borrow_mut();
        inner.follow = TimelineFollow::Following;
        inner.position = Position::Pinned { index, offset };
    }

    /// Whether a row is pinned near the top.
    pub fn is_pinned(&self) -> bool {
        matches!(self.0.borrow().position, Position::Pinned { .. })
    }

    /// Scrolls so row `index` starts `offset` below the viewport top, releasing follow.
    pub fn scroll_to_row(&self, index: usize, offset: Pixels) {
        let mut inner = self.0.borrow_mut();
        inner.follow = inner.follow.transition(TimelineFollowEvent::Navigated);
        inner.follow = inner.follow.transition(TimelineFollowEvent::LeftEnd);
        inner.position = Position::Row { index, offset: -offset };
    }

    /// Scrolls by a pixel distance as a navigation input would.
    pub fn scroll_by(&self, delta: Pixels, navigation: TimelineNavigation) {
        let mut inner = self.0.borrow_mut();
        inner.navigate(navigation);
        let target = inner.scroll_top() + delta;
        inner.set_scroll_top(target);
    }

    /// The position to remember for this thread.
    pub fn memory(&self) -> ScrollMemory {
        let inner = self.0.borrow();
        let anchor = match inner.position {
            Position::Row { index, offset } => inner.keys.get(index).map(|key| (*key, offset)),
            _ => None,
        };
        ScrollMemory { anchor, follow: inner.follow }
    }

    /// Restores a remembered position; a row that no longer exists falls back to the end.
    pub fn restore(&self, memory: ScrollMemory) {
        let mut inner = self.0.borrow_mut();
        inner.follow = memory.follow;
        inner.position = memory
            .anchor
            .filter(|_| !memory.follow.is_following())
            .and_then(|(key, offset)| {
                let index = inner.keys.iter().position(|k| *k == key)?;
                Some(Position::Row { index, offset })
            })
            .unwrap_or(Position::End);
    }
}

impl Default for VirtualListState {
    fn default() -> Self {
        Self::new()
    }
}

impl Inner {
    fn len(&self) -> usize {
        self.keys.len()
    }

    /// Sum of row heights before `index`.
    fn offset_of(&mut self, index: usize) -> Pixels {
        let index = index.min(self.len());
        if self.prefix.len() != self.len() + 1 {
            self.prefix.resize(self.len() + 1, px(0.));
            self.prefix_valid = self.prefix_valid.min(self.len());
        }
        while self.prefix_valid < index {
            let i = self.prefix_valid;
            self.prefix[i + 1] = self.prefix[i] + self.heights[i];
            self.prefix_valid += 1;
        }
        self.prefix[index]
    }

    fn rows_height(&mut self) -> Pixels {
        self.offset_of(self.len())
    }

    /// Content-space top of row `index`, insets included.
    fn row_top(&mut self, index: usize) -> Pixels {
        self.top_inset + self.offset_of(index)
    }

    fn content_height(&mut self) -> Pixels {
        self.top_inset + self.rows_height() + self.bottom_inset + self.pinned_end_space()
    }

    /// End space added under a pinned row so it can sit at the viewport top.
    fn pinned_end_space(&mut self) -> Pixels {
        let Position::Pinned { index, offset } = self.position else {
            return px(0.);
        };
        let viewport = self.viewport.bounds.size.height;
        let below = self.rows_height() - self.offset_of(index) + self.bottom_inset;
        (viewport - offset - below).max(px(0.))
    }

    fn max_scroll(&mut self) -> Pixels {
        (self.content_height() - self.viewport.bounds.size.height).max(px(0.))
    }

    fn scroll_top(&mut self) -> Pixels {
        match self.position {
            Position::End => self.max_scroll(),
            Position::Row { index, offset } => self.row_top(index) + offset,
            Position::Pinned { index, offset } => self.row_top(index) - offset,
        }
    }

    /// The row index containing content-space `y`.
    fn index_at(&mut self, y: Pixels) -> usize {
        let len = self.len();
        self.offset_of(len);
        let rows_y = y - self.top_inset;
        let upper = self.prefix[1..=len].partition_point(|end| *end <= rows_y);
        upper.min(len.saturating_sub(1))
    }

    fn set_scroll_top(&mut self, scroll_top: Pixels) {
        if self.len() == 0 {
            self.position = Position::End;
            return;
        }
        let max = self.max_scroll();
        let scroll_top = scroll_top.clamp(px(0.), max);
        if self.follow.is_following() && scroll_top >= max {
            self.position = Position::End;
            return;
        }
        let index = self.index_at(scroll_top);
        let offset = scroll_top - self.row_top(index);
        self.position = Position::Row { index, offset };
    }

    fn reach(&mut self) -> ViewportReach {
        let scroll_top = self.scroll_top();
        let distance = self.content_height() - scroll_top - self.viewport.bounds.size.height;
        ViewportReach {
            scrolls_up: scroll_top > px(0.),
            away_from_end: distance > AT_END_THRESHOLD,
        }
    }

    fn navigate(&mut self, navigation: TimelineNavigation) {
        let reach = self.reach();
        if navigation_releases_follow(navigation, reach) {
            if matches!(self.position, Position::End | Position::Pinned { .. }) {
                let scroll_top = self.scroll_top();
                let was_following = self.follow;
                self.follow = TimelineFollow::Released { at_end: true };
                self.set_scroll_top(scroll_top);
                self.follow = was_following;
            }
            self.follow = self.follow.transition(TimelineFollowEvent::Navigated);
        }
    }

    /// Feeds the at-end band into the follow state after the viewport moved.
    fn update_follow_from_reach(&mut self) {
        let reach = self.reach();
        let event = if reach.away_from_end {
            TimelineFollowEvent::LeftEnd
        } else {
            TimelineFollowEvent::ReachedEnd
        };
        let before = self.follow;
        self.follow = self.follow.transition(event);
        if !before.is_following() && self.follow.is_following() {
            self.position = Position::End;
        }
    }

    fn record_height(&mut self, index: usize, height: Pixels) {
        self.measured_by_key.insert(self.keys[index], height);
        self.measured[index] = true;
        if self.heights[index] != height {
            self.heights[index] = height;
            self.prefix_valid = self.prefix_valid.min(index);
        }
    }
}

/// Renders one row by index.
pub type RenderRow = dyn FnMut(usize, &mut Window, &mut App) -> AnyElement;

/// The list element. Build one per frame with [`virtual_list`].
pub struct VirtualList {
    id: ElementId,
    state: VirtualListState,
    render_row: Box<RenderRow>,
    /// When set, the list sizes to its content up to this height (nested lists).
    max_height: Option<Pixels>,
    scrollbar: Hsla,
    scrollbar_hover: Hsla,
}

/// A virtualized list over `state`, rendering rows with `render_row`.
pub fn virtual_list(
    id: impl Into<ElementId>,
    state: VirtualListState,
    render_row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> VirtualList {
    VirtualList {
        id: id.into(),
        state,
        render_row: Box::new(render_row),
        max_height: None,
        scrollbar: Hsla::transparent_black(),
        scrollbar_hover: Hsla::transparent_black(),
    }
}

impl VirtualList {
    /// Sizes the list to its rows, capped at `height`, instead of filling its parent.
    pub fn max_height(mut self, height: Pixels) -> Self {
        self.max_height = Some(height);
        self
    }

    /// Scrollbar thumb colours, idle and hovered.
    pub fn scrollbar_colors(mut self, idle: Hsla, hover: Hsla) -> Self {
        self.scrollbar = idle;
        self.scrollbar_hover = hover;
        self
    }
}

/// Rows laid out in the last prepaint.
pub struct VirtualListPrepaint {
    hitbox: Hitbox,
    rows: Vec<AnyElement>,
    thumb: Option<Bounds<Pixels>>,
}

impl Element for VirtualList {
    type RequestLayoutState = ();
    type PrepaintState = VirtualListPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui_kit::relative(1.).into();
        match self.max_height {
            Some(max) => {
                let content = {
                    let mut inner = self.state.0.borrow_mut();
                    inner.top_inset + inner.rows_height() + inner.bottom_inset
                };
                style.size.height = content.min(max).into();
            }
            None => {
                style.size.height = gpui_kit::relative(1.).into();
                style.flex_grow = 1.;
                style.min_size.height = px(0.).into();
            }
        }
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> VirtualListPrepaint {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let state = self.state.clone();
        let mut inner = state.0.borrow_mut();
        let width = bounds.size.width;
        if inner.last_width != Some(width) {
            // Heights measured at another width are only estimates now.
            inner.measured_by_key.clear();
            inner.measured.iter_mut().for_each(|m| *m = false);
            inner.last_width = Some(width);
        }
        inner.viewport.bounds = bounds;
        let viewport_height = bounds.size.height;
        let available = size(AvailableSpace::Definite(width), AvailableSpace::MinContent);
        let mut rendered: HashMap<usize, AnyElement> = HashMap::new();
        let len = inner.len();

        // Measure rows around the scroll position. Measuring can change the
        // heights it depends on, so it settles in at most a few passes.
        drop(inner);
        let mut measure = |index: usize, rendered: &mut HashMap<usize, AnyElement>| {
            if rendered.contains_key(&index) {
                return;
            }
            let mut element = (self.render_row)(index, window, cx);
            let measured = element.layout_as_root(available, window, cx);
            state.0.borrow_mut().record_height(index, measured.height);
            rendered.insert(index, element);
        };
        for _ in 0..3 {
            let (start, end) = {
                let mut inner = state.0.borrow_mut();
                if len == 0 {
                    break;
                }
                let scroll_top = inner.scroll_top();
                let top = (scroll_top - OVERDRAW).max(px(0.));
                let bottom = scroll_top + viewport_height + OVERDRAW;
                (inner.index_at(top), inner.index_at(bottom))
            };
            let before = state.0.borrow_mut().scroll_top();
            if matches!(state.0.borrow().position, Position::End) {
                for index in (start..=end).rev() {
                    measure(index, &mut rendered);
                }
            } else {
                for index in start..=end {
                    measure(index, &mut rendered);
                }
            }
            let after = state.0.borrow_mut().scroll_top();
            let (new_start, new_end) = {
                let mut inner = state.0.borrow_mut();
                let top = (after - OVERDRAW).max(px(0.));
                (inner.index_at(top), inner.index_at(after + viewport_height + OVERDRAW))
            };
            if before == after && new_start >= start && new_end <= end {
                break;
            }
        }

        let mut inner = state.0.borrow_mut();
        // Keep a released position inside the content after rows shrank.
        if let Position::Row { .. } = inner.position {
            let scroll_top = inner.scroll_top();
            let max = inner.max_scroll();
            if scroll_top > max || scroll_top < px(0.) {
                inner.set_scroll_top(scroll_top);
            }
        }
        let scroll_top = inner.scroll_top();
        let content_height = inner.content_height();
        let mut rows = Vec::with_capacity(rendered.len());
        let mut row_bounds = Vec::with_capacity(rendered.len());
        let mut indices: Vec<usize> = rendered.keys().copied().collect();
        indices.sort_unstable();
        let mut visible = usize::MAX..0;
        for index in indices {
            let top = inner.row_top(index) - scroll_top;
            let height = inner.heights[index];
            if top + height < -OVERDRAW || top > viewport_height + OVERDRAW {
                continue;
            }
            if top + height > px(0.) && top < viewport_height {
                visible.start = visible.start.min(index);
                visible.end = visible.end.max(index + 1);
            }
            let origin = bounds.origin + point(px(0.), top);
            row_bounds.push((index, Bounds::new(origin, size(width, height))));
            let mut element = rendered.remove(&index).expect("row was rendered");
            drop(inner);
            element.prepaint_at(origin, window, cx);
            inner = state.0.borrow_mut();
            rows.push(element);
        }
        if visible.start > visible.end {
            visible = 0..0;
        }
        inner.viewport = ListViewport {
            bounds,
            content_height,
            scroll_top,
            visible,
            row_bounds,
        };
        inner.update_follow_from_reach();

        let thumb = (content_height > viewport_height).then(|| {
            let ratio = viewport_height / content_height;
            let thumb_height = (viewport_height * ratio).max(SCROLLBAR_MIN_THUMB);
            let travel = viewport_height - thumb_height;
            let max = (content_height - viewport_height).max(px(1.));
            let y = travel * (scroll_top / max).clamp(0., 1.);
            Bounds::new(
                point(bounds.right() - SCROLLBAR_WIDTH, bounds.top() + y),
                size(SCROLLBAR_WIDTH, thumb_height),
            )
        });
        let viewport = inner.viewport.clone();
        let follow = inner.follow;
        let mut handler = inner.on_scroll.take();
        drop(inner);
        if let Some(handler) = handler.as_mut() {
            handler(&viewport, follow, window, cx);
        }
        if let Some(handler) = handler {
            state.0.borrow_mut().on_scroll.get_or_insert(handler);
        }
        VirtualListPrepaint { hitbox, rows, thumb }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut VirtualListPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let view = window.current_view();
        register_scroll_handlers(&self.state, &prepaint.hitbox, bounds, view, window);
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for row in &mut prepaint.rows {
                row.paint(window, cx);
            }
            if let Some(thumb) = prepaint.thumb {
                let inner = self.state.0.borrow();
                let color = if inner.scrollbar_hovered || inner.scrollbar_drag.is_some() {
                    self.scrollbar_hover
                } else {
                    self.scrollbar
                };
                window.paint_quad(gpui_kit::fill(thumb, color).corner_radii(Corners::all(
                    SCROLLBAR_WIDTH / 2.,
                )));
            }
        });
    }
}

/// Wheel scrolling and scrollbar dragging.
fn register_scroll_handlers(
    state: &VirtualListState,
    hitbox: &Hitbox,
    bounds: Bounds<Pixels>,
    view: EntityId,
    window: &mut Window,
) {
    let hitbox_id = hitbox.id;
    let wheel_state = state.clone();
    window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || !hitbox_id.should_handle_scroll(window) {
            return;
        }
        let delta = event.delta.pixel_delta(WHEEL_LINE_HEIGHT).y;
        if delta == px(0.) {
            return;
        }
        let up = delta > px(0.);
        wheel_state.scroll_by(-delta, TimelineNavigation::Wheel { up });
        cx.stop_propagation();
        cx.notify(view);
    });

    let in_strip = move |position: Point<Pixels>| {
        bounds.contains(&position) && position.x >= bounds.right() - SCROLLBAR_HIT_WIDTH
    };
    let down_state = state.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
        if phase != DispatchPhase::Bubble
            || event.button != MouseButton::Left
            || !in_strip(event.position)
        {
            return;
        }
        let mut inner = down_state.0.borrow_mut();
        inner.navigate(TimelineNavigation::Pointer { on_scrollbar: true });
        let scroll_top = inner.scroll_top();
        inner.set_scroll_top(scroll_top);
        inner.scrollbar_drag = Some(event.position.y);
        cx.stop_propagation();
        cx.notify(view);
    });
    let move_state = state.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        let mut inner = move_state.0.borrow_mut();
        let hovered = in_strip(event.position);
        if hovered != inner.scrollbar_hovered {
            inner.scrollbar_hovered = hovered;
            cx.notify(view);
        }
        let Some(last_y) = inner.scrollbar_drag else {
            return;
        };
        let viewport = inner.viewport.bounds.size.height;
        let content = inner.content_height();
        let ratio = if viewport > px(0.) { content / viewport } else { 1. };
        let target = inner.scroll_top() + (event.position.y - last_y) * ratio;
        inner.set_scroll_top(target);
        inner.scrollbar_drag = Some(event.position.y);
        cx.notify(view);
    });
    let up_state = state.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble && up_state.0.borrow_mut().scrollbar_drag.take().is_some() {
            cx.notify(view);
        }
    });
}

/// Rows that differ only in their estimate share these sizes. Unused rows
/// fall back to [`DEFAULT_ESTIMATE`] (`estimatedItemSize={90}`).
pub const DEFAULT_ESTIMATE: Pixels = px(90.);

/// A size helper for callers that build slots from `(key, estimate)` pairs.
pub fn slot(key: RowKey, estimate: Pixels) -> RowSlot {
    RowSlot { key, estimate }
}

/// Bounds helper: the size of a list viewport.
pub fn viewport_size(viewport: &ListViewport) -> Size<Pixels> {
    viewport.bounds.size
}
