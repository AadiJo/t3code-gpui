//! Badge. Ports `badge.tsx` at desktop width.

use gpui_kit::{rems, App, Hsla, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px, white};

use super::TextStyleExt as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

/// `badgeVariants.variant`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeVariant {
    #[default]
    Default,
    Destructive,
    Error,
    Info,
    Outline,
    Secondary,
    Success,
    Warning,
}

impl BadgeVariant {
    pub const ALL: [BadgeVariant; 8] = [
        Self::Default,
        Self::Destructive,
        Self::Error,
        Self::Info,
        Self::Outline,
        Self::Secondary,
        Self::Success,
        Self::Warning,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Destructive => "destructive",
            Self::Error => "error",
            Self::Info => "info",
            Self::Outline => "outline",
            Self::Secondary => "secondary",
            Self::Success => "success",
            Self::Warning => "warning",
        }
    }

    /// Background, border and text colours.
    fn colors(self, theme: &AppTheme) -> (Hsla, Hsla, Hsla) {
        let c = &theme.colors;
        let f = &theme.fixed;
        let tint = if theme.is_dark() { 0.16 } else { 0.08 };
        let clear = Hsla::transparent_black();
        match self {
            Self::Default => (c.primary(), clear, c.primary_foreground()),
            Self::Destructive => (c.destructive(), clear, white()),
            Self::Error => (with_alpha(c.destructive(), tint), clear, c.destructive_foreground()),
            Self::Info => (with_alpha(f.info, tint), clear, f.info_foreground),
            Self::Outline => (theme.derived.input_fill, c.input, c.foreground()),
            Self::Secondary => (c.secondary, clear, c.secondary_foreground),
            Self::Success => (with_alpha(f.success, tint), clear, f.success_foreground),
            Self::Warning => (with_alpha(c.warning, tint), clear, c.warning_foreground),
        }
    }
}

/// `badgeVariants.size`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeSize {
    #[default]
    Default,
    Sm,
    Lg,
    Control,
}

impl BadgeSize {
    pub const ALL: [BadgeSize; 4] = [Self::Sm, Self::Default, Self::Lg, Self::Control];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Sm => "sm",
            Self::Lg => "lg",
            Self::Control => "control",
        }
    }
}

/// A small label.
#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    icon: Option<IconName>,
    variant: BadgeVariant,
    size: BadgeSize,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self { label: label.into(), icon: None, variant: BadgeVariant::Default, size: BadgeSize::Default }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: BadgeSize) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let (bg, border, text) = self.variant.colors(theme);
        let sp = |n: f32| theme.sp(n);
        let (height, padding, radius) = match self.size {
            BadgeSize::Default => (sp(4.5), sp(1.) - px(1.), theme.radii.sm),
            BadgeSize::Sm => (sp(4.), sp(1.) - px(1.), theme.rem(0.25)),
            BadgeSize::Lg => (sp(5.5), sp(1.5) - px(1.), theme.radii.sm),
            BadgeSize::Control => (sp(6.), sp(2.) - px(1.), theme.radii.control),
        };
        let badge = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(sp(1.))
            .h(height)
            .min_w(height)
            .px(padding)
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(text)
            .whitespace_nowrap();
        let badge = match self.size {
            BadgeSize::Sm => badge.ui_text(theme, TextSize::Xs, weight::MEDIUM).text_size(px(10.)).line_height(px(10.)),
            BadgeSize::Lg => badge.ui_text(theme, TextSize::Sm, weight::MEDIUM),
            _ => badge.ui_text(theme, TextSize::Xs, weight::MEDIUM),
        };
        badge
            .children(self.icon.map(|icon| Icon::new(icon).size(IconSize::Xs).color(text).opacity(0.8)))
            .child(self.label)
    }
}
