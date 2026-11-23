//! The pull requests page's list logic: merging environments' answers,
//! narrowing by involvement, filters and text, grouping, ranking and sorting,
//! and the optimistic state a quick action leaves. Ports
//! `pullRequestList.logic.ts` and `pullRequestListGroups.logic.ts`.
//!
//! Each row carries its lower-cased search text and parsed timestamps, built
//! once when an answer arrives, so a keystroke over 2,000 rows only compares
//! strings that already exist.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use t3code_client_runtime::pull_request_editing::{ObservedPullRequest, is_stale_pull_request_reply, parse_instant_ms};
use t3code_client_runtime::pull_request_query::{js_trim, resolve_pull_request_author_filter};
use t3code_contracts::{
    EnvironmentId, PullRequestAction, PullRequestChecksState, PullRequestInvolvement, PullRequestListEntry,
    PullRequestListEntryAuthor, PullRequestListEntryLabelsItem, PullRequestListFilters, PullRequestListFiltersDraft,
    PullRequestListFiltersReview, PullRequestListProjectError, PullRequestListResult, PullRequestListState,
    PullRequestMergeability, PullRequestProviderSummary, PullRequestReviewDecision, PullRequestState, PullRequestSummary,
};

/// A list row: the host's entry, the environment that answered it, and what filtering reads.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestRow {
    pub environment_id: EnvironmentId,
    pub entry: PullRequestListEntry,
    /// `#<n> <title> <repo> <branch> <author>` lower-cased, what text search matches against.
    pub haystack: String,
    pub title_lower: String,
    pub updated_ms: Option<f64>,
    pub created_ms: Option<f64>,
}

impl PullRequestRow {
    /// A row with its search text and timestamps computed once.
    pub fn new(environment_id: EnvironmentId, entry: PullRequestListEntry) -> Self {
        let haystack = format!(
            "#{} {} {} {} {}",
            entry.number,
            entry.title,
            entry.repository,
            entry.head_branch,
            entry.author.as_ref().map_or("", |a| a.login.as_str())
        )
        .to_lowercase();
        Self {
            title_lower: entry.title.to_lowercase(),
            updated_ms: to_sortable_timestamp(&entry.updated_at),
            created_ms: to_sortable_timestamp(&entry.created_at),
            haystack,
            environment_id,
            entry,
        }
    }
}

/// A shared row; lists hold these so regrouping never copies an entry.
pub type Row = Arc<PullRequestRow>;

/// `toSortableTimestamp`: milliseconds, or `None` for an empty or unreadable date.
pub fn to_sortable_timestamp(iso: &str) -> Option<f64> {
    if iso.is_empty() { None } else { parse_instant_ms(iso) }
}

/// The groups the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullRequestGroupKey {
    ReviewRequested,
    Authored,
    Others,
}

impl PullRequestGroupKey {
    /// The English label, also the translation source.
    pub fn label(self) -> &'static str {
        match self {
            Self::ReviewRequested => "Review requested",
            Self::Authored => "Authored",
            Self::Others => "Others",
        }
    }

    /// The TypeScript key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReviewRequested => "reviewRequested",
            Self::Authored => "authored",
            Self::Others => "others",
        }
    }
}

/// One group of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestGroup {
    pub key: PullRequestGroupKey,
    pub entries: Vec<Row>,
}

/// The logins each environment and host answered as, by `"<environment> <host>"` or host.
pub type PullRequestViewers = BTreeMap<String, String>;

fn normalize(value: Option<&str>) -> Option<String> {
    let trimmed = js_trim(value.unwrap_or("")).to_lowercase();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// A label's color as `#rrggbb`, or `None` when the host sent something else.
pub fn pull_request_label_color(color: Option<&str>) -> Option<String> {
    let hex = js_trim(color.unwrap_or(""));
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{hex}"))
}

/// An author in the filter picker with how many listed rows they wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestAuthorFacet {
    pub actor: PullRequestListEntryAuthor,
    pub count: usize,
    pub merged_count: usize,
}

/// A label in the filter picker with how many listed rows carry it.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestLabelFacet {
    pub label: PullRequestListEntryLabelsItem,
    pub count: usize,
}

/// `String.prototype.localeCompare` close enough for logins and label names: ICU root order
/// compares letters case-insensitively first, punctuation before digits before letters.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    t3code_client_runtime::issue_list::locale_compare(a, b)
}

