//! The floating browser preview over the chat. Ports `ThreadPreviewMiniPlayer.tsx` (the
//! browser player), `previewMiniPlayerStore.ts`, `previewMiniPlayerLayout.ts`,
//! `previewMiniPlayerGesture.ts` and the frame rules of `chatCanvasLayout.ts`.
//!
//! "Float preview over chat" in a browser tab opens it for that thread and closes the
//! right panel. It shows while its thread is shown and the panel is not showing the same
//! tab. The handle at its top-right drags it; its edges and corners resize it at the
//! page's aspect ratio; hovering the handle shows "Open in right panel" and "Close".
//! Positions are kept per thread for the session, as the fork's store keeps them.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, Bounds, Context, CursorStyle, Entity, Global, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, SharedString, Subscription, WeakEntity, Window, canvas, div, prelude::*, px,
};
use t3code_contracts::{EnvironmentId, PreviewSessionSnapshot, ThreadId};

use super::preview_source::PreviewSource;
use crate::ActiveAppTheme as _;
use crate::browser::server_browser_surface::{ServerBrowserSurface, SurfaceEvent};
use crate::components::icons::IconName;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::tooltip::tooltip;
use crate::i18n::t;
use crate::theme_color::with_alpha;

/// `PREVIEW_MINI_PLAYER_EDGE_GAP`, `_CORNER_RADIUS`, the default box and the minimum size.
const GAP: f32 = 12.;
const CORNER_RADIUS: f32 = 12.;
const DEFAULT_BOX: (f32, f32) = (320., 320.);
const MIN_SIZE: (f32, f32) = (240., 150.);
/// `resolveChatCanvasLayout`'s padding and narrowest chat column.
const CANVAS_PADDING: f32 = 48.;
const MIN_CHAT_WIDTH: f32 = 640.;
/// `HANDLE_TAP_SLOP_PX`.
const TAP_SLOP: f32 = 6.;

/// One thread, as the player store keys it (`scopedThreadKey`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MiniPlayerKey {
    pub environment: EnvironmentId,
    pub thread: ThreadId,
}

/// What the last gesture was; a resized player keeps its place over the chat lane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interaction {
    #[default]
    Drag,
    Resize,
}

/// `PreviewMiniPlayerState` for a browser source.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniPlayerState {
    pub tab_id: String,
    pub position: Option<(f32, f32)>,
    pub width: Option<f32>,
    pub last_interaction: Interaction,
}

/// Every thread's floating preview (`usePreviewMiniPlayerStore`).
#[derive(Default)]
pub struct PreviewMiniPlayers {
    by_thread: HashMap<MiniPlayerKey, MiniPlayerState>,
}

impl Global for PreviewMiniPlayers {}

impl PreviewMiniPlayers {
    fn edit(cx: &mut App, edit: impl FnOnce(&mut Self) -> bool) {
        let changed = edit(cx.default_global::<Self>());
        if changed {
            // Observers of the global re-render; the store itself is notified by replacement.
            cx.update_global::<Self, _>(|_, _| {});
        }
    }

    /// The thread's player, if it has one.
    pub fn get<'a>(key: &MiniPlayerKey, cx: &'a App) -> Option<&'a MiniPlayerState> {
        cx.try_global::<Self>()?.by_thread.get(key)
    }

    /// The tab the thread's player shows.
    pub fn tab<'a>(key: &MiniPlayerKey, cx: &'a App) -> Option<&'a str> {
        Self::get(key, cx).map(|s| s.tab_id.as_str())
    }

    /// `open`: shows `tab`, keeping the place and size of a player the thread already had.
    pub fn open(key: MiniPlayerKey, tab: String, cx: &mut App) {
        Self::edit(cx, |store| {
            let current = store.by_thread.get(&key);
            if current.is_some_and(|c| c.tab_id == tab) {
                return false;
            }
            let next = MiniPlayerState {
                tab_id: tab,
                position: current.and_then(|c| c.position),
                width: current.and_then(|c| c.width),
                last_interaction: current.map(|c| c.last_interaction).unwrap_or_default(),
            };
            store.by_thread.insert(key, next);
            true
        });
    }

    /// `close`.
    pub fn close(key: &MiniPlayerKey, cx: &mut App) {
        Self::edit(cx, |store| store.by_thread.remove(key).is_some());
    }

    /// `move`; `tab` guards against a drag that outlives the tab it started on.
    fn move_to(key: &MiniPlayerKey, tab: &str, position: (f32, f32), cx: &mut App) {
        Self::edit(cx, |store| {
            let Some(state) = store.by_thread.get_mut(key).filter(|s| s.tab_id == tab) else { return false };
            if state.position == Some(position) && state.last_interaction == Interaction::Drag {
                return false;
            }
            state.position = Some(position);
            state.last_interaction = Interaction::Drag;
            true
        });
    }

    /// `resize`.
    fn resize(key: &MiniPlayerKey, tab: &str, width: f32, position: (f32, f32), cx: &mut App) {
        Self::edit(cx, |store| {
            let Some(state) = store.by_thread.get_mut(key).filter(|s| s.tab_id == tab) else { return false };
            if state.width == Some(width) && state.position == Some(position) && state.last_interaction == Interaction::Resize {
                return false;
            }
            state.width = Some(width);
            state.position = Some(position);
            state.last_interaction = Interaction::Resize;
            true
        });
    }
}

