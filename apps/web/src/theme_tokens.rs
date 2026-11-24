//! Values the stylesheet derives from a palette (`color-mix` and opacity
//! results), the Tailwind colours no theme can change, and the global
//! geometry, shadow and motion constants of `index.css` (spec 05 sections 1.4
//! to 1.6 and 3).

use std::time::Duration;

use gpui_kit::{BoxShadow, Hsla, Pixels, black, point, px, white};

use crate::theme_color::{Srgb8, flatten, mix_oklab, mix_srgb, parse_css_color, parse_css_literal, with_alpha};
use crate::theme_palette::{Appearance, ThemePalette};

/// Semantic names `index.css` maps the theme roles onto (`index.css:1328-1392`).
/// Primitives read these so the mapping lives in one place.
impl ThemePalette {
    /// `--primary`: the theme's `messageAction`, not `accent`.
    pub fn primary(&self) -> Hsla {
        self.message_action
    }
    pub fn primary_foreground(&self) -> Hsla {
        self.message_action_foreground
    }
    /// `--accent`, the hover and highlight fill: the theme's `accentSurface`.
    pub fn highlight(&self) -> Hsla {
        self.accent_surface
    }
    pub fn highlight_foreground(&self) -> Hsla {
        self.accent_surface_foreground
    }
    /// `--ring`.
    pub fn ring(&self) -> Hsla {
        self.focus
    }
    /// `--destructive`.
    pub fn destructive(&self) -> Hsla {
        self.error
    }
    pub fn destructive_foreground(&self) -> Hsla {
        self.error_foreground
    }
    /// `--popover`.
    pub fn popover(&self) -> Hsla {
        self.surface_overlay
    }
    /// `--background`.
    pub fn background(&self) -> Hsla {
        self.canvas
    }
    /// `--foreground`.
    pub fn foreground(&self) -> Hsla {
        self.text
    }
    /// `--card`.
    pub fn card(&self) -> Hsla {
        self.surface
    }
}

/// The Glass opacity client setting (`glassOpacity`), a percentage from 40
/// to 100. It is `--glass-opacity` in `index.css`: every glass fill mixes its
/// base colour with transparent at this amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlassOpacity(u8);

impl GlassOpacity {
    pub const MIN: u8 = 40;
    pub const MAX: u8 = 100;

    /// A setting value, clamped to 40..=100 as the settings schema does.
    pub fn new(percent: u8) -> Self {
        Self(percent.clamp(Self::MIN, Self::MAX))
    }

    pub fn percent(self) -> u8 {
        self.0
    }

    /// The setting as an alpha in 0..1.
    pub fn alpha(self) -> f32 {
        f32::from(self.0) / 100.
    }
}

impl Default for GlassOpacity {
    /// `DEFAULT_GLASS_OPACITY` in `packages/contracts/src/settings.ts`.
    fn default() -> Self {
        Self(80)
    }
}

/// Whether glass blurs its backdrop. The DirectX renderer cannot yet, so
/// glass there draws its opaque colour; `T3CODE_BACKDROP_BLUR=0` does the same
/// elsewhere, to measure what the blur costs.
pub fn backdrop_blur() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        !cfg!(target_os = "windows") && std::env::var("T3CODE_BACKDROP_BLUR").as_deref() != Ok("0")
    })
}

/// `--glass-blur` and `--glass-saturation` (`index.css`): the
/// `backdrop-filter: blur() saturate()` behind a glass surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBlur {
    /// The CSS blur radius, a standard deviation.
    pub radius: Pixels,
    pub saturation: f32,
}

impl GlassBlur {
    /// 16px and 1.08 dark, 12px and 1.14 light. At glass opacity 100
    /// `appInit.tsx` sets the blur to 0px and the fill hides the backdrop, so
    /// there is none.
    pub fn new(appearance: Appearance, glass: GlassOpacity) -> Option<Self> {
        if glass.percent() == GlassOpacity::MAX {
            return None;
        }
        Some(if appearance.is_dark() {
            Self { radius: px(16.), saturation: 1.08 }
        } else {
            Self { radius: px(12.), saturation: 1.14 }
        })
    }

