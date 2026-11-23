//! The issues page's list: pages held per environment, "Load more" laid over
//! them, and rows grouped by tracker. Ports `issueList.ts` and the page merge
//! in `state/issues.ts`.

use std::collections::{BTreeMap, HashMap, HashSet};

use t3code_contracts::{EnvironmentId, IssueListEntry, IssueListProjectError, IssueListResult, IssueListSort, IssueTrackerSummary};

use crate::pull_request_editing::parse_instant_ms;

/// Cursors to continue a listing, by `"<host> <scope>"`.
pub type IssueListCursors = BTreeMap<String, String>;

/// One row's identity across list pages and detail reads; hosts compare case-insensitively.
pub fn issue_entry_key(entry: &IssueListEntry) -> String {
    format!(
        "{}\u{0}{}\u{0}{}\u{0}{}",
        entry.project_id.as_str(),
        entry.host.to_lowercase(),
        entry.scope.to_lowercase(),
        entry.key
    )
}

fn by_updated_desc(a: &IssueListEntry, b: &IssueListEntry) -> std::cmp::Ordering {
    let at = |e: &IssueListEntry| parse_instant_ms(&e.updated_at);
    js_number_order(at(b), at(a))
}

/// Orders like a JavaScript comparator returning `left - right`, where `NaN` compares equal.
fn js_number_order(left: Option<f64>, right: Option<f64>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(l), Some(r)) => l.partial_cmp(&r).unwrap_or(std::cmp::Ordering::Equal),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Lays a continuation page over the rows already held. A row the page repeats replaces the
/// held one; order stays newest-updated first.
pub fn merge_issue_list_page(held: &[IssueListEntry], page: &[IssueListEntry]) -> Vec<IssueListEntry> {
    if held.is_empty() {
        return page.to_vec();
    }
    if page.is_empty() {
        return held.to_vec();
    }
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, IssueListEntry> = HashMap::new();
    for entry in held.iter().chain(page) {
        let key = issue_entry_key(entry);
        if by_key.insert(key.clone(), entry.clone()).is_none() {
            order.push(key);
        }
    }
    let mut merged: Vec<IssueListEntry> = order.into_iter().filter_map(|k| by_key.remove(&k)).collect();
    merged.sort_by(by_updated_desc);
    merged
}

/// Rows from one environment's answer with any continuation pages laid over them.
#[derive(Debug, Clone, PartialEq)]
pub struct HeldIssuePage {
    pub answer: IssueListResult,
    pub entries: Vec<IssueListEntry>,
    pub errors: Vec<IssueListProjectError>,
    /// What continuations reported, each beside the cursors it was asked to carry on.
    pub more_errors: Vec<(Vec<String>, Vec<IssueListProjectError>)>,
    pub next_cursors: IssueListCursors,
    /// Why the last "Load more" failed, until a retry succeeds or a new first page arrives.
    pub more_failure: Option<String>,
}

impl HeldIssuePage {
    /// A fresh first page.
    pub fn new(answer: IssueListResult) -> Self {
        Self {
            entries: answer.entries.clone(),
            errors: answer.errors.clone(),
            more_errors: Vec::new(),
            next_cursors: answer.next_cursors.clone(),
            more_failure: None,
            answer,
        }
    }
}

/// The tracker key rows are grouped under.
pub fn issue_tracker_key(tracker: &str, host: &str) -> String {
    format!("{tracker} {}", host.to_lowercase())
}

/// Continuations are laid over the first page they continue; a new first page forgets them.
pub fn hold_issue_pages(
    answers: &[(EnvironmentId, IssueListResult)],
    pages: &HashMap<EnvironmentId, HeldIssuePage>,
) -> HashMap<EnvironmentId, HeldIssuePage> {
    answers
        .iter()
        .map(|(env, answer)| {
            let page = match pages.get(env) {
                Some(previous) if previous.answer == *answer => previous.clone(),
                _ => HeldIssuePage::new(answer.clone()),
            };
            (env.clone(), page)
        })
        .collect()
}

