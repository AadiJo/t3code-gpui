//! Ranks a thread search's message matches as the web's sidebar and palette do
//! (`rankMessageMatches` in `paletteSearch.ts`), across environments.

use t3code_contracts::OrchestrationThreadSearchMatch;
use t3code_shared::palette_search::{ThreadSearchSubject, thread_usage_boost};

/// `MESSAGE_PREFIX`: a match at least this good is a word match, not a prefix.
const MESSAGE_PREFIX: f64 = 0.8;

/// `messageMatchWeight`.
fn message_match_weight(hits: i64) -> f64 {
    1.0 + (1.0 + hits as f64).log2()
}

/// `rankMessageMatches` with `{ picks: false }` (a list filter): word matches first, then the
/// closer match, then hits weighted by the thread's recent use, then the newer message. Matches
/// whose thread has no subject are dropped.
pub fn rank_message_matches<'a, T>(
    matches: impl IntoIterator<Item = (T, &'a OrchestrationThreadSearchMatch)>,
    subject_of: impl Fn(&T) -> Option<&'a ThreadSearchSubject>,
    now_ms: i64,
) -> Vec<T> {
    let mut ranked: Vec<(T, f64, f64, &str)> = matches
        .into_iter()
        .filter_map(|(item, m)| {
            let subject = subject_of(&item)?;
            // Older servers send no quality; their matches count as words.
            let quality = m.quality.unwrap_or(1.0);
            let score = message_match_weight(m.hits.unwrap_or(1)) * thread_usage_boost(subject, now_ms);
            Some((item, quality, score, m.message_created_at.as_deref().unwrap_or("")))
        })
        .collect();
    ranked.sort_by(|left, right| {
        let word = |q: f64| u8::from(q >= MESSAGE_PREFIX);
        word(right.1)
            .cmp(&word(left.1))
            .then(right.1.total_cmp(&left.1))
            .then(right.2.total_cmp(&left.2))
            .then(right.3.cmp(left.3))
    });
    ranked.into_iter().map(|(item, ..)| item).collect()
}
