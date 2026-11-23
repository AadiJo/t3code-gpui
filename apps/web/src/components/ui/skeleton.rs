//! Loading placeholder. Ports `skeleton.tsx`: `bg-muted-foreground/15` with
//! the stepped `animate-skeleton` breath.

use gpui_kit::{App, IntoElement, RenderOnce, StyleRefinement, Styled, Window, div, prelude::*};

use super::motion::{SteppedPulse, pulse_opacity};
use crate::ActiveAppTheme as _;

/// Corner shape: `block` 4px, `card` 8px, `pill` full.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SkeletonShape {
    #[default]
    Block,
    Card,
    Pill,
}

/// A placeholder bar; size it with the usual style methods.
#[derive(IntoElement, Default)]
pub struct Skeleton {
    shape: SkeletonShape,
    style: StyleRefinement,
}

impl Skeleton {
    pub fn new(shape: SkeletonShape) -> Self {
        Self { shape, style: StyleRefinement::default() }
    }
}

impl Styled for Skeleton {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Skeleton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let opacity = pulse_opacity(SteppedPulse::SKELETON, window, cx);
        let theme = cx.app_theme();
        let radius = match self.shape {
            SkeletonShape::Block => theme.radii.sm,
            SkeletonShape::Card => theme.radii.lg,
            SkeletonShape::Pill => theme.radii.full,
        };
        let mut bar = div().bg(theme.derived.skeleton).rounded(radius).opacity(opacity);
        bar.style().refine(&self.style);
        bar
    }
}
