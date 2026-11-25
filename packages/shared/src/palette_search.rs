//! Ranking for the command palette and thread search. Ports
//! `packages/shared/src/paletteSearch.ts` (titles, keywords, threads, group
//! ranking). Server message search is not ported: the server grades message
//! hits and the palette only lists them.
//!
//! Offsets are in `char`s of the source text. Folding maps one source char to
//! one folded char, so a range in the folded text is a range in the source.

use std::sync::{Arc, OnceLock};

use icu_normalizer::DecomposingNormalizerBorrowed;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory as _};

/// A matched `[start, end)` range, in chars of the searched text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchRange {
    pub start: usize,
    pub end: usize,
}

/// Folded text and its word starts, built once per string.
#[derive(Debug)]
pub struct SearchField {
    /// Case- and accent-folded text, one char per source char.
    text: Box<[char]>,
    /// True where a word starts in `text`.
    word_starts: Box<[bool]>,
}

/// A searchable string that folds itself on first use and keeps the result,
/// so text kept across keystrokes (titles, branches) folds once.
#[derive(Debug, Default)]
pub struct SearchText {
    value: Arc<str>,
    field: OnceLock<SearchField>,
}

impl Clone for SearchText {
    fn clone(&self) -> Self {
        Self::new(self.value.clone())
    }
}

