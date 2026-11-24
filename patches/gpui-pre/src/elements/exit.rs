use std::time::Duration;

use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, Window, point, px,
};

/// How an element leaves when it stops being rendered, like a CSS transition to
/// `data-ending-style`: it eases from as it was to faded out, scaled by `scale` about
/// `origin` and moved by `translate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExitTransition {
    /// How long it lasts. Zero removes the element at once.
    pub duration: Duration,
    /// The easing, as `cubic-bezier(x1, y1, x2, y2)`.
    pub easing: [f32; 4],
    /// The scale it ends at.
    pub scale: f32,
    /// The point it scales about, as a fraction of its bounds (CSS `transform-origin`).
    pub origin: Point<f32>,
    /// How far it moves by the end.
    pub translate: Point<Pixels>,
}

impl ExitTransition {
    /// A fade out over `ms` milliseconds along `cubic-bezier(x1, y1, x2, y2)`.
    pub fn fade(ms: u64, easing: [f32; 4]) -> Self {
        Self {
            duration: Duration::from_millis(ms),
            easing,
            scale: 1.,
            origin: point(0.5, 0.5),
            translate: point(px(0.), px(0.)),
        }
    }

    /// Also scales to `scale` about `origin`.
    pub fn scale(self, scale: f32, origin: Point<f32>) -> Self {
        Self { scale, origin, ..self }
    }

    /// Also moves by `translate`.
    pub fn translate(self, translate: Point<Pixels>) -> Self {
        Self { translate, ..self }
    }

    /// The eased progress at `t` in 0..1.
    pub(crate) fn ease(&self, t: f32) -> f32 {
        let [x1, y1, x2, y2] = self.easing;
        let bezier = |a: f32, b: f32, s: f32| {
            3. * a * s * (1. - s) * (1. - s) + 3. * b * s * s * (1. - s) + s * s * s
        };
        // Solve x(s) = t by bisection; x is monotonic for x1, x2 in 0..1.
        let (mut lo, mut hi) = (0f32, 1f32);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.;
            if bezier(x1, x2, mid) < t { lo = mid } else { hi = mid }
        }
        bezier(y1, y2, (lo + hi) / 2.)
    }
}

/// Wraps `child` so that when it stops being rendered under `id`, the window keeps
/// drawing its last frame through `transition` (see [`ExitTransition`]). The leaving copy
/// is only drawn, so it takes no input.
pub fn exit(id: impl Into<ElementId>, transition: ExitTransition, child: impl IntoElement) -> Exit {
    Exit { id: id.into(), transition, child: child.into_any_element() }
}

/// The element [`exit`] builds.
pub struct Exit {
    id: ElementId,
    transition: ExitTransition,
    child: AnyElement,
}

impl IntoElement for Exit {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Exit {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let start = window.next_frame.scene.len();
        self.child.paint(window, cx);
        if let Some(id) = id
            && !self.transition.duration.is_zero()
        {
            window.record_exit(id.clone(), start, bounds, self.transition);
        }
    }
}