/// The authors and labels the filter pickers offer for the loaded rows.
pub fn collect_pull_request_list_facets(
    entries: &[Row],
    state: PullRequestListState,
) -> (Vec<PullRequestAuthorFacet>, Vec<PullRequestLabelFacet>) {
    let mut unique: Vec<&Row> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for row in entries {
        let key = pull_request_entry_key(row);
        match index.get(&key) {
            Some(&i) => unique[i] = row,
            None => {
                index.insert(key, unique.len());
                unique.push(row);
            }
        }
    }
    let mut authors: Vec<(String, PullRequestAuthorFacet)> = Vec::new();
    let mut labels: Vec<(String, PullRequestLabelFacet)> = Vec::new();
    for row in unique {
        let entry = &row.entry;
        let in_state = state == PullRequestListState::All || entry.state.as_str() == state.as_str();
        if let Some(author) = &entry.author
            && let Some(key) = normalize(Some(&author.login))
        {
            match authors.iter_mut().find(|(k, _)| *k == key) {
                Some((_, held)) => {
                    held.count += usize::from(in_state);
                    held.merged_count += usize::from(entry.state == PullRequestState::Merged);
                }
                None => authors.push((
                    key,
                    PullRequestAuthorFacet {
                        actor: author.clone(),
                        count: usize::from(in_state),
                        merged_count: usize::from(entry.state == PullRequestState::Merged),
                    },
                )),
            }
        }
        if !in_state {
            continue;
        }
        for label in &entry.labels {
            let Some(key) = normalize(Some(&label.name)) else { continue };
            match labels.iter_mut().find(|(k, _)| *k == key) {
                Some((_, held)) => {
                    held.count += 1;
                    if held.label.color.is_none() {
                        held.label.color = label.color.clone();
                    }
                    held.label.description = label.description.clone();
                }
                None => labels.push((key, PullRequestLabelFacet { label: label.clone(), count: 1 })),
            }
        }
    }
    let mut authors: Vec<_> = authors.into_iter().map(|(_, f)| f).filter(|f| f.count > 0).collect();
    authors.sort_by(|l, r| {
        r.merged_count.cmp(&l.merged_count).then(r.count.cmp(&l.count)).then_with(|| locale_compare(&l.actor.login, &r.actor.login))
    });
    let mut labels: Vec<_> = labels.into_iter().map(|(_, f)| f).collect();
    labels.sort_by(|l, r| r.count.cmp(&l.count).then_with(|| locale_compare(&l.label.name, &r.label.name)));
    (authors, labels)
}

/// How many loaded matches come before the host's answers.
const HOSTED_CHOICE_ROOM: usize = 15;

/// What the author picker offers for `query`: loaded matches lead, then what the host found that
/// no loaded row carries.
pub fn pull_request_author_choices(
    facets: &[PullRequestAuthorFacet],
    hosted: &[PullRequestListEntryAuthor],
    query: &str,
) -> Vec<PullRequestAuthorFacet> {
    let needle = normalize(Some(query));
    let loaded: Vec<&PullRequestAuthorFacet> = facets
        .iter()
        .filter(|f| match &needle {
            None => true,
            Some(n) => f.actor.login.to_lowercase().contains(n) || f.actor.name.as_ref().is_some_and(|name| name.to_lowercase().contains(n)),
        })
        .collect();
    let mut seen: HashSet<Option<String>> = loaded.iter().map(|f| normalize(Some(&f.actor.login))).collect();
    let mut added = Vec::new();
    for actor in hosted {
        let key = normalize(Some(&actor.login));
        if !seen.insert(key.clone()) {
            continue;
        }
        added.push(
            facets
                .iter()
                .find(|f| normalize(Some(&f.actor.login)) == key)
                .cloned()
                .unwrap_or(PullRequestAuthorFacet { actor: actor.clone(), count: 0, merged_count: 0 }),
        );
    }
    let head = loaded.len().min(HOSTED_CHOICE_ROOM);
    loaded[..head].iter().map(|f| (*f).clone()).chain(added).chain(loaded[head..].iter().map(|f| (*f).clone())).collect()
}

/// What the label picker offers for `query`.
pub fn pull_request_label_choices(
    facets: &[PullRequestLabelFacet],
    hosted: &[PullRequestListEntryLabelsItem],
    query: &str,
) -> Vec<PullRequestLabelFacet> {
    let needle = normalize(Some(query));
    let loaded: Vec<PullRequestLabelFacet> =
        facets.iter().filter(|f| needle.as_ref().is_none_or(|n| f.label.name.to_lowercase().contains(n))).cloned().collect();
    let mut seen: HashSet<Option<String>> = loaded.iter().map(|f| normalize(Some(&f.label.name))).collect();
    let mut out = loaded;
    for label in hosted {
        let key = normalize(Some(&label.name));
        if !seen.insert(key.clone()) {
            continue;
        }
        out.push(
            facets
                .iter()
                .find(|f| normalize(Some(&f.label.name)) == key)
                .cloned()
                .unwrap_or(PullRequestLabelFacet { label: label.clone(), count: 0 }),
        );
    }
    out
}

