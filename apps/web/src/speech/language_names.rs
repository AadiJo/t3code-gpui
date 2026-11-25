//! Names for the language codes speech models report, in the UI language (`languageLabel`, which
//! uses `Intl.DisplayNames`). The names are generated from CLDR into `language_names_data.rs` by
//! `scripts/i18n/gen-language-names.mjs`, for the codes listed here.

use super::language_names_data::{CODES, NAMES};

/// The codes the generated table names; the generator reads this list.
pub const SPEECH_LANGUAGE_CODES: &[&str] = &[
    "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy", "da",
    "de", "el", "en", "es", "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "ha", "haw", "he", "hi",
    "hr", "ht", "hu", "hy", "id", "is", "it", "ja", "jw", "ka", "kk", "km", "kn", "ko", "la", "lb",
    "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt", "my", "ne", "nl", "nn",
    "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si", "sk", "sl", "sn", "so", "sq",
    "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl", "tr", "tt", "uk", "ur", "uz", "vi",
    "yi", "yo", "yue", "zh",
];

/// The English name of `code`, if known.
pub fn english_name(code: &str) -> Option<&'static str> {
    name_in("en", code)
}

fn name_in(tag: &str, code: &str) -> Option<&'static str> {
    let index = CODES.iter().position(|known| *known == code)?;
    NAMES.iter().find(|(locale, _)| *locale == tag).map(|(_, names)| names[index])
}

/// `languageLabel`: the name the UI language gives a language code, or the code itself.
pub fn language_label(code: &str) -> gpui_kit::SharedString {
    let tag = crate::i18n::active_locale().tag();
    name_in(tag, code).or_else(|| english_name(code)).map(gpui_kit::SharedString::from).unwrap_or_else(|| code.to_owned().into())
}
