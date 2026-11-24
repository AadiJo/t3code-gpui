//! `ThemePreviewCircle`: the shaded sphere on each theme card. The web
//! paints two radial gradients in oklab over the canvas mixed toward black
//! or white, blurs them by 3px, scales them by 1.1 and clips them to a
//! circle with a 2px background-coloured border and a 1px inset edge
//! (`ThemePreviewCircles.tsx`, `@t3tools/shared/themePreview`). GPUI has no
//! radial gradients or blur filter, so this rasterises the same recipe once
//! per palette and mode into an image at 2x.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{Hsla, ImageSource, IntoElement, RenderImage, Styled as _, img, px};
use image::{Frame, RgbaImage};

use crate::theme_palette::Appearance;

/// The three palette roles a sphere reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SphereColors {
    pub canvas: Hsla,
    pub accent: Hsla,
    pub message_action: Hsla,
}

/// `STANDARD_THEME_PREVIEW_COLORS`, used for the built-in T3 Code card.
pub fn standard_sphere_colors(mode: Appearance) -> SphereColors {
    let hex = |v: &str| crate::syntax::hex(v).unwrap_or_default();
    if mode.is_dark() {
        SphereColors { canvas: hex("#0a0a0a"), accent: hex("#1c1c1f"), message_action: hex("#8b9cff") }
    } else {
        SphereColors { canvas: hex("#fcfcfc"), accent: hex("#f4f4f5"), message_action: hex("#4f46e5") }
    }
}

/// `THEME_PREVIEW_RENDER_SPECS[mode]`.
struct Spec {
    base_target: [f32; 3],
    accent_center: (f32, f32),
    accent_middle_offset: f32,
    accent_middle_opacity: f32,
    accent_end_offset: f32,
    action_center: (f32, f32),
    action_start_opacity: f32,
    action_end_offset: f32,
}

fn spec(mode: Appearance) -> Spec {
    if mode.is_dark() {
        Spec {
            base_target: [9. / 255., 9. / 255., 11. / 255.],
            accent_center: (0.28, 0.78),
            accent_middle_offset: 0.28,
            accent_middle_opacity: 0.62,
            accent_end_offset: 0.58,
            action_center: (0.82, 0.18),
            action_start_opacity: 0.45,
            action_end_offset: 0.55,
        }
    } else {
        Spec {
            base_target: [1., 1., 1.],
            accent_center: (0.72, 0.22),
            accent_middle_offset: 0.28,
            accent_middle_opacity: 0.72,
            accent_end_offset: 0.58,
            action_center: (0.18, 0.82),
            action_start_opacity: 0.45,
            action_end_offset: 0.55,
        }
    }
}

const BASE_WEIGHT: f32 = 0.8;
const SCALE: f32 = 1.1;
const BLUR: f32 = 3.;
/// The sphere's border (`border-2`), in CSS px. Its box is `size-14`, which follows the rem.
const BORDER: f32 = 2.;
/// Samples per CSS px.
const DENSITY: f32 = 2.;

fn srgb(color: Hsla) -> [f32; 3] {
    let rgba = color.to_rgb();
    [rgba.r, rgba.g, rgba.b]
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn to_gamma(c: f32) -> f32 {
    let c = c.max(0.);
    if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1. / 2.4) - 0.055 }
}

fn oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let (r, g, b) = (to_linear(r), to_linear(g), to_linear(b));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> [f32; 3] {
    let lp = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let mp = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let sp = (l - 0.0894841775 * a - 1.291485548 * b).powi(3);
    [
        to_gamma(4.0767416621 * lp - 3.3077115913 * mp + 0.2309699292 * sp).min(1.),
        to_gamma(-1.2684380046 * lp + 2.6097574011 * mp - 0.3413193965 * sp).min(1.),
        to_gamma(-0.0041960863 * lp - 0.7034186147 * mp + 1.707614701 * sp).min(1.),
    ]
}