/// The viewer's login on the host and environment that listed `row`.
pub fn pull_request_entry_viewer(row: &PullRequestRow, viewers: &PullRequestViewers) -> Option<String> {
    let key = format!("{} {}", row.environment_id.as_str(), row.entry.host);
    normalize(viewers.get(&key).or_else(|| viewers.get(&row.entry.host)).map(String::as_str))
}

fn is_authored_by_viewer(row: &PullRequestRow, viewers: &PullRequestViewers) -> bool {
    pull_request_entry_viewer(row, viewers).is_some_and(|v| normalize(row.entry.author.as_ref().map(|a| a.login.as_str())).as_deref() == Some(&v))
}

/// Whether the row's visible fields contain the typed text. `needle` must be trimmed and lower case.
pub fn matches_pull_request_query(row: &PullRequestRow, needle: &str) -> bool {
    needle.is_empty() || row.haystack.contains(needle)
}

/// Rows the viewer is involved in as asked.
pub fn filter_pull_requests_by_involvement(rows: &[Row], viewers: &PullRequestViewers, involvement: PullRequestInvolvement) -> Vec<Row> {
    match involvement {
        PullRequestInvolvement::Reviewing => rows.iter().filter(|r| r.entry.viewer_review_requested).cloned().collect(),
        PullRequestInvolvement::Authored => rows.iter().filter(|r| is_authored_by_viewer(r, viewers)).cloned().collect(),
        PullRequestInvolvement::All => rows.to_vec(),
    }
}

/// Whether a row passes the typed filters. `viewer` resolves `author:@me`.
pub fn matches_pull_request_filters(entry: &PullRequestListEntry, filters: &PullRequestListFilters, viewer: Option<&str>) -> bool {
    let holds = |label: &str| {
        let wanted = js_trim(label).to_lowercase();
        entry.labels.iter().any(|l| js_trim(&l.name).to_lowercase() == wanted)
    };
    filters.draft.is_none_or(|d| entry.is_draft == (d == PullRequestListFiltersDraft::Only))
        && filters.review.is_none_or(|r| match r {
            PullRequestListFiltersReview::NoneType => entry.review_decision.is_none(),
            PullRequestListFiltersReview::Approved => entry.review_decision == Some(PullRequestReviewDecision::Approved),
            PullRequestListFiltersReview::ChangesRequested => entry.review_decision == Some(PullRequestReviewDecision::ChangesRequested),
            PullRequestListFiltersReview::ReviewRequired => entry.review_decision == Some(PullRequestReviewDecision::ReviewRequired),
        })
        && filters.labels.as_ref().is_none_or(|groups| groups.iter().all(|g| g.iter().any(|l| holds(l))))
        && filters.excluded_labels.as_ref().is_none_or(|ex| !ex.iter().any(|l| holds(l)))
        && filters.author.as_ref().is_none_or(|author| {
            entry.author.as_ref().map(|a| a.login.to_lowercase()) == Some(resolve_pull_request_author_filter(author, viewer).to_lowercase())
        })
}

/// Rows in authored, review-requested and other groups, empty groups left out.
pub fn group_pull_requests_by_involvement(rows: &[Row], viewers: &PullRequestViewers) -> Vec<PullRequestGroup> {
    let (mut authored, mut review, mut others) = (vec![], vec![], vec![]);
    for row in rows {
        if is_authored_by_viewer(row, viewers) {
            authored.push(row.clone());
        } else if row.entry.viewer_review_requested {
            review.push(row.clone());
        } else {
            others.push(row.clone());
        }
    }
    [(PullRequestGroupKey::Authored, authored), (PullRequestGroupKey::ReviewRequested, review), (PullRequestGroupKey::Others, others)]
        .into_iter()
        .filter(|(_, e)| !e.is_empty())
        .map(|(key, entries)| PullRequestGroup { key, entries })
        .collect()
}

/// A row's identity: environment, host, repository and number.
pub fn pull_request_entry_key(row: &PullRequestRow) -> String {
    format!("{}:{}:{}#{}", row.environment_id.as_str(), row.entry.host, row.entry.repository, row.entry.number)
}

