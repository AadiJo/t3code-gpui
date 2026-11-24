//! Surface grain: the 256px noise tile the web paints over the body, the
//! sidebar and inset panes (`index.css:1801-1818`). Generated once as an RGBA
//! tile and painted repeatedly, so it costs a few sprite quads per frame.

use std::sync::{Arc, LazyLock};

use gpui_kit::*;
use image::{Frame, RgbaImage};

use super::style::GRAIN_OPACITY;

const TILE: u32 = 256;

static GRAIN_TILE: LazyLock<Arc<RenderImage>> = LazyLock::new(|| {
    // Chromium's feTurbulence tile, measured over the Graphite sidebar on the
    // owner's Mac: +3.2 levels on #141415 with a spread (sigma) of 0.9 levels.
    // A grey of about 0.78 at 23-68% of the rect's 3.5% opacity matches both.
    let mut image = RgbaImage::new(TILE, TILE);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let h = hash(x, y);
        let value = 0.63 + 0.3 * ((h & 0xff) as f32 / 255.);
        let alpha = (0.235 + 0.442 * (((h >> 8) & 0xff) as f32 / 255.)) * GRAIN_OPACITY;
        // Grey, so BGRA and RGBA order agree; the sprite shader takes straight alpha.
        let grey = (value * 255.).round() as u8;
        *pixel = image::Rgba([grey, grey, grey, (alpha * 255.).round() as u8]);
    }
    Arc::new(RenderImage::new([Frame::new(image)]))
});

fn hash(x: u32, y: u32) -> u32 {
    let mut h = x.wrapping_mul(0x27d4_eb2d) ^ y.wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

/// An absolutely positioned layer that tiles the grain over its parent.
pub fn surface_grain() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |bounds, _, window, _| {
            let tile = GRAIN_TILE.clone();
            let step = px(TILE as f32);
            let mut y = bounds.origin.y;
            while y < bounds.bottom() {
                let mut x = bounds.origin.x;
                while x < bounds.right() {
                    let tile_bounds = Bounds::new(point(x, y), size(step, step));
                    window
                        .paint_image(
                            bounds,
                            tile_bounds,
                            Corners::default(),
                            tile.clone(),
                            0,
                            false,
                        )
                        .ok();
                    x += step;
                }
                y += step;
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}
