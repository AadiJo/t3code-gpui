//! Port of `packages/client-runtime/src/listFormat.ts`.

use super::locale_format::{format_list, locale_data};

/// "A, B, and C" in `locale`, as `Intl.ListFormat(locale, {type: "conjunction"})` prints it.
pub fn format_conjunction_list(items: &[&str], locale: &str) -> String {
    format_list(items, locale_data(locale))
}
