//! Fractional order keys for the sidebar's Pinned section (`pinOrderKey.ts`). Keys are
//! lowercase letter strings compared as plain strings; a smaller key sorts higher.

const DIGITS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

fn digit(byte: u8) -> usize {
    (byte - b'a') as usize
}

fn is_valid(key: &str) -> bool {
    // A trailing minimum digit leaves no room for a key just before this one.
    !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_lowercase()) && !key.ends_with('a')
}

/// Midpoint of two digit strings read as fractions in (0, 1); "" is the open bound.
fn midpoint(a: &str, b: &str) -> String {
    if !b.is_empty() {
        let (a_bytes, b_bytes) = (a.as_bytes(), b.as_bytes());
        let mut n = 0;
        while n < b_bytes.len() && a_bytes.get(n).copied().unwrap_or(b'a') == b_bytes[n] {
            n += 1;
        }
        if n > 0 {
            return format!("{}{}", &b[..n], midpoint(a.get(n..).unwrap_or(""), &b[n..]));
        }
    }
    let digit_a = a.bytes().next().map_or(0, digit);
    let digit_b = b.bytes().next().map_or(DIGITS.len(), digit);
    if digit_b - digit_a > 1 {
        let middle = ((digit_a + digit_b) as f64 / 2.0).round() as usize;
        return (DIGITS[middle] as char).to_string();
    }
    if b.len() > 1 {
        return b[..1].to_owned();
    }
    format!("{}{}", DIGITS[digit_a] as char, midpoint(a.get(1..).unwrap_or(""), ""))
}

/// A key strictly between two neighbours; `None` bounds mean the top of the block or the end of
/// the keyed run. `None` when existing keys are corrupt or out of order (`pinOrderKeyBetween`).
pub fn pin_order_key_between(before: Option<&str>, after: Option<&str>) -> Option<String> {
    let (a, b) = (before.unwrap_or(""), after.unwrap_or(""));
    if (!a.is_empty() && !is_valid(a)) || (!b.is_empty() && !is_valid(b)) || (!b.is_empty() && a >= b) {
        return None;
    }
    Some(midpoint(a, b))
}

/// The key for a newly pinned thread at the top of the pinned run (`topOfPinnedRunOrderKey`).
pub fn top_of_pinned_run_order_key<'a>(existing: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    let first = existing.into_iter().flatten().min();
    pin_order_key_between(None, first)
}

/// `count` evenly spread keys for rewriting a whole section (`generateSpreadPinOrderKeys`).
pub fn generate_spread_pin_order_keys(count: usize) -> Vec<String> {
    let base = DIGITS.len() as u64;
    let mut width = 2u32;
    let mut space = base.pow(width);
    while space <= (count as u64 + 1) * 2 {
        width += 1;
        space *= base;
    }
    let step = space as f64 / (count as f64 + 1.0);
    (0..count)
        .map(|i| {
            let mut value = (step * (i as f64 + 1.0)).round() as u64;
            if value % base == 0 {
                value += 1;
            }
            let mut key = vec![0u8; width as usize];
            for slot in key.iter_mut().rev() {
                *slot = DIGITS[(value % base) as usize];
                value /= base;
            }
            String::from_utf8(key).expect("ascii")
        })
        .collect()
}
