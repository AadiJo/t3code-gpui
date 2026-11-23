//! Every colour the diff surface draws, derived the way the web derives them:
//! Pierre's defaults (`@pierre/diffs` style.css) overridden by T3's
//! `DIFF_SURFACE_THEME_UNSAFE_CSS` and `DIFF_VIEW_UNSAFE_CSS`. All tints mix
//! from the code surface, which in the diff panel is the canvas colour.

use gpui_kit::{Hsla, Rgba};

use crate::{
    diff_panel_store::DiffColorScheme,
    syntax::{SyntaxPalette, hex},
    theme_palette::{AppTheme, Appearance},
};

/// Resolved diff surface colours for one theme, appearance and scheme.
#[derive(Clone, Copy, Debug)]
pub struct DiffColors {
    pub appearance: Appearance,
    pub background: Hsla,
    /// Plain code text: the syntax theme's foreground.
    pub foreground: Hsla,
    pub code_foreground: Hsla,
    pub number: Hsla,
    pub context_hover: Hsla,
    pub separator: Hsla,
    pub separator_text: Hsla,
    pub separator_hover: Hsla,
    pub buffer: Hsla,
    pub addition_base: Hsla,
    pub deletion_base: Hsla,
    pub modified_base: Hsla,
    pub addition_line: Hsla,
    pub addition_number: Hsla,
    pub deletion_line: Hsla,
    pub deletion_number: Hsla,
    pub selected_line: Hsla,
    pub selected_number: Hsla,
    pub text_selection: Hsla,
    pub search_match: Hsla,
    pub search_match_current: Hsla,
    pub header_hover_edge: Hsla,
    pub title_hover: Hsla,
    pub stat_addition: Hsla,
    pub stat_deletion: Hsla,
    pub icon_new: Hsla,
    pub icon_deleted: Hsla,
    pub icon_modified: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub primary: Hsla,
}

impl DiffColors {
    /// Colours for the app theme. `code_background` is the surface the diff sits on.
    pub fn new(theme: &AppTheme, scheme: DiffColorScheme) -> Self {
        let appearance = theme.appearance;
        let dark = appearance == Appearance::Dark;
        let colors = &theme.colors;
        let syntax = SyntaxPalette::for_appearance(appearance);
        // The panel sets `--code-background: var(--background)` on the surface.
        let bg = colors.canvas;
        let code_fg = colors.code_foreground;
        let fg = syntax.foreground;
        let pick = |dark_hex: &str, light_hex: &str| hex(if dark { dark_hex } else { light_hex }).unwrap_or_default();

        let (diff_addition, diff_deletion, stat_addition, stat_deletion, addition_base, deletion_base) = match scheme {
            DiffColorScheme::RedGreen => (
                // --success and --destructive.
                pick("#00b981", "#00b981"),
                colors.error,
                pick("#00d294", "#007857"),
                pick("#ff6568", "#bf000f"),
                // Pierre's own markers: --diffs-added-* and --diffs-deleted-*.
                pick("#5ecc71", "#0dbe4e"),
                pick("#ff6762", "#ff2e3f"),
            ),
            DiffColorScheme::BlueOrange => {
                let addition = pick("#51a2ff", "#155dfc");
                let deletion = pick("#ff8904", "#f54900");
                (addition, deletion, addition, deletion, addition, deletion)
            }
        };
        let modified_base = pick("#69b1ff", "#009fff");
        let info_foreground = pick("#56a2ff", "#1447e6");
        let icon_modified = match (scheme, dark) {
            (DiffColorScheme::RedGreen, true) => info_foreground.opacity(0.65),
            (DiffColorScheme::RedGreen, false) => mix_srgb(info_foreground, colors.muted_foreground, 0.5),
            (DiffColorScheme::BlueOrange, true) => colors.muted_foreground.opacity(0.65),
            (DiffColorScheme::BlueOrange, false) => colors.muted_foreground,
        };
        let (line_mix, number_mix) = if dark { (0.70, 0.60) } else { (0.50, 0.35) };
        let selected_inner = mix_srgb(bg, modified_base, if dark { 0.70 } else { 0.50 });
        let selected_number_inner = mix_srgb(bg, modified_base, if dark { 0.60 } else { 0.35 });

        Self {
            appearance,
            background: bg,
            foreground: fg,
            code_foreground: code_fg,
            number: mix_lab(fg, bg, 0.65),
            context_hover: mix_srgb(bg, code_fg, 0.94),
            separator: mix_srgb(bg, code_fg, 0.97),
            separator_text: mix_srgb(code_fg, bg, 0.50),
            separator_hover: mix_srgb(bg, code_fg, 0.92),
            buffer: mix_srgb(bg, code_fg, 0.90),
            addition_base,
            deletion_base,
            modified_base,
            addition_line: mix_srgb(bg, diff_addition, line_mix),
            addition_number: mix_srgb(bg, diff_addition, number_mix),
            deletion_line: mix_srgb(bg, diff_deletion, line_mix),
            deletion_number: mix_srgb(bg, diff_deletion, number_mix),
            selected_line: mix_lab(bg, selected_inner, if dark { 0.80 } else { 0.88 }),
            selected_number: mix_lab(bg, selected_number_inner, if dark { 0.85 } else { 0.91 }),
            text_selection: modified_base.opacity(0.3),
            search_match: if dark { hex("#ff963266") } else { hex("#ff963288") }.unwrap_or_default(),
            search_match_current: modified_base.opacity(0.45),
            header_hover_edge: code_fg.opacity(0.24),
            title_hover: mix_srgb(code_fg, colors.accent, 0.84),
            stat_addition,
            stat_deletion,
            icon_new: if dark { stat_addition } else { stat_addition },
            icon_deleted: if dark { deletion_base } else { stat_deletion },
            icon_modified,
            muted: colors.muted_foreground,
            border: colors.border,
            primary: colors.accent,
        }
    }