/// A player's box in the chat canvas, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A resize handle (`BrowserViewportResizeDirection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    North,
    South,
    West,
    East,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

impl Direction {
    const ALL: [Self; 8] =
        [Self::North, Self::South, Self::West, Self::East, Self::NorthWest, Self::NorthEast, Self::SouthWest, Self::SouthEast];

    fn east(self) -> bool {
        matches!(self, Self::East | Self::NorthEast | Self::SouthEast)
    }
    fn west(self) -> bool {
        matches!(self, Self::West | Self::NorthWest | Self::SouthWest)
    }
    fn north(self) -> bool {
        matches!(self, Self::North | Self::NorthEast | Self::NorthWest)
    }
    fn south(self) -> bool {
        matches!(self, Self::South | Self::SouthEast | Self::SouthWest)
    }
}

/// `minimumPreviewMiniPlayerWidth`.
fn minimum_width(source: (f32, f32)) -> f32 {
    MIN_SIZE.0.max(MIN_SIZE.1 * source.0 / source.1)
}

/// `fitPreviewMiniPlayerWidth`.
fn fit_width(desired: f32, source: (f32, f32), max: (f32, f32), minimum: f32) -> (f32, f32) {
    let aspect = source.0 / source.1;
    let width = desired.max(minimum).min(max.0.max(1.)).min((max.1 * aspect).max(1.));
    (width.round(), (width / aspect).round())
}

/// `clampToContainer`.
fn clamp_to_container(position: (f32, f32), container: (f32, f32), size: (f32, f32), minimum_x: f32) -> (f32, f32) {
    (
        position.0.max(minimum_x).max(GAP).min(GAP.max(container.0 - size.0 - GAP)),
        position.1.max(GAP).min(GAP.max(container.1 - size.1 - GAP)),
    )
}

/// `resolvePreviewMiniPlayerFrame` with no obstacles.
fn resolve_frame(width: Option<f32>, position: Option<(f32, f32)>, source: (f32, f32), container: (f32, f32)) -> Frame {
    let desired = width.unwrap_or_else(|| DEFAULT_BOX.0.min(DEFAULT_BOX.1 * source.0 / source.1));
    let available = (container.0 - GAP * 2., container.1 - GAP * 2.);
    let size = fit_width(desired, source, available, width.unwrap_or_else(|| minimum_width(source)));
    let anchored = position.unwrap_or((container.0 - GAP - size.0, GAP));
    let (x, y) = clamp_to_container(anchored, container, size, GAP);
    Frame { x, y, width: size.0, height: size.1 }
}

/// The frame `resolveChatCanvasLayout` gives the player: bottom-right until placed, kept
/// clear of the centred chat column when there is room for it.
pub fn canvas_frame(state: &MiniPlayerState, source: (f32, f32), container: (f32, f32)) -> Frame {
    let mut frame = resolve_frame(state.width, state.position, source, container);
    let minimum_x = if state.last_interaction == Interaction::Resize || container.0 <= CANVAS_PADDING + MIN_CHAT_WIDTH + frame.width + GAP * 2. {
        GAP
    } else {
        CANVAS_PADDING + MIN_CHAT_WIDTH + GAP
    };
    if state.position.is_none() {
        frame.y = container.1 - frame.height - GAP;
    }
    let (x, y) = clamp_to_container((frame.x, frame.y), container, (frame.width, frame.height), minimum_x);
    Frame { x, y, ..frame }
}

