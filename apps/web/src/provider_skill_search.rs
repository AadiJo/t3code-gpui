//! Ranked search over a provider's skills for the slash menu. Ports
//! `providerSkillSearch.ts` with the parts of `shared/searchRanking.ts`,
//! `providerSkills.ts` and `inlineSkills.ts` it uses. Strings index by UTF-16
//! unit, as JavaScript does.

use std::collections::HashSet;

use t3code_contracts::ServerConfigProvidersItemSkillsItem as ServerProviderSkill;

/// `formatProviderSkillDisplayName`.
pub fn format_provider_skill_display_name(skill: &ServerProviderSkill) -> &str {
    skill.display_name.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or(&skill.name)
}

/// `isProviderSkillUserInvocable`: enabled and not agent-only.
pub fn is_provider_skill_user_invocable(skill: &ServerProviderSkill) -> bool {
    skill.enabled && skill.user_invocable != Some(false)
}

/// `dedupeProviderSkillsByName`: the first skill of each trimmed, lowercased name.
pub fn dedupe_provider_skills_by_name<'a>(skills: impl IntoIterator<Item = &'a ServerProviderSkill>) -> Vec<&'a ServerProviderSkill> {
    let mut seen = HashSet::new();
    skills.into_iter().filter(|skill| seen.insert(skill.name.trim().to_lowercase())).collect()
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn index_of(haystack: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn score_subsequence_match(value: &[u16], query: &[u16]) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let (mut qi, mut first, mut prev, mut gap) = (0usize, None::<usize>, None::<usize>, 0i64);
    for (vi, unit) in value.iter().enumerate() {
        if *unit != query[qi] {
            continue;
        }
        let start = *first.get_or_insert(vi);
        if let Some(p) = prev {
            gap += (vi - p - 1) as i64;
        }
        prev = Some(vi);
        qi += 1;
        if qi == query.len() {
            let span = (vi - start + 1 - query.len()) as i64;
            let length = (value.len() as i64 - query.len() as i64).min(64);
            return Some(start as i64 * 2 + gap * 3 + span + length);
        }
    }
    None
}

/// The bases one field scores with (`scoreQueryMatch` input).
pub struct Bases {
    pub exact: i64,
    pub prefix: Option<i64>,
    pub boundary: Option<i64>,
    pub includes: Option<i64>,
    pub fuzzy: Option<i64>,
    pub markers: &'static [&'static str],
}

/// `scoreQueryMatch`: lower is better; `value` and `query` are trimmed and lowercased.
pub fn score_query_match(value: &str, query: &str, bases: &Bases) -> Option<i64> {
    if value.is_empty() || query.is_empty() {
        return None;
    }
    let (v, q) = (utf16(value), utf16(query));
    let penalty = (v.len() as i64 - q.len() as i64).clamp(0, 64);
    if v == q {
        return Some(bases.exact);
    }
    if let Some(prefix) = bases.prefix
        && v.starts_with(&q)
    {
        return Some(prefix + penalty);
    }
    if let Some(boundary) = bases.boundary {
        let best = bases
            .markers
            .iter()
            .filter_map(|marker| {
                let m = utf16(marker);
                let needle: Vec<u16> = m.iter().chain(&q).copied().collect();
                index_of(&v, &needle).map(|i| i + m.len())
            })
            .min();
        if let Some(index) = best {
            return Some(boundary + index as i64 * 2 + penalty);
        }
    }
    if let Some(includes) = bases.includes
        && let Some(index) = index_of(&v, &q)
    {
        return Some(includes + index as i64 * 2 + penalty);
    }
    bases.fuzzy.and_then(|fuzzy| score_subsequence_match(&v, &q).map(|s| fuzzy + s))
}

pub const DEFAULT_MARKERS: &[&str] = &[" ", "-", "_", "/"];

/// `scoreProviderSkill`: the best score over name, label, descriptions and scope (lower is better).
pub fn score_provider_skill(skill: &ServerProviderSkill, query: &str) -> Option<i64> {
    let name = skill.name.to_lowercase();
    let label = format_provider_skill_display_name(skill).to_lowercase();
    let short = skill.short_description.as_deref().unwrap_or("").to_lowercase();
    let description = skill.description.as_deref().unwrap_or("").to_lowercase();
    let scope = skill.scope.as_deref().unwrap_or("").to_lowercase();
    let fields = [
        (&name, Bases { exact: 0, prefix: Some(2), boundary: Some(4), includes: Some(6), fuzzy: Some(100), markers: &["-", "_", "/"] }),
        (&label, Bases { exact: 1, prefix: Some(3), boundary: Some(5), includes: Some(7), fuzzy: Some(110), markers: DEFAULT_MARKERS }),
        (&short, Bases { exact: 20, prefix: Some(22), boundary: Some(24), includes: Some(26), fuzzy: None, markers: DEFAULT_MARKERS }),
        (&description, Bases { exact: 30, prefix: Some(32), boundary: Some(34), includes: Some(36), fuzzy: None, markers: DEFAULT_MARKERS }),
        (&scope, Bases { exact: 40, prefix: Some(42), boundary: None, includes: Some(44), fuzzy: None, markers: DEFAULT_MARKERS }),
    ];
    fields.iter().filter_map(|(value, bases)| score_query_match(value, query, bases)).min()
}

/// `normalizeSearchQuery` with leading currency symbols (the `$skill` sigil) removed.
fn normalize_query(input: &str) -> String {
    use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};
    let trimmed = input.trim();
    trimmed.trim_start_matches(|c: char| c.general_category() == GeneralCategory::CurrencySymbol).to_lowercase()
}

/// `searchProviderSkills`: user-invocable skills ranked by score, then display name and name.
pub fn search_provider_skills<'a>(skills: &'a [ServerProviderSkill], query: &str, limit: usize) -> Vec<&'a ServerProviderSkill> {
    let enabled = dedupe_provider_skills_by_name(skills.iter().filter(|s| is_provider_skill_user_invocable(s)));
    let query = normalize_query(query);
    if query.is_empty() {
        return enabled;
    }
    let mut ranked: Vec<(i64, String, &ServerProviderSkill)> = Vec::new();
    for skill in enabled {
        let Some(score) = score_provider_skill(skill, &query) else { continue };
        let tie = format!("{}\u{0}{}", format_provider_skill_display_name(skill).to_lowercase(), skill.name);
        // Stable insertion after equal keys, as `findInsertionIndex` does.
        let at = ranked.partition_point(|(s, t, _)| (*s, t.as_str()) <= (score, tie.as_str()));
        if ranked.len() < limit {
            ranked.insert(at, (score, tie, skill));
        } else if at < limit {
            ranked.insert(at, (score, tie, skill));
            ranked.pop();
        }
    }
    ranked.into_iter().map(|(_, _, skill)| skill).collect()
}