impl SearchText {
    pub fn new(value: impl Into<Arc<str>>) -> Self {
        Self { value: value.into(), field: OnceLock::new() }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub fn arc(&self) -> &Arc<str> {
        &self.value
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    fn field(&self) -> &SearchField {
        self.field.get_or_init(|| fold_search_field(&self.value))
    }
}

fn is_alphanumeric(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

fn is_symbol(c: char) -> bool {
    !c.is_ascii() && c.general_category() == GeneralCategory::OtherSymbol
}

fn is_combining_mark(c: char) -> bool {
    matches!(
        c.general_category(),
        GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark
    )
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

fn fold_character(c: char) -> Option<char> {
    let nfkd = DecomposingNormalizerBorrowed::new_nfkd();
    let mut buffer = [0u8; 4];
    let decomposed = nfkd.normalize(c.encode_utf8(&mut buffer));
    let mut folded = decomposed.chars().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase);
    let first = folded.next()?;
    if folded.next().is_some() {
        return None;
    }
    Some(match first {
        'ς' => 'σ',
        '\u{2018}' | '\u{2019}' => '\'',
        other => other,
    })
}

/// One char folded in place: case and accents go; a char that folds to more
/// or less than one char stays as it is (`foldInPlace`).
fn fold_char_in_place(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let folded = fold_character(c).unwrap_or(c);
    let mut lower = folded.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => folded,
    }
}

fn fold_in_place(text: &str) -> String {
    text.chars().map(fold_char_in_place).collect()
}

/// Chinese and Japanese text has no spaces, so each ideograph or kana starts a word.
fn is_cjk(c: char) -> bool {
    let code = c as u32;
    (0x3040..=0x30ff).contains(&code)
        || (0x3400..=0x4dbf).contains(&code)
        || (0x4e00..=0x9fff).contains(&code)
        || (0xf900..=0xfaff).contains(&code)
}

/// Whether `c` starts a word after `previous` and `before_previous`: after a
/// non-letter, at a camelCase or digit boundary, or at a CJK character.
fn is_word_start(before_previous: Option<char>, previous: Option<char>, c: char, next: Option<char>) -> bool {
    let after_apostrophe = previous.is_some_and(is_apostrophe) && before_previous.is_some_and(is_alphanumeric);
    if !(is_alphanumeric(c) || is_symbol(c)) || after_apostrophe {
        return false;
    }
    let Some(previous) = previous else { return true };
    !is_alphanumeric(previous)
        || (previous.is_lowercase() && c.is_uppercase())
        || (previous.is_uppercase() && c.is_uppercase() && next.is_some_and(char::is_lowercase))
        || previous.is_numeric() != c.is_numeric()
        || is_cjk(c)
}

fn fold_search_field(source: &str) -> SearchField {
    let chars: Vec<char> = source.chars().collect();
    let word_starts = (0..chars.len())
        .map(|i| {
            let before_previous = i.checked_sub(2).map(|j| chars[j]);
            let previous = i.checked_sub(1).map(|j| chars[j]);
            is_word_start(before_previous, previous, chars[i], chars.get(i + 1).copied())
        })
        .collect();
    SearchField { text: chars.iter().map(|c| fold_char_in_place(*c)).collect(), word_starts }
}

fn is_wrapping_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`' | '\u{201c}' | '\u{201d}' | '\u{2018}' | '\u{2019}')
}

/// The folded, whitespace-collapsed query; empty when nothing is left to
/// match (`foldSearchQuery`). Quotes around a pasted query are dropped.
pub fn fold_search_query(query: &str) -> String {
    let trimmed = query.trim_start().trim_start_matches(is_wrapping_quote);
    let trimmed = trimmed.trim_end().trim_end_matches(is_wrapping_quote);
    let folded = fold_in_place(trimmed);
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn find_from(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    let last_start = haystack.len() - needle.len();
    let mut index = from;
    while index <= last_start {
        if haystack[index] == first && haystack[index..index + needle.len()] == *needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn starts_with_at(haystack: &[char], needle: &[char], at: usize) -> bool {
    haystack.len() >= at + needle.len() && haystack[at..at + needle.len()] == *needle
}

fn word_end(field: &SearchField, index: usize) -> usize {
    let mut end = index + 1;
    while end < field.text.len() && !field.word_starts[end] && is_alphanumeric(field.text[end]) {
        end += 1;
    }
    end
}

/// One substitution, insertion, deletion, or swap of neighbours; with `swap_only`, only a swap.
fn within_one_edit(left: &[char], right: &[char], swap_only: bool) -> bool {
    if left.len().abs_diff(right.len()) > if swap_only { 0 } else { 1 } {
        return false;
    }
    let mut index = 0;
    while index < left.len() && index < right.len() && left[index] == right[index] {
        index += 1;
    }
    let (rest_left, rest_right) = (&left[index..], &right[index..]);
    let swapped = rest_left.len() > 1
        && rest_right.len() > 1
        && rest_left[0] == rest_right[1]
        && rest_left[1] == rest_right[0]
        && rest_left[2..] == rest_right[2..];
    if swap_only {
        return swapped;
    }
    fn tail(s: &[char]) -> &[char] {
        s.get(1..).unwrap_or(s)
    }
    swapped
        || rest_left == rest_right
        || tail(rest_left) == tail(rest_right)
        || tail(rest_left) == rest_right
        || rest_left == tail(rest_right)
}

const WHOLE_WORD: f64 = 1.0;
const WHOLE_NAME: f64 = 1.5;
const MID_WORD: f64 = 0.3;
const TYPO: f64 = 0.45;
const JOINED_WORDS: f64 = 0.6;

#[derive(Clone, Debug)]
struct TokenMatch {
    score: f64,
    start: usize,
    end: usize,
    /// Separate ranges, for words typed together.
    letters: Option<Vec<SearchRange>>,
}

/// The best place `token` matches `field` (`matchToken`). Mid-word hits need
/// four characters and `mid_word`; typos keep the first letter unless they
/// swap the first two; numbers never match with a typo.
fn match_token(field: &SearchField, token: &[char], typos: bool, is_primary: bool, mid_word: bool) -> Option<TokenMatch> {
    if token.is_empty() {
        return None;
    }
    let text = &field.text;
    let own_boundary = !is_alphanumeric(token[0]) && !is_symbol(token[0]);
    let mut best: Option<TokenMatch> = None;
    let mut from = 0;
    while let Some(index) = find_from(text, token, from) {
        from = index + 1;
        if field.word_starts[index] || own_boundary {
            let end = index + token.len();
            let whole = end >= text.len() || field.word_starts[end] || !is_alphanumeric(text[end]);
            let mut run_end = end;
            while run_end < text.len() && is_alphanumeric(text[run_end]) {
                run_end += 1;
            }
            let score = if whole { WHOLE_WORD } else { 0.7 + (0.2 * token.len() as f64) / (run_end - index) as f64 };
            if best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(TokenMatch { score, start: index, end, letters: None });
            }
            if score == WHOLE_WORD {
                return best;
            }
        } else if mid_word && token.len() >= 4 && best.is_none() {
            best = Some(TokenMatch { score: MID_WORD, start: index, end: index + token.len(), letters: None });
        }
    }
    if best.as_ref().is_none_or(|b| b.score <= MID_WORD)
        && is_primary
        && let Some(joined) = match_joined_words(field, token)
    {
        return Some(joined);
    }
    if best.as_ref().is_some_and(|b| b.score > MID_WORD) || !typos || token.len() < 3 {
        return best;
    }
    if token.iter().any(|c| c.is_alphabetic()) {
        for index in 0..text.len() {
            if !field.word_starts[index] {
                continue;
            }
            let first = text[index];
            let first_kept =
                first == token[0] || (token.len() > 3 && first == token[1] && text.get(index + 1) == Some(&token[0]));
            if !first_kept {
                continue;
            }
            let mut run_end = index + 1;
            while run_end < text.len() && is_alphanumeric(text[run_end]) {
                run_end += 1;
            }
            let word = &text[index..run_end];
            let short = token.len() <= 4;
            let lengths: &[usize] = if short {
                &[token.len(), token.len() + 1]
            } else {
                &[token.len(), token.len() - 1, token.len() + 1]
            };
            for &length in lengths {
                if length > word.len() {
                    continue;
                }
                if within_one_edit(token, &word[..length], short && length == token.len()) {
                    return Some(TokenMatch { score: TYPO, start: index, end: index + length, letters: None });
                }
            }
        }
    }
    best
}

/// Whole words typed together ("vscode" for "VS Code"), with the noise rules
/// of `matchJoinedWords`.
fn match_joined_words(field: &SearchField, token: &[char]) -> Option<TokenMatch> {
    let text = &field.text;
    let first_word_start = field.word_starts.iter().position(|s| *s);
    let mut from = 0;
    while let Some(start) = find_from(text, &token[..1], from) {
        from = start + 1;
        if !field.word_starts[start] {
            continue;
        }
        let mut runs: Vec<SearchRange> = Vec::new();
        let mut at = start;
        let mut rest = token;
        while !rest.is_empty() && at < text.len() {
            let end = word_end(field, at);
            if rest.len() <= end - at && starts_with_at(text, rest, at) {
                runs.push(SearchRange { start: at, end: at + rest.len() });
                rest = &[];
            } else if rest.starts_with(&text[at..end]) {
                runs.push(SearchRange { start: at, end });
                rest = &rest[end - at..];
                at = end;
                while at < text.len() && !field.word_starts[at] {
                    at += 1;
                }
            } else {
                break;
            }
        }
        let Some(last) = runs.last().copied() else { continue };
        if !rest.is_empty() || runs.len() < 2 {
            continue;
        }
        if last.end == word_end(field, last.start)
            || last.end - last.start >= 3
            || Some(start) == first_word_start
            || runs[..runs.len() - 1].iter().all(|run| run.end - run.start >= 3)
        {
            return Some(TokenMatch { score: JOINED_WORDS, start, end: last.end, letters: Some(runs) });
        }
    }
    None
}

/// How one field counts toward a match.
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldOptions {
    pub weight: f64,
    /// Only a whole-word match counts.
    pub whole_words: bool,
    /// Only a word-start match without typos counts (hidden keywords).
    pub keyword: bool,
    /// Where the item lives (project, environment): helps but never makes a match strong.
    pub context: bool,
    /// A token of four or more letters may match inside a word (thread titles only).
    pub mid_word: bool,
}

impl FieldOptions {
    pub const fn weight(weight: f64) -> Self {
        Self { weight, whole_words: false, keyword: false, context: false, mid_word: false }
    }
}

#[derive(Clone, Debug)]
struct SearchFieldsMatch {
    quality: f64,
    whole_words: bool,
    strong: bool,
    primary_ranges: Vec<SearchRange>,
}

const FILLER_WORDS: [&str; 10] = ["a", "an", "and", "for", "in", "of", "on", "the", "to", "with"];

fn is_filler(token: &str) -> bool {
    FILLER_WORDS.contains(&token)
}

fn merge_ranges(mut ranges: Vec<SearchRange>) -> Vec<SearchRange> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<SearchRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// `MESSAGE_TOKEN_MIN_LENGTH`: shorter words do not search messages.
const MESSAGE_TOKEN_MIN_LENGTH: usize = 3;
/// `MESSAGE_WORD_START_LENGTH`: shorter tokens match only at a word start.
const MESSAGE_WORD_START_LENGTH: usize = 4;

/// Hangul syllables count as words for message search, as kana and ideographs do.
fn has_cjk_word(token: &str) -> bool {
    token.chars().any(|c| is_cjk(c) || (0xac00..=0xd7af).contains(&(c as u32)))
}

/// `messageSearchTokens(query).long`: the lowercased words the server searches messages for,
/// accents kept, apostrophes straight; two CJK characters count as a word.
fn message_search_long_tokens(query: &str) -> Vec<String> {
    let trimmed = query.trim_start().trim_start_matches(is_wrapping_quote);
    let trimmed = trimmed.trim_end().trim_end_matches(is_wrapping_quote);
    let normalized = icu_normalizer::ComposingNormalizerBorrowed::new_nfc().normalize(trimmed).replace(['\u{2018}', '\u{2019}'], "'");
    normalized
        .to_lowercase()
        .split_whitespace()
        .filter(|token| {
            let length = token.chars().count();
            length >= MESSAGE_TOKEN_MIN_LENGTH || (length == 2 && has_cjk_word(token))
        })
        .map(str::to_owned)
        .collect()
}

/// `findSearchTokenRanges`: where the query's message-search words appear in `text` (a match
/// excerpt), merged. A word shorter than four chars matches only at a word start unless it opens
/// with punctuation.
pub fn find_search_token_ranges(text: &str, query: &str) -> Vec<SearchRange> {
    let mut tokens: Vec<(Vec<char>, bool)> = Vec::new();
    for token in message_search_long_tokens(query) {
        let folded: Vec<char> = fold_search_query(&token).chars().collect();
        // Some tokens fold to nothing ("´´´"), which would match everywhere.
        if folded.is_empty() {
            continue;
        }
        let needs_word_start = token.chars().count() < MESSAGE_WORD_START_LENGTH;
        match tokens.iter_mut().find(|(existing, _)| *existing == folded) {
            Some(entry) => entry.1 = needs_word_start,
            None => tokens.push((folded, needs_word_start)),
        }
    }
    if tokens.is_empty() {
        return Vec::new();
    }
    let field = fold_search_field(text);
    let mut ranges = Vec::new();
    for (token, needs_word_start) in &tokens {
        let own_boundary = !is_alphanumeric(token[0]) && !is_symbol(token[0]);
        let mut found = find_from(&field.text, token, 0);
        while let Some(index) = found {
            if *needs_word_start && !own_boundary && !field.word_starts[index] {
                found = find_from(&field.text, token, index + 1);
                continue;
            }
            ranges.push(SearchRange { start: index, end: index + token.len() });
            found = find_from(&field.text, token, index + token.len());
        }
    }
    merge_ranges(ranges)
}

/// A folded query split into tokens once per keystroke.
#[derive(Clone, Debug, Default)]
pub struct SearchQuery {
    folded: String,
    chars: Vec<char>,
    tokens: Vec<(String, Vec<char>)>,
    filler_optional: bool,
}

impl SearchQuery {
    /// Prepares an already folded query (see [`fold_search_query`]).
    pub fn new(folded: &str) -> Self {
        let tokens: Vec<(String, Vec<char>)> =
            folded.split(' ').filter(|t| !t.is_empty()).map(|t| (t.to_owned(), t.chars().collect())).collect();
        let filler_optional = tokens.iter().any(|(t, _)| !is_filler(t));
        Self { folded: folded.to_owned(), chars: folded.chars().collect(), tokens, filler_optional }
    }

    pub fn as_str(&self) -> &str {
        &self.folded
    }

    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }
}

const PICK_PREFIX_MIN_LENGTH: usize = 3;

/// Every query token must match some field. Quality is the mean of each
/// token's best weighted match, plus a bonus when the first field holds the
/// whole query or equals it (`matchSearchFields`).
fn match_search_fields<'a, I>(query: &SearchQuery, fields: I, typos: bool) -> Option<SearchFieldsMatch>
where
    I: Iterator<Item = (&'a SearchText, FieldOptions)> + Clone,
{
    if query.tokens.is_empty() {
        return None;
    }
    let primary = fields.clone().next().filter(|(text, _)| !text.is_empty()).map(|(text, _)| text.field());
    let mut primary_ranges = Vec::new();
    let (mut total, mut counted) = (0.0, 0usize);
    let (mut not_whole, mut strong) = (false, true);
    for (token_str, token) in &query.tokens {
        let mut best = 0.0f64;
        let mut best_primary: Option<TokenMatch> = None;
        let mut at_word_start = false;
        for (text, options) in fields.clone() {
            if text.is_empty() {
                continue;
            }
            let field = text.field();
            let is_primary = primary.is_some_and(|p| std::ptr::eq(p, field));
            if best >= options.weight && !is_primary && at_word_start {
                continue;
            }
            let field_typos = typos && !options.keyword;
            let Some(found) = match_token(field, token, field_typos, is_primary, options.mid_word) else { continue };
            let floor = if options.whole_words {
                WHOLE_WORD
            } else if options.keyword {
                0.7
            } else {
                0.0
            };
            if found.score < floor {
                continue;
            }
            if found.score >= 0.7 && !options.context {
                at_word_start = true;
            }
            if found.score * options.weight > best {
                best = found.score * options.weight;
            }
            if is_primary {
                best_primary = Some(found);
            }
        }
        if best == 0.0 {
            if query.filler_optional && is_filler(token_str) {
                continue;
            }
            return None;
        }
        strong &= at_word_start;
        if let Some(found) = best_primary {
            match found.letters {
                Some(letters) => primary_ranges.extend(letters),
                None => primary_ranges.push(SearchRange { start: found.start, end: found.end }),
            }
        }
        total += best;
        counted += 1;
        if is_filler(token_str) {
            not_whole |= !query.filler_optional;
        } else {
            not_whole |= token.len() < PICK_PREFIX_MIN_LENGTH || best < WHOLE_WORD;
        }
    }
    let mut quality = total / counted as f64;
    if let Some(primary) = primary {
        if *primary.text == *query.chars {
            quality += WHOLE_NAME - WHOLE_WORD;
        } else if query.tokens.len() > 1 && find_from(&primary.text, &query.chars, 0).is_some() {
            quality += 0.25;
        }
    }
    Some(SearchFieldsMatch { quality, whole_words: !not_whole, strong, primary_ranges: merge_ranges(primary_ranges) })
}

const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;
/// Activity recency and open counts halve every two weeks.
pub const SEARCH_RECENCY_HALF_LIFE_MS: f64 = 14.0 * DAY_MS;
/// Opens closer together than this count once.
pub const THREAD_OPEN_DEDUPE_MS: i64 = 10 * 60 * 1000;
const THREAD_SEARCH_PICK_LIMIT: usize = 3;
const THREAD_SEARCH_PICK_MAX_LENGTH: usize = 32;
const PICK_WINDOW_MS: i64 = 21 * 24 * 60 * 60 * 1000;

/// `query` as a pick is stored and compared: folded and cut to 32 chars (`pickedQuery`).
pub fn picked_query(query: &str) -> String {
    let folded = fold_search_query(query);
    folded.chars().take(THREAD_SEARCH_PICK_MAX_LENGTH).collect::<String>().trim().to_owned()
}

fn decay(age_ms: f64) -> f64 {
    2f64.powf(-age_ms.max(0.0) / SEARCH_RECENCY_HALF_LIFE_MS)
}

/// A palette query that picked a thread, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadPick {
    pub query: String,
    pub at: i64,
}

/// Server-stored open history of a thread (`OrchestrationV2ThreadOpens`).
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadOpenStats {
    pub opened_at: i64,
    pub score: f64,
    pub picks: Vec<ThreadPick>,
}

/// The stats after an open at `at`, picked by `query` when opened from search (`recordThreadOpen`).
pub fn record_thread_open(previous: Option<&ThreadOpenStats>, at: i64, query: Option<&str>) -> ThreadOpenStats {
    let pick = picked_query(query.unwrap_or(""));
    let earlier: Vec<ThreadPick> = previous
        .map(|p| p.picks.iter().filter(|entry| at - entry.at <= PICK_WINDOW_MS).cloned().collect())
        .unwrap_or_default();
    let picks = if pick.is_empty() {
        earlier
    } else {
        std::iter::once(ThreadPick { query: pick.clone(), at })
            .chain(earlier.into_iter().filter(|entry| entry.query != pick))
            .take(THREAD_SEARCH_PICK_LIMIT)
            .collect()
    };
    if let Some(previous) = previous
        && at - previous.opened_at < THREAD_OPEN_DEDUPE_MS
    {
        return ThreadOpenStats { picks, ..previous.clone() };
    }
    let score = previous.map_or(1.0, |p| p.score * decay((at - p.opened_at) as f64) + 1.0);
    ThreadOpenStats { opened_at: at, score, picks }
}

/// A pull request a thread is linked to, for search.
#[derive(Clone, Debug)]
pub struct PullRequestSubject {
    pub number: u64,
    /// `#<number>`.
    pub number_text: SearchText,
    pub title: Option<SearchText>,
    /// Values such as URLs, matched only by the whole query.
    pub references: Vec<Arc<str>>,
}

/// What thread search ranks, built once per thread (`ThreadSearchSubject`).
#[derive(Clone, Debug)]
pub struct ThreadSearchSubject {
    /// Matched only by the whole query, like references.
    pub id: Option<Arc<str>>,
    pub title: SearchText,
    /// The branch with the worktree prefix removed.
    branch: Option<SearchText>,
    raw_branch: Option<Arc<str>>,
    pub project_title: Option<SearchText>,
    /// Only set with two or more environments.
    pub environment_label: Option<SearchText>,
    pub pull_requests: Vec<PullRequestSubject>,
    /// Latest user activity, epoch ms.
    pub activity_at: Option<i64>,
    pub opens: Option<ThreadOpenStats>,
}

impl ThreadSearchSubject {
    pub fn new(title: impl Into<Arc<str>>) -> Self {
        Self {
            id: None,
            title: SearchText::new(title),
            branch: None,
            raw_branch: None,
            project_title: None,
            environment_label: None,
            pull_requests: Vec::new(),
            activity_at: None,
            opens: None,
        }
    }