/// `resizePreviewMiniPlayer` with no obstacles.
fn resize_frame(start: Frame, direction: Direction, delta: (f32, f32), source: (f32, f32), container: (f32, f32)) -> Frame {
    let (east, west, north, south) = (direction.east(), direction.west(), direction.north(), direction.south());
    let right = start.x + start.width;
    let bottom = start.y + start.height;
    let max = (
        if west {
            right - GAP
        } else if east {
            container.0 - GAP - start.x
        } else {
            container.0 - GAP * 2.
        },
        if north {
            bottom - GAP
        } else if south {
            container.1 - GAP - start.y
        } else {
            container.1 - GAP * 2.
        },
    );
    let desired_width = start.width + if east { delta.0 } else if west { -delta.0 } else { 0. };
    let desired_height = start.height + if south { delta.1 } else if north { -delta.1 } else { 0. };
    let aspect = source.0 / source.1;
    let (horizontal, vertical) = (east || west, north || south);
    // Projects onto the aspect-ratio diagonal so opposite axis moves do not jump.
    let desired = if horizontal && vertical {
        (desired_width + desired_height / aspect) / (1. + 1. / (aspect * aspect))
    } else if horizontal {
        desired_width
    } else {
        desired_height * aspect
    };
    let size = fit_width(desired, source, max, start.width.min(minimum_width(source)));
    let anchor = (if west { right - size.0 } else { start.x }, if north { bottom - size.1 } else { start.y });
    let (x, y) = clamp_to_container(anchor, container, size, GAP);
    Frame { x, y, width: size.0, height: size.1 }
}

/// A drag or resize in progress (`PreviewMiniPlayerGesture`).
#[derive(Clone, Copy, Debug)]
struct Gesture {
    pointer: Point<Pixels>,
    frame: Frame,
    direction: Option<Direction>,
    source: (f32, f32),
    moved: bool,
}

/// What the host knows about the shown thread, set each time the main pane renders.
#[derive(Clone, PartialEq)]
struct Shown {
    key: MiniPlayerKey,
    /// The browser tab the right panel shows right now, if any.
    panel_tab: Option<String>,
}

/// The players' layer over the chat. One exists per window; [`mount`] places it.
pub struct ThreadPreviewMiniPlayer {
    data: crate::state::app_data::SharedAppData,
    right_panel: WeakEntity<crate::components::right_panel_tabs::RightPanel>,
    shown: Option<Shown>,
    sources: HashMap<EnvironmentId, Rc<dyn PreviewSource>>,
    /// The streamed tab on screen, its environment and thread.
    surface: Option<(MiniPlayerKey, String, Entity<ServerBrowserSurface>, Subscription)>,
    snapshots: HashMap<(MiniPlayerKey, String), PreviewSessionSnapshot>,
    container: Rc<Cell<Bounds<Pixels>>>,
    gesture: Option<Gesture>,
    _players: Subscription,
}

/// The layer for this window, kept in a global so the main pane can place it with one line.
struct MiniPlayerLayer(Entity<ThreadPreviewMiniPlayer>);

impl Global for MiniPlayerLayer {}

/// The floating preview layer for the shown thread, to put over the chat column.
pub fn mount(app: &WeakEntity<crate::app_root::AppRoot>, cx: &mut App) -> Option<gpui_kit::AnyElement> {
    let app = app.upgrade()?;
    let (data, right_panel, store) = {
        let root = app.read(cx);
        (root.data().clone(), root.right_panel().clone(), root.right_panels().clone())
    };
    let shown = right_panel.read(cx).shown_thread().and_then(|(panel_key, target)| match target {
        crate::router::ThreadTarget::Server(key) => {
            let (environment, thread) = crate::state::shell::runtime_ids(key);
            let state = store.read(cx).state(panel_key);
            let panel_tab = state.is_open.then(|| state.active().cloned()).flatten().and_then(|surface| match surface {
                crate::right_panel_store::RightPanelSurface::Preview { resource_id, .. } => resource_id,
                _ => None,
            });
            Some(Shown { key: MiniPlayerKey { environment, thread }, panel_tab })
        }
        crate::router::ThreadTarget::Draft(_) => None,
    });
    let layer = match cx.try_global::<MiniPlayerLayer>() {
        Some(layer) => layer.0.clone(),
        None => {
            let weak = right_panel.downgrade();
            let layer = cx.new(|cx| ThreadPreviewMiniPlayer::new(data, weak, cx));
            cx.set_global(MiniPlayerLayer(layer.clone()));
            layer
        }
    };
    layer.update(cx, |layer, cx| layer.set_shown(shown, cx));
    Some(layer.into_any_element())
}