/// Newest update first; rows without a readable date go last.
pub fn compare_pull_requests_by_recency(left: &PullRequestRow, right: &PullRequestRow) -> Ordering {
    match (left.updated_ms, right.updated_ms) {
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (None, None) => Ordering::Equal,
        (Some(l), Some(r)) => r.partial_cmp(&l).unwrap_or(Ordering::Equal),
    }
}

/// Recency pagination can introduce older priority rows; grouping pages would reorder read rows.
pub fn partition_pull_requests_with_priority(entries: &[Row], authored: &[Row], review_requested: &[Row]) -> Vec<PullRequestGroup> {
    // Matched by URL: each list may hold the pull request under a different environment.
    let mut authored_by: Vec<(String, Row)> = Vec::new();
    for row in authored {
        upsert(&mut authored_by, &row.entry.url, row.clone());
    }
    let mut review_by: Vec<(String, Row)> = Vec::new();
    for row in review_requested {
        if !authored_by.iter().any(|(k, _)| *k == row.entry.url) {
            upsert(&mut review_by, &row.entry.url, row.clone());
        }
    }
    let mut others = Vec::new();
    for row in entries {
        let key = &row.entry.url;
        if let Some(slot) = authored_by.iter_mut().find(|(k, _)| k == key) {
            slot.1 = row.clone();
        } else if let Some(slot) = review_by.iter_mut().find(|(k, _)| k == key) {
            slot.1 = row.clone();
        } else {
            others.push(row.clone());
        }
    }
    let sorted = |v: Vec<(String, Row)>| {
        let mut rows: Vec<Row> = v.into_iter().map(|(_, r)| r).collect();
        rows.sort_by(|a, b| compare_pull_requests_by_recency(a, b));
        rows
    };
    [(PullRequestGroupKey::Authored, sorted(authored_by)), (PullRequestGroupKey::ReviewRequested, sorted(review_by)), (PullRequestGroupKey::Others, others)]
        .into_iter()
        .filter(|(_, e)| !e.is_empty())
        .map(|(key, entries)| PullRequestGroup { key, entries })
        .collect()
}

fn upsert(map: &mut Vec<(String, Row)>, key: &str, row: Row) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = row,
        None => map.push((key.to_owned(), row)),
    }
}

/// Diff sizes fetched for rows the list read without them, by [`pull_request_diff_stat_key`].
pub type PullRequestDiffStats = HashMap<String, (u64, u64)>;

/// The key a row's fetched diff size is held under.
pub fn pull_request_diff_stat_key(environment_id: &str, project_id: &str, number: u64) -> String {
    format!("{environment_id} {project_id} {number}")
}

/// The row with a fetched diff size when the host listed it as 0/0.
pub fn with_diff_stat(row: &Row, stats: &PullRequestDiffStats) -> Row {
    if row.entry.additions != 0 || row.entry.deletions != 0 {
        return row.clone();
    }
    match stats.get(&pull_request_diff_stat_key(row.environment_id.as_str(), row.entry.project_id.as_str(), row.entry.number)) {
        None => row.clone(),
        Some(&(additions, deletions)) => {
            let mut next = (**row).clone();
            next.entry.additions = additions;
            next.entry.deletions = deletions;
            Arc::new(next)
        }
    }
}

/// Every environment's answer as one list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MergedPullRequestList {
    pub viewers: PullRequestViewers,
    pub providers: Vec<PullRequestProviderSummary>,
    pub entries: Vec<Row>,
    pub errors: Vec<(EnvironmentId, PullRequestListProjectError)>,
    pub truncated: bool,
    pub next_cursors: BTreeMap<String, BTreeMap<String, String>>,
    pub truncated_environments: Vec<EnvironmentId>,
}