    pub fn with_branch(mut self, branch: Option<&str>) -> Self {
        self.raw_branch = branch.map(Into::into);
        self.branch = branch.map(searchable_branch).filter(|b| !b.is_empty()).map(SearchText::new);
        self
    }

    pub fn branch(&self) -> Option<&str> {
        self.raw_branch.as_deref()
    }

    fn fields(&self) -> impl Iterator<Item = (&SearchText, FieldOptions)> + Clone {
        let title = std::iter::once((&self.title, FieldOptions { mid_word: true, ..FieldOptions::weight(1.0) }));
        let pull_requests = self.pull_requests.iter().flat_map(|pr| {
            std::iter::once((&pr.number_text, FieldOptions::weight(1.0)))
                .chain(pr.title.iter().map(|title| (title, FieldOptions::weight(0.5))))
        });
        let context = FieldOptions { context: true, ..FieldOptions::weight(0.25) };
        title
            .chain(pull_requests)
            .chain(self.branch.iter().map(|b| (b, FieldOptions::weight(0.6))))
            .chain(self.project_title.iter().map(move |p| (p, context)))
            .chain(self.environment_label.iter().map(move |e| (e, context)))
    }
}

/// Removes the app's worktree prefix (`t3/`, `t3code/`, `t3-<hex8>`) from a branch.
fn searchable_branch(branch: &str) -> String {
    for prefix in ["t3/", "t3code/"] {
        if let Some(rest) = branch.strip_prefix(prefix)
            && rest.chars().next().is_some_and(|c| !c.is_whitespace())
        {
            return rest.to_owned();
        }
    }
    if let Some(rest) = branch.strip_prefix("t3-")
        && rest.len() == 8
        && rest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return rest.to_owned();
    }
    branch.to_owned()
}

