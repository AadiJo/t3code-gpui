//! Loading spinner. Ports `spinner.tsx`: Lucide `loader-circle` turning once
//! a second. The scene steps the rotation at a few frames per second without
//! re-rendering the view.

use gpui_kit::{App, Hsla, IntoElement, RenderOnce, Window};

use crate::ActiveAppTheme as _;
use crate::components::icons::{Icon, IconName, IconSize};

/// `xs` 12px, `sm` 14px, `md` 16px, `lg` 20px.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpinnerSize {
    Xs,
    Sm,
    #[default]
    Md,
    Lg,
}

impl SpinnerSize {
    pub const ALL: [SpinnerSize; 4] = [Self::Xs, Self::Sm, Self::Md, Self::Lg];

    fn icon(self) -> IconSize {
        match self {
            Self::Xs => IconSize::Xs,
            Self::Sm => IconSize::Sm,
            Self::Md => IconSize::Md,
            Self::Lg => IconSize::Xl,
        }
    }
}

/// A spinning `loader-circle`. Tone `current` follows the text colour; `muted` is muted.
#[derive(IntoElement, Default)]
pub struct Spinner {
    size: SpinnerSize,
    muted: bool,
    color: Option<Hsla>,
}

impl Spinner {
    pub fn new(size: SpinnerSize) -> Self {
        Self { size, ..Self::default() }
    }

    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Spinner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let icon = Icon::new(IconName::LoaderCircle)
            .size(self.size.icon())
            .spin(theme.motion.spin_cycle, theme.motion.spin_steps_per_second);
        match (self.color, self.muted) {
            (Some(color), _) => icon.color(color),
            (None, true) => icon.color(theme.colors.muted_foreground),
            (None, false) => icon,
        }
    }
}