/// Merges the environments' answers. The first environment to answer keeps a pull request that
/// several of them read.
pub fn merge_pull_request_lists(answers: &[(EnvironmentId, Arc<PullRequestListResult>)]) -> Option<MergedPullRequestList> {
    if answers.is_empty() {
        return None;
    }
    let mut merged = MergedPullRequestList::default();
    let mut entries: Vec<(String, Row)> = Vec::new();
    let mut by_url: HashMap<String, usize> = HashMap::new();
    for (env, answer) in answers {
        for (host, login) in &answer.viewers {
            merged.viewers.insert(format!("{} {host}", env.as_str()), login.clone());
        }
        for provider in &answer.providers {
            match merged.providers.iter_mut().find(|p| p.host == provider.host) {
                None => merged.providers.push(provider.clone()),
                Some(held) => {
                    let base = if held.configured { held.clone() } else { provider.clone() };
                    *held = PullRequestProviderSummary {
                        project_count: held.project_count + provider.project_count,
                        searches_on_host: held.searches_on_host && provider.searches_on_host,
                        configured: held.configured || provider.configured,
                        ..base
                    };
                }
            }
        }
        for entry in &answer.entries {
            match by_url.get(&entry.url) {
                None => {
                    by_url.insert(entry.url.clone(), entries.len());
                    entries.push((entry.url.clone(), Arc::new(PullRequestRow::new(env.clone(), entry.clone()))));
                }
                Some(&i) => {
                    let held = &entries[i].1;
                    if held.environment_id == *env && entry.updated_at > held.entry.updated_at {
                        entries[i].1 = Arc::new(PullRequestRow::new(env.clone(), entry.clone()));
                    }
                }
            }
        }
        merged.errors.extend(answer.errors.iter().map(|e| (env.clone(), e.clone())));
        merged.truncated |= answer.truncated;
        if answer.truncated && !merged.truncated_environments.contains(env) {
            merged.truncated_environments.push(env.clone());
        }
        if !answer.next_cursors.is_empty() {
            merged.next_cursors.entry(env.as_str().to_owned()).or_default().extend(answer.next_cursors.clone());
        }
    }
    let mut rows: Vec<Row> = entries.into_iter().map(|(_, r)| r).collect();
    rows.sort_by(|a, b| compare_pull_requests_by_recency(a, b));
    merged.entries = rows;
    Some(merged)
}

/// How well a row matches text the host searched, for ordering rows found locally.
pub fn score_pull_request_match(row: &PullRequestRow, query: &str) -> u32 {
    let needle = js_trim(query).to_lowercase();
    if needle.is_empty() {
        return 0;
    }
    let number = needle.strip_prefix('#').unwrap_or(&needle);
    if row.entry.number.to_string() == number {
        return 100;
    }
    let title = &row.title_lower;
    let terms: Vec<&str> = needle.split_whitespace().collect();
    if *title == needle {
        90
    } else if title.contains(&needle) {
        80
    } else if terms.len() > 1 && terms.iter().all(|t| title.contains(t)) {
        70
    } else if row.entry.head_branch.to_lowercase().contains(&needle) {
        60
    } else if row.entry.author.as_ref().is_some_and(|a| a.login.to_lowercase().contains(&needle)) {
        50
    } else if row.entry.repository.to_lowercase().contains(&needle) {
        40
    } else if terms.iter().any(|t| title.contains(t)) {
        30
    } else {
        10
    }
}

/// Rows by match score, then recency.
pub fn rank_pull_request_matches(rows: &[Row], query: &str) -> Vec<Row> {
    if js_trim(query).is_empty() {
        return rows.to_vec();
    }
    let mut scored: Vec<(u32, Row)> = rows.iter().map(|r| (score_pull_request_match(r, query), r.clone())).collect();
    scored.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then_with(|| compare_pull_requests_by_recency(a, b)));
    scored.into_iter().map(|(_, r)| r).collect()
}

fn measured(row: &PullRequestRow) -> bool {
    row.entry.additions + row.entry.deletions > 0
}

fn size(row: &PullRequestRow) -> i64 {
    (row.entry.additions + row.entry.deletions) as i64
}

/// Rows closest to merging first.
pub fn rank_pull_requests_by_merge_readiness(rows: &[Row]) -> Vec<Row> {
    let tier = |r: &PullRequestRow| {
        let e = &r.entry;
        if e.mergeability == PullRequestMergeability::Conflicting {
            4
        } else if e.state != PullRequestState::Open {
            3
        } else if e.is_draft {
            2
        } else if e.checks_state == Some(PullRequestChecksState::Passing) && e.review_decision == Some(PullRequestReviewDecision::Approved) {
            0
        } else if e.checks_state == Some(PullRequestChecksState::Passing) {
            1
        } else {
            2
        }
    };
    let mut out = rows.to_vec();
    out.sort_by(|l, r| {
        tier(l)
            .cmp(&tier(r))
            .then(measured(r).cmp(&measured(l)))
            .then(size(l).cmp(&size(r)))
            .then_with(|| compare_pull_requests_by_recency(l, r))
    });
    out
}

fn rank_by_tier_then_recency(rows: &[Row], tier: impl Fn(&PullRequestListEntry) -> u8) -> Vec<Row> {
    let mut out = rows.to_vec();
    out.sort_by(|l, r| tier(&l.entry).cmp(&tier(&r.entry)).then_with(|| compare_pull_requests_by_recency(l, r)));
    out
}

