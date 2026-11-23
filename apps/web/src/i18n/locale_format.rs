//! Locale patterns (from `locale_data.rs`) and the printers that apply them: the
//! subset of `Intl.DateTimeFormat`, `Intl.NumberFormat` and `Intl.ListFormat` the UI uses.

use super::locale_data::LOCALES;

/// CLDR data for one locale tag, read from V8's Intl by `locale_data.mjs`.
pub struct LocaleData {
    pub tag: &'static str,
    pub number: NumberSymbols,
    pub currency: CurrencyPattern,
    pub list: ListPattern,
    /// Whether `hour: "numeric"` resolves to a 12-hour clock.
    pub hour12: bool,
    /// First day of the week as a `Date#getDay` index (0 is Sunday).
    pub week_first_day: u8,
    /// `{hour: "numeric", minute: "2-digit"}` for the locale default, `hour12: true`, `hour12: false`.
    pub time: [&'static [Part]; 3],
    /// `Date#toLocaleString` for the locale default, `hour12: true`, `hour12: false`.
    pub full: [&'static [Part]; 3],
    pub month_day: &'static [Part],
    pub year_month_day: &'static [Part],
    /// `{day: "numeric", month: "long", year: "numeric"}`.
    pub long_date: &'static [Part],
}

/// Decimal formatting symbols for `Intl.NumberFormat(tag)`.
pub struct NumberSymbols {
    pub decimal: &'static str,
    pub group: &'static str,
    pub minus: &'static str,
    /// 2 when four-digit numbers stay ungrouped ("1234" in Spanish).
    pub min_grouping: u8,
    /// Groups of two above the thousands ("12,34,567").
    pub indian: bool,
}

/// Text around a USD amount with two fraction digits.
pub struct CurrencyPattern {
    pub prefix: &'static str,
    pub suffix: &'static str,
}

/// Separators of a conjunction list: `A{two}B`, `A{start}B{middle}C{end}D`.
pub struct ListPattern {
    pub two: &'static str,
    pub start: &'static str,
    pub middle: &'static str,
    pub end: &'static str,
}

/// One field or literal of a date/time pattern.
pub enum Part {
    Lit(&'static str),
    Hour(HourStyle),
    Minute { two_digit: bool },
    Second { two_digit: bool },
    Day { two_digit: bool },
    Month { two_digit: bool },
    MonthName(&'static [&'static str; 12]),
    Year,
    DayPeriod { am: &'static str, pm: &'static str },
}

/// The hour cycle and padding of an hour field.
#[derive(Clone, Copy)]
pub enum HourStyle {
    H23,
    H23Pad,
    H12,
    H12Pad,
    H11,
    H11Pad,
}

/// Wall-clock fields to print. `month` and `day` start at 1.
#[derive(Clone, Copy, Debug)]
pub struct WallClock {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Finds the data for a BCP 47 tag the way Intl falls back: the exact tag, then
/// with trailing subtags dropped, then English. `_` separators are accepted.
pub fn locale_data(tag: &str) -> &'static LocaleData {
    let normalized = tag.trim().replace('_', "-");
    let mut candidate = normalized.as_str();
    loop {
        if let Some(data) = LOCALES.iter().find(|l| l.tag.eq_ignore_ascii_case(candidate)) {
            return data;
        }
        match candidate.rfind('-') {
            Some(i) => candidate = &candidate[..i],
            None => return &LOCALES[0],
        }
    }
}

/// Prints a date/time pattern.
pub fn format_parts(parts: &[Part], clock: WallClock) -> String {
    let mut out = String::with_capacity(24);
    let pad = |out: &mut String, value: u8, two: bool| {
        if two && value < 10 {
            out.push('0');
        }
        out.push_str(&value.to_string());
    };
    for part in parts {
        match part {
            Part::Lit(text) => out.push_str(text),
            Part::Hour(style) => {
                let (value, two) = match style {
                    HourStyle::H23 => (clock.hour, false),
                    HourStyle::H23Pad => (clock.hour, true),
                    HourStyle::H12 | HourStyle::H12Pad => {
                        let h = clock.hour % 12;
                        (if h == 0 { 12 } else { h }, matches!(style, HourStyle::H12Pad))
                    }
                    HourStyle::H11 => (clock.hour % 12, false),
                    HourStyle::H11Pad => (clock.hour % 12, true),
                };
                pad(&mut out, value, two);
            }
            Part::Minute { two_digit } => pad(&mut out, clock.minute, *two_digit),
            Part::Second { two_digit } => pad(&mut out, clock.second, *two_digit),
            Part::Day { two_digit } => pad(&mut out, clock.day, *two_digit),
            Part::Month { two_digit } => pad(&mut out, clock.month, *two_digit),
            Part::MonthName(names) => out.push_str(names[usize::from(clock.month - 1)]),
            Part::Year => out.push_str(&clock.year.to_string()),
            Part::DayPeriod { am, pm } => out.push_str(if clock.hour < 12 { am } else { pm }),
        }
    }
    out
}

/// `Intl.NumberFormat(tag, {minimumFractionDigits, maximumFractionDigits}).format(value)`.
pub fn format_decimal(value: f64, symbols: &NumberSymbols, min_fraction: usize, max_fraction: usize) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    let mut out = String::new();
    if value.is_sign_negative() {
        out.push_str(symbols.minus);
    }
    if value.is_infinite() {
        out.push('∞');
        return out;
    }
    let (integer, fraction) = round_decimal(value.abs(), max_fraction);
    push_grouped(&mut out, &integer, symbols);
    let mut fraction = fraction;
    while fraction.len() < min_fraction {
        fraction.push('0');
    }
    if !fraction.is_empty() {
        out.push_str(symbols.decimal);
        out.push_str(&fraction);
    }
    out
}

/// Rounds the shortest decimal form of `value` half away from zero, as ICU does,
/// and drops trailing fraction zeros.
fn round_decimal(value: f64, max_fraction: usize) -> (String, String) {
    let text = value.to_string();
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let mut digits: Vec<u8> = integer.bytes().chain(fraction.bytes().take(max_fraction)).map(|b| b - b'0').collect();
    let round_up = fraction.as_bytes().get(max_fraction).is_some_and(|&b| b >= b'5');
    if round_up {
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, 1);
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let int_len = digits.len() - fraction.len().min(max_fraction);
    let to_string = |d: &[u8]| d.iter().map(|d| char::from(b'0' + d)).collect::<String>();
    let integer = to_string(&digits[..int_len]);
    let fraction = to_string(&digits[int_len..]).trim_end_matches('0').to_owned();
    let integer = match integer.trim_start_matches('0') {
        "" => "0".to_owned(),
        trimmed => trimmed.to_owned(),
    };
    (integer, fraction)
}

fn push_grouped(out: &mut String, integer: &str, symbols: &NumberSymbols) {
    let len = integer.len();
    if len < 4 || (symbols.min_grouping == 2 && len < 5) {
        out.push_str(integer);
        return;
    }
    // Positions (counted from the right) where a separator goes.
    let first = len - 3;
    let head = &integer[..first];
    let tail = &integer[first..];
    let step = if symbols.indian { 2 } else { 3 };
    let lead = head.len() % step;
    if lead > 0 {
        out.push_str(&head[..lead]);
    }
    for (i, chunk) in head.as_bytes()[lead..].chunks(step).enumerate() {
        if i > 0 || lead > 0 {
            out.push_str(symbols.group);
        }
        out.push_str(std::str::from_utf8(chunk).unwrap());
    }
    out.push_str(symbols.group);
    out.push_str(tail);
}

/// `Intl.ListFormat(tag, {type: "conjunction"}).format(items)`.
pub fn format_list(items: &[&str], data: &LocaleData) -> String {
    let pattern = &data.list;
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a}{}{b}", conjunction(data, pattern.two, b)),
        [first, middle @ .., last] => {
            let mut out = String::from(*first);
            for (i, item) in middle.iter().enumerate() {
                out.push_str(if i == 0 { pattern.start } else { pattern.middle });
                out.push_str(item);
            }
            out.push_str(&conjunction(data, pattern.end, last));
            out.push_str(last);
            out
        }
    }
}

/// Spanish writes "e" for "y" before an /i/ sound, as CLDR's contextual list rule does.
fn conjunction<'a>(data: &LocaleData, separator: &'a str, next: &str) -> std::borrow::Cow<'a, str> {
    let spanish = data.tag == "es" || data.tag.starts_with("es-");
    if spanish && separator == " y " && starts_with_i_sound(next) {
        return " e ".into();
    }
    separator.into()
}

fn starts_with_i_sound(word: &str) -> bool {
    let lower: String = word.chars().take(3).flat_map(char::to_lowercase).collect();
    let mut chars = lower.chars();
    match chars.next() {
        Some('i') => true,
        Some('h') => chars.next() == Some('i') && !matches!(chars.next(), Some('a' | 'e')),
        _ => false,
    }
}
