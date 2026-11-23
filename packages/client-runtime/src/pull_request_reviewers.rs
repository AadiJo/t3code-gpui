//! The reviewer picker's list: loaded candidates narrowed by the typed text,
//! plus what the host found when its first page was truncated. Ports
//! `pullRequestReviewers.ts`.

use std::collections::HashSet;

use t3code_contracts::{PullRequestReviewerCandidate, PullRequestReviewerCandidateList};

use crate::pull_request_query::js_trim;

fn key(candidate: &PullRequestReviewerCandidate) -> String {
    format!("{}:{}", candidate.kind.as_str(), candidate.id)
}

/// Loaded rows that match `query` (they keep the request state mutations updated), then host
/// matches no loaded row carries.
pub fn merge_reviewer_candidates(
    loaded: &[PullRequestReviewerCandidate],
    searched: &[PullRequestReviewerCandidate],
    query: &str,
) -> Vec<PullRequestReviewerCandidate> {
    let needle = js_trim(query).to_lowercase();
    let known: HashSet<String> = loaded.iter().map(key).collect();
    let host_matches: HashSet<String> = searched.iter().map(key).collect();
    loaded
        .iter()
        .filter(|c| {
            host_matches.contains(&key(c))
                || needle.is_empty()
                || c.login.to_lowercase().contains(&needle)
                || c.name.as_deref().unwrap_or("").to_lowercase().contains(&needle)
        })
        // The host matched these on its own terms, an email for one, so they are not narrowed again.
        .chain(searched.iter().filter(|c| !known.contains(&key(c))))
        .cloned()
        .collect()
}

/// What the picker knows before asking the host.
#[derive(Debug, Clone, Copy)]
pub struct ReviewerSearchInput<'a> {
    pub loaded: Option<&'a PullRequestReviewerCandidateList>,
    /// The host declared `reviewers.searchCandidates`.
    pub searchable: bool,
    pub query: &'a str,
    pub debounced_query: &'a str,
}

/// What to ask the host for, or `None` when the loaded page already answers the query.
pub fn host_reviewer_query(input: &ReviewerSearchInput<'_>) -> Option<String> {
    let debounced = js_trim(input.debounced_query);
    (input.searchable && input.loaded.is_some_and(|l| l.truncated) && !debounced.is_empty())
        .then(|| debounced.to_owned())
}

/// Why the list may be missing people.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerTruncation {
    /// The host cannot be asked.
    Unsearchable,
    /// The host can be asked once something is typed.
    Searchable,
    SearchFailed,
    SearchTruncated,
}

impl ReviewerTruncation {
    /// The TypeScript name, for comparison tables.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsearchable => "unsearchable",
            Self::Searchable => "searchable",
            Self::SearchFailed => "search-failed",
            Self::SearchTruncated => "search-truncated",
        }
    }
}

/// The host search's state for the current text.
#[derive(Debug, Clone, Copy)]
pub struct ReviewerSearchState<'a> {
    pub data: Option<&'a PullRequestReviewerCandidateList>,
    pub is_pending: bool,
    pub error: Option<&'a str>,
}

/// What the reviewer picker lists.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewerCandidateView {
    pub candidates: Vec<PullRequestReviewerCandidate>,
    /// Nothing matches yet and the host has not answered, which is not "nobody matches".
    pub search_pending: bool,
    pub search_error: Option<String>,
    pub truncation: Option<ReviewerTruncation>,
}

/// The picker's rows and notices for the typed text and the host search.
pub fn reviewer_candidate_view(input: &ReviewerSearchInput<'_>, search: &ReviewerSearchState<'_>) -> ReviewerCandidateView {
    let query = js_trim(input.query);
    let host_query = host_reviewer_query(input);
    // Only once the debounce has caught up, or rows matching the previous text would linger.
    let searched = host_query.as_deref() == Some(query);
    let empty = Vec::new();
    let candidates = merge_reviewer_candidates(
        input.loaded.map_or(&empty, |l| &l.candidates),
        if searched { search.data.map_or(&empty, |d| &d.candidates) } else { &empty },
        query,
    );
    let truncated = input.loaded.is_some_and(|l| l.truncated);
    let searching = input.searchable && truncated && !query.is_empty();
    let search_error = if searched { search.error.map(str::to_owned) } else { None };
    let truncation = if !truncated {
        None
    } else if !input.searchable {
        Some(ReviewerTruncation::Unsearchable)
    } else if query.is_empty() {
        Some(ReviewerTruncation::Searchable)
    } else if !searched {
        None
    } else if search_error.is_some() {
        Some(ReviewerTruncation::SearchFailed)
    } else if search.data.is_some_and(|d| d.truncated) {
        Some(ReviewerTruncation::SearchTruncated)
    } else {
        None
    };
    ReviewerCandidateView {
        search_pending: candidates.is_empty() && searching && (!searched || search.is_pending),
        candidates,
        search_error,
        truncation,
    }
}