    /// `dialog-backdrop` and the sheet backdrop (`backdrop-blur-xs`): `blur(4px)`.
    pub const OVERLAY: Self = Self { radius: px(4.), saturation: 1. };
}

/// A glass surface's background: `fill` over the backdrop blurred by `blur`,
/// or `opaque`, the fill flattened over the canvas, where the renderer cannot
/// blur. Paint it with `GlassExt::glass` in `components::ui`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    pub fill: Hsla,
    pub opaque: Hsla,
    pub blur: Option<GlassBlur>,
}

impl Glass {
    /// A translucent overlay behind a modal: `fill` over a 4px blur, or
    /// `fill` alone where the renderer cannot blur.
    pub fn overlay(fill: Hsla) -> Self {
        Self { fill, opaque: fill, blur: Some(GlassBlur::OVERLAY) }
    }
}

/// The derived colours of spec tables 1.5 and 1.6. Colours with alpha keep
/// it; `*_opaque` fields are the same colour flattened over the surface it
/// sits on, for glass where the renderer cannot blur. Glass fills use the
/// theme's [`GlassOpacity`], so a lower setting gives darker surfaces; the
/// `glass_*` fields pair each with its backdrop blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DerivedColors {
    pub sidebar_icon: Hsla,
    pub dropdown_glass: Hsla,
    /// `dropdown-glass` over the canvas, the fallback for menus, popovers and toasts.
    pub dropdown_glass_opaque: Hsla,
    pub dropdown_glass_border: Hsla,
    pub surface_glass: Hsla,
    pub surface_glass_opaque: Hsla,
    /// The composer card's glass (`--chat-composer-glass-surface` at the
    /// glass opacity: `surfaceRaised` for an imported theme), over the canvas.
    pub composer_glass_opaque: Hsla,
    /// `dropdown-glass`: menus, popovers, selects, tooltips and toasts.
    pub glass_dropdown: Glass,
    /// `surface-glass` and `dialog-glass`.
    pub glass_surface: Glass,
    /// The composer card (`ComposerSurface.tsx`).
    pub glass_composer: Glass,
    /// `dialog-backdrop` behind dialogs and the command palette.
    pub glass_dialog_backdrop: Glass,
    pub dialog_glass_border: Hsla,
    pub dialog_backdrop: Hsla,
    pub titlebar_toggle_icon: Hsla,
    pub titlebar_toggle_icon_disabled: Hsla,
    pub sidebar_edge: Hsla,
    pub sidebar_menu_button_text: Hsla,
    pub receded_row_text: Hsla,
    pub receded_card_title: Hsla,
    pub failed_card_title: Hsla,
    pub folder_name: Hsla,
    pub slim_row_read_title: Hsla,
    pub pin_icon: Hsla,
    pub empty_state_text: Hsla,
    pub section_header_rule: Hsla,
    pub drag_boundary_rule: Hsla,
    pub draft_row: Hsla,
    pub draft_row_hover: Hsla,
    pub drop_badge: Hsla,
    pub drop_badge_border: Hsla,
    /// `outline-ring/50`, every element's focus outline.
    pub focus_outline: Hsla,
    /// `ring-ring/24`, the 3px inset ring of a focused input.
    pub input_focus_ring: Hsla,
    pub sidebar_search_focus_ring: Hsla,
    /// Input, textarea and checkbox fill: `input/32` dark, `background` light.
    pub input_fill: Hsla,
    /// Outline button and select fill: `input/32` dark, `popover` light.
    pub outline_fill: Hsla,
    /// Outline hover: `input/64` dark, `accent/50` light.
    pub outline_hover: Hsla,
    pub toggle_pressed: Hsla,
    /// `bg-primary/90`.
    pub primary_hover: Hsla,
    pub kbd: Hsla,
    pub kbd_outline_border: Hsla,
    /// `foreground/8%`, a checked select or radio item.
    pub item_selected: Hsla,
    pub skeleton: Hsla,
    pub dialog_footer: Hsla,
    pub terminal_drawer_border: Hsla,
    pub destructive_highlight: Hsla,
    pub inline_code: Hsla,
    pub scrollbar_thumb: Hsla,
    pub scrollbar_thumb_hover: Hsla,
}