/// The author's own rows, what blocks them first.
pub fn rank_pull_requests_blocked_on_author(rows: &[Row]) -> Vec<Row> {
    rank_by_tier_then_recency(rows, |e| {
        if e.state != PullRequestState::Open {
            6
        } else if e.mergeability == PullRequestMergeability::Conflicting {
            0
        } else if e.review_decision == Some(PullRequestReviewDecision::ChangesRequested) {
            1
        } else if e.checks_state == Some(PullRequestChecksState::Failing) {
            2
        } else if e.is_draft {
            3
        } else if e.checks_state == Some(PullRequestChecksState::Passing) && e.review_decision == Some(PullRequestReviewDecision::Approved) {
            5
        } else {
            4
        }
    })
}

/// How the list orders rows within each group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestListSort {
    Ready,
    Blocked,
    #[default]
    Updated,
    Newest,
    Oldest,
    Largest,
    Smallest,
}

impl PullRequestListSort {
    /// Every sort, in menu order.
    pub const ALL: [Self; 7] = [Self::Updated, Self::Ready, Self::Blocked, Self::Newest, Self::Oldest, Self::Largest, Self::Smallest];

    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Blocked => "blocked",
            Self::Updated => "updated",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
            Self::Largest => "largest",
            Self::Smallest => "smallest",
        }
    }

    /// Reads a TypeScript name.
    pub fn from_str(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.as_str() == value)
    }
}

/// Orders the rows within each group.
pub fn sort_pull_request_groups(
    groups: Vec<PullRequestGroup>,
    sort: PullRequestListSort,
    search_text: &str,
    involvement: PullRequestInvolvement,
) -> Vec<PullRequestGroup> {
    let searching = !js_trim(search_text).is_empty();
    let map = |groups: Vec<PullRequestGroup>, rank: &dyn Fn(&[Row]) -> Vec<Row>| {
        groups.into_iter().map(|g| PullRequestGroup { entries: rank(&g.entries), key: g.key }).collect()
    };
    match sort {
        PullRequestListSort::Ready => {
            if searching { groups } else { map(groups, &rank_pull_requests_by_merge_readiness) }
        }
        PullRequestListSort::Blocked => {
            if searching {
                return groups;
            }
            groups
                .into_iter()
                .map(|g| {
                    let role = match g.key {
                        PullRequestGroupKey::Others => involvement,
                        PullRequestGroupKey::Authored => PullRequestInvolvement::Authored,
                        PullRequestGroupKey::ReviewRequested => PullRequestInvolvement::Reviewing,
                    };
                    let entries = match role {
                        PullRequestInvolvement::All => g.entries,
                        PullRequestInvolvement::Authored => rank_pull_requests_blocked_on_author(&g.entries),
                        PullRequestInvolvement::Reviewing => {
                            rank_by_tier_then_recency(&g.entries, |e| u8::from(e.state != PullRequestState::Open))
                        }
                    };
                    PullRequestGroup { key: g.key, entries }
                })
                .collect()
        }
        PullRequestListSort::Updated => groups,
        _ => {
            let timestamp = |r: &PullRequestRow| r.updated_ms.or(r.created_ms).unwrap_or(0.0);
            let by_time = |l: &PullRequestRow, r: &PullRequestRow| timestamp(r).partial_cmp(&timestamp(l)).unwrap_or(Ordering::Equal);
            map(groups, &|rows: &[Row]| {
                let mut out = rows.to_vec();
                out.sort_by(|l, r| match sort {
                    PullRequestListSort::Newest | PullRequestListSort::Oldest => {
                        let dated = l.created_ms.unwrap_or(0.0) - r.created_ms.unwrap_or(0.0);
                        let dated = if sort == PullRequestListSort::Newest { -dated } else { dated };
                        (i8::from(r.created_ms.is_some()) - i8::from(l.created_ms.is_some()))
                            .cmp(&0)
                            .then(dated.partial_cmp(&0.0).unwrap_or(Ordering::Equal))
                            .then_with(|| by_time(l, r))
                    }
                    _ => {
                        let sized = size(l) - size(r);
                        let sized = if sort == PullRequestListSort::Largest { -sized } else { sized };
                        measured(r).cmp(&measured(l)).then(sized.cmp(&0)).then_with(|| by_time(l, r))
                    }
                });
                out
            })
        }
    }
}

