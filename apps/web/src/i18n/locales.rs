//! The UI languages and how the `language` setting picks one. Ports
//! `apps/web/src/i18n/locales.ts` and `packages/client-runtime/src/locales.ts`.

/// The `language` setting value that follows the host's language order.
pub const SYSTEM_LANGUAGE: &str = "system";

/// A UI language with a translation catalog. English is the source text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Locale {
    #[default]
    En,
    De,
    Es,
    Fr,
    Ja,
    Ko,
    PtBr,
    Ru,
    Sv,
    Zh,
}

impl Locale {
    /// Every UI language, English first, in `gt.config.json` order.
    pub const ALL: [Locale; 10] = [
        Self::En,
        Self::De,
        Self::Es,
        Self::Fr,
        Self::Ja,
        Self::Ko,
        Self::PtBr,
        Self::Ru,
        Self::Sv,
        Self::Zh,
    ];

    /// The catalog's BCP 47 tag, also the saved `language` setting value.
    pub fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::De => "de",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::Ja => "ja",
            Self::Ko => "ko",
            Self::PtBr => "pt-BR",
            Self::Ru => "ru",
            Self::Sv => "sv",
            Self::Zh => "zh",
        }
    }

    /// The exact catalog tag, or `None`.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.tag() == tag)
    }

    /// The language's name in itself, so a user can find theirs from any UI language.
    pub fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::De => "Deutsch",
            Self::Es => "Español",
            Self::Fr => "Français",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
            Self::PtBr => "Português (Brasil)",
            Self::Ru => "Русский",
            Self::Sv => "Svenska",
            Self::Zh => "中文（简体）",
        }
    }
}

/// The catalog to load, plus the matching system tag for regional formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocaleSelection {
    pub catalog: Locale,
    pub formatting: String,
}

/// Saved setting to active locale; "system" and unknown values follow the host's languages.
pub fn resolve_locale(language: &str, system_locales: &[String]) -> Locale {
    resolve_locale_selection(language, system_locales).catalog
}

/// `resolveLocaleSelection` in `client-runtime/src/locales.ts`.
pub fn resolve_locale_selection(language: &str, system_locales: &[String]) -> LocaleSelection {
    if let Some(locale) = Locale::from_tag(language) {
        return LocaleSelection {
            catalog: locale,
            formatting: language.to_owned(),
        };
    }
    for tag in system_locales {
        if let Some(locale) = match_supported_locale(tag) {
            return LocaleSelection {
                catalog: locale,
                formatting: tag.trim().replace('_', "-"),
            };
        }
    }
    LocaleSelection {
        catalog: Locale::En,
        formatting: Locale::En.tag().to_owned(),
    }
}

fn match_supported_locale(tag: &str) -> Option<Locale> {
    let normalized = tag.trim().to_lowercase();
    let exact = Locale::ALL.into_iter().find(|l| l.tag().to_lowercase() == normalized);
    // Our `zh` is Simplified, so Traditional tags only match an exact locale.
    if exact.is_some() || is_traditional_chinese(&normalized) {
        return exact;
    }
    let language = normalized.split(['-', '_']).next().unwrap_or_default();
    Locale::ALL.into_iter().find(|l| l.tag().to_lowercase() == language)
}

/// `/^zh[-_](hant|tw|hk|mo)\b/` on a lowercased tag.
fn is_traditional_chinese(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix("zh-").or_else(|| tag.strip_prefix("zh_")) else {
        return false;
    };
    ["hant", "tw", "hk", "mo"].iter().any(|subtag| {
        rest.strip_prefix(subtag)
            .is_some_and(|after| after.chars().next().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_')))
    })
}
