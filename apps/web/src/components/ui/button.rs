//! Button. Ports `button.tsx` at desktop width (the `sm:` values).

use gpui_kit::{
    App, BoxShadow, ClickEvent, ElementId, FocusHandle, Hsla, IntoElement, Pixels, RenderOnce,
    SharedString, StyleRefinement, Window, black, div, prelude::*, px, white,
};

use super::{ControlState, DISABLED_OPACITY, TextStyleExt as _, control_highlight, focus_ring_inset};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::theme_color::with_alpha;
use crate::theme_tokens::Shadows;
use crate::{ActiveAppTheme as _, AppTheme};

const GROUP: &str = "ui-button";

/// `buttonVariants.variant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Destructive,
    DestructiveOutline,
    Ghost,
    GhostMuted,
    GhostDestructive,
    Outline,
    Secondary,
    Glass,
    Link,
    WarningOutline,
    Overlay,
}

impl ButtonVariant {
    pub const ALL: [ButtonVariant; 12] = [
        Self::Default,
        Self::Destructive,
        Self::DestructiveOutline,
        Self::Ghost,
        Self::GhostMuted,
        Self::GhostDestructive,
        Self::Outline,
        Self::Secondary,
        Self::Glass,
        Self::Link,
        Self::WarningOutline,
        Self::Overlay,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Destructive => "destructive",
            Self::DestructiveOutline => "destructive-outline",
            Self::Ghost => "ghost",
            Self::GhostMuted => "ghost-muted",
            Self::GhostDestructive => "ghost-destructive",
            Self::Outline => "outline",
            Self::Secondary => "secondary",
            Self::Glass => "glass",
            Self::Link => "link",
            Self::WarningOutline => "warning-outline",
            Self::Overlay => "overlay",
        }
    }
}

/// `buttonVariants.size`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    #[default]
    Default,
    Sm,
    SmPill,
    Xs,
    Compact,
    Micro,
    Lg,
    Xl,
    Icon,
    IconSm,
    IconSmPill,
    IconXs,
    IconMicro,
    IconTiny,
    IconLg,
    IconXl,
}

impl ButtonSize {
    pub const ALL: [ButtonSize; 16] = [
        Self::Default,
        Self::Sm,
        Self::SmPill,
        Self::Xs,
        Self::Compact,
        Self::Micro,
        Self::Lg,
        Self::Xl,
        Self::Icon,
        Self::IconSm,
        Self::IconSmPill,
        Self::IconXs,
        Self::IconMicro,
        Self::IconTiny,
        Self::IconLg,
        Self::IconXl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Sm => "sm",
            Self::SmPill => "sm-pill",
            Self::Xs => "xs",
            Self::Compact => "compact",
            Self::Micro => "micro",
            Self::Lg => "lg",
            Self::Xl => "xl",
            Self::Icon => "icon",
            Self::IconSm => "icon-sm",
            Self::IconSmPill => "icon-sm-pill",
            Self::IconXs => "icon-xs",
            Self::IconMicro => "icon-micro",
            Self::IconTiny => "icon-tiny",
            Self::IconLg => "icon-lg",
            Self::IconXl => "icon-xl",
        }
    }

    pub fn is_icon(self) -> bool {
        matches!(
            self,
            Self::Icon
                | Self::IconSm
                | Self::IconSmPill
                | Self::IconXs
                | Self::IconMicro
                | Self::IconTiny
                | Self::IconLg
                | Self::IconXl
        )
    }
}

/// Pixel geometry of one size.
#[derive(Clone, Copy)]
struct SizeSpec {
    height: Pixels,
    padding_x: Pixels,
    gap: Pixels,
    text: ButtonText,
    icon: Pixels,
    radius: Pixels,
}

#[derive(Clone, Copy)]
enum ButtonText {
    Scale(TextSize),
    Px(f32),
}

