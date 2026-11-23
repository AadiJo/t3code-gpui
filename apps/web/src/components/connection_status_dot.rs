//! A connection's status dot. Ports `ConnectionStatusDot.tsx`. The web's
//! `animate-status-ping` halo repaints every frame, so here a connecting
//! dot gets a static halo instead.

use gpui_kit::{App, Hsla, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px};

use crate::ActiveAppTheme as _;
use crate::components::ui::tooltip::tooltip;
use crate::connection::presentation::PresentedPhase;
use crate::theme_color::with_alpha;

/// An 8px dot in a 12px box, with an optional tooltip.
#[derive(IntoElement)]
pub struct ConnectionStatusDot {
    id: SharedString,
    /// `None` is the muted `muted-foreground/40` dot.
    color: Option<Hsla>,
    halo: bool,
    tooltip: Option<SharedString>,
}

impl ConnectionStatusDot {
    pub fn new(color: Option<Hsla>, tooltip: Option<SharedString>, id: SharedString) -> Self {
        Self { id, color, halo: false, tooltip }
    }

    /// `connectionPhaseDotClassName` and `connectionPhasePingClassName`.
    pub fn for_phase(phase: PresentedPhase, tooltip: Option<SharedString>, id: SharedString) -> PhaseDot {
        PhaseDot { phase, tooltip, id }
    }
}

/// A dot whose colour comes from the theme at render time.
#[derive(IntoElement)]
pub struct PhaseDot {
    phase: PresentedPhase,
    tooltip: Option<SharedString>,
    id: SharedString,
}

impl RenderOnce for PhaseDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let (color, halo) = match self.phase {
            PresentedPhase::Connected => (Some(theme.fixed.success), false),
            PresentedPhase::Connecting | PresentedPhase::Reconnecting | PresentedPhase::AwaitingApproval => {
                (Some(theme.colors.warning), true)
            }
            PresentedPhase::Error => (Some(theme.colors.destructive()), false),
            _ => (None, false),
        };
        ConnectionStatusDot { id: self.id, color, halo, tooltip: self.tooltip }
    }
}

impl RenderOnce for ConnectionStatusDot {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let color = self.color.unwrap_or(with_alpha(theme.colors.muted_foreground, 0.4));
        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(12.))
            .rounded(theme.radii.full)
            .when(self.halo, |d| d.bg(with_alpha(color, 0.25)))
            .when_some(self.tooltip, |d, text| d.tooltip(tooltip(text)))
            .child(div().size(px(8.)).rounded(theme.radii.full).bg(color))
    }
}
