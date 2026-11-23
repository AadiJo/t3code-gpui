//! Colours and sizes the composer draws with, resolved from the active theme.
//! Sizes are the spec's numbers at the default 16px interface size and 14px
//! prompt size (`06-ui-chat.md` sections 0, 6 and 7).

use gpui_kit::{App, Hsla, Pixels, px, rgb};

use crate::ActiveAppTheme as _;
use crate::composer_editor_mentions::{ChipKind, ContextKind};
use crate::theme_palette::Appearance;

/// `--chat-content-max-width` 46rem plus the fork's 2rem.
pub const COMPOSER_MAX_WIDTH: Pixels = px(768.);
/// Prompt font size (`--font-size-prompt`).
pub const PROMPT_FONT_SIZE: Pixels = px(14.);
/// `leading-relaxed` at 14px.
pub const PROMPT_LINE_HEIGHT: Pixels = px(22.75);
/// Code font size (`--font-size-code`).
pub const CODE_FONT_SIZE: Pixels = px(13.);
/// Code line height inside composer code blocks.
pub const CODE_LINE_HEIGHT: Pixels = px(20.);
/// `calc(2lh + 8px)` minus the editor's 4px inset on each side.
pub const EDITOR_MIN_HEIGHT: Pixels = px(53.5);
/// `max-h-52`.
pub const EDITOR_MAX_HEIGHT: Pixels = px(208.);
/// Card radius `rounded-2xl`.
pub const SURFACE_RADIUS: Pixels = px(16.);
/// Body padding `px-4 pt-4 pb-2`.
pub const BODY_PADDING_X: Pixels = px(16.);
pub const BODY_PADDING_TOP: Pixels = px(16.);
pub const BODY_PADDING_BOTTOM: Pixels = px(8.);
/// Footer padding `px-3 pb-3`.
pub const FOOTER_PADDING_X: Pixels = px(12.);
pub const FOOTER_PADDING_BOTTOM: Pixels = px(12.);
/// Composer controls and icon actions: `h-7`.
pub const CONTROL_HEIGHT: Pixels = px(28.);
/// `--control-radius`.
pub const CONTROL_RADIUS: Pixels = px(8.);
/// Icon action radius `rounded-md`.
pub const ICON_ACTION_RADIUS: Pixels = px(6.);
/// Footer goes compact below this composer width.
pub const FOOTER_COMPACT_BREAKPOINT: Pixels = px(620.);
/// Attached banners and the command menu sit this far in from the card's sides.
pub const DRAWER_INSET: Pixels = px(22.);
/// How far an attached surface overlaps the card's top: `1rem + 1px`.
pub const ATTACHMENT_OVERLAP: Pixels = px(17.);
/// Model picker body height `h-80`.
pub const MODEL_PICKER_HEIGHT: Pixels = px(320.);
/// Model list column including the rail `max-w-90`.
pub const MODEL_PICKER_LIST_WIDTH: Pixels = px(360.);
/// Provider rail `w-11`.
pub const MODEL_PICKER_RAIL_WIDTH: Pixels = px(44.);
/// Effort column `w-48`.
pub const MODEL_PICKER_EFFORT_WIDTH: Pixels = px(192.);
/// Command menu list `max-h-72`.
pub const COMMAND_MENU_MAX_HEIGHT: Pixels = px(288.);
/// Popup radius `rounded-lg`.
pub const POPUP_RADIUS: Pixels = px(8.);

/// UI font family. The shell owns fonts; these match its resolved stack.
pub const UI_FONT_FAMILY: &str = if cfg!(target_os = "macos") { ".SystemUIFont" } else { "Inter" };
/// Monospace family for code.
pub const MONO_FONT_FAMILY: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

/// Mixes `color` over `base` at `amount` (0..1), in sRGB like `color-mix(in srgb, ...)`.
pub fn mix(color: Hsla, base: Hsla, amount: f32) -> Hsla {
    let a = color.to_rgb();
    let b = base.to_rgb();
    let lerp = |x: f32, y: f32| y + (x - y) * amount;
    gpui_kit::Rgba { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b), a: 1.0 }.into()
}

