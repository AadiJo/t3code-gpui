//! Sizes and colours the shell draws with, taken from `05-ui-shell.md`.
//! Colours resolve from the active [`AppTheme`] plus the non-themeable
//! Tailwind colours the web app hard-codes, so dark and light both work.

use gpui_kit::{Hsla, Pixels, Rgba, px, rgb};

use crate::theme_palette::{AppTheme, Appearance};

/// Topbar height on macOS (`--workspace-topbar-height`).
pub const TOPBAR_HEIGHT_MAC: Pixels = px(52.);
/// Topbar height with a caption-button overlay (Windows, Linux).
pub const TOPBAR_HEIGHT_OVERLAY: Pixels = px(40.);
/// Titlebar toggle buttons are 28x28 (`--workspace-titlebar-control-size`).
pub const TITLEBAR_CONTROL_SIZE: Pixels = px(28.);
/// Gap between the right-hand titlebar toggles (`gap-1`).
pub const TITLEBAR_CONTROL_GAP: Pixels = px(4.);
/// macOS: 70px traffic lights + 12px + `ml-px`.
pub const MAC_CONTROLS_LEFT: Pixels = px(83.);
/// Overlay platforms: 12px + `ml-px`.
pub const OVERLAY_CONTROLS_LEFT: Pixels = px(13.);
/// 12px + `mr-px` from the right edge (plus caption width on overlay platforms).
pub const CONTROLS_RIGHT: Pixels = px(13.);
/// One native caption button on Windows.
pub const CAPTION_BUTTON_WIDTH: Pixels = px(46.);

pub const SIDEBAR_DEFAULT_WIDTH: Pixels = px(256.);
pub const SIDEBAR_MIN_WIDTH: Pixels = px(208.);
/// The main pane keeps at least this much width (`threadSidebarWidth.ts`).
pub const SIDEBAR_MAIN_RESERVE: Pixels = px(640.);
/// Width of the rail's hit area straddling the sidebar edge.
pub const SIDEBAR_RAIL_WIDTH: Pixels = px(16.);
pub const RIGHT_PANEL_DEFAULT_WIDTH: Pixels = px(540.);
pub const RIGHT_PANEL_MIN_WIDTH: Pixels = px(360.);
pub const TERMINAL_DEFAULT_HEIGHT: Pixels = px(280.);
pub const TERMINAL_MIN_HEIGHT: Pixels = px(180.);

/// Card row: `li py-0.5` around a 78px box (60px without a meta line).
pub const CARD_ROW_HEIGHT: Pixels = px(78.);
pub const CARD_ROW_HEIGHT_NO_META: Pixels = px(60.);
pub const CARD_ROW_PADDING_Y: Pixels = px(2.);
/// Slim, folder and "show more" rows are `h-9`.
pub const SLIM_ROW_HEIGHT: Pixels = px(36.);
/// Shelf headers and folder headers are `h-8`.
pub const SECTION_HEADER_HEIGHT: Pixels = px(32.);
/// `mt-2` above every folder header but the first.
pub const FOLDER_HEADER_GAP: Pixels = px(8.);
/// `gap-px` between list items.
pub const LIST_GAP: Pixels = px(1.);
/// `--sidebar-row-content-inset`.
pub const ROW_CONTENT_INSET: Pixels = px(10.);
/// `--sidebar-content-inset`, also the list group's padding.
pub const CONTENT_INSET: Pixels = px(8.);
/// Mask fade at the top and bottom of the scrolling list.
pub const LIST_FADE: Pixels = px(12.);
pub const ROW_RADIUS: Pixels = px(6.);
pub const CONTROL_RADIUS: Pixels = px(8.);
pub const MENU_MIN_WIDTH: Pixels = px(128.);
pub const MENU_MAX_WIDTH: Pixels = px(384.);
pub const MENU_ROW_HEIGHT: Pixels = px(28.);
/// Opacity of the surface grain rect (`index.css:1801-1818`).
pub const GRAIN_OPACITY: f32 = 0.035;

