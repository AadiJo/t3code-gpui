//! Project icon colours and the identity a project without a favicon gets.
//! Ports `projectIconColors.ts` and `projectIdentity.ts`.

use gpui_kit::Hsla;
use t3code_contracts::ProjectIconColor;

use crate::theme_palette::{Appearance, parse_oklch};

/// The colours in the order `PROJECT_ICON_COLORS` lists them (the schema
/// order); the identity hash indexes into this list.
pub const PROJECT_ICON_COLORS: &[ProjectIconColor] = ProjectIconColor::ALL;

/// The ink of a project icon colour: Tailwind's `-400` shade in dark mode and
/// `-600` in light mode (`text-X-600 dark:text-X-400`).
pub fn project_icon_ink(color: ProjectIconColor, appearance: Appearance) -> Hsla {
    use ProjectIconColor as C;
    let (dark, light) = match color {
        C::Gray => ("oklch(70.7% 0.022 261.325)", "oklch(44.6% 0.03 256.802)"),
        C::Red => ("oklch(70.4% 0.191 22.216)", "oklch(57.7% 0.245 27.325)"),
        C::Orange => ("oklch(75% 0.183 55.934)", "oklch(64.6% 0.222 41.116)"),
        C::Amber => ("oklch(82.8% 0.189 84.429)", "oklch(66.6% 0.179 58.318)"),
        C::Yellow => ("oklch(85.2% 0.199 91.936)", "oklch(68.1% 0.162 75.834)"),
        C::Lime => ("oklch(84.1% 0.238 128.85)", "oklch(64.8% 0.2 131.684)"),
        C::Green => ("oklch(79.2% 0.209 151.711)", "oklch(62.7% 0.194 149.214)"),
        C::Emerald => ("oklch(76.5% 0.177 163.223)", "oklch(59.6% 0.145 163.225)"),
        C::Teal => ("oklch(77.7% 0.152 181.912)", "oklch(60% 0.118 184.704)"),
        C::Cyan => ("oklch(78.9% 0.154 211.53)", "oklch(60.9% 0.126 221.723)"),
        C::Sky => ("oklch(74.6% 0.16 232.661)", "oklch(58.8% 0.158 241.966)"),
        C::Blue => ("oklch(70.7% 0.165 254.624)", "oklch(54.6% 0.245 262.881)"),
        C::Indigo => ("oklch(67.3% 0.182 276.935)", "oklch(51.1% 0.262 276.966)"),
        C::Violet => ("oklch(70.2% 0.183 293.541)", "oklch(54.1% 0.281 293.009)"),
        C::Purple => ("oklch(71.4% 0.203 305.504)", "oklch(55.8% 0.288 302.321)"),
        C::Fuchsia => ("oklch(74% 0.238 322.16)", "oklch(59.1% 0.293 322.896)"),
        C::Pink => ("oklch(71.8% 0.202 349.761)", "oklch(59.2% 0.249 0.584)"),
        C::Rose => ("oklch(71.2% 0.194 13.428)", "oklch(58.6% 0.253 17.585)"),
    };
    parse_oklch(if appearance.is_dark() { dark } else { light }).expect("valid oklch literal")
}

/// The monogram and colour a project gets from its name (`deriveProjectIdentity`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectIdentity {
    pub monogram: String,
    pub color: ProjectIconColor,
}

impl ProjectIdentity {
    pub fn from_name(name: &str) -> Self {
        let name = name.trim();
        Self { monogram: monogram(name), color: color(name) }
    }
}

/// The first letter of the first word, then its first digit, else the last
/// word's first letter, else the first word's last letter.
fn monogram(name: &str) -> String {
    let words: Vec<Vec<char>> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.chars().collect())
        .collect();
    let Some(first_word) = words.first() else { return "PR".into() };
    let first = first_word[0];
    let second = first_word[1..]
        .iter()
        .copied()
        .find(|c| c.is_numeric())
        .or_else(|| if words.len() > 1 { words.last().map(|w| w[0]) } else { first_word.last().copied() })
        .unwrap_or(first);
    format!("{first}{second}").to_uppercase().chars().take(2).collect()
}

/// A stable hash of the lowercased name into [`PROJECT_ICON_COLORS`].
fn color(name: &str) -> ProjectIconColor {
    let seed = name.to_lowercase();
    let seed = if seed.is_empty() { "project".to_owned() } else { seed };
    let len = PROJECT_ICON_COLORS.len() as u64;
    let index = seed.chars().fold(0_u64, |index, c| (index * 31 + u64::from(c)) % len);
    PROJECT_ICON_COLORS[index as usize]
}