fn size_spec(size: ButtonSize, theme: &AppTheme) -> SizeSpec {
    let sp = |n: f32| theme.sp(n);
    let r = theme.radii;
    let base = SizeSpec {
        height: sp(8.),
        padding_x: sp(3.) - px(1.),
        gap: sp(2.),
        text: ButtonText::Scale(TextSize::Sm),
        icon: px(16.),
        radius: r.control,
    };
    let square = |side: f32| SizeSpec { height: sp(side), padding_x: px(0.), ..base };
    match size {
        ButtonSize::Default => base,
        ButtonSize::Sm => SizeSpec { height: sp(7.), padding_x: sp(2.5) - px(1.), gap: sp(1.5), ..base },
        ButtonSize::SmPill => SizeSpec { height: sp(7.), padding_x: sp(2.5), gap: sp(1.5), radius: r.full, ..base },
        ButtonSize::Xs => SizeSpec {
            height: sp(6.),
            padding_x: sp(2.) - px(1.),
            gap: sp(1.),
            text: ButtonText::Scale(TextSize::Xs),
            icon: px(14.),
            ..base
        },
        ButtonSize::Compact => SizeSpec {
            height: sp(7.),
            padding_x: sp(2.) - px(1.),
            gap: sp(1.),
            text: ButtonText::Scale(TextSize::Xs),
            icon: px(14.),
            radius: r.md,
            ..base
        },
        ButtonSize::Micro => SizeSpec {
            height: sp(5.),
            padding_x: sp(1.5) - px(1.),
            gap: sp(1.),
            text: ButtonText::Px(11.),
            icon: px(12.),
            radius: r.sm,
            ..base
        },
        ButtonSize::Lg => SizeSpec { height: sp(9.), padding_x: sp(3.5) - px(1.), ..base },
        ButtonSize::Xl => SizeSpec {
            height: sp(10.),
            padding_x: sp(4.) - px(1.),
            text: ButtonText::Scale(TextSize::Base),
            icon: px(18.),
            ..base
        },
        ButtonSize::Icon => square(8.),
        ButtonSize::IconSm => square(7.),
        ButtonSize::IconSmPill => SizeSpec { radius: r.full, ..square(7.) },
        ButtonSize::IconXs => SizeSpec { icon: px(14.), ..square(6.) },
        ButtonSize::IconMicro => SizeSpec { icon: px(12.), radius: r.sm, ..square(5.) },
        ButtonSize::IconTiny => SizeSpec { icon: px(12.), ..square(4.) },
        ButtonSize::IconLg => square(9.),
        ButtonSize::IconXl => SizeSpec { icon: px(18.), ..square(10.) },
    }
}

/// The colours and shadows of one variant in one state.
#[derive(Clone)]
struct Paint {
    bg: Hsla,
    border: Hsla,
    text: Hsla,
    icon: Hsla,
    shadows: Vec<BoxShadow>,
    /// Whether the `before:` control highlight shows.
    highlight: bool,
    underline: bool,
}

fn paint(variant: ButtonVariant, state: ControlState, theme: &AppTheme) -> Paint {
    let c = &theme.colors;
    let d = &theme.derived;
    let dark = theme.is_dark();
    let hover = matches!(state, ControlState::Hover | ControlState::Active);
    let active = state == ControlState::Active;
    let disabled = state == ControlState::Disabled;
    let clear = Hsla::transparent_black();
    let quiet = active || disabled;
    let base = Paint {
        bg: clear,
        border: clear,
        text: c.foreground(),
        icon: c.foreground(),
        shadows: vec![],
        highlight: false,
        underline: false,
    };
    let filled = |fill: Hsla, text: Hsla| {
        let mut shadows = vec![];
        if !disabled {
            shadows.push(Shadows::button_inset(active));
        }
        if !quiet {
            shadows.extend(Shadows::xs_tinted(fill));
        }
        Paint {
            bg: if hover { with_alpha(fill, 0.9) } else { fill },
            border: fill,
            text,
            icon: text,
            shadows,
            ..base.clone()
        }
    };
    let outlined = |shadows: bool| Paint {
        shadows: if quiet || !shadows { vec![] } else { theme.shadows.xs5.clone() },
        highlight: !quiet,
        ..base.clone()
    };
    match variant {
        ButtonVariant::Default => filled(c.primary(), c.primary_foreground()),
        ButtonVariant::Destructive => filled(c.destructive(), white()),
        ButtonVariant::DestructiveOutline => Paint {
            border: if hover { with_alpha(c.destructive(), 0.32) } else { c.input },
            bg: if hover { with_alpha(c.destructive(), 0.04) } else { d.outline_fill },
            text: c.destructive_foreground(),
            icon: c.destructive_foreground(),
            ..outlined(true)
        },
        ButtonVariant::Ghost => Paint {
            bg: if hover { c.highlight() } else { clear },
            icon: c.muted_foreground,
            ..base
        },
        ButtonVariant::GhostMuted | ButtonVariant::GhostDestructive => {
            let text = match (hover, variant) {
                (false, _) => c.muted_foreground,
                (true, ButtonVariant::GhostMuted) => c.foreground(),
                (true, _) => c.destructive(),
            };
            Paint { bg: if hover { c.highlight() } else { clear }, text, icon: text, ..base }
        }
        ButtonVariant::Outline => Paint {
            border: c.input,
            bg: if hover { d.outline_hover } else { d.outline_fill },
            icon: c.muted_foreground,
            ..outlined(true)
        },
        ButtonVariant::Secondary => Paint {
            bg: match state {
                ControlState::Hover => with_alpha(c.secondary, 0.9),
                ControlState::Active => with_alpha(c.secondary, 0.8),
                _ => c.secondary,
            },
            text: c.secondary_foreground,
            icon: c.secondary_foreground,
            ..base
        },
        ButtonVariant::Glass => Paint {
            bg: d.surface_glass_opaque,
            border: if hover { c.border } else { with_alpha(c.border, 0.6) },
            icon: c.muted_foreground,
            shadows: vec![
                BoxShadow::new(px(0.), px(1.), with_alpha(black(), 0.1)).blur_radius(px(3.)),
                BoxShadow::new(px(0.), px(1.), with_alpha(black(), 0.1))
                    .blur_radius(px(2.))
                    .spread_radius(px(-1.)),
            ],
            ..base
        },
        ButtonVariant::Link => Paint { underline: hover, ..base },
        ButtonVariant::WarningOutline => Paint {
            border: with_alpha(c.warning, if hover { 0.40 } else { 0.32 }),
            bg: if hover {
                with_alpha(c.warning, if dark { 0.24 } else { 0.16 })
            } else {
                c.warning_surface
            },
            text: c.warning_foreground,
            icon: c.warning_foreground,
            shadows: if quiet { vec![] } else { theme.shadows.xs5.clone() },
            ..base
        },
        ButtonVariant::Overlay => Paint {
            bg: with_alpha(black(), if hover { 0.9 } else { 0.7 }),
            text: with_alpha(white(), 0.65),
            icon: with_alpha(white(), 0.65),
            ..base
        },
    }
}

