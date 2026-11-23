//! Colour maths that must match the web app exactly. Ports the OKLCH to sRGB
//! path from `themePalette.ts` (OKLab matrices plus chroma bisection gamut
//! mapping) and the two `color-mix` spaces the stylesheet uses.

use gpui_kit::{Hsla, Rgba};

/// A colour in OKLCH: lightness 0..1, chroma, hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

/// An 8-bit sRGB colour with straight alpha, as the browser stores a resolved colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Srgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Srgb8 {
    /// `#rrggbb`, or `#rrggbbaa` when not opaque.
    pub fn hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// The GPUI colour for this value.
    pub fn hsla(self) -> Hsla {
        Rgba {
            r: self.r as f32 / 255.,
            g: self.g as f32 / 255.,
            b: self.b as f32 / 255.,
            a: self.a as f32 / 255.,
        }
        .into()
    }
}

impl From<Hsla> for Srgb8 {
    fn from(color: Hsla) -> Self {
        let rgba = color.to_rgb();
        let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        Self {
            r: channel(rgba.r),
            g: channel(rgba.g),
            b: channel(rgba.b),
            a: channel(rgba.a),
        }
    }
}

fn oklch_to_linear(color: Oklch) -> [f64; 3] {
    let hr = color.h.to_radians();
    let a = color.c * hr.cos();
    let b = color.c * hr.sin();
    let l = (color.l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (color.l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (color.l - 0.0894841775 * a - 1.291485548 * b).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ]
}

fn linear_to_oklab([r, g, b]: [f64; 3]) -> [f64; 3] {
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    ]
}

fn oklab_to_linear([l, a, b]: [f64; 3]) -> [f64; 3] {
    let h = b.atan2(a).to_degrees();
    oklch_to_linear(Oklch { l, c: a.hypot(b), h })
}

/// Reduces chroma by bisection until the colour fits sRGB (`mapThemeOklchToSrgbGamut`).
fn map_to_gamut(color: Oklch) -> Oklch {
    let in_gamut = |c: f64| {
        oklch_to_linear(Oklch { c, ..color })
            .iter()
            .all(|channel| (-0.0001..=1.0001).contains(channel))
    };
    if in_gamut(color.c) {
        return color;
    }
    let resolution = 0.000001_f64;
    let steps = (color.c.max(resolution).log2() - resolution.log2()).ceil().max(1.) as usize;
    let (mut low, mut high) = (0., color.c);
    for _ in 0..steps {
        let mid = (low + high) / 2.;
        if in_gamut(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    Oklch { c: low, ..color }
}

fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(channel: f64) -> f64 {
    let c = if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1. / 2.4) - 0.055
    };
    c.clamp(0., 1.)
}

fn to_byte(unit: f64) -> u8 {
    (unit.clamp(0., 1.) * 255.).round() as u8
}

/// Converts OKLCH to 8-bit sRGB the way `themeOklchToRgb` does.
pub fn oklch_to_srgb(color: Oklch, alpha: f64) -> Srgb8 {
    linear_to_srgb8(oklch_to_linear(map_to_gamut(color)), alpha)
}

/// Converts OKLCH to 8-bit sRGB the way Chromium draws a CSS literal: an
/// out-of-gamut colour is clipped per channel, not chroma-reduced. Tailwind's
/// palette (emerald-400, teal-400, ...) lies partly outside sRGB.
pub fn oklch_to_srgb_clipped(color: Oklch, alpha: f64) -> Srgb8 {
    linear_to_srgb8(oklch_to_linear(color), alpha)
}

fn linear_to_srgb8([r, g, b]: [f64; 3], alpha: f64) -> Srgb8 {
    Srgb8 {
        r: to_byte(linear_to_srgb(r)),
        g: to_byte(linear_to_srgb(g)),
        b: to_byte(linear_to_srgb(b)),
        a: to_byte(alpha),
    }
}

/// Parses a literal CSS colour a theme file may hold: `oklch(L C H [/ A])`,
/// `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(...)` or `rgba(...)`.
/// OKLCH is gamut-mapped as the theme pipeline does.
pub fn parse_css_color(value: &str) -> Option<Srgb8> {
    parse_color(value, oklch_to_srgb)
}

/// [`parse_css_color`] for a stylesheet literal the browser draws itself
/// (the Tailwind palette): OKLCH is clipped, as Chromium does.
pub fn parse_css_literal(value: &str) -> Option<Srgb8> {
    parse_color(value, oklch_to_srgb_clipped)
}