impl ThreadPreviewMiniPlayer {
    fn new(data: crate::state::app_data::SharedAppData, right_panel: WeakEntity<crate::components::right_panel_tabs::RightPanel>, cx: &mut Context<Self>) -> Self {
        let players = cx.observe_global::<PreviewMiniPlayers>(|_, cx| cx.notify());
        Self {
            data,
            right_panel,
            shown: None,
            sources: HashMap::new(),
            surface: None,
            snapshots: HashMap::new(),
            container: Rc::default(),
            gesture: None,
            _players: players,
        }
    }

    fn set_shown(&mut self, shown: Option<Shown>, cx: &mut Context<Self>) {
        if self.shown != shown {
            self.shown = shown;
            cx.notify();
        }
    }

    /// The thread and tab to float now, if any (`shownPreviewMiniPlayer`): the thread has a
    /// player, the panel is not showing that tab, and the session may operate previews.
    fn visible(&self, cx: &mut App) -> Option<(MiniPlayerKey, MiniPlayerState)> {
        let shown = self.shown.as_ref()?;
        let state = PreviewMiniPlayers::get(&shown.key, cx)?.clone();
        if shown.panel_tab.as_deref() == Some(state.tab_id.as_str()) {
            return None;
        }
        let fixture = shown.key.environment.as_str() == "fixture";
        if !fixture
            && !crate::components::server_update_action::ServerUpdateStore::grants(
                &shown.key.environment,
                t3code_contracts::AuthEnvironmentScope::PreviewOperate,
                cx,
            )
        {
            return None;
        }
        Some((shown.key.clone(), state))
    }

    fn source(&mut self, environment: &EnvironmentId, cx: &mut App) -> Option<Rc<dyn PreviewSource>> {
        if let Some(source) = self.sources.get(environment) {
            return Some(source.clone());
        }
        let source = self.data.preview_source(environment, cx)?;
        self.sources.insert(environment.clone(), source.clone());
        Some(source)
    }