/// Strips pull request words, `#`, and worktree prefixes so a query matches stored fields.
fn thread_field_query(normalized: &str) -> String {
    let mut query = normalized;
    for word in ["pull request", "pull", "pr"] {
        if let Some(rest) = query.strip_prefix(word) {
            let trimmed = rest.trim_start();
            let digits = trimmed.strip_prefix('#').unwrap_or(trimmed);
            if digits.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                query = trimmed;
                break;
            }
        }
    }
    if let Some(rest) = query.strip_prefix('#') {
        query = rest.trim_start();
    }
    let stripped = searchable_branch(query);
    // "t3-<hex>" anywhere while the hex is still being typed.
    let mut out: Vec<&str> = Vec::new();
    for word in stripped.split(' ') {
        match word.strip_prefix("t3-") {
            Some(hex) if (1..=8).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) => {
                out.push(hex)
            }
            _ => out.push(word),
        }
    }
    out.join(" ")
}

const REFERENCE_PREFIX_MIN: usize = 8;

fn match_reference(normalized: &str, thread: &ThreadSearchSubject) -> bool {
    if normalized.chars().count() < REFERENCE_PREFIX_MIN {
        return false;
    }
    let anywhere = normalized.chars().any(char::is_numeric);
    let not_cut = |text: &str, end: usize| !text[end..].chars().next().is_some_and(|c| c.is_numeric());
    let matches = |reference: &str| {
        let folded = reference.to_lowercase();
        if anywhere {
            folded.find(normalized).is_some_and(|at| not_cut(&folded, at + normalized.len()))
                || (normalized.starts_with(&folded) && not_cut(normalized, folded.len()))
        } else {
            folded.starts_with(normalized)
        }
    };
    thread.id.as_deref().is_some_and(matches)
        || thread.pull_requests.iter().any(|pr| pr.references.iter().any(|r| matches(r)))
}