fn apply(style: StyleRefinement, paint: &Paint) -> StyleRefinement {
    let style = style.bg(paint.bg).border_color(paint.border).text_color(paint.text).shadow(paint.shadows.clone());
    if paint.underline { style.underline() } else { style }
}

/// A button: a label, an icon, or both.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<IconName>,
    trailing_icon: Option<IconName>,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    pressed: bool,
    state: Option<ControlState>,
    focus_handle: Option<FocusHandle>,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            trailing_icon: None,
            variant: ButtonVariant::Default,
            size: ButtonSize::Default,
            disabled: false,
            pressed: false,
            state: None,
            focus_handle: None,
            on_click: None,
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

    pub fn trailing_icon(mut self, icon: IconName) -> Self {
        self.trailing_icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// `data-pressed` / `aria-pressed`: draws the hover fill while true.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Draws one interaction state without a pointer (gallery and tests).
    pub fn state(mut self, state: ControlState) -> Self {
        self.state = Some(state);
        self
    }

    /// Makes the button keyboard-focusable; it shows the focus ring when focused by keyboard.
    pub fn track_focus(mut self, handle: &FocusHandle) -> Self {
        self.focus_handle = Some(handle.clone());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let spec = size_spec(self.size, theme);
        let disabled = self.disabled || self.state == Some(ControlState::Disabled);
        let state = match self.state {
            Some(state) => state,
            None if disabled => ControlState::Disabled,
            None if self.pressed => ControlState::Hover,
            None => ControlState::Idle,
        };
        let live = self.state.is_none() && !disabled;
        let rest = paint(self.variant, state, theme);
        let hover = paint(self.variant, ControlState::Hover, theme);
        let active = paint(self.variant, ControlState::Active, theme);
        let mut shadows = rest.shadows.clone();
        if state == ControlState::FocusVisible {
            shadows.extend(focus_ring_inset(theme));
        }
        let ring = focus_ring_inset(theme);
        let text_weight = weight::MEDIUM;
        let icon_margin = if self.size.is_icon() { px(0.) } else { px(-2.) };
        let icon = |name: IconName| {
            div().mx(icon_margin).child(
                Icon::new(name)
                    .size(IconSize::Custom(spec.icon))
                    .color(rest.icon)
                    .group_color(GROUP, hover.icon),
            )
        };
        let mut button = div()
            .id(self.id)
            .group(GROUP)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(spec.gap)
            .h(spec.height)
            .px(spec.padding_x)
            .when(self.size.is_icon(), |b| b.w(spec.height))
            .rounded(spec.radius)
            .border_1()
            .border_color(rest.border)
            .bg(rest.bg)
            .text_color(rest.text)
            .shadow(shadows)
            .whitespace_nowrap()
            .when(rest.underline, |b| b.underline());
        button = match spec.text {
            ButtonText::Scale(size) => button.ui_text(theme, size, text_weight),
            ButtonText::Px(size) => button.ui_text(theme, TextSize::Xs, text_weight).text_size(px(size)),
        };
        if rest.highlight {
            button = button.child(control_highlight(theme, spec.radius));
        }
        button
            .children(self.icon.map(icon))
            .children(self.label)
            .children(self.trailing_icon.map(icon))
            .when(disabled, |b| b.opacity(DISABLED_OPACITY))
            .when(live, |b| {
                b.cursor_pointer()
                    .hover(|s| apply(s, &hover))
                    .active(|s| apply(s, &active))
            })
            .when_some(self.focus_handle.filter(|_| live), |b, handle| {
                b.track_focus(&handle).focus_visible(move |s| s.shadow(ring.clone()))
            })
            .when_some(self.on_click.filter(|_| live), |b, handler| {
                b.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}
