//! A project's default monogram and colour. Ports `projectIdentity.ts`.

use t3code_contracts::ProjectIconColor;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

/// The monogram and colour a project gets without a chosen icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectIdentity {
    pub monogram: String,
    pub color: ProjectIconColor,
}

fn normalize(name: &str) -> String {
    use icu_normalizer::ComposingNormalizerBorrowed;
    ComposingNormalizerBorrowed::new_nfkc().normalize(name).trim().to_owned()
}

fn is_letter_or_number(c: char) -> bool {
    matches!(c.general_category_group(), GeneralCategoryGroup::Letter | GeneralCategoryGroup::Number)
}

fn is_number(c: char) -> bool {
    c.general_category_group() == GeneralCategoryGroup::Number
}

/// `projectMonogram`: the first letter, then the first digit after it, else
/// the last word's initial, else the first word's last letter.
fn project_monogram(name: &str) -> String {
    let normalized = normalize(name);
    let words: Vec<Vec<char>> = normalized
        .split(|c: char| !is_letter_or_number(c))
        .filter(|w| !w.is_empty())
        .map(|w| w.chars().collect())
        .collect();
    let Some(first_word) = words.first() else { return "PR".into() };
    let first = first_word[0];
    let second = first_word[1..]
        .iter()
        .copied()
        .find(|c| is_number(*c))
        .or_else(|| if words.len() > 1 { words.last().and_then(|w| w.first().copied()) } else { first_word.last().copied() })
        .unwrap_or(first);
    format!("{first}{second}").to_uppercase().chars().take(2).collect()
}

/// `projectColor`: a stable colour from the lowercased name.
fn project_color(name: &str) -> ProjectIconColor {
    let lower = normalize(name).to_lowercase();
    let seed = if lower.is_empty() { "project".to_owned() } else { lower };
    let colors = ProjectIconColor::ALL;
    let index = seed.chars().fold(0usize, |index, c| (index * 31 + c as usize) % colors.len());
    colors.get(index).copied().unwrap_or(ProjectIconColor::Blue)
}

/// `deriveProjectIdentity`.
pub fn derive_project_identity(name: &str) -> ProjectIdentity {
    ProjectIdentity { monogram: project_monogram(name), color: project_color(name) }
}