impl DerivedColors {
    /// Resolves every derived colour for one palette.
    pub fn new(c: &ThemePalette, appearance: Appearance, glass: GlassOpacity) -> Self {
        let dark = appearance.is_dark();
        let pick = |d: f32, l: f32| if dark { d } else { l };
        let transparent = Hsla::transparent_black();
        let glass_alpha = glass.alpha();
        let dropdown_glass = mix_srgb(
            c.popover(),
            0.18,
            mix_srgb(c.popover(), glass_alpha, transparent),
        );
        let surface_glass = mix_srgb(c.canvas, glass_alpha, transparent);
        let composer_glass = mix_srgb(c.surface_raised, glass_alpha, transparent);
        let blur = GlassBlur::new(appearance, glass);
        let glass_over_canvas = |fill: Hsla| Glass { fill, opaque: flatten(fill, c.canvas), blur };
        let dialog_backdrop = with_alpha(c.canvas, pick(0.64, 0.60));
        let input_alpha = with_alpha(c.input, 0.32);
        Self {
            sidebar_icon: mix_srgb(c.sidebar_muted_foreground, 0.85, c.sidebar),
            dropdown_glass,
            dropdown_glass_opaque: flatten(dropdown_glass, c.canvas),
            dropdown_glass_border: with_alpha(c.text, 0.10),
            surface_glass,
            surface_glass_opaque: flatten(surface_glass, c.canvas),
            composer_glass_opaque: flatten(composer_glass, c.canvas),
            glass_dropdown: glass_over_canvas(dropdown_glass),
            glass_surface: glass_over_canvas(surface_glass),
            glass_composer: glass_over_canvas(composer_glass),
            glass_dialog_backdrop: Glass::overlay(dialog_backdrop),
            dialog_glass_border: if dark {
                with_alpha(white(), 0.08)
            } else {
                with_alpha(c.text, 0.10)
            },
            dialog_backdrop,
            titlebar_toggle_icon: c.muted_foreground,
            titlebar_toggle_icon_disabled: mix_oklab(c.muted_foreground, 0.55, c.toolbar),
            sidebar_edge: with_alpha(c.sidebar_foreground, pick(0.08, 0.10)),
            sidebar_menu_button_text: with_alpha(c.sidebar_muted_foreground, 0.80),
            receded_row_text: with_alpha(c.sidebar_muted_foreground, 0.75),
            receded_card_title: with_alpha(c.text, 0.75),
            failed_card_title: with_alpha(c.text, 0.95),
            folder_name: with_alpha(c.sidebar_foreground, 0.90),
            slim_row_read_title: with_alpha(c.secondary_label, 0.70),
            pin_icon: with_alpha(c.muted_foreground, 0.65),
            empty_state_text: with_alpha(c.muted_foreground, 0.60),
            section_header_rule: with_alpha(c.sidebar_border, 0.50),
            drag_boundary_rule: with_alpha(c.sidebar_foreground, 0.25),
            draft_row: with_alpha(c.warning, 0.04),
            draft_row_hover: with_alpha(c.warning, 0.08),
            drop_badge: with_alpha(c.primary(), 0.10),
            drop_badge_border: with_alpha(c.primary(), 0.40),
            focus_outline: with_alpha(c.ring(), 0.50),
            input_focus_ring: with_alpha(c.ring(), 0.24),
            sidebar_search_focus_ring: with_alpha(c.ring(), 0.70),
            input_fill: if dark { input_alpha } else { c.background() },
            outline_fill: if dark { input_alpha } else { c.popover() },
            outline_hover: if dark {
                with_alpha(c.input, 0.64)
            } else {
                with_alpha(c.highlight(), 0.50)
            },
            toggle_pressed: with_alpha(c.input, 0.64),
            primary_hover: with_alpha(c.primary(), 0.90),
            kbd: with_alpha(c.text, 0.07),
            kbd_outline_border: with_alpha(c.text, 0.15),
            item_selected: with_alpha(c.text, 0.08),
            skeleton: with_alpha(c.muted_foreground, 0.15),
            dialog_footer: with_alpha(c.muted, 0.72),
            terminal_drawer_border: with_alpha(c.border, 0.80),
            destructive_highlight: with_alpha(c.destructive(), 0.10),
            inline_code: with_alpha(c.text, 0.10),
            scrollbar_thumb: if dark { with_alpha(white(), 0.36) } else { hex("#d9d9d9") },
            scrollbar_thumb_hover: if dark { with_alpha(white(), 0.45) } else { hex("#bfbfbf") },
        }
    }
}