/// The state an action leaves a row in until the host says otherwise.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestListOverride {
    pub state: PullRequestState,
    pub is_draft: Option<bool>,
    pub updated_at: String,
    pub token: u64,
    pub at: f64,
    /// Set when the host reported this state, so a newer row can outrank it.
    pub from_reply: bool,
    pub observed_at: Option<f64>,
}

/// The override a quick action leaves, from the host's reply when it sent one.
pub fn pull_request_override_after_action(
    entry: &PullRequestListEntry,
    action: PullRequestAction,
    now_ms: f64,
    now_iso: &str,
    token: u64,
    reply: Option<&PullRequestSummary>,
) -> Option<PullRequestListOverride> {
    if let Some(reply) = reply {
        return Some(PullRequestListOverride {
            state: reply.state,
            is_draft: reply.is_draft,
            updated_at: reply.updated_at.clone(),
            token,
            at: now_ms,
            from_reply: true,
            observed_at: reply.observed_at,
        });
    }
    let stamp = |state, is_draft| PullRequestListOverride {
        state,
        is_draft,
        updated_at: now_iso.to_owned(),
        token,
        at: now_ms,
        from_reply: false,
        observed_at: None,
    };
    Some(match action {
        PullRequestAction::Close => stamp(PullRequestState::Closed, None),
        PullRequestAction::Reopen => stamp(PullRequestState::Open, None),
        PullRequestAction::Merge => stamp(PullRequestState::Merged, None),
        PullRequestAction::Draft => stamp(entry.state, Some(true)),
        PullRequestAction::Ready => stamp(entry.state, Some(false)),
        _ => return None,
    })
}

fn state_matches(state: PullRequestState, list: PullRequestListState) -> bool {
    list == PullRequestListState::All || state.as_str() == list.as_str()
}

/// Rows with pending overrides applied and filtered to the list's state.
pub fn apply_pull_request_overrides(
    rows: &[Row],
    overrides: &HashMap<String, PullRequestListOverride>,
    state: PullRequestListState,
) -> Vec<Row> {
    if overrides.is_empty() && state == PullRequestListState::All {
        return rows.to_vec();
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let held = overrides.get(&pull_request_entry_key(row));
        let override_ = held.filter(|o| {
            !(o.from_reply
                && is_stale_pull_request_reply(
                    &ObservedPullRequest { state: row.entry.state, updated_at: Some(&row.entry.updated_at), observed_at: row.entry.observed_at },
                    &ObservedPullRequest { state: o.state, updated_at: Some(&o.updated_at), observed_at: o.observed_at },
                ))
        });
        if !state_matches(override_.map_or(row.entry.state, |o| o.state), state) {
            continue;
        }
        out.push(match override_ {
            None => row.clone(),
            Some(o) => {
                let mut entry = row.entry.clone();
                entry.state = o.state;
                if let Some(d) = o.is_draft {
                    entry.is_draft = d;
                }
                entry.updated_at = o.updated_at.clone();
                if o.observed_at.is_some() {
                    entry.observed_at = o.observed_at;
                }
                Arc::new(PullRequestRow::new(row.environment_id.clone(), entry))
            }
        });
    }
    out
}

const PULL_REQUEST_OVERRIDE_TRUST_MS: f64 = 60_000.0;

/// Retire overrides only when returned rows agree; partial-feed absence proves nothing.
pub fn settle_pull_request_overrides(
    overrides: &HashMap<String, PullRequestListOverride>,
    answered: &[Row],
    now_ms: f64,
) -> HashMap<String, PullRequestListOverride> {
    let by_key: HashMap<String, &Row> = answered.iter().map(|r| (pull_request_entry_key(r), r)).collect();
    overrides
        .iter()
        .filter(|(key, o)| match by_key.get(*key) {
            None => true,
            Some(row) => {
                let agrees = row.entry.state == o.state && o.is_draft.is_none_or(|d| row.entry.is_draft == d);
                !agrees && now_ms - o.at <= PULL_REQUEST_OVERRIDE_TRUST_MS
            }
        })
        .map(|(k, o)| (k.clone(), o.clone()))
        .collect()
}

/// The actions a row's hover buttons offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestQuickAction {
    Close,
    Merge,
    Ready,
    Reopen,
}

impl PullRequestQuickAction {
    /// The host action it runs.
    pub fn action(self) -> PullRequestAction {
        match self {
            Self::Close => PullRequestAction::Close,
            Self::Merge => PullRequestAction::Merge,
            Self::Ready => PullRequestAction::Ready,
            Self::Reopen => PullRequestAction::Reopen,
        }
    }
}