    /// The streamed surface for `tab`, opened once per shown tab.
    fn surface_for(&mut self, key: &MiniPlayerKey, tab: &str, cx: &mut Context<Self>) -> Option<Entity<ServerBrowserSurface>> {
        if let Some((k, t, surface, _)) = &self.surface
            && k == key
            && t == tab
        {
            return Some(surface.clone());
        }
        let source = self.source(&key.environment, cx)?;
        let list = source.list(&key.thread);
        let (list_key, list_tab) = (key.clone(), tab.to_owned());
        cx.spawn(async move |this, cx| {
            if let Ok(result) = list.await {
                this.update(cx, |this, cx| {
                    if let Some(snapshot) = result.sessions.into_iter().find(|s| s.tab_id == list_tab) {
                        this.snapshots.insert((list_key, list_tab), snapshot);
                    } else {
                        // The tab is gone on the server: nothing is left to float.
                        PreviewMiniPlayers::close(&list_key, cx);
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        let (thread, tab_id) = (key.thread.to_string(), tab.to_owned());
        let surface = cx.new(|cx| ServerBrowserSurface::new(source, thread, tab_id, cx).floating());
        let gone_key = key.clone();
        let subscription = cx.subscribe(&surface, move |this, _, event: &SurfaceEvent, cx| match event {
            SurfaceEvent::Gone => PreviewMiniPlayers::close(&gone_key, cx),
            // `showPreviewPopup(…, "floating")`: the page's new tab replaces the floating one.
            SurfaceEvent::Popup(tab) => PreviewMiniPlayers::open(gone_key.clone(), tab.clone(), cx),
            SurfaceEvent::Control(_) | SurfaceEvent::Settled => cx.notify(),
        });
        self.surface = Some((key.clone(), tab.to_owned(), surface.clone(), subscription));
        Some(surface)
    }

    /// The page's size: the stream's viewport, else the tab's fixed viewport, else 1280x800.
    fn source_size(&self, key: &MiniPlayerKey, tab: &str, surface: &Entity<ServerBrowserSurface>, cx: &App) -> (f32, f32) {
        if let Some(viewport) = surface.read(cx).viewport().filter(|v| v.width >= 1. && v.height >= 1.) {
            return (viewport.width as f32, viewport.height as f32);
        }
        let snapshot = self.snapshots.get(&(key.clone(), tab.to_owned()));
        match snapshot.and_then(|s| s.viewport.clone()) {
            Some(t3code_contracts::PreviewViewportSetting::Freeform(v)) => (v.width as f32, v.height as f32),
            Some(t3code_contracts::PreviewViewportSetting::Preset(v)) => (v.width as f32, v.height as f32),
            _ => (1280., 800.),
        }
    }

    fn open_in_panel(&mut self, key: &MiniPlayerKey, tab: &str, cx: &mut Context<Self>) {
        PreviewMiniPlayers::close(key, cx);
        let tab = tab.to_owned();
        self.right_panel.update(cx, |panel, cx| panel.open_browser_tab(&tab, cx)).ok();
    }

    fn begin(&mut self, event: &MouseDownEvent, frame: Frame, direction: Option<Direction>, source: (f32, f32), cx: &mut Context<Self>) {
        self.gesture = Some(Gesture { pointer: event.position, frame, direction, source, moved: false });
        cx.stop_propagation();
        cx.notify();
    }

    /// `stepPreviewMiniPlayerGesture`.
    fn step(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(gesture) = self.gesture else { return };
        if event.pressed_button != Some(MouseButton::Left) {
            return self.end(cx);
        }
        let Some((key, state)) = self.visible(cx) else { return self.end(cx) };
        let delta = (f32::from(event.position.x - gesture.pointer.x), f32::from(event.position.y - gesture.pointer.y));
        if !gesture.moved && delta.0.hypot(delta.1) < TAP_SLOP {
            return;
        }
        self.gesture = Some(Gesture { moved: true, ..gesture });
        let bounds = self.container.get();
        let container = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        match gesture.direction {
            None => {
                let f = gesture.frame;
                let position = clamp_to_container((f.x + delta.0, f.y + delta.1), container, (f.width, f.height), GAP);
                PreviewMiniPlayers::move_to(&key, &state.tab_id, position, cx);
            }
            Some(direction) => {
                let frame = resize_frame(gesture.frame, direction, delta, gesture.source, container);
                PreviewMiniPlayers::resize(&key, &state.tab_id, frame.width, (frame.x, frame.y), cx);
            }
        }
    }

    fn end(&mut self, cx: &mut Context<Self>) {
        if self.gesture.take().is_some() {
            cx.notify();
        }
    }

    fn render_player(&mut self, key: MiniPlayerKey, state: MiniPlayerState, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let surface = self.surface_for(&key, &state.tab_id, cx)?;
        let bounds = self.container.get();
        let container = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if container.0 < 1. || container.1 < 1. {
            return None;
        }
        let source = self.source_size(&key, &state.tab_id, &surface, cx);
        let frame = canvas_frame(&state, source, container);
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let radius = px(CORNER_RADIUS);
        let pill_inset = px(8f32.max((CORNER_RADIUS * 0.55).round()));
        let (open_key, open_tab, close_key) = (key.clone(), state.tab_id.clone(), key.clone());
        let pill_button = |id: &'static str, icon: IconName, label: SharedString| {
            div().id(SharedString::from(format!("{id}-tip"))).tooltip(tooltip(label)).child(Button::new(id).variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(icon))
        };
        let pill = div()
            .id("mini-player-pill")
            .absolute()
            .right_0()
            .top_0()
            .flex()
            .items_center()
            .gap(theme.sp(0.5))
            .h(theme.sp(8.))
            .p(theme.sp(0.5))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(with_alpha(c.border, 0.8))
            .bg(with_alpha(c.popover(), 0.92))
            .shadow(theme.shadows.lg.clone())
            .invisible()
            .group_hover("mini-player-handle", |s| s.visible())
            .when(self.gesture.is_some_and(|g| g.direction.is_none()), |el| el.visible())
            .child(pill_button("mini-player-open-panel", IconName::PanelRight, t!("Open in right panel")).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).on_click(
                cx.listener(move |this, _, _, cx| this.open_in_panel(&open_key, &open_tab, cx)),
            ))
            .child(pill_button("mini-player-close", IconName::X, t!("Close floating preview")).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).on_click(
                move |_, _, cx| PreviewMiniPlayers::close(&close_key, cx),
            ));
        let dot = div()
            .absolute()
            .right_0()
            .top_0()
            .size(theme.sp(2.))
            .rounded_full()
            .bg(with_alpha(c.foreground(), 0.25))
            .border_1()
            .border_color(with_alpha(c.background(), 0.7))
            .shadow(theme.shadows.sm5.clone())
            .group_hover("mini-player-handle", |s| s.invisible());
        let handle = div()
            .id("mini-player-handle")
            .group("mini-player-handle")
            .absolute()
            .right(pill_inset)
            .top(pill_inset)
            .size(theme.sp(3.))
            .cursor(if self.gesture.is_some() { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| this.begin(event, frame, None, source, cx)))
            .child(dot)
            .child(pill);
        let rail = theme.sp(2.);
        let corner = theme.sp(4.);
        let handles = Direction::ALL.map(|direction| {
            let el = div().id(SharedString::from(format!("mini-player-resize-{direction:?}"))).absolute();
            let el = match direction {
                Direction::North => el.left_0().right_0().top(-theme.sp(1.)).h(rail).cursor(CursorStyle::ResizeUpDown),
                Direction::South => el.left_0().right_0().bottom(-theme.sp(1.)).h(rail).cursor(CursorStyle::ResizeUpDown),
                Direction::West => el.top_0().bottom_0().left(-theme.sp(1.)).w(rail).cursor(CursorStyle::ResizeLeftRight),
                Direction::East => el.top_0().bottom_0().right(-theme.sp(1.)).w(rail).cursor(CursorStyle::ResizeLeftRight),
                Direction::NorthWest => el.left(-theme.sp(2.)).top(-theme.sp(2.)).size(corner).cursor(CursorStyle::ResizeUpLeftDownRight),
                Direction::NorthEast => el.right(-theme.sp(2.)).top(-theme.sp(2.)).size(corner).cursor(CursorStyle::ResizeUpRightDownLeft),
                Direction::SouthWest => el.left(-theme.sp(2.)).bottom(-theme.sp(2.)).size(corner).cursor(CursorStyle::ResizeUpRightDownLeft),
                Direction::SouthEast => el.right(-theme.sp(2.)).bottom(-theme.sp(2.)).size(corner).cursor(CursorStyle::ResizeUpLeftDownRight),
            };
            el.on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| this.begin(event, frame, Some(direction), source, cx)))
        });
        // While a gesture runs, follow the pointer anywhere in the window.
        let tracker = self.gesture.is_some().then(|| {
            let entity = cx.entity();
            canvas(|_, _, _| {}, move |_, _, window, _| {
                let moved = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| moved.update(cx, |this, cx| this.step(event, cx)));
                let up = entity.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| up.update(cx, |this, cx| this.end(cx)));
            })
            .absolute()
            .size_0()
        });
        Some(
            div()
                .id("preview-mini-player")
                .absolute()
                .left(px(frame.x))
                .top(px(frame.y))
                .w(px(frame.width))
                .h(px(frame.height))
                .rounded(radius)
                .occlude()
                // `bg-muted shadow-2xl/35` under the page; the 2xl shadow token is the route modal's /40.
                .child(div().absolute().inset_0().rounded(radius).bg(c.muted).shadow(theme.shadows.app_modal.clone()))
                .child(div().absolute().inset_0().rounded(radius).overflow_hidden().child(surface))
                .child(div().absolute().inset_0().rounded(radius).border_1().border_color(with_alpha(c.border, 0.8)))
                .children(handles)
                .child(handle)
                .children(tracker)
                .into_any_element(),
        )
    }
}

impl Render for ThreadPreviewMiniPlayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let container = self.container.clone();
        let entity = cx.entity();
        let player = match self.visible(cx) {
            Some((key, state)) => self.render_player(key, state, cx),
            None => {
                // Nothing floats: drop the stream so a hidden player costs nothing.
                self.surface = None;
                None
            }
        };
        div()
            .absolute()
            .inset_0()
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let previous = container.replace(bounds);
                        if previous.size != bounds.size {
                            entity.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(player)
    }
}

/// Offsets for checks: the frame the player has now, relative to the chat canvas.
pub fn current_frame(cx: &App) -> Option<Frame> {
    let layer = cx.try_global::<MiniPlayerLayer>()?.0.clone();
    let this = layer.read(cx);
    let shown = this.shown.as_ref()?;
    let state = PreviewMiniPlayers::get(&shown.key, cx)?;
    let (_, tab, surface, _) = this.surface.as_ref()?;
    let bounds = this.container.get();
    let source = this.source_size(&shown.key, tab, surface, cx);
    Some(canvas_frame(state, source, (f32::from(bounds.size.width), f32::from(bounds.size.height))))
}