fn active_pick<'a>(thread: &'a ThreadSearchSubject, normalized: &str, now: i64) -> Option<&'a ThreadPick> {
    let query = picked_query(normalized);
    let prefix = query.chars().count() >= PICK_PREFIX_MIN_LENGTH;
    let (mut exact, mut latest): (Option<&ThreadPick>, Option<&ThreadPick>) = (None, None);
    for pick in thread.opens.iter().flat_map(|o| o.picks.iter()) {
        if now - pick.at > PICK_WINDOW_MS {
            continue;
        }
        if pick.query == query && exact.is_none_or(|e| pick.at > e.at) {
            exact = Some(pick);
        }
        let matches = pick.query == query || (prefix && pick.query.starts_with(&query));
        if matches && latest.is_none_or(|l| pick.at > l.at) {
            latest = Some(pick);
        }
    }
    exact.or(latest)
}

const USE_BOOST_LIMIT: f64 = 2.5;

/// 1 for a stale, never-opened thread, about 2 for one active today, up to 2.5 for often-opened ones.
pub fn thread_usage_boost(thread: &ThreadSearchSubject, now: i64) -> f64 {
    let recency = thread.activity_at.map_or(0.0, |at| decay((now - at) as f64));
    let opens = thread.opens.as_ref().map_or(0.0, |o| o.score * decay((now - o.opened_at) as f64));
    (1.0 + recency + 0.5 * (1.0 + opens).log2()).min(USE_BOOST_LIMIT)
}

