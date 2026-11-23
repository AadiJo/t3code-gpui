//! Toggle button with an on state. Ports `toggle.tsx` at desktop width.

use gpui_kit::{
    App, ClickEvent, ElementId, Hsla, IntoElement, Pixels, RenderOnce, SharedString, StyleRefinement,
    Window, div, prelude::*, px,
};

use super::{ControlState, DISABLED_OPACITY, TextStyleExt as _, ControlHighlightExt as _, focus_ring_inset, BORDER};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

const GROUP: &str = "ui-toggle";

/// `toggleVariants.variant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleVariant {
    #[default]
    Default,
    Pill,
    Ghost,
    Outline,
    Segmented,
}

impl ToggleVariant {
    pub const ALL: [ToggleVariant; 5] = [Self::Default, Self::Pill, Self::Ghost, Self::Outline, Self::Segmented];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Pill => "pill",
            Self::Ghost => "ghost",
            Self::Outline => "outline",
            Self::Segmented => "segmented",
        }
    }
}

/// `toggleVariants.size`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToggleSize {
    #[default]
    Default,
    Sm,
    Xs,
    Compact,
    Lg,
    Segmented,
}

impl ToggleSize {
    pub const ALL: [ToggleSize; 6] = [Self::Default, Self::Sm, Self::Xs, Self::Compact, Self::Lg, Self::Segmented];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Sm => "sm",
            Self::Xs => "xs",
            Self::Compact => "compact",
            Self::Lg => "lg",
            Self::Segmented => "segmented",
        }
    }
}

struct Paint {
    bg: Hsla,
    text: Hsla,
    shadows: Vec<gpui_kit::BoxShadow>,
}

fn paint(variant: ToggleVariant, pressed: bool, state: ControlState, theme: &AppTheme) -> Paint {
    let c = &theme.colors;
    let d = &theme.derived;
    let dark = theme.is_dark();
    let hover = matches!(state, ControlState::Hover | ControlState::Active);
    let clear = Hsla::transparent_black();
    let (bg, text) = match variant {
        ToggleVariant::Default | ToggleVariant::Pill => {
            let idle_text = if variant == ToggleVariant::Pill { c.muted_foreground } else { c.foreground() };
            match (pressed, hover) {
                (true, _) => (d.toggle_pressed, if variant == ToggleVariant::Pill { c.foreground() } else { c.highlight_foreground() }),
                (false, true) => (c.highlight(), idle_text),
                (false, false) => (clear, idle_text),
            }
        }
        ToggleVariant::Ghost => match (pressed, hover) {
            (true, _) => (c.highlight(), c.highlight_foreground()),
            (false, true) => (c.highlight(), c.foreground()),
            (false, false) if state == ControlState::Disabled => (clear, c.muted_foreground),
            (false, false) => (clear, c.foreground()),
        },
        ToggleVariant::Outline => match (pressed, hover, dark) {
            (true, _, true) => (c.input, c.highlight_foreground()),
            (true, _, false) => (d.toggle_pressed, c.highlight_foreground()),
            (false, true, true) => (with_alpha(c.input, 0.64), c.foreground()),
            (false, true, false) => (c.highlight(), c.foreground()),
            (false, false, _) => (d.input_fill, c.foreground()),
        },
        ToggleVariant::Segmented => match (pressed, hover, dark) {
            (true, _, true) => (with_alpha(c.input, 0.72), c.foreground()),
            (true, _, false) => (c.background(), c.foreground()),
            (false, true, true) => (with_alpha(c.input, 0.32), c.foreground()),
            (false, true, false) => (with_alpha(c.background(), 0.55), c.foreground()),
            (false, false, _) => (clear, c.muted_foreground),
        },
    };
    let quiet = pressed || matches!(state, ControlState::Active | ControlState::Disabled);
    let shadows = match variant {
        ToggleVariant::Outline if !quiet => theme.shadows.xs5.clone(),
        ToggleVariant::Segmented if pressed => vec![
            gpui_kit::BoxShadow::new(px(0.), px(1.), with_alpha(gpui_kit::black(), 0.10)).blur_radius(px(2.)),
        ],
        _ => vec![],
    };
    Paint { bg, text, shadows }
}