fn parse_color(value: &str, from_oklch: fn(Oklch, f64) -> Srgb8) -> Option<Srgb8> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex);
    }
    let (name, args) = value.strip_suffix(')')?.split_once('(')?;
    let (channels, alpha) = match args.split_once('/') {
        Some((channels, alpha)) => (channels, Some(alpha.trim())),
        None => (args, None),
    };
    let mut parts: Vec<&str> = channels
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    let alpha = match alpha.or_else(|| (parts.len() == 4).then(|| parts.pop()).flatten()) {
        Some(alpha) => parse_number(alpha, 1.)?,
        None => 1.,
    };
    let [a, b, c] = parts.as_slice() else { return None };
    match name.trim().to_ascii_lowercase().as_str() {
        "oklch" => {
            let color = Oklch {
                l: parse_number(a, 1.)?.clamp(0., 1.),
                c: parse_number(b, 0.4)?.max(0.),
                h: parse_number(c.trim_end_matches("deg"), 1.)?,
            };
            Some(from_oklch(color, alpha))
        }
        "rgb" | "rgba" => Some(Srgb8 {
            r: to_byte(parse_number(a, 1.)?.min(255.) / percent_or(a, 255.)),
            g: to_byte(parse_number(b, 1.)?.min(255.) / percent_or(b, 255.)),
            b: to_byte(parse_number(c, 1.)?.min(255.) / percent_or(c, 255.)),
            a: to_byte(alpha),
        }),
        _ => None,
    }
}

/// A number, or a percentage scaled so 100% equals `full`. `none` is zero.
fn parse_number(value: &str, full: f64) -> Option<f64> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        return Some(0.);
    }
    match value.strip_suffix('%') {
        Some(percent) => Some(percent.parse::<f64>().ok()? / 100. * full),
        None => value.parse().ok(),
    }
}

fn percent_or(value: &str, divisor: f64) -> f64 {
    if value.trim().ends_with('%') { 1. } else { divisor }
}

fn parse_hex(hex: &str) -> Option<Srgb8> {
    let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
    let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 | 4 => Some(Srgb8 {
            r: digit(0)? * 17,
            g: digit(1)? * 17,
            b: digit(2)? * 17,
            a: if hex.len() == 4 { digit(3)? * 17 } else { 255 },
        }),
        6 | 8 => Some(Srgb8 {
            r: pair(0)?,
            g: pair(2)?,
            b: pair(4)?,
            a: if hex.len() == 8 { pair(6)? } else { 255 },
        }),
        _ => None,
    }
}

/// `color` with its alpha multiplied by `alpha`: Tailwind's `bg-x/NN`, which
/// compiles to `color-mix(in oklab, x NN%, transparent)` and only changes alpha.
pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: color.a * alpha, ..color }
}

/// `color-mix(in srgb, a weight, b)`, interpolating premultiplied channels as CSS does.
pub fn mix_srgb(a: Hsla, weight: f32, b: Hsla) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let w = weight.clamp(0., 1.);
    let alpha = a.a * w + b.a * (1. - w);
    if alpha <= 0. {
        return Hsla::transparent_black();
    }
    let channel = |x: f32, y: f32| (x * a.a * w + y * b.a * (1. - w)) / alpha;
    Rgba {
        r: channel(a.r, b.r),
        g: channel(a.g, b.g),
        b: channel(a.b, b.b),
        a: alpha,
    }
    .into()
}

/// `color-mix(in oklab, a weight, b)` for two opaque colours.
pub fn mix_oklab(a: Hsla, weight: f32, b: Hsla) -> Hsla {
    let lab = |color: Hsla| {
        let rgba = color.to_rgb();
        linear_to_oklab([rgba.r, rgba.g, rgba.b].map(|c| srgb_to_linear(c as f64)))
    };
    let (x, y) = (lab(a), lab(b));
    let w = weight.clamp(0., 1.) as f64;
    let mixed = [0, 1, 2].map(|i| x[i] * w + y[i] * (1. - w));
    let [r, g, b] = oklab_to_linear(mixed).map(linear_to_srgb);
    Srgb8 { r: to_byte(r), g: to_byte(g), b: to_byte(b), a: 255 }.hsla()
}

/// Composites `top` over an opaque `bottom` in sRGB, as the browser does.
pub fn flatten(top: Hsla, bottom: Hsla) -> Hsla {
    let (t, b) = (top.to_rgb(), bottom.to_rgb());
    let channel = |x: f32, y: f32| x * t.a + y * (1. - t.a);
    Rgba {
        r: channel(t.r, b.r),
        g: channel(t.g, b.g),
        b: channel(t.b, b.b),
        a: 1.,
    }
    .into()
}
