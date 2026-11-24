//! Theme import. Ports `vscodeThemeImport.ts` and the parts of
//! `themePalette.ts` it needs: `parseThemeImport` (a T3 theme file or a VS
//! Code colour theme), `parseVsCodeThemeFile`, `createVividThemeColors`,
//! `parseThemeFile` and the OKLCH maths behind them. Colours are kept as
//! the canonical strings the web stores (`oklch(L C H)`), so an imported
//! theme saves the same file the web would.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde_json::{Map, Value};

use crate::Assets;

/// `THEME_FILE_VERSION`.
pub const THEME_FILE_VERSION: u64 = 1;

/// `RESERVED_THEME_IDS`.
pub const RESERVED_THEME_IDS: [&str; 13] = [
    "system", "light", "dark", "t3-chat", "grove", "ocean", "ember", "iris", "t3-chat-dark", "t3-grove", "t3-ocean",
    "t3-ember", "t3-iris",
];

/// A role → canonical colour map (`ThemeColors`).
pub type ThemeColors = BTreeMap<String, String>;

/// Why an import failed, with the web's message.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ImportError(pub String);

fn fail<T>(message: &str) -> Result<T, ImportError> {
    Err(ImportError(message.to_owned()))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgb {
    r: f64,
    g: f64,
    b: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Oklch {
    l: f64,
    c: f64,
    h: f64,
}

/// `parseThemeColor`: any literal CSS colour, through culori's conversion to OKLCH.
fn parse_theme_color(value: &str) -> Option<(Oklch, f64)> {
    let input = value.trim();
    let lower = input.to_ascii_lowercase();
    if let Some(body) = lower.strip_prefix("oklch(").and_then(|rest| rest.strip_suffix(')')) {
        let (channels, alpha) = match body.split_once('/') {
            Some((channels, alpha)) => (channels, Some(alpha.trim())),
            None => (body, None),
        };
        let parts: Vec<&str> = channels.split_whitespace().collect();
        let [l, c, h] = parts.as_slice() else { return None };
        let number = |part: &str, percent_of: f64| -> Option<f64> {
            if part == "none" {
                return Some(0.);
            }
            match part.strip_suffix('%') {
                Some(p) => p.parse::<f64>().ok().map(|p| p / 100. * percent_of),
                None => part.trim_end_matches("deg").parse().ok(),
            }
        };
        let alpha = match alpha {
            Some("none") => 0.,
            Some(alpha) => number(alpha, 1.)?,
            None => 1.,
        };
        let color = Oklch { l: number(l, 1.)?.clamp(0., 1.), c: number(c, 0.4)?.max(0.), h: number(h, 1.)? };
        return Some((color, alpha.clamp(0., 1.)));
    }
    let (rgb, alpha) = parse_rgb(input)?;
    Some((culori_rgb_to_oklch(rgb), alpha.clamp(0., 1.)))
}

/// Hex and `rgb()` in 0-255 channels.
fn parse_rgb(input: &str) -> Option<(Rgb, f64)> {
    if let Some(hex) = input.strip_prefix('#') {
        let expand = |part: &str| u8::from_str_radix(&part.repeat(if part.len() == 1 { 2 } else { 1 }), 16).ok();
        let chars: Vec<String> = match hex.len() {
            3 | 4 => hex.chars().map(String::from).collect(),
            6 | 8 => hex.as_bytes().chunks(2).map(|c| String::from_utf8_lossy(c).into_owned()).collect(),
            _ => return None,
        };
        let values: Vec<u8> = chars.iter().map(|part| expand(part)).collect::<Option<_>>()?;
        let alpha = values.get(3).map_or(1., |a| f64::from(*a) / 255.);
        return Some((Rgb { r: values[0].into(), g: values[1].into(), b: values[2].into() }, alpha));
    }
    let srgb = crate::theme_color::parse_css_color(input)?;
    Some((Rgb { r: srgb.r.into(), g: srgb.g.into(), b: srgb.b.into() }, f64::from(srgb.a) / 255.))
}

/// culori's `rgb → oklch`, which canonical colours go through.
fn culori_rgb_to_oklch(color: Rgb) -> Oklch {
    let lin = |c: f64| {
        let c = c / 255.;
        let abs = c.abs();
        if abs <= 0.04045 { c / 12.92 } else { c.signum() * ((abs + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(color.r), lin(color.g), lin(color.b));
    let l = (0.412221469470763 * r + 0.5363325372617348 * g + 0.0514459932675022 * b).cbrt();
    let m = (0.2119034958178252 * r + 0.6806995506452344 * g + 0.1073969535369406 * b).cbrt();
    let s = (0.0883024591900564 * r + 0.2817188391361215 * g + 0.6299787016738222 * b).cbrt();
    let ll = 0.210454268309314 * l + 0.7936177747023054 * m - 0.0040720430116193 * s;
    let a = 1.9779985324311684 * l - 2.4285922420485799 * m + 0.450593709617411 * s;
    let bb = 0.0259040424655478 * l + 0.7827717124575296 * m - 0.8086757549230774 * s;
    let c = (a * a + bb * bb).sqrt();
    let h = if c != 0. { (bb.atan2(a).to_degrees() % 360. + 360.) % 360. } else { 0. };
    Oklch { l: ll.clamp(0., 1.), c: c.max(0.), h }
}

/// `formatThemeColorNumber`: fixed precision with trailing zeros removed.
fn format_number(value: f64, precision: usize) -> String {
    let rounded = if value.abs() < 10f64.powi(-(precision as i32)) / 2. { 0. } else { value };
    let text = format!("{rounded:.precision$}");
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_owned() } else { text }
}

/// `formatOklchThemeColor`.
fn format_oklch(color: Oklch, alpha: f64) -> String {
    let hue = if color.c < 0.0000005 { 0. } else { (color.h % 360. + 360.) % 360. };
    let body = format!("{} {} {}", format_number(color.l, 6), format_number(color.c, 6), format_number(hue, 3));
    if alpha < 1. { format!("oklch({body} / {})", format_number(alpha, 4)) } else { format!("oklch({body})") }
}

/// `toCanonicalThemeColor`.
pub fn canonical_color(value: &str) -> Option<String> {
    parse_theme_color(value).map(|(color, alpha)| format_oklch(color, alpha))
}

fn srgb_channel_to_linear(channel: f64) -> f64 {
    let c = channel / 255.;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_channel_to_srgb(channel: f64, round: bool) -> f64 {
    let c = if channel <= 0.0031308 { channel * 12.92 } else { 1.055 * channel.powf(1. / 2.4) - 0.055 };
    let srgb = c.clamp(0., 1.) * 255.;
    if round { js_round(srgb) } else { srgb }
}

/// `Math.round`: halves round up.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// `themeRgbToOklch` (the app's own matrices, not culori's).
fn rgb_to_oklch(color: Rgb) -> Oklch {
    let r = srgb_channel_to_linear(color.r);
    let g = srgb_channel_to_linear(color.g);
    let b = srgb_channel_to_linear(color.b);
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let ll = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
    let a = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
    let bb = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
    Oklch { l: ll, c: a.hypot(bb), h: bb.atan2(a).to_degrees() }
}

fn oklch_to_rgb_unclamped(color: Oklch) -> [f64; 3] {
    let hr = color.h.to_radians();
    let a = color.c * hr.cos();
    let bb = color.c * hr.sin();
    let l = (color.l + 0.3963377774 * a + 0.2158037573 * bb).powi(3);
    let m = (color.l - 0.1055613458 * a - 0.0638541728 * bb).powi(3);
    let s = (color.l - 0.0894841775 * a - 1.291485548 * bb).powi(3);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ]
}

/// `mapThemeOklchToSrgbGamut`: lowers chroma until the colour fits sRGB.
fn map_to_gamut(color: Oklch) -> Oklch {
    let in_gamut = |c: f64| oklch_to_rgb_unclamped(Oklch { c, ..color }).iter().all(|ch| (-0.0001..=1.0001).contains(ch));
    if in_gamut(color.c) {
        return color;
    }
    let (mut low, mut high) = (0., color.c);
    let resolution: f64 = 0.000001;
    let steps = ((color.c.max(resolution).log2() - resolution.log2()).ceil() as i32).max(1);
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

/// `themeOklchToRgb`.
fn oklch_to_rgb(color: Oklch) -> Rgb {
    let [r, g, b] = oklch_to_rgb_unclamped(map_to_gamut(color));
    Rgb { r: linear_channel_to_srgb(r, true), g: linear_channel_to_srgb(g, true), b: linear_channel_to_srgb(b, true) }
}

fn oklch_color(color: Oklch) -> String {
    format_oklch(map_to_gamut(color), 1.)
}

fn rgb_color(color: Rgb) -> String {
    format_oklch(rgb_to_oklch(color), 1.)
}

fn rgb_hex(color: Rgb) -> String {
    let channel = |v: f64| js_round(v.clamp(0., 255.)) as u8;
    format!("#{:02x}{:02x}{:02x}", channel(color.r), channel(color.g), channel(color.b))
}

/// `themeColorToHex`.
pub fn theme_color_to_hex(value: &str) -> Option<String> {
    let (color, alpha) = parse_theme_color(value)?;
    let opaque = rgb_hex(oklch_to_rgb(color));
    if alpha >= 1. { Some(opaque) } else { Some(format!("{opaque}{:02x}", js_round(alpha * 255.) as u8)) }
}

fn luminance(color: Rgb) -> f64 {
    let lin = |c: f64| {
        let n = c / 255.;
        if n <= 0.03928 { n / 12.92 } else { ((n + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(color.r) + 0.7152 * lin(color.g) + 0.0722 * lin(color.b)
}

fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn mix(base: Rgb, overlay: Rgb, amount: f64) -> Rgb {
    Rgb {
        r: base.r + (overlay.r - base.r) * amount,
        g: base.g + (overlay.g - base.g) * amount,
        b: base.b + (overlay.b - base.b) * amount,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Lighter,
    Darker,
}

/// `solveOklchLightness`.
fn solve_lightness(base: Oklch, against: &[Rgb], min_contrast: f64, direction: Direction) -> Oklch {
    let contrast_of = |color: Oklch| against.iter().map(|s| contrast(oklch_to_rgb(color), *s)).fold(f64::INFINITY, f64::min);
    let (mut low, mut high) = match direction {
        Direction::Lighter => (base.l, 1.),
        Direction::Darker => (0., base.l),
    };
    if contrast_of(base) >= min_contrast {
        return base;
    }
    for _ in 0..18 {
        let mid = (low + high) / 2.;
        let candidate = Oklch { l: mid, ..base };
        let ok = contrast_of(candidate) >= min_contrast;
        match (direction, ok) {
            (Direction::Lighter, true) | (Direction::Darker, false) => high = mid,
            (Direction::Lighter, false) | (Direction::Darker, true) => low = mid,
        }
    }
    Oklch { l: if direction == Direction::Lighter { high } else { low }, ..base }
}

/// `readableThemeColorOn`: moves only the lightness to reach `min_contrast`.
fn readable_color_on(color: &str, surface: &str, min_contrast: f64) -> String {
    let (Some((color_oklch, _)), Some((surface_oklch, _))) = (parse_theme_color(color), parse_theme_color(surface)) else {
        return color.to_owned();
    };
    let surface_rgb = oklch_to_rgb(surface_oklch);
    if contrast(oklch_to_rgb(color_oklch), surface_rgb) >= min_contrast {
        return color.to_owned();
    }
    let direction = if luminance(surface_rgb) < 0.179 { Direction::Lighter } else { Direction::Darker };
    rgb_hex(oklch_to_rgb(solve_lightness(color_oklch, &[surface_rgb], min_contrast, direction)))
}

fn readable_text(background: Rgb, foreground: Rgb, amount: f64, minimum: f64) -> Rgb {
    let softened = mix(foreground, background, amount);
    if contrast(softened, background) >= minimum {
        return softened;
    }
    let (mut readable, mut lower, mut upper) = (foreground, 0., amount);
    for _ in 0..12 {
        let candidate_amount = (lower + upper) / 2.;
        let candidate = mix(foreground, background, candidate_amount);
        if contrast(candidate, background) >= minimum {
            readable = candidate;
            lower = candidate_amount;
        } else {
            upper = candidate_amount;
        }
    }
    readable
}

const LIGHT: Rgb = Rgb { r: 255., g: 250., b: 255. };
const DARK: Rgb = Rgb { r: 36., g: 21., b: 35. };
const WHITE: Rgb = Rgb { r: 255., g: 255., b: 255. };
const BLACK: Rgb = Rgb { r: 0., g: 0., b: 0. };

fn readable_foreground(background: Rgb) -> Rgb {
    let (light, dark) = (contrast(background, LIGHT), contrast(background, DARK));
    if light.max(dark) >= 4.5 {
        return if light >= dark { LIGHT } else { DARK };
    }
    if contrast(background, WHITE) >= contrast(background, BLACK) { WHITE } else { BLACK }
}

fn muted_text(background: Rgb, foreground: Rgb) -> Rgb {
    let target = if luminance(background) < 0.179 { 5.082 } else { 4.705 };
    readable_text(background, foreground, 1., target)
}

fn parse_rgb_or(value: &str, fallback: Rgb) -> Rgb {
    parse_theme_color(value).map_or(fallback, |(color, _)| oklch_to_rgb(color))
}

/// Light or dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeAppearance {
    Light,
    Dark,
}

impl ThemeAppearance {
    pub fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }
}

static T3_CHAT: LazyLock<Value> = LazyLock::new(|| {
    let bytes = Assets::get("themes/t3-chat.json").expect("t3 chat theme is embedded");
    serde_json::from_slice(&bytes).expect("t3 chat theme parses")
});

fn object_colors(value: &Value) -> ThemeColors {
    value
        .as_object()
        .map(|map| map.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect())
        .unwrap_or_default()
}

/// `getDefaultThemeColors`, canonicalised as `T3_CHAT_THEME` is.
pub fn default_theme_colors(appearance: ThemeAppearance) -> ThemeColors {
    let raw = match appearance {
        ThemeAppearance::Light => &T3_CHAT["colors"],
        ThemeAppearance::Dark => &T3_CHAT["variants"]["dark"],
    };
    object_colors(raw).into_iter().map(|(role, color)| {
        let canonical = canonical_color(&color).unwrap_or(color);
        (role, canonical)
    }).collect()
}

static STANDARD: LazyLock<Value> = LazyLock::new(|| {
    let bytes = Assets::get("themes/t3-code.json").expect("t3 code theme is embedded");
    serde_json::from_slice(&bytes).expect("t3 code theme parses")
});

/// `getStandardThemeColors`: the T3 Code palette, on a black canvas in dark mode, with every role.
pub fn standard_theme_colors(appearance: ThemeAppearance) -> ThemeColors {
    let raw = match appearance {
        ThemeAppearance::Light => &STANDARD["colors"],
        ThemeAppearance::Dark => &STANDARD["variants"]["dark"],
    };
    let mut colors = default_theme_colors(appearance);
    for (role, color) in object_colors(raw) {
        let canonical = canonical_color(&color).unwrap_or(color);
        colors.insert(role, canonical);
    }
    if appearance == ThemeAppearance::Dark {
        colors.insert("canvas".into(), canonical_color("#000000").unwrap_or_default());
    }
    colors
}

/// `getStandardThemeColors` accent and input, from the bundled T3 Code theme.
fn standard_color(appearance: ThemeAppearance, role: &str) -> String {
    let raw = match appearance {
        ThemeAppearance::Light => &STANDARD["colors"][role],
        ThemeAppearance::Dark => &STANDARD["variants"]["dark"][role],
    };
    raw.as_str().and_then(canonical_color).unwrap_or_default()
}

fn standard_status_colors(canvas: Rgb) -> [(&'static str, String); 6] {
    let dark = luminance(canvas) < 0.179;
    let (error, error_fg, warning, warning_fg) = if dark {
        ("#fb414a", "#ff6467", "#fe9a00", "#ffb900")
    } else {
        ("#fb2c36", "#c10007", "#fe9a00", "#bb4d00")
    };
    let surface_mix = if dark { 0.16 } else { 0.08 };
    let surface_of = |value: &str| mix(canvas, parse_rgb_or(value, canvas), surface_mix);
    let readable_on = |foreground: &str, surface: Rgb| {
        let color = rgb_to_oklch(parse_rgb_or(foreground, canvas));
        oklch_color(solve_lightness(color, &[surface], 4.6, if dark { Direction::Lighter } else { Direction::Darker }))
    };
    let error_surface = surface_of(error);
    let warning_surface = surface_of(warning);
    [
        ("error", canonical_color(error).unwrap_or_default()),
        ("errorForeground", readable_on(error_fg, error_surface)),
        ("errorSurface", rgb_color(error_surface)),
        ("warning", canonical_color(warning).unwrap_or_default()),
        ("warningForeground", readable_on(warning_fg, warning_surface)),
        ("warningSurface", rgb_color(warning_surface)),
    ]
}

/// `updateThemeColorFamily`: sets `role` and rederives the roles the theme editor's
/// advanced mode groups with it (foregrounds, hovers, surfaces) on this palette.
pub fn update_theme_color_family(appearance: ThemeAppearance, colors: &ThemeColors, role: &str, value: &str) -> ThemeColors {
    let mut next = colors.clone();
    let Some((selected_oklch, alpha)) = parse_theme_color(value) else {
        next.insert(role.to_owned(), value.to_owned());
        return next;
    };
    let normalized = format_oklch(selected_oklch, alpha);
    let dark_fallback = if appearance == ThemeAppearance::Dark { Rgb { r: 24., g: 15., b: 27. } } else { Rgb { r: 250., g: 245., b: 250. } };
    let canvas = parse_rgb_or(colors.get("canvas").map_or("", String::as_str), dark_fallback);
    let selected = oklch_to_rgb(selected_oklch);
    let selected_on = |background: Rgb| mix(background, selected, alpha);
    let on_canvas = selected_on(canvas);
    let accent = parse_rgb_or(colors.get("accent").map_or("", String::as_str), Rgb { r: 168., g: 67., b: 112. });
    let canvas_dark = luminance(canvas) < 0.179;
    let terminal_dark = luminance(on_canvas) < 0.179;
    let foreground_on = |background: Rgb| rgb_color(readable_foreground(background));
    let tone_on = |background: Rgb| {
        let direction = if luminance(background) < 0.179 { Direction::Lighter } else { Direction::Darker };
        oklch_color(solve_lightness(selected_oklch, &[background], 4.6, direction))
    };
    let status = || {
        let surface = mix(canvas, on_canvas, if canvas_dark { 0.16 } else { 0.08 });
        (tone_on(surface), rgb_color(surface))
    };
    let all = |keys: &[&str]| keys.iter().map(|k| (k.to_string(), normalized.clone())).collect::<Vec<_>>();
    let pairs: Vec<(String, String)> = match role {
        "canvas" => all(&["canvas", "chrome", "toolbar"]),
        "text" => all(&["text", "toolbarForeground", "toolbarControlForeground"]),
        "mutedForeground" => all(&["textMuted", "mutedForeground", "placeholder", "secondaryLabel", "iconMuted", "sidebarMutedForeground"]),
        "border" => all(&["border", "toolbarBorder", "sidebarBorder"]),
        "secondary" => {
            let mut v = all(&["secondary", "muted", "toolbarControl"]);
            v.push(("secondaryForeground".into(), foreground_on(on_canvas)));
            v
        }
        "accentSurface" => {
            let mut v = all(&["accentSurface", "toolbarControlHover"]);
            v.push(("accentSurfaceForeground".into(), foreground_on(on_canvas)));
            v
        }
        "accent" => {
            let update_surface = mix(canvas, on_canvas, if canvas_dark { 0.32 } else { 0.16 });
            let mut v = all(&["accent", "focus", "update", "terminalCursor"]);
            v.push(("accentForeground".into(), foreground_on(on_canvas)));
            v.push(("updateForeground".into(), tone_on(update_surface)));
            v.push(("updateSurface".into(), rgb_color(update_surface)));
            v
        }
        "messageAction" => {
            let action_foreground = readable_foreground(on_canvas);
            let toward = if action_foreground == LIGHT || action_foreground == WHITE { BLACK } else { WHITE };
            let hover = mix(selected, toward, 0.12);
            vec![
                ("messageAction".into(), normalized.clone()),
                ("messageActionForeground".into(), rgb_color(action_foreground)),
                ("messageActionHover".into(), format_oklch(rgb_to_oklch(hover), alpha)),
            ]
        }
        "messageSurface" => vec![("messageSurface".into(), normalized.clone()), ("messageForeground".into(), foreground_on(on_canvas))],
        "searchMatchBackground" | "searchMatchActiveBackground" => {
            let foreground = role.replace("Background", "Foreground");
            vec![(role.to_owned(), rgb_color(on_canvas)), (foreground, foreground_on(on_canvas))]
        }
        "codeBackground" => vec![("codeBackground".into(), normalized.clone()), ("codeForeground".into(), foreground_on(on_canvas))],
        "sidebar" => vec![("sidebar".into(), normalized.clone()), ("sidebarForeground".into(), foreground_on(on_canvas))],
        "sidebarRowSelected" => {
            let sidebar = parse_rgb_or(colors.get("sidebar").map_or("", String::as_str), canvas);
            let on_sidebar = selected_on(sidebar);
            vec![
                ("sidebarRowHover".into(), rgb_color(mix(sidebar, on_sidebar, 0.5))),
                ("sidebarRowActive".into(), rgb_color(mix(sidebar, on_sidebar, 0.8))),
                ("sidebarRowSelected".into(), normalized.clone()),
            ]
        }
        "terminalBackground" => {
            let foreground = readable_foreground(on_canvas);
            vec![
                ("terminalBackground".into(), normalized.clone()),
                ("terminalForeground".into(), rgb_color(foreground)),
                ("terminalSelection".into(), rgb_color(mix(on_canvas, accent, if terminal_dark { 0.35 } else { 0.18 }))),
                ("terminalScrollbar".into(), rgb_color(mix(on_canvas, foreground, if terminal_dark { 0.42 } else { 0.22 }))),
                ("terminalScrollbarHover".into(), rgb_color(mix(on_canvas, foreground, if terminal_dark { 0.55 } else { 0.32 }))),
            ]
        }
        "error" | "warning" => {
            let (foreground, surface) = status();
            vec![(role.to_owned(), normalized.clone()), (format!("{role}Foreground"), foreground), (format!("{role}Surface"), surface)]
        }
        _ => vec![(role.to_owned(), normalized.clone())],
    };
    next.extend(pairs);
    next
}

/// `createVividThemeColors`: a whole palette from a canvas and an accent.
pub fn create_vivid_theme_colors(appearance: ThemeAppearance, background: &str, accent_value: &str) -> ThemeColors {
    let mut colors = default_theme_colors(appearance);
    let canvas_rgb = parse_rgb_or(
        background,
        if appearance == ThemeAppearance::Dark { Rgb { r: 24., g: 15., b: 27. } } else { Rgb { r: 250., g: 245., b: 250. } },
    );
    let accent_rgb = parse_rgb_or(accent_value, Rgb { r: 168., g: 67., b: 112. });
    let canvas = rgb_to_oklch(canvas_rgb);
    let accent = rgb_to_oklch(accent_rgb);
    let dark = luminance(canvas_rgb) < 0.179;
    let hue = if accent.c < 0.02 { canvas.h } else { accent.h };
    let tint_c = (accent.c * 0.22).clamp(0.008, 0.045);
    let step = if dark { 1. } else { -1. };
    let surface_at = |delta: f64, chroma: f64| Oklch { l: (canvas.l + step * delta).clamp(0.05, 0.98), c: chroma, h: hue };
    let pick = |d: f64, l: f64| if dark { d } else { l };
    let text_base = Oklch { l: pick(0.95, 0.2), c: (accent.c * 0.25).min(0.035), h: hue };
    let direction = if dark { Direction::Lighter } else { Direction::Darker };
    let text = solve_lightness(text_base, &[canvas_rgb], 7., direction);
    let text_rgb = oklch_to_rgb(text);
    let text_muted_rgb = muted_text(canvas_rgb, text_rgb);
    let action = Oklch {
        l: (accent.l + pick(0.06, -0.02)).clamp(0.35, 0.85),
        c: (accent.c * 0.9).max(0.06),
        h: (hue + 50.) % 360.,
    };
    let action_rgb = oklch_to_rgb(action);
    let action_foreground = readable_foreground(action_rgb);
    let accent_foreground = readable_foreground(accent_rgb);
    let sidebar = surface_at(0.045, tint_c * 1.4);
    let sidebar_rgb = oklch_to_rgb(sidebar);
    let surface_raised = surface_at(0.05, tint_c);
    let surface_raised_rgb = oklch_to_rgb(surface_raised);
    let secondary = surface_at(pick(0.1, 0.06), (accent.c * 0.5).min(0.09));
    let muted = surface_at(pick(0.06, 0.04), (accent.c * 0.35).min(0.06));
    let accent_surface = surface_at(pick(0.13, 0.08), (accent.c * 0.55).min(0.11));
    let message_surface = surface_at(pick(0.16, 0.1), (accent.c * 0.6).min(0.13));
    let update_surface = surface_at(pick(0.14, 0.09), (accent.c * 0.55).min(0.12));
    let foreground_on = |surface: Rgb| oklch_color(solve_lightness(text_base, &[surface], 4.6, direction));
    let search_match = oklch_to_rgb(surface_at(0.22, accent.c.min(0.12)));
    let search_match_active = oklch_to_rgb(surface_at(0.4, accent.c.min(0.18)));
    let action_hover = Oklch { l: action.l + pick(0.06, -0.06), ..action };

    let mut set = |role: &str, value: String| {
        colors.insert(role.to_owned(), value);
    };
    for (role, value) in standard_status_colors(canvas_rgb) {
        set(role, value);
    }
    set("canvas", rgb_color(canvas_rgb));
    set("chrome", rgb_color(canvas_rgb));
    set("toolbar", rgb_color(canvas_rgb));
    set("toolbarForeground", rgb_color(text_rgb));
    set("toolbarBorder", oklch_color(surface_at(pick(0.14, 0.1), (accent.c * 0.4).min(0.08))));
    set("toolbarControl", oklch_color(surface_at(pick(0.09, 0.05), tint_c * 1.3)));
    set("toolbarControlForeground", rgb_color(text_rgb));
    set("toolbarControlHover", oklch_color(surface_at(pick(0.14, 0.09), tint_c * 1.6)));
    set("surface", oklch_color(surface_at(0.015, tint_c)));
    set("surfaceRaised", oklch_color(surface_raised));
    set("surfaceOverlay", oklch_color(surface_at(0.075, tint_c)));
    set("text", rgb_color(text_rgb));
    set("textMuted", rgb_color(text_muted_rgb));
    set("border", oklch_color(surface_at(pick(0.16, 0.12), (accent.c * 0.35).min(0.07))));
    set("input", oklch_color(surface_at(pick(0.21, 0.16), (accent.c * 0.4).min(0.08))));
    set("focus", rgb_color(accent_rgb));
    set("accent", rgb_color(accent_rgb));
    set("accentForeground", rgb_color(accent_foreground));
    set("secondary", oklch_color(secondary));
    set("secondaryForeground", foreground_on(oklch_to_rgb(secondary)));
    set("muted", oklch_color(muted));
    set("mutedForeground", rgb_color(readable_text(oklch_to_rgb(muted), text_rgb, 1., 4.6)));
    set("placeholder", rgb_color(readable_text(surface_raised_rgb, text_rgb, 1., 4.6)));
    set("secondaryLabel", rgb_color(text_muted_rgb));
    set("iconMuted", rgb_color(text_muted_rgb));
    set("update", rgb_color(accent_rgb));
    set("updateForeground", foreground_on(oklch_to_rgb(update_surface)));
    set("updateSurface", oklch_color(update_surface));
    set("accentSurface", oklch_color(accent_surface));
    set("accentSurfaceForeground", foreground_on(oklch_to_rgb(accent_surface)));
    set("messageSurface", oklch_color(message_surface));
    set("messageForeground", foreground_on(oklch_to_rgb(message_surface)));
    set("messageAction", rgb_color(action_rgb));
    set("messageActionForeground", rgb_color(action_foreground));
    set("messageActionHover", oklch_color(action_hover));
    set("codeBackground", oklch_color(surface_at(0.035, tint_c * 0.8)));
    set("codeForeground", rgb_color(text_rgb));
    set("searchMatchBackground", rgb_color(search_match));
    set("searchMatchForeground", rgb_color(readable_foreground(search_match)));
    set("searchMatchActiveBackground", rgb_color(search_match_active));
    set("searchMatchActiveForeground", rgb_color(readable_foreground(search_match_active)));
    set("sidebar", oklch_color(sidebar));
    set("sidebarForeground", foreground_on(sidebar_rgb));
    set("sidebarMutedForeground", rgb_color(muted_text(sidebar_rgb, text_rgb)));
    set("sidebarControlSurface", oklch_color(surface_at(pick(0.1, 0.07), tint_c * 1.5)));
    set("sidebarRowHover", oklch_color(surface_at(pick(0.08, 0.06), (accent.c * 0.45).min(0.08))));
    set("sidebarRowActive", oklch_color(surface_at(pick(0.12, 0.09), (accent.c * 0.55).min(0.1))));
    set("sidebarRowSelected", oklch_color(surface_at(pick(0.14, 0.1), (accent.c * 0.6).min(0.11))));
    set("sidebarBorder", oklch_color(surface_at(pick(0.17, 0.12), (accent.c * 0.4).min(0.08))));
    set("terminalBackground", rgb_color(canvas_rgb));
    set("terminalForeground", rgb_color(text_rgb));
    set("terminalCursor", rgb_color(accent_rgb));
    set("terminalSelection", oklch_color(surface_at(pick(0.18, 0.12), (accent.c * 0.55).min(0.12))));
    set("terminalScrollbar", oklch_color(surface_at(pick(0.22, 0.16), tint_c)));
    set("terminalScrollbarHover", oklch_color(surface_at(pick(0.3, 0.22), tint_c)));
    colors
}

/// `themeIdFromName`.
pub fn theme_id_from_name(name: &str) -> String {
    let label = name.trim().to_lowercase();
    let mut normalized = String::new();
    for c in label.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            normalized.push(c);
        } else if !normalized.ends_with('-') {
            normalized.push('-');
        }
    }
    let normalized: String = normalized.trim_matches('-').chars().take(48).collect();
    if !normalized.is_empty() {
        return normalized;
    }
    if label.is_empty() {
        return "custom-theme".into();
    }
    // FNV-1a over UTF-16 code units, as the web hashes `charCodeAt`.
    let mut hash: u32 = 2_166_136_261;
    for unit in label.encode_utf16() {
        hash ^= u32::from(unit);
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("theme-{}", radix36(hash))
}

fn radix36(mut value: u32) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// A theme as `parseThemeFile` returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedTheme {
    pub id: String,
    pub label: String,
    pub appearance: ThemeAppearance,
    pub colors: ThemeColors,
    pub variants: BTreeMap<&'static str, ThemeColors>,
}

impl ParsedTheme {
    /// The T3 theme file the web would save (`serializeThemeFile`).
    pub fn to_file(&self) -> Value {
        let mut file = Map::new();
        file.insert("version".into(), THEME_FILE_VERSION.into());
        file.insert("id".into(), self.id.clone().into());
        file.insert("name".into(), self.label.clone().into());
        file.insert("appearance".into(), self.appearance.name().into());
        file.insert("colors".into(), serde_json::to_value(&self.colors).expect("colors encode"));
        if !self.variants.is_empty() {
            file.insert("variants".into(), serde_json::to_value(&self.variants).expect("variants encode"));
        }
        Value::Object(file)
    }

    /// Which halves it has.
    pub fn modes(&self) -> usize {
        1 + self.variants.len()
    }
}

fn parse_overrides(value: &Value) -> Result<ThemeColors, ImportError> {
    let Some(map) = value.as_object() else { return fail("Theme colors must be objects.") };
    let roles = default_theme_colors(ThemeAppearance::Light);
    let mut overrides = ThemeColors::new();
    for (role, color) in map {
        if !roles.contains_key(role) {
            return Err(ImportError(format!("\"{role}\" is not a supported theme color role.")));
        }
        let Some(normalized) = color.as_str().and_then(canonical_color) else {
            return Err(ImportError(format!(
                "The color for \"{role}\" must be a literal CSS color such as oklch(0.62 0.2 280)."
            )));
        };
        overrides.insert(role.clone(), normalized);
    }
    Ok(overrides)
}

/// `parseThemeFile`.
pub fn parse_theme_file(value: &Value) -> Result<ParsedTheme, ImportError> {
    let Some(object) = value.as_object() else { return fail("Theme files must contain a JSON object.") };
    if object.get("version").and_then(Value::as_u64) != Some(THEME_FILE_VERSION) {
        return fail("This theme file uses an unsupported version. Expected 1.");
    }
    let Some(name) = object.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty() && n.trim().chars().count() <= 48)
    else {
        return fail("Theme files need a name (48 characters or fewer).");
    };
    let Some(appearance) = object.get("appearance").and_then(Value::as_str).and_then(ThemeAppearance::parse) else {
        return fail("Theme files need an appearance of \"light\" or \"dark\".");
    };
    let Some(raw_colors) = object.get("colors").filter(|c| c.is_object()) else {
        return fail("Theme files need a colors object.");
    };
    let id = match object.get("id") {
        None => theme_id_from_name(name),
        Some(id) => id.as_str().unwrap_or_default().to_owned(),
    };
    let valid_id = !id.is_empty()
        && id.len() <= 48
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_id {
        return fail("Theme ids may only contain lowercase letters, numbers, and hyphens.");
    }
    if RESERVED_THEME_IDS.contains(&id.as_str()) {
        return Err(ImportError(format!("The theme id \"{id}\" is reserved.")));
    }
    let overrides = parse_overrides(raw_colors)?;
    let mut colors = match (object.get("canvas").and_then(Value::as_str), object.get("accent").and_then(Value::as_str)) {
        (Some(canvas), Some(accent)) => create_vivid_theme_colors(appearance, canvas, accent),
        _ => default_theme_colors(appearance),
    };
    colors.extend(overrides);
    let mut variants = BTreeMap::new();
    if let Some(raw) = object.get("variants") {
        let Some(raw) = raw.as_object() else { return fail("Theme variants must be an object.") };
        for (name, variant_colors) in raw {
            let Some(variant) = ThemeAppearance::parse(name) else {
                return fail("Theme variants may only be named \"light\" or \"dark\".");
            };
            if variant == appearance {
                return Err(ImportError(format!(
                    "Theme variants must not repeat the base appearance \"{}\".",
                    appearance.name()
                )));
            }
            let mut filled = default_theme_colors(variant);
            filled.extend(parse_overrides(variant_colors)?);
            variants.insert(variant.name(), filled);
        }
    }
    Ok(ParsedTheme { id, label: name.trim().to_owned(), appearance, colors, variants })
}

/// Strips `//` and `/* */` comments and trailing commas: the JSONC VS Code
/// theme files use (`jsonc-parser` with `allowTrailingComma`).
pub fn strip_jsonc(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                while chars.next_if(|&n| n != '\n').is_some() {}
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = ' ';
                for n in chars.by_ref() {
                    if previous == '*' && n == '/' {
                        break;
                    }
                    previous = n;
                }
            }
            ',' => {
                let rest: String = chars.clone().collect();
                let next = rest.trim_start().chars().next();
                if !matches!(next, Some('}') | Some(']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// `isVsCodeThemeFile`.
pub fn is_vscode_theme_file(value: &Value) -> bool {
    let Some(object) = value.as_object() else { return false };
    if object.get("version").and_then(Value::as_u64) == Some(THEME_FILE_VERSION) {
        return false;
    }
    let workbench = object.get("colors").and_then(Value::as_object).is_some_and(|c| c.keys().any(|k| k.contains('.')));
    workbench || object.get("tokenColors").is_some_and(Value::is_array)
}

/// `parseThemeImport`: a T3 theme file or a VS Code theme, as JSONC.
pub fn parse_theme_import(source: &str) -> Result<ParsedTheme, ImportError> {
    let Ok(value) = serde_json::from_str::<Value>(&strip_jsonc(source)) else {
        return fail("That theme file is not valid JSON.");
    };
    if is_vscode_theme_file(&value) { parse_vscode_theme_file(&value) } else { parse_theme_file(&value) }
}

/// `humanizeThemeName`.
pub fn humanize_theme_name(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().any(char::is_whitespace) || !trimmed.contains(['-', '_', '.']) {
        return trimmed.to_owned();
    }
    trimmed
        .split(['-', '_', '.'])
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A VS Code colour: hex or CSS `color(display-p3|srgb …)`, 0-255 channels.
#[derive(Clone, Copy)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

impl Rgba {
    fn rgb(self) -> Rgb {
        Rgb { r: self.r, g: self.g, b: self.b }
    }
}

fn parse_vscode_color(value: Option<&Value>) -> Option<Rgba> {
    let trimmed = value?.as_str()?.trim();
    if trimmed.starts_with("color(") {
        return parse_color_function(trimmed);
    }
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if !matches!(hex.len(), 3 | 4 | 6 | 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let expand = |part: &str| f64::from(u8::from_str_radix(&part.repeat(if part.len() == 1 { 2 } else { 1 }), 16).unwrap_or(0));
    if hex.len() <= 4 {
        return Some(Rgba {
            r: expand(&hex[0..1]),
            g: expand(&hex[1..2]),
            b: expand(&hex[2..3]),
            a: if hex.len() == 4 { expand(&hex[3..4]) / 255. } else { 1. },
        });
    }
    Some(Rgba {
        r: expand(&hex[0..2]),
        g: expand(&hex[2..4]),
        b: expand(&hex[4..6]),
        a: if hex.len() == 8 { expand(&hex[6..8]) / 255. } else { 1. },
    })
}

fn parse_color_function(value: &str) -> Option<Rgba> {
    let body = value.strip_prefix("color(")?.strip_suffix(')')?.trim();
    let (space, rest) = body.split_once(char::is_whitespace)?;
    let space = space.to_ascii_lowercase();
    if space != "display-p3" && space != "srgb" {
        return None;
    }
    let (channels, alpha) = match rest.split_once('/') {
        Some((c, a)) => (c, Some(a.trim())),
        None => (rest, None),
    };
    let number = |part: &str| match part.strip_suffix('%') {
        Some(p) => p.parse::<f64>().ok().map(|p| p / 100.),
        None => part.parse::<f64>().ok(),
    };
    let channels: Vec<f64> = channels.split_whitespace().map(number).collect::<Option<_>>()?;
    let [red, green, blue] = channels.as_slice() else { return None };
    let alpha = match alpha {
        Some(a) => number(a)?,
        None => 1.,
    }
    .clamp(0., 1.);
    if space == "srgb" {
        return Some(Rgba { r: red * 255., g: green * 255., b: blue * 255., a: alpha });
    }
    let decode = |v: f64| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let encode = |v: f64| {
        let c = v.clamp(0., 1.);
        if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1. / 2.4) - 0.055 }
    };
    let (lr, lg, lb) = (decode(*red), decode(*green), decode(*blue));
    Some(Rgba {
        r: encode(1.2249401762805 * lr - 0.2249401762805 * lg) * 255.,
        g: encode(-0.042056961239 * lr + 1.042056961239 * lg) * 255.,
        b: encode(-0.0196375547643 * lr - 0.0786360655012 * lg + 1.0982736202656 * lb) * 255.,
        a: alpha,
    })
}

fn flatten_over(color: Rgba, base: Rgb) -> String {
    if color.a >= 1. {
        return rgb_hex(color.rgb());
    }
    rgb_hex(Rgb {
        r: color.r * color.a + base.r * (1. - color.a),
        g: color.g * color.a + base.g * (1. - color.a),
        b: color.b * color.a + base.b * (1. - color.a),
    })
}

fn hex_to_rgb(value: &str) -> Rgb {
    let hex = theme_color_to_hex(value).unwrap_or_else(|| value.to_owned());
    parse_vscode_color(Some(&Value::String(hex))).map_or(Rgb { r: 0., g: 0., b: 0. }, Rgba::rgb)
}

/// `parseVsCodeThemeFile`: builds a palette from a VS Code colour theme.
pub fn parse_vscode_theme_file(value: &Value) -> Result<ParsedTheme, ImportError> {
    let Some(object) = value.as_object() else { return fail("Theme files must contain a JSON object.") };
    let empty = Map::new();
    let colors = object.get("colors").and_then(Value::as_object).unwrap_or(&empty);
    let pick = |keys: &[&str]| keys.iter().find_map(|key| parse_vscode_color(colors.get(*key)));
    let solid_over = |base: Rgb, keys: &[&str]| pick(keys).map(|c| flatten_over(c, base));

    let Some(canvas_color) = pick(&["editor.background", "editorPane.background"]) else {
        return fail("That VS Code theme has no \"editor.background\" color, so there is nothing to build a palette from.");
    };
    let canvas = canvas_color.rgb();
    let kind = object.get("type").and_then(Value::as_str).map(str::to_lowercase);
    let appearance = match kind.as_deref() {
        Some("light" | "hc-light") => ThemeAppearance::Light,
        Some("dark" | "hc-black") => ThemeAppearance::Dark,
        _ if luminance(canvas) < 0.179 => ThemeAppearance::Dark,
        _ => ThemeAppearance::Light,
    };
    let canvas_hex = rgb_hex(canvas);
    let raised_candidate = solid_over(canvas, &["editorWidget.background", "dropdown.background"]);
    let stands_apart = |a: &str, b: &str| contrast(hex_to_rgb(a), hex_to_rgb(b)) >= 1.1;

    let mut accent: Option<(Rgba, String)> = None;
    for key in ["focusBorder", "button.background", "textLink.foreground", "activityBarBadge.background", "progressBar.background", "badge.background"] {
        let Some(candidate) = parse_vscode_color(colors.get(key)) else { continue };
        let candidate_hex = flatten_over(candidate, canvas);
        if !stands_apart(&candidate_hex, &canvas_hex) || raised_candidate.as_ref().is_some_and(|r| !stands_apart(&candidate_hex, r)) {
            continue;
        }
        accent = Some((candidate, candidate_hex));
        break;
    }
    let (accent_color, accent_hex) = match accent {
        Some(found) => found,
        None => {
            let standard = theme_color_to_hex(&standard_color(appearance, "accent")).unwrap_or_default();
            let hex = [standard.clone(), "#ffffff".into(), "#000000".into()]
                .into_iter()
                .find(|c| stands_apart(c, &canvas_hex) && raised_candidate.as_ref().is_none_or(|r| stands_apart(c, r)))
                .unwrap_or(standard);
            let color = parse_vscode_color(Some(&Value::String(hex.clone()))).expect("hex parses");
            (color, hex)
        }
    };
    let muted_accent_hex = flatten_over(Rgba { a: 0.2, ..accent_color }, canvas);
    let derived = create_vivid_theme_colors(appearance, &canvas_hex, &muted_accent_hex);
    let d = |role: &str| derived.get(role).cloned().unwrap_or_default();
    let sidebar_hex = solid_over(canvas, &["sideBar.background", "activityBar.background"]).unwrap_or_else(|| d("sidebar"));
    let sidebar = hex_to_rgb(&sidebar_hex);
    let terminal_hex = solid_over(canvas, &["terminal.background", "panel.background"]).unwrap_or_else(|| d("terminalBackground"));
    let terminal = hex_to_rgb(&terminal_hex);

    let readable_on = |surface: &str, fallback: String, keys: &[&str]| -> String {
        let surface_rgb = hex_to_rgb(surface);
        let readable = |candidate: &str| contrast(hex_to_rgb(candidate), surface_rgb) >= 4.5;
        if let Some(specified) = solid_over(surface_rgb, keys).filter(|s| readable(s)) {
            return specified;
        }
        if readable(&fallback) {
            return fallback;
        }
        if luminance(surface_rgb) < 0.179 { "#ffffff".into() } else { "#000000".into() }
    };
    let readable_status = |fallback: String, keys: &[&str]| -> String {
        let specified = solid_over(canvas, keys);
        let base = specified.filter(|s| stands_apart(s, &canvas_hex)).unwrap_or(fallback);
        readable_color_on(&base, &canvas_hex, 4.5)
    };

    let surface_raised_hex = raised_candidate.clone().unwrap_or_else(|| d("surfaceRaised"));
    let button_hex = solid_over(canvas, &["button.background"]);
    let action_hex = button_hex
        .filter(|b| stands_apart(b, &canvas_hex) && stands_apart(b, &surface_raised_hex))
        .unwrap_or_else(|| accent_hex.clone());
    let input_candidates =
        [d("input"), d("surfaceRaised"), standard_color(appearance, "input"), "#000000".into(), "#ffffff".into(), "#808080".into()];
    let mut input_hex = input_candidates
        .into_iter()
        .find(|c| stands_apart(c, &canvas_hex) && stands_apart(c, &action_hex))
        .unwrap_or_else(|| "#808080".into());
    for key in ["input.background", "input.border"] {
        if let Some(candidate) = solid_over(canvas, &[key]).filter(|c| stands_apart(c, &canvas_hex) && stands_apart(c, &action_hex)) {
            input_hex = candidate;
            break;
        }
    }
    let search_bg = solid_over(canvas, &["editor.findMatchHighlightBackground"]).unwrap_or_else(|| d("searchMatchBackground"));
    let search_active_bg = solid_over(canvas, &["editor.findMatchBackground"]).unwrap_or_else(|| d("searchMatchActiveBackground"));

    let mut overrides: Vec<(&str, String)> = vec![
        ("canvas", canvas_hex.clone()),
        ("text", readable_on(&canvas_hex, d("text"), &["editor.foreground", "foreground"])),
        ("textMuted", readable_on(&canvas_hex, d("textMuted"), &["descriptionForeground", "disabledForeground"])),
        ("surface", solid_over(canvas, &["editorWidget.background"]).unwrap_or_else(|| d("surface"))),
        ("surfaceRaised", surface_raised_hex.clone()),
        (
            "surfaceOverlay",
            solid_over(canvas, &["menu.background", "quickInput.background", "dropdown.background"]).unwrap_or_else(|| d("surfaceOverlay")),
        ),
        ("border", solid_over(canvas, &["panel.border", "editorGroup.border", "contrastBorder"]).unwrap_or_else(|| d("border"))),
        ("input", input_hex),
        ("placeholder", readable_on(&surface_raised_hex, d("placeholder"), &["input.placeholderForeground"])),
        ("error", readable_status(d("error"), &["editorError.foreground", "errorForeground"])),
        ("warning", readable_status(d("warning"), &["editorWarning.foreground"])),
        (
            "accentSurface",
            solid_over(canvas, &["list.activeSelectionBackground", "list.hoverBackground"]).unwrap_or_else(|| d("accentSurface")),
        ),
        ("codeBackground", solid_over(canvas, &["textCodeBlock.background"]).unwrap_or_else(|| d("codeBackground"))),
        ("searchMatchBackground", search_bg.clone()),
        (
            "searchMatchForeground",
            readable_on(&search_bg, d("searchMatchForeground"), &["editor.findMatchHighlightForeground", "editor.foreground", "foreground"]),
        ),
        ("searchMatchActiveBackground", search_active_bg.clone()),
        (
            "searchMatchActiveForeground",
            readable_on(&search_active_bg, d("searchMatchActiveForeground"), &["editor.findMatchForeground", "editor.foreground", "foreground"]),
        ),
        ("sidebar", sidebar_hex.clone()),
        ("sidebarForeground", readable_on(&sidebar_hex, d("sidebarForeground"), &["sideBar.foreground"])),
        ("sidebarBorder", solid_over(sidebar, &["sideBar.border"]).unwrap_or_else(|| d("sidebarBorder"))),
        ("sidebarRowHover", solid_over(sidebar, &["list.hoverBackground"]).unwrap_or_else(|| d("sidebarRowHover"))),
        (
            "sidebarRowActive",
            solid_over(sidebar, &["list.inactiveSelectionBackground", "list.hoverBackground"]).unwrap_or_else(|| d("sidebarRowActive")),
        ),
        ("sidebarRowSelected", solid_over(sidebar, &["list.activeSelectionBackground"]).unwrap_or_else(|| d("sidebarRowSelected"))),
        ("terminalBackground", terminal_hex.clone()),
        ("terminalForeground", readable_on(&terminal_hex, d("terminalForeground"), &["terminal.foreground"])),
        (
            "terminalCursor",
            solid_over(terminal, &["terminalCursor.foreground", "editorCursor.foreground"]).unwrap_or_else(|| d("terminalCursor")),
        ),
        (
            "terminalSelection",
            solid_over(terminal, &["terminal.selectionBackground", "editor.selectionBackground"]).unwrap_or_else(|| d("terminalSelection")),
        ),
        ("terminalScrollbar", solid_over(terminal, &["scrollbarSlider.background"]).unwrap_or_else(|| d("terminalScrollbar"))),
    ];
    overrides.push(("accent", accent_hex.clone()));
    overrides.push(("focus", accent_hex.clone()));
    overrides.push(("messageAction", action_hex.clone()));
    overrides.push(("messageActionForeground", readable_on(&action_hex, d("messageActionForeground"), &["button.foreground"])));
    overrides.push(("accentForeground", readable_on(&accent_hex, d("accentForeground"), &["button.foreground"])));

    let name = ["displayName", "name"]
        .iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_str))
        .map(humanize_theme_name)
        .find(|name| !name.is_empty())
        .map(|name| name.chars().take(48).collect::<String>())
        .unwrap_or_else(|| "VS Code theme".into());
    let generated = theme_id_from_name(&name);
    let mut merged = Map::new();
    for (role, color) in derived {
        merged.insert(role, Value::String(color));
    }
    for (role, color) in overrides {
        merged.insert(role.to_owned(), Value::String(color));
    }
    let mut file = Map::new();
    file.insert("version".into(), THEME_FILE_VERSION.into());
    if RESERVED_THEME_IDS.contains(&generated.as_str()) {
        file.insert("id".into(), format!("{generated}-vscode").into());
    }
    file.insert("name".into(), name.into());
    file.insert("appearance".into(), appearance.name().into());
    file.insert("colors".into(), Value::Object(merged));
    parse_theme_file(&Value::Object(file))
}

/// `pairVsCodeThemes`: a "Foo Light" and a "Foo Dark" become one "Foo"
/// theme with both halves.
pub fn pair_vscode_themes(themes: Vec<ParsedTheme>) -> Vec<ParsedTheme> {
    let strip = |label: &str| {
        let words: Vec<&str> =
            label.split_whitespace().filter(|w| !w.eq_ignore_ascii_case("light") && !w.eq_ignore_ascii_case("dark")).collect();
        words.join(" ")
    };
    let mut groups: Vec<(String, Vec<ParsedTheme>, Vec<ParsedTheme>, usize)> = Vec::new();
    let mut out: Vec<(usize, ParsedTheme)> = Vec::new();
    for (order, theme) in themes.into_iter().enumerate() {
        let key = strip(&theme.label);
        if theme.modes() != 1 || key == theme.label || !key.chars().any(|c| c.is_ascii_alphanumeric()) {
            out.push((order, theme));
            continue;
        }
        let index = match groups.iter().position(|g| g.0 == key) {
            Some(index) => index,
            None => {
                groups.push((key, Vec::new(), Vec::new(), order));
                groups.len() - 1
            }
        };
        match theme.appearance {
            ThemeAppearance::Light => groups[index].1.push(theme),
            ThemeAppearance::Dark => groups[index].2.push(theme),
        }
    }
    for (key, light, dark, order) in groups {
        if let ([light], [dark]) = (light.as_slice(), dark.as_slice()) {
            let mut file = Map::new();
            file.insert("version".into(), THEME_FILE_VERSION.into());
            file.insert("name".into(), key.clone().into());
            file.insert("appearance".into(), "light".into());
            file.insert("colors".into(), serde_json::to_value(&light.colors).expect("encode"));
            file.insert("variants".into(), serde_json::json!({ "dark": dark.colors }));
            if let Ok(paired) = parse_theme_file(&Value::Object(file)) {
                out.push((order, paired));
                continue;
            }
        }
        for theme in light.into_iter().chain(dark) {
            out.push((order, theme));
        }
    }
    out.sort_by_key(|(order, _)| *order);
    out.into_iter().map(|(_, theme)| theme).collect()
}