/// When the user last used a thread: their last message or open (`threadRecentUse`).
pub fn thread_recent_use(thread: &ThreadSearchSubject) -> i64 {
    thread.activity_at.unwrap_or(0).max(thread.opens.as_ref().map_or(0, |o| o.opened_at))
}

/// Which field explains a match the title does not show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMatchHintField {
    PullRequest,
    Branch,
    Project,
    Environment,
    Reference,
}

/// A line under a result saying where it matched.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatchHint {
    pub field: SearchMatchHintField,
    /// "PR", "Branch", "Project", "Environment", "ID or link".
    pub label: &'static str,
    pub text: Arc<str>,
    pub ranges: Vec<SearchRange>,
}

fn match_hint(query: &str, thread: &ThreadSearchSubject, typos: bool) -> Option<SearchMatchHint> {
    let pr_texts: Vec<SearchText> = thread
        .pull_requests
        .iter()
        .map(|pr| {
            SearchText::new(match &pr.title {
                Some(title) => format!("#{} {}", pr.number, title.as_str()),
                None => format!("#{}", pr.number),
            })
        })
        .collect();
    let mut candidates: Vec<(SearchMatchHintField, &'static str, &SearchText)> =
        pr_texts.iter().map(|text| (SearchMatchHintField::PullRequest, "PR", text)).collect();
    candidates.extend(thread.branch.iter().map(|b| (SearchMatchHintField::Branch, "Branch", b)));
    candidates.extend(thread.project_title.iter().map(|p| (SearchMatchHintField::Project, "Project", p)));
    candidates.extend(thread.environment_label.iter().map(|e| (SearchMatchHintField::Environment, "Environment", e)));
    let holds = |text: &SearchText, query: &str| {
        let query = SearchQuery::new(query);
        match_search_fields(&query, std::iter::once((text, FieldOptions::weight(1.0))), typos).map(|m| m.primary_ranges)
    };
    let hint_for = |query: &str| {
        candidates.iter().find_map(|(field, label, text)| {
            holds(text, query).map(|ranges| SearchMatchHint { field: *field, label, text: text.arc().clone(), ranges })
        })
    };
    if let Some(whole) = hint_for(query) {
        return Some(whole);
    }
    let contexts: Vec<&SearchText> = thread.project_title.iter().chain(thread.environment_label.iter()).collect();
    let rest: Vec<&str> =
        query.split(' ').filter(|token| !contexts.iter().any(|text| holds(text, token).is_some())).collect();
    let rest = rest.join(" ");
    if !rest.is_empty() && rest != query { hint_for(&rest) } else { None }
}

/// A thread's score for a query (`ThreadSearchMatch`).
#[derive(Clone, Debug)]
pub struct ThreadSearchMatch {
    /// Match quality times use; what results sort by.
    pub score: f64,
    pub quality: f64,
    pub whole_words: bool,
    pub strong: bool,
    /// Matched ranges in the title.
    pub ranges: Vec<SearchRange>,
    pub picked_at: Option<i64>,
    pub picked_exactly: bool,
    pub group_quality: f64,
    /// Set when the title shows no match; call [`ThreadSearchMatch::explain`] for the row.
    explain: Option<ThreadHintSource>,
}

#[derive(Clone, Debug)]
enum ThreadHintSource {
    Reference,
    Checked(SearchMatchHint),
    Lazy { field_query: String, typos: bool },
}

impl ThreadSearchMatch {
    /// Why the thread matched when its title shows none, built only for shown rows.
    pub fn explain(&self, thread: &ThreadSearchSubject, normalized: &str) -> Option<SearchMatchHint> {
        match self.explain.as_ref()? {
            ThreadHintSource::Reference => Some(SearchMatchHint {
                field: SearchMatchHintField::Reference,
                label: "ID or link",
                text: normalized.into(),
                ranges: Vec::new(),
            }),
            ThreadHintSource::Checked(hint) => Some(hint.clone()),
            ThreadHintSource::Lazy { field_query, typos } => match_hint(field_query, thread, *typos),
        }
    }
}

/// Scores a thread, or `None` when it does not match (`scoreThreadSearch`).
/// Titles and pull request numbers count fully, PR titles and branches less,
/// project and environment least.
pub fn score_thread_search(
    query: &SearchQuery,
    thread: &ThreadSearchSubject,
    now: i64,
    typos: bool,
    picks: bool,
) -> Option<ThreadSearchMatch> {
    let field_query_text = thread_field_query(query.as_str());
    let field_query_owned;
    let field_query = if field_query_text == query.as_str() {
        query
    } else {
        field_query_owned = SearchQuery::new(&field_query_text);
        &field_query_owned
    };
    let field_match = match_search_fields(field_query, thread.fields(), typos);
    let reference = field_match.is_none() && match_reference(query.as_str(), thread);
    let found = match field_match {
        Some(found) => found,
        None if reference => SearchFieldsMatch { quality: 0.6, whole_words: false, strong: true, primary_ranges: Vec::new() },
        None => return None,
    };
    let title_shows = !found.primary_ranges.is_empty();
    let check_hint = !title_shows && !reference && field_query_text.contains(' ');
    let checked = if check_hint { match_hint(&field_query_text, thread, typos) } else { None };
    if check_hint && checked.is_none() {
        return None;
    }
    let explain = if title_shows {
        None
    } else if reference {
        Some(ThreadHintSource::Reference)
    } else if let Some(hint) = checked {
        Some(ThreadHintSource::Checked(hint))
    } else {
        Some(ThreadHintSource::Lazy { field_query: field_query_text.clone(), typos })
    };
    let pick = if picks { active_pick(thread, query.as_str(), now) } else { None };
    Some(ThreadSearchMatch {
        score: found.quality * thread_usage_boost(thread, now),
        quality: found.quality,
        whole_words: found.whole_words,
        group_quality: found.quality.min(WHOLE_NAME - 0.01),
        strong: found.strong,
        ranges: found.primary_ranges,
        picked_at: pick.map(|p| p.at),
        picked_exactly: pick.is_some_and(|p| p.query == picked_query(query.as_str())),
        explain,
    })
}

/// Palette limits shared by web and mobile.
pub const PALETTE_THREAD_LIMIT: usize = 20;
pub const PALETTE_MESSAGE_LIMIT: usize = 10;
/// Next to matching threads, other groups show only their best few rows.
pub const PALETTE_SECONDARY_LIMIT: usize = 3;

fn match_class(quality: f64) -> u8 {
    if quality >= WHOLE_NAME {
        3
    } else if quality >= 0.7 {
        2
    } else if quality >= 0.4 {
        1
    } else {
        0
    }
}

/// The parts of a match that ranking reads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RankKey {
    pub score: f64,
    pub quality: f64,
    pub strong: bool,
    pub picked_at: Option<i64>,
    pub picked_exactly: bool,
    /// The quality the group ranks by, when lower than `quality`.
    pub group_quality: Option<f64>,
    pub whole_words: Option<bool>,
}

