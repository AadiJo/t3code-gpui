//! Maps terminal colours to theme colours the way the web terminal does
//! (`ThreadTerminalDrawer.tsx` `TERMINAL_PALETTES` and `terminalThemeFromApp`):
//! the 16 ANSI colours come from a fixed per-appearance table, 16..=255 are the
//! xterm cube and grey ramp, truecolor passes through, and the default
//! foreground, background, cursor and selection come from the app theme.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use gpui_kit::{Hsla, Rgba};

use crate::theme_palette::{AppTheme, Appearance};

const DARK_ANSI: [u32; 16] = [
    0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666,
    0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xe5e5e5,
];

const LIGHT_ANSI: [u32; 16] = [
    0x000000, 0xcd3131, 0x107c10, 0x949800, 0x0451a5, 0xbc05bc, 0x0598bc, 0x555555, 0x666666,
    0xcd3131, 0x14ce14, 0xb5ba00, 0x0451a5, 0xbc05bc, 0x0598bc, 0xa5a5a5,
];

/// Resolved colours for one theme. Built once per theme change, read per dirty cell.
#[derive(Clone, Debug)]
pub struct TerminalPalette {
    indexed: [Rgb; 256],
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub scrollbar: Hsla,
    pub scrollbar_hover: Hsla,
    pub background_hsla: Hsla,
}

impl TerminalPalette {
    pub fn new(theme: &AppTheme) -> Self {
        let ansi = match theme.appearance {
            Appearance::Dark => &DARK_ANSI,
            Appearance::Light => &LIGHT_ANSI,
        };
        let mut indexed = [Rgb::default(); 256];
        for (slot, hex) in indexed.iter_mut().zip(ansi) {
            *slot = hex_rgb(*hex);
        }
        // The xterm 6x6x6 cube, then the 24-step grey ramp.
        let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
        for index in 16..232 {
            let i = index - 16;
            indexed[index] = Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) };
        }
        for index in 232..256 {
            let v = (8 + (index - 232) * 10) as u8;
            indexed[index] = Rgb { r: v, g: v, b: v };
        }
        let colors = &theme.colors;
        Self {
            indexed,
            foreground: hsla_rgb(colors.terminal_foreground),
            background: hsla_rgb(colors.terminal_background),
            cursor: colors.terminal_cursor,
            selection: colors.terminal_selection,
            scrollbar: colors.terminal_scrollbar,
            scrollbar_hover: colors.terminal_scrollbar_hover,
            background_hsla: colors.terminal_background,
        }
    }

    /// The colour a cell attribute names, honouring OSC 4/10/11 overrides the program set.
    pub fn resolve(&self, color: Color, overrides: &Colors) -> Rgb {
        match color {
            Color::Spec(rgb) => rgb,
            Color::Indexed(index) => overrides[index as usize].unwrap_or(self.indexed[index as usize]),
            Color::Named(named) => {
                if let Some(rgb) = overrides[named] {
                    return rgb;
                }
                match named {
                    NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
                    NamedColor::Background => self.background,
                    NamedColor::Cursor => hsla_rgb(self.cursor),
                    NamedColor::DimForeground => blend(self.foreground, self.background),
                    named if (named as usize) < 16 => self.indexed[named as usize],
                    // Dim ANSI colours: the normal colour, faded like SGR 2.
                    named => {
                        let base = self.indexed[named.to_bright() as usize & 15];
                        blend(base, self.background)
                    }
                }
            }
        }
    }

    /// An indexed colour for OSC 4 queries.
    pub fn indexed(&self, index: usize) -> Rgb {
        self.indexed[index & 255]
    }
}

/// Half-way between two colours: how SGR 2 (dim) fades text towards its background.
pub fn blend(foreground: Rgb, background: Rgb) -> Rgb {
    let mix = |a: u8, b: u8| ((a as u16 + b as u16) / 2) as u8;
    Rgb {
        r: mix(foreground.r, background.r),
        g: mix(foreground.g, background.g),
        b: mix(foreground.b, background.b),
    }
}

/// Converts to the colour GPUI paints with.
pub fn to_hsla(rgb: Rgb) -> Hsla {
    Rgba { r: rgb.r as f32 / 255., g: rgb.g as f32 / 255., b: rgb.b as f32 / 255., a: 1. }.into()
}

fn hex_rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

fn hsla_rgb(color: Hsla) -> Rgb {
    let rgba = color.to_rgb();
    let channel = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    Rgb { r: channel(rgba.r), g: channel(rgba.g), b: channel(rgba.b) }
}