/// Tailwind palette colours the shell uses that no theme can change (spec 1.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FixedColors {
    pub info: Hsla,
    pub info_foreground: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub guest: Hsla,
    pub guest_foreground: Hsla,
    /// `text-indigo-600 dark:text-indigo-300`, the "Input" status.
    pub input_status: Hsla,
    pub diff_addition_foreground: Hsla,
    pub diff_deletion_foreground: Hsla,
}

impl FixedColors {
    pub fn new(appearance: Appearance) -> Self {
        let dark = appearance.is_dark();
        let pick = |d: &str, l: &str| oklch(if dark { d } else { l });
        // Tailwind v4 literals, converted the way the browser converts them.
        let (blue_400, blue_500, blue_700) = ("oklch(70.7% 0.165 254.624)", "oklch(62.3% 0.214 259.815)", "oklch(48.8% 0.243 264.376)");
        let (emerald_400, emerald_500, emerald_700) = ("oklch(76.5% 0.177 163.223)", "oklch(69.6% 0.17 162.48)", "oklch(50.8% 0.118 165.612)");
        let (fuchsia_300, fuchsia_500, fuchsia_600) = ("oklch(83.3% 0.145 321.434)", "oklch(66.7% 0.295 322.15)", "oklch(59.1% 0.293 322.896)");
        let (indigo_300, indigo_600) = ("oklch(78.5% 0.115 274.713)", "oklch(51.1% 0.262 276.966)");
        let (red_400, red_700) = ("oklch(70.4% 0.191 22.216)", "oklch(50.5% 0.213 27.518)");
        Self {
            info: oklch(blue_500),
            info_foreground: pick(blue_400, blue_700),
            success: oklch(emerald_500),
            success_foreground: pick(emerald_400, emerald_700),
            guest: oklch(fuchsia_500),
            guest_foreground: pick(fuchsia_300, fuchsia_600),
            input_status: pick(indigo_300, indigo_600),
            diff_addition_foreground: pick(emerald_400, emerald_700),
            diff_deletion_foreground: pick(red_400, red_700),
        }
    }
}

fn oklch(value: &str) -> Hsla {
    parse_css_literal(value).map(Srgb8::hsla).expect("valid oklch literal")
}

fn hex(value: &str) -> Hsla {
    parse_css_color(value).map(Srgb8::hsla).expect("valid colour literal")
}

/// Corner radii (`index.css:292-297`), all derived from `--radius` = 0.5rem.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    pub sm: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
    pub xl: Pixels,
    pub xxl: Pixels,
    pub xxxl: Pixels,
    /// `--control-radius`.
    pub control: Pixels,
    pub full: Pixels,
}

