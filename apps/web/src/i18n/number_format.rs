//! The number formatting the usage views use: ports of `formatUsd`, `formatCount`,
//! `formatTokens` and `formatPercent` in `packages/shared/src/usageFormat.ts`.

use super::locale_format::{format_decimal, locale_data};

/// Clients pass their UI locale; English (and no locale) keeps en-US.
fn format_locale(locale: Option<&str>) -> &str {
    match locale {
        None | Some("en") => "en-US",
        Some(tag) => tag,
    }
}

/// "$1,234.50" in the locale's currency layout.
pub fn format_usd(value: f64, locale: Option<&str>) -> String {
    let data = locale_data(format_locale(locale));
    let number = format_decimal(value.abs(), &data.number, 2, 2);
    let sign = if value.is_sign_negative() && value != 0.0 { data.number.minus } else { "" };
    format!("{sign}{}{number}{}", data.currency.prefix, data.currency.suffix)
}

/// A rounded, grouped integer: "1,235".
pub fn format_count(value: f64, locale: Option<&str>) -> String {
    let data = locale_data(format_locale(locale));
    format_decimal((value + 0.5).floor(), &data.number, 0, 3)
}

const TOKEN_SUFFIXES: [&str; 4] = ["K", "M", "B", "T"];

/// A compact token count: "999", "1.2K", "35M".
pub fn format_tokens(value: f64) -> String {
    let rounded = (value + 0.5).floor();
    if rounded.abs() < 1e3 {
        return format_decimal(rounded, &locale_data("en-US").number, 0, 3);
    }
    let mut scaled = value / 1e3;
    let mut unit = 0;
    // Choose the unit after rounding, so 999,999 reads 1M rather than 1000K.
    while trim(scaled).parse::<f64>().unwrap_or(0.0).abs() >= 1e3 && unit < TOKEN_SUFFIXES.len() - 1 {
        scaled /= 1e3;
        unit += 1;
    }
    format!("{}{}", trim(scaled), TOKEN_SUFFIXES[unit])
}

fn trim(value: f64) -> String {
    let abs = value.abs();
    let digits = if abs >= 100.0 { 0 } else if abs >= 10.0 { 1 } else { 2 };
    let fixed = js_to_fixed(value, digits);
    match fixed.split_once('.') {
        Some((int, frac)) if frac.bytes().all(|b| b == b'0') => int.to_owned(),
        _ => fixed,
    }
}

/// A share as a percentage: "12.5%", or "<0.1%" for a tiny non-zero share.
pub fn format_percent(share: f64, digits: usize) -> String {
    let percent = share * 100.0;
    let smallest = 10f64.powi(-(digits as i32));
    if percent > 0.0 && percent < smallest {
        return format!("<{}%", js_to_fixed(smallest, digits));
    }
    format!("{}%", js_to_fixed(percent, digits))
}

/// `Number.prototype.toFixed`: rounds the exact binary value, ties away from zero.
pub fn js_to_fixed(value: f64, digits: usize) -> String {
    if !value.is_finite() || value.abs() >= 1e21 {
        return super::messages::js_number_string(value);
    }
    // Enough extra digits to tell a true tie (an exact ...5000…) from a near one.
    let wide = format!("{:.*}", digits + 30, value.abs());
    let cut = wide.len() - 30;
    let (kept, extra) = wide.split_at(cut);
    let round_up = extra.as_bytes()[0] >= b'5';
    let mut digits_vec: Vec<u8> = kept.bytes().collect();
    if round_up {
        let mut i = digits_vec.len();
        loop {
            if i == 0 {
                digits_vec.insert(0, b'1');
                break;
            }
            i -= 1;
            match digits_vec[i] {
                b'.' => continue,
                b'9' => digits_vec[i] = b'0',
                d => {
                    digits_vec[i] = d + 1;
                    break;
                }
            }
        }
    }
    let mut out = String::from_utf8(digits_vec).unwrap();
    if out.ends_with('.') {
        out.pop();
    }
    if value < 0.0 {
        out.insert(0, '-');
    }
    out
}
