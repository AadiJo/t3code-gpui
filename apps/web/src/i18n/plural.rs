//! CLDR cardinal plural rules for the UI languages, matching `Intl.PluralRules`.

use super::locales::Locale;

/// A CLDR plural category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluralCategory {
    One,
    Few,
    Many,
    Other,
}

impl PluralCategory {
    /// The ICU selector keyword.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::One => "one",
            Self::Few => "few",
            Self::Many => "many",
            Self::Other => "other",
        }
    }
}

/// `new Intl.PluralRules(locale).select(n)`.
pub fn plural_category(locale: Locale, n: f64) -> PluralCategory {
    if !n.is_finite() {
        return PluralCategory::Other;
    }
    let ops = Operands::new(n);
    let million = ops.v == 0 && ops.i != 0 && ops.i % 1_000_000 == 0;
    match locale {
        Locale::En | Locale::De | Locale::Sv => {
            if ops.i == 1 && ops.v == 0 {
                PluralCategory::One
            } else {
                PluralCategory::Other
            }
        }
        Locale::Es => {
            if ops.n == 1.0 {
                PluralCategory::One
            } else if million {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }
        Locale::Fr | Locale::PtBr => {
            if ops.i <= 1 {
                PluralCategory::One
            } else if million {
                PluralCategory::Many
            } else {
                PluralCategory::Other
            }
        }
        Locale::Ru => {
            let (i10, i100) = (ops.i % 10, ops.i % 100);
            if ops.v != 0 {
                PluralCategory::Other
            } else if i10 == 1 && i100 != 11 {
                PluralCategory::One
            } else if (2..=4).contains(&i10) && !(12..=14).contains(&i100) {
                PluralCategory::Few
            } else {
                PluralCategory::Many
            }
        }
        Locale::Ja | Locale::Ko | Locale::Zh => PluralCategory::Other,
    }
}

/// CLDR operands of the number as `Intl.PluralRules` formats it (at most three
/// fraction digits, trailing zeros dropped).
struct Operands {
    n: f64,
    i: u64,
    v: usize,
}

impl Operands {
    fn new(value: f64) -> Self {
        let abs = value.abs();
        let rounded = format!("{abs:.3}");
        let (int, frac) = rounded.split_once('.').unwrap_or((&rounded, ""));
        let frac = frac.trim_end_matches('0');
        Self {
            n: rounded.parse().unwrap_or(abs),
            i: int.parse().unwrap_or(u64::MAX),
            v: frac.len(),
        }
    }
}