/// A radial gradient of one colour whose alpha falls along `stops`
/// (offset, alpha), sampled at `t` of the gradient ray. Premultiplied
/// interpolation of a single colour keeps the colour and moves the alpha.
fn ramp(stops: &[(f32, f32)], t: f32) -> f32 {
    let Some(first) = stops.first() else { return 0. };
    if t <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let ((t0, a0), (t1, a1)) = (pair[0], pair[1]);
        if t <= t1 {
            return a0 + (a1 - a0) * ((t - t0) / (t1 - t0).max(1e-6));
        }
    }
    stops.last().map_or(0., |s| s.1)
}

/// Source-over in gamma space, as the browser composites layers.
fn over(dst: [f32; 4], src: [f32; 3], alpha: f32) -> [f32; 4] {
    let out_a = alpha + dst[3] * (1. - alpha);
    if out_a <= 0. {
        return [0.; 4];
    }
    let mix = |s: f32, d: f32| (s * alpha + d * dst[3] * (1. - alpha)) / out_a;
    [mix(src[0], dst[0]), mix(src[1], dst[1]), mix(src[2], dst[2]), out_a]
}

/// Rasterises one sphere at 2x, composited over `backdrop` (the card),
/// with its border in `ring` (the page background).
fn render(colors: SphereColors, mode: Appearance, ring: Hsla, backdrop: Hsla, size: f32) -> RgbaImage {
    let spec = spec(mode);
    let canvas = oklab(srgb(colors.canvas));
    let target = oklab(spec.base_target);
    let base = from_oklab(std::array::from_fn(|i| canvas[i] * BASE_WEIGHT + target[i] * (1. - BASE_WEIGHT)));
    let accent = srgb(colors.accent);
    let action = srgb(colors.message_action);
    // The inner layer is the padding box, blurred with a transparent margin.
    let inner = size - 2. * BORDER;
    let margin = 4. * BLUR;
    let n = ((inner + 2. * margin) * DENSITY).round() as usize;
    let at = |i: usize| i as f32 / DENSITY - margin + 0.5 / DENSITY;
    let farthest = |(cx, cy): (f32, f32)| {
        let (x, y) = (cx * inner, cy * inner);
        [(0., 0.), (inner, 0.), (0., inner), (inner, inner)]
            .iter()
            .map(|(px, py)| ((px - x).powi(2) + (py - y).powi(2)).sqrt())
            .fold(0., f32::max)
    };
    let (accent_r, action_r) = (farthest(spec.accent_center), farthest(spec.action_center));
    let accent_stops = [(0., 1.), (spec.accent_middle_offset, spec.accent_middle_opacity), (spec.accent_end_offset, 0.)];
    let action_stops = [(0., spec.action_start_opacity), (spec.action_end_offset, 0.)];
    let mut layer = vec![[0f32; 4]; n * n];
    for j in 0..n {
        for i in 0..n {
            let (x, y) = (at(i), at(j));
            if !(0. ..inner).contains(&x) || !(0. ..inner).contains(&y) {
                continue;
            }
            let dist = |(cx, cy): (f32, f32)| ((x - cx * inner).powi(2) + (y - cy * inner).powi(2)).sqrt();
            let mut px = [base[0], base[1], base[2], 1.];
            px = over(px, action, ramp(&action_stops, dist(spec.action_center) / action_r));
            px = over(px, accent, ramp(&accent_stops, dist(spec.accent_center) / accent_r));
            // Premultiply for the blur.
            layer[j * n + i] = [px[0] * px[3], px[1] * px[3], px[2] * px[3], px[3]];
        }
    }
    let sigma = BLUR * DENSITY;
    let radius = (3. * sigma).ceil() as isize;
    let kernel: Vec<f32> = (-radius..=radius).map(|k| (-(k * k) as f32 / (2. * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    let blur = |src: &[[f32; 4]], horizontal: bool| {
        let mut out = vec![[0f32; 4]; n * n];
        for j in 0..n {
            for i in 0..n {
                let mut acc = [0f32; 4];
                for (k, w) in kernel.iter().enumerate() {
                    let o = k as isize - radius;
                    let (si, sj) = if horizontal { (i as isize + o, j as isize) } else { (i as isize, j as isize + o) };
                    if si < 0 || sj < 0 || si >= n as isize || sj >= n as isize {
                        continue;
                    }
                    let s = src[sj as usize * n + si as usize];
                    for c in 0..4 {
                        acc[c] += s[c] * w;
                    }
                }
                out[j * n + i] = acc.map(|v| v / sum);
            }
        }
        out
    };
    let blurred = blur(&blur(&layer, true), false);
    let sample = |x: f32, y: f32| -> [f32; 4] {
        // Bilinear in the blurred grid; (x, y) in padding-box px.
        let gx = (x + margin) * DENSITY - 0.5;
        let gy = (y + margin) * DENSITY - 0.5;
        let (x0, y0) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - x0, gy - y0);
        let get = |i: f32, j: f32| {
            if i < 0. || j < 0. || i >= n as f32 || j >= n as f32 { [0.; 4] } else { blurred[j as usize * n + i as usize] }
        };
        let (a, b, c, d) = (get(x0, y0), get(x0 + 1., y0), get(x0, y0 + 1.), get(x0 + 1., y0 + 1.));
        std::array::from_fn(|k| (a[k] * (1. - fx) + b[k] * fx) * (1. - fy) + (c[k] * (1. - fx) + d[k] * fx) * fy)
    };
    let ring = srgb(ring);
    let backdrop = srgb(backdrop);
    let edge = if mode.is_dark() { ([1., 1., 1.], 0.14) } else { ([0., 0., 0.], 0.10) };
    let out = (size * DENSITY).round() as u32;
    let center = out as f32 / DENSITY / 2.;
    let mut image = RgbaImage::new(out, out);
    for (i, j, pixel) in image.enumerate_pixels_mut() {
        let (x, y) = ((i as f32 + 0.5) / DENSITY, (j as f32 + 0.5) / DENSITY);
        let d = ((x - center).powi(2) + (y - center).powi(2)).sqrt();
        let coverage = (center - d + 0.5 / DENSITY).clamp(0., 1. / DENSITY) * DENSITY;
        if coverage <= 0. {
            continue;
        }
        let padding_radius = center - BORDER;
        let mut color = [backdrop[0], backdrop[1], backdrop[2], 1.];
        if d < padding_radius + 0.5 {
            // Scaled about the padding box's centre.
            let (u, v) = ((x - center) / SCALE + padding_radius, (y - center) / SCALE + padding_radius);
            let s = sample(u, v);
            if s[3] > 0. {
                color = over(color, [s[0] / s[3], s[1] / s[3], s[2] / s[3]], s[3]);
            }
            if d > padding_radius - 1. {
                color = over(color, edge.0, edge.1);
            }
        }
        let inside = (padding_radius - d + 0.5).clamp(0., 1.);
        let rgb: [f32; 3] = std::array::from_fn(|k| color[k] * inside + ring[k] * (1. - inside));
        // BGRA, the order RenderImage frames take.
        *pixel = image::Rgba([
            (rgb[2] * 255.).round() as u8,
            (rgb[1] * 255.).round() as u8,
            (rgb[0] * 255.).round() as u8,
            (coverage * 255.).round() as u8,
        ]);
    }
    image
}

type Key = ([u32; 4], [u32; 4], [u32; 4], [u32; 4], [u32; 4], bool, u32);

thread_local! {
    static CACHE: RefCell<HashMap<Key, Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

fn key_of(color: Hsla) -> [u32; 4] {
    [color.h.to_bits(), color.s.to_bits(), color.l.to_bits(), color.a.to_bits()]
}

/// The sphere as a `size`-px image (`size-14`). Rendered once per palette, mode, page
/// colours and size, then reused.
pub fn theme_sphere(colors: SphereColors, mode: Appearance, ring: Hsla, backdrop: Hsla, size: gpui_kit::Pixels) -> impl IntoElement {
    let size = f32::from(size);
    let key = (key_of(colors.canvas), key_of(colors.accent), key_of(colors.message_action), key_of(ring), key_of(backdrop), mode.is_dark(), size.to_bits());
    let image = CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| Arc::new(RenderImage::new([Frame::new(render(colors, mode, ring, backdrop, size))])))
            .clone()
    });
    img(ImageSource::Render(image)).size(px(size)).flex_none()
}