impl From<&ThreadSearchMatch> for RankKey {
    fn from(m: &ThreadSearchMatch) -> Self {
        Self {
            score: m.score,
            quality: m.quality,
            strong: m.strong,
            picked_at: m.picked_at,
            picked_exactly: m.picked_exactly,
            group_quality: Some(m.group_quality),
            whole_words: Some(m.whole_words),
        }
    }
}

/// Ranks groups by match class so typing stays stable and use never lifts a
/// thread over an equally good command (`rankSearchGroups`). Typos are tried
/// only when nothing matched at a word start. `score(item, typos)` returns
/// the match and its rank key. Returns `(group index, ranked item indices)`
/// best group first, each group's items best first.
pub fn rank_search_groups<M>(
    group_sizes: &[usize],
    mut score: impl FnMut(usize, usize, bool) -> Option<(M, RankKey)>,
) -> Vec<(usize, Vec<(usize, M, RankKey)>)> {
    let mut rank = |typos: bool| -> Vec<Vec<(usize, M, RankKey)>> {
        group_sizes
            .iter()
            .enumerate()
            .map(|(group, &size)| (0..size).filter_map(|item| score(group, item, typos).map(|(m, k)| (item, m, k))).collect())
            .collect()
    };
    let mut ranked = rank(false);
    if !ranked.iter().flatten().any(|(_, _, key)| key.strong) {
        ranked = rank(true);
    }
    let word_match = |k: &RankKey| match_class(k.quality).min(2);
    let exact_pick_at = |k: &RankKey| if k.picked_exactly { k.picked_at.unwrap_or(0) } else { 0 };
    let whole_word = |k: &RankKey| k.whole_words.unwrap_or(k.quality >= WHOLE_WORD) as u8;
    let mut groups: Vec<(usize, Vec<(usize, M, RankKey)>, u8)> = ranked
        .into_iter()
        .enumerate()
        .filter(|(_, items)| !items.is_empty())
        .map(|(order, mut items)| {
            items.sort_by(|(_, _, l), (_, _, r)| {
                word_match(r)
                    .cmp(&word_match(l))
                    .then(exact_pick_at(r).cmp(&exact_pick_at(l)))
                    .then(match_class(r.quality).cmp(&match_class(l.quality)))
                    .then(r.picked_at.unwrap_or(0).cmp(&l.picked_at.unwrap_or(0)))
                    .then(whole_word(r).cmp(&whole_word(l)))
                    .then(r.score.total_cmp(&l.score))
            });
            let best = items.iter().map(|(_, _, k)| match_class(k.group_quality.unwrap_or(k.quality))).max().unwrap_or(0);
            (order, items, best)
        })
        .collect();
    groups.sort_by(|(lo, _, lb), (ro, _, rb)| rb.cmp(lb).then(lo.cmp(ro)));
    groups.into_iter().map(|(order, items, _)| (order, items)).collect()
}