/// Quick actions for a row's state that its host supports.
pub fn pull_request_quick_actions(entry: &PullRequestListEntry, host_actions: &[PullRequestAction]) -> Vec<PullRequestQuickAction> {
    use PullRequestQuickAction as Q;
    let for_state: &[Q] = match entry.state {
        PullRequestState::Merged => &[],
        PullRequestState::Closed => &[Q::Reopen],
        PullRequestState::Open if entry.is_draft => &[Q::Close, Q::Ready],
        PullRequestState::Open => &[Q::Close, Q::Merge],
    };
    for_state.iter().copied().filter(|q| host_actions.contains(&q.action())).collect()
}

/// The actions each listed host supports, by lower-cased host.
pub fn pull_request_host_actions(providers: &[PullRequestProviderSummary]) -> HashMap<String, Vec<PullRequestAction>> {
    providers.iter().filter_map(|p| p.actions.as_ref().map(|a| (p.host.to_lowercase(), a.clone()))).collect()
}

/// What the page asks of the held rows.
#[derive(Debug, Clone, Copy)]
pub struct PullRequestGroupsInput<'a> {
    pub known: &'a [Row],
    /// The text-free rows already loaded, or `None` when the typed filters cannot vet them.
    pub baseline: Option<&'a [Row]>,
    pub providers: &'a [PullRequestProviderSummary],
    pub viewers: &'a PullRequestViewers,
    pub involvement: PullRequestInvolvement,
    pub local_filters: &'a PullRequestListFilters,
    pub has_local_filters: bool,
    pub text: &'a str,
    pub answered_locally: bool,
    pub baseline_empty: bool,
    pub authored: Option<&'a [Row]>,
    pub reviewing: Option<&'a [Row]>,
    pub partitions_wanted: bool,
}

/// The held rows narrowed to what the reader asked for, in the groups the list shows. Ports
/// `pullRequestGroups`.
pub fn pull_request_groups(input: PullRequestGroupsInput<'_>) -> (Vec<Row>, Vec<PullRequestGroup>) {
    let searching_hosts: HashSet<&str> =
        input.providers.iter().filter(|p| p.searches_on_host).map(|p| p.host.as_str()).collect();
    let passes_filters = |row: &Row| {
        !input.has_local_filters
            || matches_pull_request_filters(&row.entry, input.local_filters, pull_request_entry_viewer(row, input.viewers).as_deref())
    };
    let narrow_rows = |rows: &[Row]| -> Vec<Row> {
        filter_pull_requests_by_involvement(rows, input.viewers, input.involvement).into_iter().filter(passes_filters).collect()
    };
    let needle = js_trim(input.text).to_lowercase();
    let entries: Vec<Row> = {
        let narrowed = narrow_rows(input.known);
        if input.text.is_empty() {
            let mut sorted = narrowed;
            sorted.sort_by(|a, b| compare_pull_requests_by_recency(a, b));
            sorted
        } else {
            // Host text searches cover fields absent from rows, so do not narrow their matches locally.
            let matched: Vec<Row> = narrowed
                .into_iter()
                .filter(|r| (input.answered_locally && searching_hosts.contains(r.entry.host.as_str())) || matches_pull_request_query(r, &needle))
                .collect();
            let listed: HashSet<String> = matched.iter().map(|r| pull_request_entry_key(r)).collect();
            let missed: Vec<Row> = narrow_rows(input.baseline.unwrap_or(&[]))
                .into_iter()
                .filter(|r| !listed.contains(&pull_request_entry_key(r)) && matches_pull_request_query(r, &needle))
                .collect();
            if missed.is_empty() { matched } else { matched.into_iter().chain(rank_pull_request_matches(&missed, input.text)).collect() }
        }
    };
    let groups = if input.involvement != PullRequestInvolvement::All {
        vec![PullRequestGroup { key: PullRequestGroupKey::Others, entries: entries.clone() }]
    } else if input.baseline_empty {
        group_pull_requests_by_involvement(&entries, input.viewers)
    } else {
        let narrow = |rows: Option<&[Row]>| rows.map(|r| r.iter().filter(|row| passes_filters(row)).cloned().collect::<Vec<_>>());
        let authored = narrow(input.partitions_wanted.then_some(input.authored).flatten());
        let reviewing = narrow(input.partitions_wanted.then_some(input.reviewing).flatten());
        match (authored, reviewing) {
            (Some(a), Some(r)) => partition_pull_requests_with_priority(&entries, &a, &r),
            _ => group_pull_requests_by_involvement(&entries, input.viewers),
        }
    };
    (entries, groups)
}