impl Radii {
    /// Radii for a root font size; `--radius` is 0.5rem, the steps are px offsets.
    pub fn new(rem: f32) -> Self {
        let radius = 0.5 * rem;
        Self {
            sm: px(radius - 4.),
            md: px(radius - 2.),
            lg: px(radius),
            xl: px(radius + 4.),
            xxl: px(radius + 8.),
            xxxl: px(radius + 12.),
            control: px(radius),
            full: px(9999.),
        }
    }
}

/// Box shadows (spec 3.4). GPUI draws CSS shadows directly, including inset ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Shadows {
    pub xs5: Vec<BoxShadow>,
    pub sm5: Vec<BoxShadow>,
    pub md5: Vec<BoxShadow>,
    pub lg: Vec<BoxShadow>,
    /// `shadow-xl shadow-black/25`: toasts and glass tooltips.
    pub xl25: Vec<BoxShadow>,
    /// Menus, selects and popovers.
    pub menu: Vec<BoxShadow>,
    /// `dialog-glass`.
    pub dialog: Vec<BoxShadow>,
    /// `shadow-2xl/40`: the Settings and Usage route modal (`AppRouteModal`).
    pub app_modal: Vec<BoxShadow>,
    /// The 1px `before:` highlight a control draws across its top border in
    /// dark mode (white 6%) or bottom border in light mode (black 4%).
    pub control_highlight: ControlHighlight,
}

/// A 1px line a control draws over one edge of its border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlHighlight {
    pub color: Hsla,
    /// The top edge in dark mode, the bottom edge in light mode.
    pub top: bool,
}

fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Hsla) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(x), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

fn shade(alpha: f32) -> Hsla {
    with_alpha(black(), alpha)
}

impl Shadows {
    pub fn new(appearance: Appearance) -> Self {
        let dark = appearance.is_dark();
        Self {
            xs5: vec![shadow(0., 1., 2., 0., shade(0.05))],
            sm5: vec![shadow(0., 1., 3., 0., shade(0.05)), shadow(0., 1., 2., -1., shade(0.05))],
            md5: vec![shadow(0., 4., 6., -1., shade(0.05)), shadow(0., 2., 4., -2., shade(0.05))],
            lg: vec![shadow(0., 10., 15., -3., shade(0.10)), shadow(0., 4., 6., -4., shade(0.10))],
            xl25: vec![shadow(0., 20., 25., -5., shade(0.25)), shadow(0., 8., 10., -6., shade(0.25))],
            menu: vec![if dark {
                shadow(0., 18., 44., -18., shade(0.80))
            } else {
                shadow(0., 16., 40., -18., shade(0.55))
            }],
            dialog: if dark {
                vec![
                    // 2px: GPUI's inset shadows start under the 1px border.
                    BoxShadow { inset: true, ..shadow(0., 2., 0., 0., with_alpha(white(), 0.04)) },
                    shadow(0., 24., 72., -20., shade(0.90)),
                ]
            } else {
                vec![shadow(0., 24., 64., -24., shade(0.65))]
            },
            app_modal: vec![shadow(0., 25., 50., -12., shade(0.40))],
            control_highlight: ControlHighlight {
                color: if dark { with_alpha(white(), 0.06) } else { shade(0.04) },
                top: dark,
            },
        }
    }

    /// `shadow-xs` tinted with a colour at 24%, as default and destructive buttons use.
    pub fn xs_tinted(color: Hsla) -> Vec<BoxShadow> {
        vec![shadow(0., 1., 2., 0., with_alpha(color, 0.24))]
    }

    /// The 1px inset top line of a filled button: white 16%, or black 8% while
    /// pressed. Offset 2px because GPUI's inset shadows start under the 1px border.
    pub fn button_inset(pressed: bool) -> BoxShadow {
        let color = if pressed { shade(0.08) } else { with_alpha(white(), 0.16) };
        BoxShadow { inset: true, ..shadow(0., 2., 0., 0., color) }
    }
}