/// The cursors of one tracker host, so its "Load more" leaves the other trackers where they are.
pub fn issue_cursors_for_host(cursors: &IssueListCursors, host: &str) -> IssueListCursors {
    let prefix = format!("{host} ");
    cursors.iter().filter(|(k, _)| k.starts_with(&prefix)).map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Lays the next page over the rows held; a failure keeps them and says why. Only the cursors
/// in `asked` move, and what an earlier continuation reported for any of them is replaced.
pub fn continue_issue_page(page: &HeldIssuePage, next: Result<&IssueListResult, &str>, asked: &IssueListCursors) -> HeldIssuePage {
    let next = match next {
        Err(failure) => return HeldIssuePage { more_failure: Some(failure.to_owned()), ..page.clone() },
        Ok(next) => next,
    };
    let mut more_errors: Vec<_> =
        page.more_errors.iter().filter(|(keys, _)| !keys.iter().any(|k| asked.contains_key(k))).cloned().collect();
    if !next.errors.is_empty() {
        more_errors.push((asked.keys().cloned().collect(), next.errors.clone()));
    }
    let mut next_cursors: IssueListCursors =
        page.next_cursors.iter().filter(|(k, _)| !asked.contains_key(*k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    next_cursors.extend(next.next_cursors.iter().map(|(k, v)| (k.clone(), v.clone())));
    HeldIssuePage {
        answer: page.answer.clone(),
        entries: merge_issue_list_page(&page.entries, &next.entries),
        errors: page.answer.errors.iter().cloned().chain(more_errors.iter().flat_map(|(_, e)| e.iter().cloned())).collect(),
        more_errors,
        next_cursors,
        more_failure: None,
    }
}

/// Newest first by the date the trackers were asked to sort on, so merged rows keep that order.
pub fn issue_order(sort: IssueListSort) -> impl Fn(&IssueListEntry, &IssueListEntry) -> std::cmp::Ordering {
    move |left, right| {
        let date = |e: &IssueListEntry| parse_instant_ms(if sort == IssueListSort::Created { &e.created_at } else { &e.updated_at });
        js_number_order(date(right), date(left))
    }
}

/// An issue row with the environment that listed it.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentIssueEntry {
    pub environment_id: EnvironmentId,
    pub entry: IssueListEntry,
}

/// A project error shown under a tracker group.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueGroupError {
    pub project_title: String,
    pub message: String,
    pub environment_id: EnvironmentId,
}

/// One tracker's rows on the issues page.
#[derive(Debug, Clone, PartialEq)]
pub struct IssueTrackerGroup {
    pub tracker: IssueTrackerSummary,
    pub rows: Vec<EnvironmentIssueEntry>,
    pub errors: Vec<IssueGroupError>,
    /// Environments with another page for this tracker.
    pub more: Vec<EnvironmentId>,
}

/// Rows from every environment grouped by tracker, deduplicated by URL, narrowed locally for
/// trackers that cannot search. `error_host` names the tracker key a project's error belongs to.
pub fn group_issues(
    held: &[(EnvironmentId, HeldIssuePage)],
    query: &str,
    sort: IssueListSort,
    error_tracker_key: impl Fn(&EnvironmentId, &IssueListProjectError) -> Option<String>,
) -> Vec<IssueTrackerGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, IssueTrackerGroup> = HashMap::new();
    let mut listed: HashSet<&str> = HashSet::new();
    let needle = query.to_lowercase();
    for (env, page) in held {
        for tracker in &page.answer.trackers {
            let key = issue_tracker_key(tracker.tracker.as_str(), &tracker.host);
            let group = groups.entry(key.clone()).or_insert_with(|| {
                order.push(key);
                IssueTrackerGroup { tracker: tracker.clone(), rows: vec![], errors: vec![], more: vec![] }
            });
            if !group.tracker.configured && tracker.configured {
                group.tracker = tracker.clone();
            }
            let prefix = format!("{} ", tracker.host);
            if page.next_cursors.keys().any(|c| c.starts_with(&prefix)) {
                group.more.push(env.clone());
            }
        }
        for entry in &page.entries {
            let Some(group) = groups.get_mut(&issue_tracker_key(entry.tracker.as_str(), &entry.host)) else { continue };
            // A tracker that cannot search is narrowed here instead.
            if !needle.is_empty()
                && group.tracker.capabilities.as_ref().map(|c| c.search) != Some(true)
                && !entry.title.to_lowercase().contains(&needle)
                && !entry.display_key.to_lowercase().contains(&needle)
            {
                continue;
            }
            // Environments holding the same repository answer the same issue; the first one keeps it.
            if !listed.insert(entry.url.as_str()) {
                continue;
            }
            group.rows.push(EnvironmentIssueEntry { environment_id: env.clone(), entry: entry.clone() });
        }
        for error in &page.errors {
            let target = error_tracker_key(env, error).filter(|k| groups.contains_key(k)).or_else(|| order.first().cloned());
            if let Some(group) = target.and_then(|k| groups.get_mut(&k)) {
                group.errors.push(IssueGroupError {
                    project_title: error.project_title.clone(),
                    message: error.message.clone(),
                    environment_id: env.clone(),
                });
            }
        }
    }
    let compare = issue_order(sort);
    let mut out: Vec<IssueTrackerGroup> = order.into_iter().filter_map(|k| groups.remove(&k)).collect();
    for group in &mut out {
        group.rows.sort_by(|a, b| compare(&a.entry, &b.entry));
    }
    out.sort_by(|a, b| {
        locale_compare(
            &issue_tracker_key(a.tracker.tracker.as_str(), &a.tracker.host),
            &issue_tracker_key(b.tracker.tracker.as_str(), &b.tracker.host),
        )
    });
    out
}

/// An approximation of `String.prototype.localeCompare` for ASCII keys: case-insensitive first,
/// lower case before upper on ties, as ICU's root collation orders them.
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let fold = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    fold(a)
        .cmp(&fold(b))
        .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        .then_with(|| b.cmp(a))
}
