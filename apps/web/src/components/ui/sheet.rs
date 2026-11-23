//! Sheet. Ports `sheet.tsx`: an opaque popover-coloured panel docked to a
//! window edge, 448px wide at most, over a `background/60` backdrop.

use std::rc::Rc;

use gpui_kit::{AnyElement, App, BoxShadow, ElementId, IntoElement, ParentElement, RenderOnce, SharedString, Window, black, div, prelude::*, px};

use super::TextStyleExt as _;
use super::dialog::Modal;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::theme_color::with_alpha;

/// Which edge the sheet docks to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SheetSide {
    Left,
    #[default]
    Right,
}

/// The sheet panel, also drawn inline by the gallery.
#[derive(IntoElement)]
pub struct SheetSurface {
    title: SharedString,
    description: Option<SharedString>,
    side: SheetSide,
    children: Vec<AnyElement>,
}

impl SheetSurface {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self { title: title.into(), description: None, side: SheetSide::Right, children: Vec::new() }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn side(mut self, side: SheetSide) -> Self {
        self.side = side;
        self
    }
}

impl ParentElement for SheetSurface {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SheetSurface {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.app_theme();
        let c = &theme.colors;
        let sp = |n: f32| theme.sp(n);
        let shade = with_alpha(black(), 0.05);
        div()
            .flex()
            .flex_col()
            .h_full()
            .w_full()
            .bg(c.popover())
            .text_color(c.foreground())
            .border_color(c.border)
            .map(|s| match self.side {
                SheetSide::Left => s.border_r_1(),
                SheetSide::Right => s.border_l_1(),
            })
            .shadow(vec![
                BoxShadow::new(px(0.), px(10.), shade).blur_radius(px(15.)).spread_radius(px(-3.)),
                BoxShadow::new(px(0.), px(4.), shade).blur_radius(px(6.)).spread_radius(px(-4.)),
            ])
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(2.))
                    .p(sp(6.))
                    .child(div().ui_text(theme, TextSize::Xl, weight::SEMIBOLD).child(self.title))
                    .children(self.description.map(|d| {
                        div().ui_text(theme, TextSize::Sm, weight::NORMAL).text_color(c.muted_foreground).child(d)
                    })),
            )
            .child(div().flex().flex_col().gap(sp(4.)).px(sp(6.)).pb(sp(6.)).ui_text(theme, TextSize::Sm, weight::NORMAL).children(self.children))
    }
}

/// A sheet over the window; a press on the backdrop or Escape runs `on_close`.
pub fn sheet(id: impl Into<ElementId>, surface: SheetSurface, on_close: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    Modal::sheet(id, surface.side, surface, Rc::new(on_close))
}
