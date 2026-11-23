//! Model picker search ranking. Ports `packages/client-runtime/src/modelSearch.ts`
//! and `scoreQueryMatch` from `packages/shared/src/searchRanking.ts`. Lower
//! scores rank first.

/// The fields a model is searched by.
pub struct SearchableModel<'a> {
    pub driver_kind: &'a str,
    pub provider_display_name: &'a str,
    pub name: &'a str,
    pub short_name: Option<&'a str>,
    pub sub_provider: Option<&'a str>,
    pub is_favorite: bool,
}

const FAVORITE_SCORE_BOOST: i64 = 24;
const EXACT_NAME_SCORE: i64 = -1000;
const BOUNDARY_MARKERS: [char; 4] = [' ', '-', '_', '/'];

fn normalize(input: &str) -> String {
    input.trim().to_lowercase()
}

fn subsequence_score(value: &str, query: &str) -> Option<i64> {
    let value: Vec<char> = value.chars().collect();
    let query: Vec<char> = query.chars().collect();
    if query.is_empty() {
        return Some(0);
    }
    let (mut qi, mut first, mut prev, mut gap) = (0, None, None::<usize>, 0i64);
    for (vi, c) in value.iter().enumerate() {
        if *c != query[qi] {
            continue;
        }
        first.get_or_insert(vi);
        if let Some(p) = prev {
            gap += (vi - p - 1) as i64;
        }
        prev = Some(vi);
        qi += 1;
        if qi == query.len() {
            let span = (vi - first.unwrap_or(vi) + 1 - query.len()) as i64;
            return Some(gap * 3 + span);
        }
    }
    None
}

fn boundary_match(value: &str, query: &str) -> bool {
    BOUNDARY_MARKERS.iter().any(|m| value.contains(&format!("{m}{query}")))
}

/// `scoreQueryMatch` with `ignorePosition: true`.
fn score_token(field: &str, token: &str, base: i64) -> Option<i64> {
    if field.is_empty() || token.is_empty() {
        return None;
    }
    if field == token {
        return Some(base);
    }
    if field.starts_with(token) {
        return Some(base + 2);
    }
    if boundary_match(field, token) {
        return Some(base + 4);
    }
    if field.contains(token) {
        return Some(base + 6);
    }
    if token.chars().count() >= 3 {
        return subsequence_score(field, token).map(|s| base + 100 + s);
    }
    None
}

/// A model's score for a query, or `None` when some word matches nothing.
/// Ports `scoreModelPickerSearch`.
pub fn score_model_search(model: &SearchableModel<'_>, query: &str) -> Option<i64> {
    let tokens: Vec<String> = normalize(query).split_whitespace().map(str::to_owned).collect();
    if tokens.is_empty() {
        return Some(0);
    }
    let joined = tokens.join(" ");
    let collapse = |s: &str| normalize(s).split_whitespace().collect::<Vec<_>>().join(" ");
    if joined == collapse(model.name) || model.short_name.is_some_and(|s| joined == collapse(s)) {
        return Some(EXACT_NAME_SCORE);
    }
    let all = [Some(model.name), model.short_name, model.sub_provider, Some(model.driver_kind), Some(model.provider_display_name)]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut fields = vec![normalize(model.name)];
    fields.extend(model.short_name.map(normalize));
    fields.extend(model.sub_provider.map(normalize));
    fields.push(normalize(model.driver_kind));
    fields.push(normalize(model.provider_display_name));
    fields.push(normalize(&all));
    let mut score = 0;
    for token in &tokens {
        score += fields
            .iter()
            .enumerate()
            .filter_map(|(i, field)| score_token(field, token, i as i64 * 10))
            .min()?;
    }
    Some(if model.is_favorite { score - FAVORITE_SCORE_BOOST } else { score })
}

/// Ranks items best first, favourites before ties, then original order.
/// Ports `rankModelPickerSearch`.
pub fn rank_model_search<T>(items: Vec<T>, query: &str, searchable: impl Fn(&T) -> SearchableModel<'_>) -> Vec<T> {
    let mut ranked: Vec<(i64, bool, usize, T)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(order, item)| {
            let s = searchable(&item);
            let favorite = s.is_favorite;
            let score = score_model_search(&s, query)?;
            Some((score, favorite, order, item))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    ranked.into_iter().map(|(.., item)| item).collect()
}
