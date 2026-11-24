//! Tooltip. Ports `tooltip.tsx`: `default` is an opaque popover card,
//! `glass` uses `dropdown-glass`, `code` is mono for paths and commands.
//! Attach with GPUI's `.tooltip(tooltip("text"))` on an element with an id; it opens at the
//! pointer. [`TooltipAnchor`] opens it on a side of its trigger instead (`side`, `sideOffset 4`,
//! `align center`), and [`TooltipSurface::content`] draws custom content such as a hover card.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use crate::components::ui::GlassExt as _;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, IntoElement, Pixels, Point, Render, RenderOnce, SharedString, Task, Window, canvas, div,
    point, prelude::*, px,
};

use super::motion::Transition;
use super::{TextStyleExt as _, ControlHighlightExt as _, kbd::Kbd};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};

/// `TooltipPopup` variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipVariant {
    #[default]
    Default,
    Glass,
    Code,
}

/// Custom tooltip content, built on every frame the tooltip shows.
pub type TooltipContent = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The side of its trigger a [`TooltipAnchor`] tooltip opens on (`side`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipSide {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

/// The tooltip card, also usable inline (the gallery draws it statically).
#[derive(IntoElement, Clone)]
pub struct TooltipSurface {
    text: SharedString,
    /// Muted lines under the text (`flex-col gap-0.5`, `text-muted-foreground`).
    lines: Vec<SharedString>,
    /// The text above muted lines is `font-medium`.
    medium_title: bool,
    content: Option<TooltipContent>,
    /// Redraw once a second while open, for a running timer in the content.
    ticking: bool,
    shortcut: Option<SharedString>,
    variant: TooltipVariant,
}

impl TooltipSurface {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), lines: Vec::new(), medium_title: false, content: None, ticking: false, shortcut: None, variant: TooltipVariant::Default }
    }

    /// Custom content in place of the text (`ThreadHoverCard` inside a glass popup).
    pub fn content(content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Self { content: Some(Rc::new(content)), ..Self::new("") }
    }

    /// Sets the text above the muted lines in `font-medium` (`<span className="font-medium">`).
    pub fn medium_title(mut self) -> Self {
        self.medium_title = true;
        self
    }

    /// Redraws the content every second while it shows.
    pub fn ticking(mut self, ticking: bool) -> Self {
        self.ticking = ticking;
        self
    }

    /// A muted line under the text.
    pub fn line(mut self, text: impl Into<SharedString>) -> Self {
        self.lines.push(text.into());
        self
    }

    pub fn variant(mut self, variant: TooltipVariant) -> Self {
        self.variant = variant;
        self
    }

    /// A key hint after the text.
    pub fn shortcut(mut self, keys: impl Into<SharedString>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }
}

impl RenderOnce for TooltipSurface {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let theme = &theme;
        let c = &theme.colors;
        let d = &theme.derived;
        let radius = theme.radii.md;
        let card = div()
            .relative()
            .flex()
            .items_center()
            .gap(theme.sp(2.))
            .px(theme.sp(2.))
            .py(theme.sp(1.))
            .rounded(radius)
            .border_1()
            .text_color(theme.foregrounds.popover)
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .line_height(px(TextSize::Xs.size(theme.fonts.interface) * 1.375));
        let card = match self.variant {
            TooltipVariant::Glass => card
                .glass(d.glass_dropdown)
                .border_color(d.dropdown_glass_border)
                .shadow(theme.shadows.xl25.clone()),
            TooltipVariant::Default | TooltipVariant::Code => {
                card.bg(c.popover()).border_color(c.border).shadow(theme.shadows.md5.clone())
            }
        };
        let card = match self.variant {
            TooltipVariant::Code => card.max_w(theme.rem(30.)).mono_text(px(11.), weight::NORMAL).line_height(px(11. * 1.625)),
            _ => card.max_w(theme.rem(20.)),
        };
        let muted = theme.colors.muted_foreground;
        let medium = self.medium_title;
        let content = if let Some(content) = &self.content {
            // `max-w-80 text-left whitespace-normal`: the content lays itself out.
            div().min_w_0().child(content(window, cx))
        } else if self.lines.is_empty() {
            div().child(self.text)
        } else {
            div()
                .flex()
                .flex_col()
                .gap(theme.sp(0.5))
                .child(div().text_color(c.foreground()).when(medium, |d| d.font_weight(weight::MEDIUM)).child(self.text))
                .children(self.lines.into_iter().map(|line| div().text_color(muted).child(line)))
        };
        card.control_highlight(theme)
            .child(content)
            .children(self.shortcut.map(Kbd::new))
    }
}

