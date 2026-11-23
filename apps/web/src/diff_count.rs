//! Diff count labels. Ports `packages/client-runtime/src/diffCount.ts` and
//! `formatCompactDiffCount` from `components/chat/DiffStatLabel.tsx`.

/// A count with thousands separators, as `Intl.NumberFormat` writes it in English.
pub fn format_diff_count(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A short count: `999`, `1.2k`, `12k`, `3.4m`.
pub fn format_compact_diff_count(value: u64) -> String {
    let scaled = |value: f64, suffix: &str| {
        if value < 10. {
            let text = format!("{value:.1}");
            format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
        } else {
            format!("{}{suffix}", value.round() as u64)
        }
    };
    match value {
        0..1_000 => value.to_string(),
        1_000..999_500 => scaled(value as f64 / 1e3, "k"),
        999_500..999_500_000 => scaled(value as f64 / 1e6, "m"),
        _ => scaled(value as f64 / 1e9, "b"),
    }
}
