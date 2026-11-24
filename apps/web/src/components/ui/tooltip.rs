//! Tooltip. Ports `tooltip.tsx`: `default` is an opaque popover card,
//! `glass` uses `dropdown-glass`, `code` is mono for paths and commands.
//! Attach with GPUI's `.tooltip(tooltip("text"))` on an element with an id.

use gpui_kit::{AnyView, App, AppContext as _, Context, IntoElement, Render, RenderOnce, SharedString, Window, div, prelude::*, px};

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

/// The tooltip card, also usable inline (the gallery draws it statically).
#[derive(IntoElement, Clone)]
pub struct TooltipSurface {
    text: SharedString,
    shortcut: Option<SharedString>,
    variant: TooltipVariant,
}

impl TooltipSurface {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into(), shortcut: None, variant: TooltipVariant::Default }
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
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
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
                .bg(d.dropdown_glass_opaque)
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
        card.control_highlight(theme)
            .child(div().child(self.text))
            .children(self.shortcut.map(Kbd::new))
    }
}

struct TooltipView(TooltipSurface);

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // `data-starting-style:opacity-0` (the `scale-98` half needs a transform GPUI lacks).
        Transition::STANDARD.fade_in("tooltip-enter", div().pb(px(4.)).child(self.0.clone()))
    }
}

/// A tooltip builder for GPUI's `.tooltip(...)`.
pub fn tooltip(surface: impl Into<TooltipSurface>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let surface = surface.into();
    move |_, cx| cx.new(|_| TooltipView(surface.clone())).into()
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
