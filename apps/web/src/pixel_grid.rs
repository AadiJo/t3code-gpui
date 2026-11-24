//! Where stacked rows land on the device-pixel grid.
//!
//! Chromium lays a page out in 1/64px and snaps each box only when it paints, by rounding the
//! box's absolute edges to device pixels; so at a 15px rem on a 2x display, rows of 26.25px end
//! at exactly 26.25px intervals (alternating 53 and 52 device pixels). GPUI rounds every authored
//! length to device pixels before layout instead (midpoints toward zero, so 26.25px draws at
//! 26.0px) and rounds measured text up, so the same rows drift by a quarter pixel each.
//!
//! The rule: a stack snaps the sum, not the parts. It walks its lengths with a [`PixelGrid`] and
//! authors each one as the difference between two rounded cumulative offsets, which is already
//! whole device pixels, so GPUI's own rounding leaves it alone and every edge lands where
//! Chromium paints it. A run of text whose height is known (one line) is given that snapped
//! height explicitly; wrapped text keeps GPUI's measured height and is the residual error.

use gpui_kit::{Pixels, Window, px};

/// A cursor down one stack, in device pixels. See the module docs.
#[derive(Clone, Copy, Debug)]
pub struct PixelGrid {
    scale: f32,
    /// The exact offset so far, in device pixels.
    exact: f32,
    /// The rounded offset so far, in device pixels.
    snapped: f32,
}

impl PixelGrid {
    /// A cursor at the top of a stack in `window` (whose edge is already on the grid).
    pub fn new(window: &Window) -> Self {
        Self::at_scale(window.scale_factor())
    }

    /// A cursor for a display of `scale` device pixels per logical pixel.
    pub fn at_scale(scale: f32) -> Self {
        Self { scale, exact: 0., snapped: 0. }
    }

    /// The next length of the stack, snapped so its far edge lands on Chromium's pixel.
    pub fn next(&mut self, length: Pixels) -> Pixels {
        self.exact += f32::from(length) * self.scale;
        // Chromium rounds a painted edge half up.
        let edge = (self.exact + 0.5).floor();
        let snapped = edge - self.snapped;
        self.snapped = edge;
        px(snapped / self.scale)
    }

    /// `n` equal lengths in a row (rows of a list): each one's snapped length.
    pub fn run(mut self, length: Pixels, n: usize) -> impl Iterator<Item = Pixels> {
        (0..n).map(move |_| self.next(length))
    }
}