/// The composer's resolved colours.
#[derive(Clone, Copy, Debug)]
pub struct ComposerColors {
    pub appearance: Appearance,
    pub canvas: Hsla,
    pub foreground: Hsla,
    pub muted_foreground: Hsla,
    pub placeholder: Hsla,
    pub secondary_label: Hsla,
    pub surface: Hsla,
    pub glass_surface: Hsla,
    pub outline: Hsla,
    pub highlight: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub row_hover: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub primary: Hsla,
    pub ring: Hsla,
    pub code_background: Hsla,
    pub code_foreground: Hsla,
    pub selection: Hsla,
    pub popup_background: Hsla,
    pub popup_border: Hsla,
    pub muted: Hsla,
    pub warning: Hsla,
    pub error: Hsla,
    pub error_foreground: Hsla,
    pub kbd_background: Hsla,
}

impl ComposerColors {
    /// Reads the active theme.
    pub fn new(cx: &App) -> Self {
        let theme = cx.app_theme();
        let c = theme.colors;
        let dark = theme.appearance == Appearance::Dark;
        Self {
            appearance: theme.appearance,
            canvas: c.canvas,
            foreground: c.text,
            muted_foreground: c.muted_foreground,
            placeholder: c.placeholder,
            secondary_label: c.secondary_label,
            surface: c.surface,
            // `surfaceRaised` at the glass opacity over the canvas (GPUI has no backdrop blur).
            glass_surface: theme.derived.composer_glass_opaque,
            // dark: mix(input 30%, background); light: toolbar border.
            outline: if dark { mix(c.input, c.canvas, 0.30) } else { c.toolbar_border },
            highlight: if dark { c.input.opacity(0.12) } else { gpui_kit::transparent_black() },
            border: c.border,
            input: c.input,
            row_hover: c.sidebar_row_hover,
            accent: c.accent_surface,
            accent_foreground: c.accent_surface_foreground,
            primary: c.message_action,
            ring: c.focus,
            code_background: c.code_background,
            code_foreground: c.code_foreground,
            selection: c.focus.opacity(if dark { 0.35 } else { 0.25 }),
            // dropdown-glass at the glass opacity, flattened over the canvas.
            popup_background: theme.derived.dropdown_glass_opaque,
            popup_border: if dark { rgb(0x313131).into() } else { rgb(0xe7e7e7).into() },
            muted: c.muted,
            warning: c.warning,
            error: c.error,
            error_foreground: c.error_foreground,
            kbd_background: if dark { rgb(0x2b2b2c).into() } else { rgb(0xefefef).into() },
        }
    }

    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }

    /// The chip ink for a chip kind: the per-theme contrast-solved accent
    /// (`contextChipVariables` in `themePalette.ts`, solved for Graphite).
    pub fn chip_ink(&self, kind: &ChipKind) -> Hsla {
        let (dark, light) = match kind {
            ChipKind::Mention { .. } => (0x1aa0b9, 0x00788d),
            ChipKind::Skill { .. } => (0xc673d2, 0x9d4daa),
            ChipKind::Model { .. } => (0x9e951f, 0x776f00),
            ChipKind::Citation => (0x5392f3, 0x2e6bca),
            ChipKind::Context { kind, .. } => match kind {
                ContextKind::Image | ContextKind::File => (0x1d9cd9, 0x0074a6),
                ContextKind::Terminal => (0x19a776, 0x007d57),
                ContextKind::Thread => (0x00a49d, 0x007a76),
                ContextKind::PullRequest => (0x7e89f5, 0x5c62cc),
                ContextKind::Text | ContextKind::ReviewComment | ContextKind::PreviewAnnotation
                | ContextKind::Element | ContextKind::Other => return self.foreground,
            },
        };
        rgb(if self.is_dark() { dark } else { light }).into()
    }
}