/// Where an anchored tooltip goes: its trigger's box and side, and the point GPUI places the
/// tooltip's root at (the pointer, plus a pixel).
#[derive(Clone, Copy)]
struct Placement {
    side: TooltipSide,
    trigger: Bounds<Pixels>,
    origin: Point<Pixels>,
}

/// `sideOffset`.
const SIDE_OFFSET: Pixels = px(4.);

struct TooltipView {
    surface: TooltipSurface,
    placement: Option<Placement>,
    _tick: Option<Task<()>>,
}

impl TooltipView {
    fn new(surface: TooltipSurface, placement: Option<Placement>, cx: &mut Context<Self>) -> Self {
        let tick = surface.ticking.then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        });
        Self { surface, placement, _tick: tick }
    }
}

impl Render for TooltipView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // `data-starting-style:opacity-0` (the `scale-98` half needs a transform GPUI lacks).
        let Some(Placement { side, trigger, origin }) = self.placement else {
            return Transition::STANDARD.fade_in("tooltip-enter", div().pb(px(4.)).child(self.surface.clone()));
        };
        // GPUI places the root at `origin` and sizes it to its content, so the root stays empty
        // and the card hangs off it: a box beside the trigger, as wide as the widest tooltip
        // (insets on both sides give it a definite width), aligns the card to the trigger.
        let span = window.viewport_size().width.max(window.viewport_size().height);
        let (left, right, top, bottom) = match side {
            TooltipSide::Left => {
                let edge = trigger.left() - SIDE_OFFSET;
                (edge - span - origin.x, origin.x - edge, trigger.top() - origin.y, origin.y - trigger.bottom())
            }
            TooltipSide::Right => {
                let edge = trigger.right() + SIDE_OFFSET;
                (edge - origin.x, origin.x - edge - span, trigger.top() - origin.y, origin.y - trigger.bottom())
            }
            TooltipSide::Top => {
                let edge = trigger.top() - SIDE_OFFSET;
                let center = trigger.center().x;
                (center - span - origin.x, origin.x - center - span, edge - span - origin.y, origin.y - edge)
            }
            TooltipSide::Bottom => {
                let edge = trigger.bottom() + SIDE_OFFSET;
                let center = trigger.center().x;
                (center - span - origin.x, origin.x - center - span, edge - origin.y, origin.y - edge - span)
            }
        };
        let frame = div()
            .absolute()
            .left(left)
            .right(right)
            .top(top)
            .bottom(bottom)
            .flex()
            .map(|d| match side {
                TooltipSide::Left => d.justify_end().items_center(),
                TooltipSide::Right => d.justify_start().items_center(),
                TooltipSide::Top => d.justify_center().items_end(),
                TooltipSide::Bottom => d.justify_center().items_start(),
            })
            .child(div().flex_none().child(self.surface.clone()));
        Transition::STANDARD.fade_in("tooltip-enter", div().size_0().child(frame))
    }
}

/// A tooltip builder for GPUI's `.tooltip(...)`.
pub fn tooltip(surface: impl Into<TooltipSurface>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let surface = surface.into();
    move |_, cx| cx.new(|cx| TooltipView::new(surface.clone(), None, cx)).into()
}

/// Opens a tooltip on a side of its trigger, as `TooltipPopup side=…` does. Put
/// [`TooltipAnchor::measure`] inside the element whose box the tooltip aligns to and pass
/// [`TooltipAnchor::tooltip`] to the hovered element's `.tooltip(...)`.
#[derive(Clone, Default)]
pub struct TooltipAnchor(Rc<Cell<Bounds<Pixels>>>);

impl TooltipAnchor {
    pub fn new() -> Self {
        Self::default()
    }

    /// An invisible child filling its parent's padding box, recording it every frame.
    pub fn measure(&self) -> AnyElement {
        let cell = self.0.clone();
        canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {}).absolute().top_0().left_0().size_full().into_any_element()
    }

    /// The `.tooltip(...)` builder for `surface` on `side` of the measured box.
    pub fn tooltip(&self, surface: impl Into<TooltipSurface>, side: TooltipSide) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let (surface, cell) = (surface.into(), self.0.clone());
        move |window, cx| {
            let placement = Placement { side, trigger: cell.get(), origin: window.mouse_position() + point(px(1.), px(1.)) };
            cx.new(|cx| TooltipView::new(surface.clone(), Some(placement), cx)).into()
        }
    }
}

impl From<&'static str> for TooltipSurface {
    fn from(text: &'static str) -> Self {
        Self::new(text)
    }
}

impl From<SharedString> for TooltipSurface {
    fn from(text: SharedString) -> Self {
        Self::new(text)
    }
}