fn apply(style: StyleRefinement, paint: &Paint) -> StyleRefinement {
    style.bg(paint.bg).text_color(paint.text).shadow(paint.shadows.clone())
}

/// A two-state button.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<IconName>,
    variant: ToggleVariant,
    size: ToggleSize,
    pressed: bool,
    disabled: bool,
    state: Option<ControlState>,
    on_toggle: Option<Box<dyn Fn(bool, &mut Window, &mut App) + 'static>>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            variant: ToggleVariant::Default,
            size: ToggleSize::Default,
            pressed: false,
            disabled: false,
            state: None,
            on_toggle: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ToggleVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ToggleSize) -> Self {
        self.size = size;
        self
    }

    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn state(mut self, state: ControlState) -> Self {
        self.state = Some(state);
        self
    }

    /// Called with the new pressed value.
    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let sp = |n: f32| theme.sp(n);
        let r = theme.radii;
        let disabled = self.disabled || self.state == Some(ControlState::Disabled);
        let state = self.state.unwrap_or(if disabled { ControlState::Disabled } else { ControlState::Idle });
        let live = self.state.is_none() && !disabled;
        let (height, padding, radius, text, icon): (Pixels, Pixels, Pixels, TextSize, IconSize) = match self.size {
            ToggleSize::Default => (sp(8.), sp(2.) - px(1.), r.lg, TextSize::Sm, IconSize::Md),
            ToggleSize::Sm => (sp(7.), sp(1.5) - px(1.), r.lg, TextSize::Sm, IconSize::Md),
            ToggleSize::Xs => (sp(6.), sp(1.) - px(1.), r.md, TextSize::Sm, IconSize::Md),
            ToggleSize::Compact => (sp(7.), sp(1.) - px(1.), r.md, TextSize::Xs, IconSize::Sm),
            ToggleSize::Lg => (sp(9.), sp(2.5) - px(1.), r.lg, TextSize::Sm, IconSize::Md),
            ToggleSize::Segmented => (sp(6.), sp(2.5), r.md, TextSize::Xs, IconSize::Md),
        };
        let (radius, padding) = match self.variant {
            ToggleVariant::Pill => (r.full, sp(3.)),
            _ => (radius, padding),
        };
        let font_weight = if self.variant == ToggleVariant::Pill { weight::NORMAL } else { weight::MEDIUM };
        let rest = paint(self.variant, self.pressed, state, theme);
        let hover = paint(self.variant, self.pressed, ControlState::Hover, theme);
        let mut shadows = rest.shadows.clone();
        if state == ControlState::FocusVisible {
            shadows.extend(focus_ring_inset(theme, BORDER));
        }
        let outline = self.variant == ToggleVariant::Outline;
        let icon_opacity = if self.variant == ToggleVariant::Ghost && disabled { 1. } else { 0.8 };
        let pressed = self.pressed;
        div()
            .id(self.id)
            .group(GROUP)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(sp(2.))
            .h(height)
            .min_w(if self.size == ToggleSize::Segmented { px(0.) } else { height })
            .px(padding)
            .rounded(radius)
            .border_1()
            .border_color(if outline { theme.colors.input } else { Hsla::transparent_black() })
            .bg(rest.bg)
            .text_color(rest.text)
            .shadow(shadows)
            .ui_text(theme, text, font_weight)
            .whitespace_nowrap()
            .when(outline && !pressed && !disabled, |t| t.control_highlight(theme))
            .children(self.icon.map(|name| {
                div().mx(px(-2.)).child(
                    Icon::new(name).size(icon).color(rest.text).opacity(icon_opacity).group_color(GROUP, hover.text),
                )
            }))
            .children(self.label)
            .when(disabled && !(self.variant == ToggleVariant::Ghost), |t| t.opacity(DISABLED_OPACITY))
            .when(live, |t| t.cursor_pointer().hover(|s| apply(s, &hover)))
            .when_some(self.on_toggle.filter(|_| live), |t, handler| {
                t.on_click(move |_: &ClickEvent, window, cx| handler(!pressed, window, cx))
            })
    }
}
