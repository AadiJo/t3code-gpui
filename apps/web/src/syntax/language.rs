//! File path → `@pierre/highlights` lexer, as `getFiletypeFromFileName` in
//! `@pierre/diffs` resolves it, with the app's custom extensions
//! (`lib/syntaxHighlighting.ts`: luau → lua, xsl/xslt → xml).

use super::pierre_tables::{EXTENSIONS, LANGUAGE_ALIASES, LANGUAGE_NAMES};

/// A lexer of `@pierre/highlights`, by its id. `Copy` and cheap to hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LanguageId(pub(super) u8);

impl LanguageId {
    /// The lexer's name, such as "rust".
    pub fn name(self) -> &'static str {
        LANGUAGE_NAMES.get(self.0 as usize).copied().unwrap_or("text")
    }
}

fn lookup(key: &str) -> Option<u8> {
    EXTENSIONS.binary_search_by(|(name, _)| (*name).cmp(key)).ok().map(|index| EXTENSIONS[index].1)
}

/// Exact name, then a two-part extension (`blade.php`), then the last extension.
fn match_name(name: &str) -> Option<u8> {
    if let Some(id) = lookup(name) {
        return Some(id);
    }
    let last = name.rfind('.')?;
    // `\.([^/\\]+\.[^/\\]+)$`: from the first dot of the last segment that has another dot after it.
    let segment = name.rfind(['/', '\\']).map_or(0, |at| at + 1);
    if let Some(first) = name[segment..].find('.').map(|at| segment + at).filter(|at| *at < last) {
        if let Some(id) = lookup(&name[first + 1..]) {
            return Some(id);
        }
    }
    lookup(&name[last + 1..])
}

/// The lexer for a file path, or `None` for plain text.
pub fn language_for_path(path: &str) -> Option<LanguageId> {
    let found = match_name(path).or_else(|| {
        let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
        (base != path).then(|| match_name(base)).flatten()
    })?;
    Some(LanguageId(found))
}

/// The lexer for a language name or alias (`langIdOf`), case-insensitive.
/// `None` for unknown names and for the plain-text ids, which draw uncoloured.
pub fn language_for_name(name: &str) -> Option<LanguageId> {
    let name = name.to_lowercase();
    let index = LANGUAGE_ALIASES.binary_search_by(|(alias, _)| (*alias).cmp(name.as_str())).ok()?;
    Some(LanguageId(LANGUAGE_ALIASES[index].1)).filter(|id| id.0 != 0)
}