    /// Background and gutter tints for a line kind.
    pub fn line_backgrounds(&self, kind: super::layout::LineKind) -> Option<(Hsla, Hsla, Hsla)> {
        use super::layout::LineKind;
        match kind {
            LineKind::Context => None,
            LineKind::Addition => Some((self.addition_line, self.addition_number, self.addition_base)),
            LineKind::Deletion => Some((self.deletion_line, self.deletion_number, self.deletion_base)),
        }
    }
}

/// `color-mix(in srgb, a weight, b)`.
pub fn mix_srgb(a: Hsla, b: Hsla, weight: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    let w = weight.clamp(0., 1.);
    // CSS premultiplies by alpha before mixing.
    let alpha = a.a * w + b.a * (1. - w);
    let channel = |x: f32, y: f32| {
        if alpha == 0. { 0. } else { (x * a.a * w + y * b.a * (1. - w)) / alpha }
    };
    Rgba { r: channel(a.r, b.r), g: channel(a.g, b.g), b: channel(a.b, b.b), a: alpha }.into()
}

/// `color-mix(in lab, a weight, b)` for opaque colours.
pub fn mix_lab(a: Hsla, b: Hsla, weight: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    let (la, lb) = (to_lab(a), to_lab(b));
    let w = weight.clamp(0., 1.);
    let mixed = [0, 1, 2].map(|i| la[i] * w + lb[i] * (1. - w));
    let mut out = from_lab(mixed);
    out.a = a.a * w + b.a * (1. - w);
    out.into()
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0., 1.);
    if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1. / 2.4) - 0.055 }
}

// CIE Lab with a D50 white, as CSS uses, through Bradford-adapted XYZ.
fn to_lab(color: Rgba) -> [f32; 3] {
    let [r, g, b] = [color.r, color.g, color.b].map(srgb_to_linear);
    let x = 0.4360747 * r + 0.3850649 * g + 0.1430804 * b;
    let y = 0.2225045 * r + 0.7168786 * g + 0.0606169 * b;
    let z = 0.0139322 * r + 0.0971045 * g + 0.7141733 * b;
    let white = [0.96422, 1.0, 0.82521];
    let f = |t: f32| if t > 216. / 24389. { t.cbrt() } else { (24389. / 27. * t + 16.) / 116. };
    let (fx, fy, fz) = (f(x / white[0]), f(y / white[1]), f(z / white[2]));
    [116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz)]
}

fn from_lab([l, a, b]: [f32; 3]) -> Rgba {
    let fy = (l + 16.) / 116.;
    let (fx, fz) = (fy + a / 500., fy - b / 200.);
    let inv = |t: f32| if t.powi(3) > 216. / 24389. { t.powi(3) } else { (116. * t - 16.) / (24389. / 27.) };
    let white = [0.96422, 1.0, 0.82521];
    let (x, y, z) = (inv(fx) * white[0], inv(fy) * white[1], inv(fz) * white[2]);
    let r = 3.1338561 * x - 1.6168667 * y - 0.4906146 * z;
    let g = -0.9787684 * x + 1.9161415 * y + 0.0334540 * z;
    let bl = 0.0719453 * x - 0.2289914 * y + 1.4052427 * z;
    Rgba { r: linear_to_srgb(r), g: linear_to_srgb(g), b: linear_to_srgb(bl), a: 1. }
}