/// A title-and-keywords match (`SearchTermsMatch`).
#[derive(Clone, Debug)]
pub struct SearchTermsMatch {
    pub key: RankKey,
    /// Ranges in the first term.
    pub ranges: Vec<SearchRange>,
}

const KEYWORD_QUALITY: f64 = 0.65;
const KEYWORD_QUERY_MIN_LENGTH: usize = 4;

/// Scores a name and its keywords (`scoreSearchTerms`): keywords count less,
/// and a short query must match a keyword whole.
pub fn score_search_terms(query: &SearchQuery, terms: &[SearchText], typos: bool) -> Option<SearchTermsMatch> {
    let meaningful: Vec<&str> = query.as_str().split(' ').filter(|t| !is_filler(t)).collect();
    let meaningful = meaningful.join(" ");
    let short =
        if meaningful.is_empty() { query.as_str().chars().count() } else { meaningful.chars().count() } < KEYWORD_QUERY_MIN_LENGTH;
    let keyword = FieldOptions { weight: if short { 0.8 } else { 0.6 }, whole_words: short, keyword: true, ..FieldOptions::default() };
    let fields = terms.iter().enumerate().map(move |(i, t)| (t, if i == 0 { FieldOptions::weight(1.0) } else { keyword }));
    let found = match_search_fields(query, fields, typos)?;
    let group_quality = found.primary_ranges.is_empty().then(|| found.quality.min(KEYWORD_QUALITY));
    Some(SearchTermsMatch {
        key: RankKey {
            score: found.quality,
            quality: found.quality,
            strong: found.strong,
            group_quality,
            ..RankKey::default()
        },
        ranges: found.primary_ranges,
    })
}

/// Byte ranges of `text` for char ranges, for drawing highlights.
pub fn char_ranges_to_bytes(text: &str, ranges: &[SearchRange]) -> Vec<std::ops::Range<usize>> {
    if ranges.is_empty() {
        return Vec::new();
    }
    let offsets: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
    ranges
        .iter()
        .filter_map(|r| Some(*offsets.get(r.start)?..*offsets.get(r.end.min(offsets.len() - 1))?))
        .collect()
}
