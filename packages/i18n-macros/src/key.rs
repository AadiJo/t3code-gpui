//! Catalog keys, computed the way `generaltranslation@9.5.9` `hashSource` does: the
//! first 16 hex digits of SHA-256 over the key-sorted JSON of
//! `{context?, dataFormat, source}`.

use sha2::{Digest, Sha256};

/// How gt hashed an entry: `msg()`/`gt()` strings are ICU, `<T>` children are JSX.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DataFormat {
    Icu,
    Jsx,
}

/// The key for an ICU source or a plain-text `<T>` child, as a `u64`.
pub fn text_key(source: &str, context: Option<&str>, format: DataFormat) -> u64 {
    key_for(&json_string(source), context, format)
}

/// The key for `<T>` children whose sanitized JSON is `sanitized_source`.
pub fn jsx_key(sanitized_source: &str, context: Option<&str>) -> u64 {
    key_for(sanitized_source, context, DataFormat::Jsx)
}

fn key_for(source_json: &str, context: Option<&str>, format: DataFormat) -> u64 {
    // Keys sort as context < dataFormat < source.
    let mut json = String::from("{");
    if let Some(context) = context.filter(|c| !c.is_empty()) {
        json.push_str("\"context\":");
        json.push_str(&json_string(context));
        json.push(',');
    }
    json.push_str(match format {
        DataFormat::Icu => "\"dataFormat\":\"ICU\"",
        DataFormat::Jsx => "\"dataFormat\":\"JSX\"",
    });
    json.push_str(",\"source\":");
    json.push_str(source_json);
    json.push('}');
    let digest = Sha256::digest(json.as_bytes());
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

/// `JSON.stringify` of a string. serde_json escapes the same characters.
pub fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

/// Parses a catalog key (16 hex digits) into the `u64` the macros emit.
pub fn parse_key(hex: &str) -> Option<u64> {
    (hex.len() == 16).then(|| u64::from_str_radix(hex, 16).ok()).flatten()
}
