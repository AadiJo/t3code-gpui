//! The pull request list's search syntax. Ports `pullRequestQuery.ts`: typed
//! qualifiers (`label:`, `author:`, `draft:`, `review:`, `checks:`) become list
//! filters and the rest stays free text.

use std::sync::LazyLock;

use regex::Regex;
use t3code_contracts::{PullRequestListFilters, PullRequestListFiltersChecks, PullRequestListFiltersDraft, PullRequestListFiltersReview};

const MAX_QUALIFIER_VALUES: usize = 10;
const MAX_QUALIFIER_LENGTH: usize = 200;
const MAX_TEXT_LENGTH: usize = 200;

static QUERY_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:[^\s"]|"[^"]*")+"#).unwrap());
static LIST_PART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:[^,"]|"[^"]*")+"#).unwrap());
static QUALIFIER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?s)(-?)([A-Za-z][A-Za-z0-9_-]*):(.*)$").unwrap());

/// A query split into its free text and the filters its qualifiers name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedPullRequestQuery {
    pub text: String,
    pub filters: PullRequestListFilters,
}

fn review_value(key: &str) -> Option<PullRequestListFiltersReview> {
    Some(match key {
        "approved" => PullRequestListFiltersReview::Approved,
        "changes_requested" | "changes-requested" => PullRequestListFiltersReview::ChangesRequested,
        "required" | "review-required" => PullRequestListFiltersReview::ReviewRequired,
        "none" => PullRequestListFiltersReview::NoneType,
        _ => return None,
    })
}

fn checks_value(key: &str) -> Option<PullRequestListFiltersChecks> {
    Some(match key {
        "success" | "passing" => PullRequestListFiltersChecks::Passing,
        "failure" | "failing" => PullRequestListFiltersChecks::Failing,
        _ => return None,
    })
}

fn qualifier_value(raw: &str) -> String {
    js_trim(&raw.replace('"', "")).to_owned()
}

/// `String.prototype.trim`: strips JavaScript whitespace, which is what Rust's `trim` strips too
/// except for U+FEFF.
pub fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// `String.prototype.slice(0, n)` over UTF-16 code units, never splitting a scalar value.
pub fn js_slice_utf16(value: &str, n: usize) -> &str {
    let mut units = 0;
    for (index, c) in value.char_indices() {
        units += c.len_utf16();
        if units > n {
            return &value[..index];
        }
    }
    value
}

fn split_qualifier_list(raw: &str) -> Vec<String> {
    LIST_PART
        .find_iter(raw)
        .map(|part| qualifier_value(part.as_str()))
        .filter(|part| !part.is_empty())
        .collect()
}

/// `me` and `@me` both name the viewer; logins are case-insensitive.
fn author_key(login: &str) -> String {
    let lower = login.to_lowercase();
    if lower == "@me" { "me".into() } else { lower }
}

fn bounded_names(names: Vec<String>) -> Vec<String> {
    names
        .into_iter()
        .take(MAX_QUALIFIER_VALUES)
        .map(|name| js_trim(js_slice_utf16(&name, MAX_QUALIFIER_LENGTH)).to_owned())
        .filter(|name| !name.is_empty())
        .collect()
}