/// Every colour the shell uses, resolved for one theme and appearance.
#[derive(Clone, Copy, Debug)]
pub struct ShellColors {
    pub dark: bool,
    pub chrome: Hsla,
    pub canvas: Hsla,
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub secondary_label: Hsla,
    pub border: Hsla,
    pub ring: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub popover: Hsla,
    pub secondary: Hsla,
    pub primary: Hsla,
    pub destructive: Hsla,
    pub destructive_foreground: Hsla,
    pub warning: Hsla,
    pub warning_foreground: Hsla,
    pub error: Hsla,
    pub error_foreground: Hsla,
    pub sidebar: Hsla,
    pub sidebar_foreground: Hsla,
    pub sidebar_muted_foreground: Hsla,
    pub sidebar_row_hover: Hsla,
    pub sidebar_row_active: Hsla,
    pub sidebar_row_selected: Hsla,
    pub sidebar_border: Hsla,
    /// `color-mix(in srgb, sidebarMutedForeground 85%, sidebar)`.
    pub sidebar_icon: Hsla,
    /// Sidebar right edge: sidebar foreground at 8% (dark) or 10% (light).
    pub sidebar_edge: Hsla,
    /// Titlebar toggle icon colour (`--contrast-muted-foreground`).
    pub titlebar_icon: Hsla,
    pub info: Hsla,
    pub info_foreground: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub guest_foreground: Hsla,
    pub input_status: Hsla,
    pub diff_addition: Hsla,
    pub diff_deletion: Hsla,
    /// Running terminal icon: teal-300 at 90% (dark), teal-600 (light).
    pub terminal_running: Hsla,
    /// `dropdown-glass` fill at the glass opacity, flattened over the canvas.
    pub glass: Hsla,
    /// `dropdown-glass` border: foreground at 10%.
    pub glass_border: Hsla,
    /// Context menu destructive highlight: destructive at 10%.
    pub destructive_highlight: Hsla,
    pub kbd_background: Hsla,
}

impl ShellColors {
    /// Resolves the shell palette for the active theme.
    pub fn from_theme(theme: &AppTheme) -> Self {
        let c = &theme.colors;
        let dark = theme.appearance == Appearance::Dark;
        let pick = |dark_hex: u32, light_hex: u32| -> Hsla {
            rgb(if dark { dark_hex } else { light_hex }).into()
        };
        Self {
            dark,
            chrome: c.chrome,
            canvas: c.canvas,
            foreground: c.text,
            muted_foreground: c.muted_foreground,
            secondary_label: c.secondary_label,
            border: c.border,
            ring: c.focus,
            accent: c.accent_surface,
            accent_foreground: c.accent_surface_foreground,
            popover: c.surface_overlay,
            secondary: c.secondary,
            primary: c.message_action,
            destructive: c.error,
            destructive_foreground: c.error_foreground,
            warning: c.warning,
            warning_foreground: c.warning_foreground,
            error: c.error,
            error_foreground: c.error_foreground,
            sidebar: c.sidebar,
            sidebar_foreground: c.sidebar_foreground,
            sidebar_muted_foreground: c.sidebar_muted_foreground,
            sidebar_row_hover: c.sidebar_row_hover,
            sidebar_row_active: c.sidebar_row_active,
            sidebar_row_selected: c.sidebar_row_selected,
            sidebar_border: c.sidebar_border,
            sidebar_icon: mix(c.sidebar_muted_foreground, c.sidebar, 0.85),
            sidebar_edge: c.sidebar_foreground.opacity(if dark { 0.08 } else { 0.10 }),
            titlebar_icon: c.muted_foreground,
            info: pick(0x3280ff, 0x3280ff),
            info_foreground: pick(0x56a2ff, 0x1447e6),
            success: pick(0x00b981, 0x00b981),
            success_foreground: pick(0x00d294, 0x007857),
            guest_foreground: pick(0xf2a9ff, 0xc500da),
            input_status: pick(0xa4b4ff, 0x4f39f6),
            diff_addition: pick(0x00d294, 0x007857),
            diff_deletion: pick(0xff6568, 0xbf000f),
            terminal_running: if dark { Hsla::from(rgb(0x46ecd5)).opacity(0.9) } else { rgb(0x009689).into() },
            glass: theme.derived.dropdown_glass_opaque,
            glass_border: c.text.opacity(0.10),
            destructive_highlight: c.error.opacity(0.10),
            kbd_background: c.text.opacity(0.07),
        }
    }
}

/// `color-mix(in srgb, a weight, b)`.
pub fn mix(a: Hsla, b: Hsla, weight: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let lerp = |x: f32, y: f32| x * weight + y * (1. - weight);
    Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}
