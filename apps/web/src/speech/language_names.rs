//! English names for the language codes speech models report (`languageLabel` uses
//! `Intl.DisplayNames`, which this app does not have; English labels show in every locale).

const NAMES: &[(&str, &str)] = &[
    ("af", "Afrikaans"), ("am", "Amharic"), ("ar", "Arabic"), ("as", "Assamese"), ("az", "Azerbaijani"), ("ba", "Bashkir"),
    ("be", "Belarusian"), ("bg", "Bulgarian"), ("bn", "Bangla"), ("bo", "Tibetan"), ("br", "Breton"), ("bs", "Bosnian"),
    ("ca", "Catalan"), ("cs", "Czech"), ("cy", "Welsh"), ("da", "Danish"), ("de", "German"), ("el", "Greek"), ("en", "English"),
    ("es", "Spanish"), ("et", "Estonian"), ("eu", "Basque"), ("fa", "Persian"), ("fi", "Finnish"), ("fo", "Faroese"),
    ("fr", "French"), ("gl", "Galician"), ("gu", "Gujarati"), ("ha", "Hausa"), ("haw", "Hawaiian"), ("he", "Hebrew"),
    ("hi", "Hindi"), ("hr", "Croatian"), ("ht", "Haitian Creole"), ("hu", "Hungarian"), ("hy", "Armenian"), ("id", "Indonesian"),
    ("is", "Icelandic"), ("it", "Italian"), ("ja", "Japanese"), ("jw", "Javanese"), ("ka", "Georgian"), ("kk", "Kazakh"),
    ("km", "Khmer"), ("kn", "Kannada"), ("ko", "Korean"), ("la", "Latin"), ("lb", "Luxembourgish"), ("ln", "Lingala"),
    ("lo", "Lao"), ("lt", "Lithuanian"), ("lv", "Latvian"), ("mg", "Malagasy"), ("mi", "Māori"), ("mk", "Macedonian"),
    ("ml", "Malayalam"), ("mn", "Mongolian"), ("mr", "Marathi"), ("ms", "Malay"), ("mt", "Maltese"), ("my", "Burmese"),
    ("ne", "Nepali"), ("nl", "Dutch"), ("nn", "Norwegian Nynorsk"), ("no", "Norwegian"), ("oc", "Occitan"), ("pa", "Punjabi"),
    ("pl", "Polish"), ("ps", "Pashto"), ("pt", "Portuguese"), ("ro", "Romanian"), ("ru", "Russian"), ("sa", "Sanskrit"),
    ("sd", "Sindhi"), ("si", "Sinhala"), ("sk", "Slovak"), ("sl", "Slovenian"), ("sn", "Shona"), ("so", "Somali"),
    ("sq", "Albanian"), ("sr", "Serbian"), ("su", "Sundanese"), ("sv", "Swedish"), ("sw", "Swahili"), ("ta", "Tamil"),
    ("te", "Telugu"), ("tg", "Tajik"), ("th", "Thai"), ("tk", "Turkmen"), ("tl", "Tagalog"), ("tr", "Turkish"), ("tt", "Tatar"),
    ("uk", "Ukrainian"), ("ur", "Urdu"), ("uz", "Uzbek"), ("vi", "Vietnamese"), ("yi", "Yiddish"), ("yo", "Yoruba"),
    ("yue", "Cantonese"), ("zh", "Chinese"),
];

/// The English name of `code`, if known.
pub fn english_name(code: &str) -> Option<&'static str> {
    NAMES.iter().find(|(known, _)| *known == code).map(|(_, name)| *name)
}