/// Splits a typed query into free text and list filters.
pub fn parse_pull_request_query(raw: &str) -> ParsedPullRequestQuery {
    let mut text: Vec<&str> = Vec::new();
    let mut labels: Vec<Vec<String>> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    // `Some(None)` once two different logins are named: GitHub ORs them, the contract carries one.
    let mut author: Option<Option<String>> = None;
    let mut draft = None;
    let mut review = None;
    let mut checks = None;
    for token in QUERY_TOKEN.find_iter(raw).map(|m| m.as_str()) {
        let captures = QUALIFIER.captures(token);
        let rest = captures.as_ref().and_then(|c| c.get(3)).map_or("", |m| m.as_str());
        let name = captures.as_ref().and_then(|c| c.get(2)).map_or("", |m| m.as_str());
        let value = if captures.is_none() { String::new() } else { qualifier_value(rest) };
        let negated = captures.as_ref().and_then(|c| c.get(1)).is_some_and(|m| m.as_str() == "-");
        let key = if value.is_empty() { String::new() } else { name.to_lowercase() };
        let consumed = match key.as_str() {
            "label" => {
                let names = bounded_names(split_qualifier_list(rest));
                if names.is_empty() {
                    false
                } else {
                    if negated { excluded.extend(names) } else { labels.push(names) }
                    true
                }
            }
            "author" => {
                if negated {
                    false
                } else {
                    let login = js_trim(js_slice_utf16(&value, MAX_QUALIFIER_LENGTH)).to_owned();
                    author = Some(match author {
                        None => Some(login),
                        Some(Some(held)) if author_key(&held) == author_key(&login) => Some(held),
                        Some(_) => None,
                    });
                    true
                }
            }
            "draft" => {
                let lower = value.to_lowercase();
                if lower != "true" && lower != "false" {
                    false
                } else {
                    draft = Some(if (lower == "true") != negated {
                        PullRequestListFiltersDraft::Only
                    } else {
                        PullRequestListFiltersDraft::Hide
                    });
                    true
                }
            }
            "review" => match (!negated).then(|| review_value(&value.to_lowercase())).flatten() {
                Some(decision) => {
                    review = Some(decision);
                    true
                }
                None => false,
            },
            "status" | "checks" => match (!negated).then(|| checks_value(&value.to_lowercase())).flatten() {
                Some(state) => {
                    checks = Some(state);
                    true
                }
                None => false,
            },
            // GitHub's own search qualifiers, not labels: the list sets scope and order itself.
            "is" | "archived" | "state" | "repo" | "org" | "user" | "sort" => true,
            "" => false,
            _ if value.starts_with('/') => false,
            _ => {
                let names = bounded_names(
                    split_qualifier_list(rest)
                        .into_iter()
                        .map(|label| if label.contains(':') { label } else { format!("{name}:{label}") })
                        .collect(),
                );
                if names.is_empty() {
                    false
                } else {
                    if negated { excluded.extend(names) } else { labels.push(names) }
                    true
                }
            }
        };
        if !consumed {
            text.push(token);
        }
    }
    let joined = text.join(" ");
    labels.truncate(MAX_QUALIFIER_VALUES);
    excluded.truncate(MAX_QUALIFIER_VALUES);
    ParsedPullRequestQuery {
        text: js_trim(js_slice_utf16(&joined, MAX_TEXT_LENGTH)).to_owned(),
        filters: PullRequestListFilters {
            draft,
            review,
            checks,
            labels: (!labels.is_empty()).then_some(labels),
            excluded_labels: (!excluded.is_empty()).then_some(excluded),
            author: author.flatten(),
        },
    }
}

/// The query and menu filters after the Draft menu picks `next`. The menu shows a typed `draft:`
/// qualifier, so picking another value takes the qualifier out of the query: left in, it would
/// keep outranking whatever the menu stores.
pub fn apply_pull_request_draft_filter_change(
    query: &str,
    menu_filters: &PullRequestListFilters,
    next: &PullRequestListFilters,
) -> (String, PullRequestListFilters) {
    let typed = parse_pull_request_query(query).filters.draft;
    if next.draft == typed.or(menu_filters.draft) {
        return (query.to_owned(), PullRequestListFilters { draft: menu_filters.draft, ..next.clone() });
    }
    let query = if typed.is_none() {
        query.to_owned()
    } else {
        QUERY_TOKEN
            .find_iter(query)
            .map(|m| m.as_str())
            .filter(|token| parse_pull_request_query(token).filters.draft.is_none())
            .collect::<Vec<_>>()
            .join(" ")
    };
    (query, next.clone())
}

/// The login an `author:` filter names: `me` and `@me` become the viewer when one is known.
/// Ports `resolvePullRequestAuthorFilter` from the contracts.
pub fn resolve_pull_request_author_filter(author: &str, viewer: Option<&str>) -> String {
    let trimmed = js_trim(author);
    let lower = trimmed.to_lowercase();
    if lower != "me" && lower != "@me" {
        return trimmed.to_owned();
    }
    match viewer {
        Some(viewer) if !js_trim(viewer).is_empty() => viewer.to_owned(),
        _ => trimmed.to_owned(),
    }
}

/// Whether the filters narrow anything.
pub fn has_pull_request_filters(filters: &PullRequestListFilters) -> bool {
    filters.draft.is_some()
        || filters.review.is_some()
        || filters.checks.is_some()
        || filters.labels.is_some()
        || filters.excluded_labels.is_some()
        || filters.author.is_some()
}