/// Semantic geometry (`index.css:91-155`), in pixels at the current root size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spacing {
    pub sidebar_content_inset: Pixels,
    pub sidebar_control_gap: Pixels,
    pub sidebar_row_content_inset: Pixels,
    pub floating_content_inset: Pixels,
    pub command_shell_inset: Pixels,
    pub command_content_inset: Pixels,
    /// 52px on web and macOS; the Windows overlay uses 40px.
    pub workspace_topbar_height: Pixels,
    pub titlebar_control_size: Pixels,
    pub titlebar_control_gap: Pixels,
    /// 1.25rem at a window width of 640px or more, 0.75rem below.
    pub workspace_gutter: Pixels,
    pub chat_content_max_width: Pixels,
    pub thread_details_panel_width: Pixels,
    pub app_scrollbar_width: Pixels,
}

impl Spacing {
    pub fn new(rem: f32) -> Self {
        let r = |n: f32| px(n * rem);
        Self {
            sidebar_content_inset: r(0.5),
            sidebar_control_gap: r(0.5),
            sidebar_row_content_inset: r(0.625),
            floating_content_inset: r(0.75),
            command_shell_inset: r(0.5),
            command_content_inset: r(1.),
            workspace_topbar_height: if cfg!(target_os = "macos") { px(52.) } else { px(40.) },
            titlebar_control_size: r(1.75),
            titlebar_control_gap: r(0.75),
            workspace_gutter: r(1.25),
            chat_content_max_width: r(46.),
            thread_details_panel_width: r(17.5),
            app_scrollbar_width: px(6.),
        }
    }
}

/// A CSS cubic-bezier easing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier(pub f32, pub f32, pub f32, pub f32);

impl CubicBezier {
    /// Progress at time `t` in 0..1, solved by bisection on the x curve.
    pub fn ease(self, t: f32) -> f32 {
        let Self(x1, y1, x2, y2) = self;
        let curve = |a: f32, b: f32, s: f32| {
            3. * a * s * (1. - s).powi(2) + 3. * b * s.powi(2) * (1. - s) + s.powi(3)
        };
        let (mut low, mut high) = (0_f32, 1_f32);
        for _ in 0..24 {
            let mid = (low + high) / 2.;
            if curve(x1, x2, mid) < t { low = mid } else { high = mid }
        }
        curve(y1, y2, (low + high) / 2.)
    }
}

/// Durations and easings (spec 3.7). The app avoids per-frame animation:
/// pulses and spinners advance in a few discrete steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// Tailwind's default `transition` duration.
    pub fast: Duration,
    pub standard: CubicBezier,
    pub ease_out: CubicBezier,
    pub ease_in: CubicBezier,
    pub drawer: CubicBezier,
    pub toast: CubicBezier,
    pub dialog: Duration,
    pub toast_enter: Duration,
    pub switch_thumb: Duration,
    /// Panel open and close, 0ms by default (setting 0 to 400ms).
    pub panel: Duration,
    pub skeleton_cycle: Duration,
    pub status_pulse_cycle: Duration,
    pub spin_cycle: Duration,
    /// Frames per second a spinner turns at: stepped, never every vsync.
    pub spin_steps_per_second: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            fast: Duration::from_millis(150),
            standard: CubicBezier(0.4, 0., 0.2, 1.),
            ease_out: CubicBezier(0., 0., 0.2, 1.),
            ease_in: CubicBezier(0.4, 0., 1., 1.),
            drawer: CubicBezier(0.32, 0.72, 0., 1.),
            toast: CubicBezier(0.22, 1., 0.36, 1.),
            dialog: Duration::from_millis(200),
            toast_enter: Duration::from_millis(500),
            switch_thumb: Duration::from_millis(150),
            panel: Duration::ZERO,
            skeleton_cycle: Duration::from_millis(2400),
            status_pulse_cycle: Duration::from_millis(2000),
            spin_cycle: Duration::from_secs(1),
            spin_steps_per_second: 12.,
        }
    }
}
