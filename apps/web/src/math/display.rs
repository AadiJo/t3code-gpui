//! A display formula (`$$ … $$` or a ```` ```math ```` fence) as `MarkdownMath.tsx`
//! draws it: the viewport (`px-0.5 py-0.75`) around `.katex-display`, which centres
//! the formula and scrolls it sideways when it is wider than the column (the
//! caller puts this element in a horizontally scrolling div). It is one selection
//! participant that copies as `\[ … \]`, like the fork's `data-markdown-copy`.

use std::sync::Arc;

use gpui_kit::{
    App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    LayoutId, Pixels, SharedString, Subscription, Window, fill, point, px, size,
    base::{TextSelectionHandle, TextSelectionRegistration},
};

use super::{Formula, atom};
use crate::ActiveAppTheme as _;

/// A display formula's box.
pub struct DisplayMath {
    id: ElementId,
    tex: SharedString,
    formula: Arc<Formula>,
    color: Hsla,
    selection_color: Hsla,
}

impl DisplayMath {
    pub fn new(id: impl Into<ElementId>, tex: SharedString, formula: Arc<Formula>, color: Hsla, selection_color: Hsla) -> Self {
        Self { id: id.into(), tex, formula, color, selection_color }
    }
}

struct Retained {
    handle: TextSelectionHandle,
    _refresh: Subscription,
}

/// The formula's line inside the viewport, its em and the selection participant.
pub struct DisplayLayout {
    handle: TextSelectionHandle,
    line: atom::Extent,
    em: Pixels,
}

impl Element for DisplayMath {
    type RequestLayoutState = DisplayLayout;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, DisplayLayout) {
        let source = atom::display_source(&self.tex);
        let handle = window.with_element_state(global_id.expect("DisplayMath has an id"), |retained: Option<Retained>, window| {
            let retained = retained.unwrap_or_else(|| {
                let handle = TextSelectionHandle::new(source.clone(), cx);
                let refresh = handle.refresh_window_on_change(window, cx);
                Retained { handle, _refresh: refresh }
            });
            (retained.handle.clone(), retained)
        });
        super::ensure_fonts(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = style.line_height.to_pixels(font_size.into(), window.rem_size());
        let (line, em) = atom::display_line(&self.formula, &style.font(), font_size, line_height, window.text_system(), window.scale_factor());
        let theme = cx.app_theme();
        let (pad_x, pad_y) = (theme.rem(0.125), theme.rem(0.1875));
        let natural = em * self.formula.width + pad_x * 2.;
        let height = line.height() + pad_y * 2.;
        let layout_id = window.request_measured_layout(Default::default(), move |known, available, _, _| {
            let column = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            size(column.map_or(natural, |column| column.max(natural)), height)
        });
        (layout_id, DisplayLayout { handle, line, em })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut DisplayLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(&layout.handle, window, cx);
        layout.handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut DisplayLayout,
        _: &mut Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) {
        if layout.handle.snapshot(cx).is_some() {
            window.paint_quad(fill(bounds, self.selection_color));
        }
        let theme = cx.app_theme();
        let (pad_x, pad_y) = (theme.rem(0.125), theme.rem(0.1875));
        let width = layout.em * self.formula.width;
        let inner = bounds.size.width - pad_x * 2.;
        let baseline = bounds.top() + pad_y + layout.line.above;
        let left = bounds.left() + pad_x + ((inner - width) / 2.).max(px(0.));
        if super::dump::enabled() {
            super::dump::write(serde_json::json!({
                "kind": "display", "tex": self.tex.as_ref(),
                "box": [f32::from(bounds.left()), f32::from(bounds.top()), f32::from(bounds.size.width), f32::from(bounds.size.height)],
            }));
        }
        super::paint(&self.formula, point(left, baseline), layout.em, self.color, window, cx);
        if let Some(tag) = &self.formula.tag {
            let tag_left = bounds.right() - pad_x - layout.em * tag.width;
            super::paint(tag, point(tag_left, baseline), layout.em, self.color, window, cx);
        }
    }
}
