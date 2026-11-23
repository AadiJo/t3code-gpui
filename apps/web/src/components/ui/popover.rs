//! Popover and the glass surface shared by menus, selects and toasts.
//! Ports `popover.tsx` and the `dropdown-glass` utility (`index.css:367-470`).
//!
//! GPUI cannot blur what is behind an element, so glass surfaces paint the
//! spec's opaque fallback: the translucent popover colour flattened over the
//! canvas.

use gpui_kit::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, IntoElement, ParentElement, Pixels, Point, RenderOnce, SharedString,
    Window, anchored, deferred, div, point, prelude::*, px,
};

use super::{TextStyleExt as _, ControlHighlightExt as _};
use crate::appearance_fonts::{TextSize, weight};
use crate::{ActiveAppTheme as _, AppTheme};

/// `dropdown-glass` with radius 8px and the menu shadow.
pub fn glass_surface(theme: &AppTheme) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .rounded(theme.radii.lg)
        .border_1()
        .border_color(theme.derived.dropdown_glass_border)
        .bg(theme.derived.dropdown_glass_opaque)
        .shadow(theme.shadows.menu.clone())
        .text_color(theme.colors.foreground())
}

/// Floats `content` over the window just below where this element is laid
/// out, offset 4px and kept 8px inside the window (Base UI's positioner).
pub fn below(content: impl IntoElement) -> impl IntoElement {
    deferred(
        anchored()
            .offset(point(px(0.), px(4.)))
            .snap_to_window_with_margin(px(8.))
            .child(content),
    )
    .with_priority(5)
}

/// `PopoverPopup` width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PopoverWidth {
    #[default]
    Auto,
    /// 256px.
    Sm,
    /// 320px.
    Md,
    /// 384px.
    Lg,
}

/// `PopoverPopup` padding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PopoverPadding {
    /// 16px all round.
    #[default]
    Default,
    /// 12px x 8px.
    Compact,
    None,
}

/// The popover card: title and content on a glass surface.
#[derive(IntoElement)]
pub struct PopoverSurface {
    title: Option<SharedString>,
    width: PopoverWidth,
    padding: PopoverPadding,
    children: Vec<AnyElement>,
}

impl PopoverSurface {
    pub fn new() -> Self {
        Self { title: None, width: PopoverWidth::Auto, padding: PopoverPadding::Default, children: Vec::new() }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn width(mut self, width: PopoverWidth) -> Self {
        self.width = width;
        self
    }

    pub fn padding(mut self, padding: PopoverPadding) -> Self {
        self.padding = padding;
        self
    }
}

impl Default for PopoverSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for PopoverSurface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for PopoverSurface {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let width: Option<Pixels> = match self.width {
            PopoverWidth::Auto => None,
            PopoverWidth::Sm => Some(theme.rem(16.)),
            PopoverWidth::Md => Some(theme.rem(20.)),
            PopoverWidth::Lg => Some(theme.rem(24.)),
        };
        let (px_, py_) = match self.padding {
            PopoverPadding::Default => (theme.sp(4.), theme.sp(4.)),
            PopoverPadding::Compact => (theme.sp(3.), theme.sp(2.)),
            PopoverPadding::None => (px(0.), px(0.)),
        };
        glass_surface(theme)
            .when_some(width, |s, w| s.w(w))
            .gap(theme.sp(2.))
            .px(px_)
            .py(py_)
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .control_highlight(theme)
            .children(self.title.map(|title| {
                div().ui_text(theme, TextSize::Sm, weight::SEMIBOLD).line_height(px(TextSize::Sm.size(theme.fonts.interface))).child(title)
            }))
            .children(self.children)
    }
}

/// Where a click's press happened, to match it against the press that closed a popup.
pub fn press_position(event: &ClickEvent) -> Point<Pixels> {
    match event {
        ClickEvent::Mouse(event) => event.down.position,
        other => other.position(),
    }
}

/// Whether a popup is open. Kept per trigger with `window.use_keyed_state`.
#[derive(Default)]
pub struct PopupOpen {
    pub open: bool,
    /// Where the outside press that last closed it happened. A click on the
    /// trigger at the same spot is that same press, so it must not reopen.
    closed_at: Option<Point<Pixels>>,
}

impl PopupOpen {
    /// Opens or closes. A close records the press position when there is one.
    pub fn set(state: &Entity<PopupOpen>, open: bool, press: Option<Point<Pixels>>, cx: &mut App) {
        state.update(cx, |state, cx| {
            state.open = open;
            state.closed_at = if open { None } else { press };
            cx.notify();
        });
    }

    /// The trigger was clicked: toggles, unless this press just closed it.
    pub fn toggle_from(state: &Entity<PopupOpen>, press: Point<Pixels>, cx: &mut App) {
        let current = state.read(cx);
        if current.closed_at == Some(press) {
            state.update(cx, |state, _| state.closed_at = None);
            return;
        }
        let open = !current.open;
        Self::set(state, open, None, cx);
    }

    /// Wraps floating content so a press outside it or Escape closes it.
    pub fn dismissable(state: &Entity<PopupOpen>, content: impl IntoElement) -> impl IntoElement {
        let close = state.clone();
        let escape = state.clone();
        div()
            .occlude()
            .on_mouse_down_out(move |event, _, cx| PopupOpen::set(&close, false, Some(event.position), cx))
            .on_key_down(move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    PopupOpen::set(&escape, false, None, cx);
                }
            })
            .child(content)
    }
}

/// A trigger that opens floating content below itself on click and closes
/// on a click outside or Escape. `content` builds the surface, typically a
/// [`PopoverSurface`] or a menu.
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    trigger: AnyElement,
    content: Box<dyn Fn(PopupHandle, &mut Window, &mut App) -> AnyElement + 'static>,
}

/// Closes the popup that owns it; handed to popup content so a menu item can dismiss its menu.
#[derive(Clone)]
pub struct PopupHandle(Entity<PopupOpen>);

impl PopupHandle {
    pub fn close(&self, cx: &mut App) {
        PopupOpen::set(&self.0, false, None, cx);
    }
}

impl Popover {
    pub fn new(
        id: impl Into<ElementId>,
        trigger: impl IntoElement,
        content: impl Fn(PopupHandle, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self { id: id.into(), trigger: trigger.into_any_element(), content: Box::new(content) }
    }
}

impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| PopupOpen::default());
        let open = state.read(cx).open;
        let toggle = state.clone();
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("trigger")
                    .child(self.trigger)
                    .on_click(move |event, _, cx| PopupOpen::toggle_from(&toggle, press_position(event), cx)),
            )
            .when(open, |el| el.child(below(PopupOpen::dismissable(&state, (self.content)(PopupHandle(state.clone()), window, cx)))))
    }
}
