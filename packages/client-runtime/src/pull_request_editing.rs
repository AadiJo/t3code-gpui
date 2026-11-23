//! Who may edit a pull request's title, description, comments and maintainer
//! edits, and how long a locally saved description outranks the host's copy.
//! Ports `pullRequestEditing.ts` and `pullRequestReply.ts`.

use t3code_contracts::{PullRequestCommentKind, PullRequestDetail, PullRequestReviewThread, PullRequestState};

use crate::pull_request_query::js_trim;

fn same_login(one: Option<&str>, other: Option<&str>) -> bool {
    match (one, other) {
        (Some(a), Some(b)) => js_trim(a).to_lowercase() == js_trim(b).to_lowercase(),
        _ => false,
    }
}

/// Whether the viewer may edit the title and description.
pub fn can_edit_pull_request_change_request(detail: &PullRequestDetail) -> bool {
    if detail.capabilities.edit.as_ref().map(|e| e.change_request) != Some(true) {
        return false;
    }
    if let Some(allowed) = detail.viewer_permissions.edit_change_request {
        return allowed;
    }
    same_login(detail.viewer.as_deref(), detail.author.as_ref().map(|a| a.login.as_str()))
        || detail.viewer_permissions.actions.contains(&t3code_contracts::PullRequestAction::Merge)
}

/// Whether the "Allow edits by maintainers" switch is shown and enabled.
pub fn can_edit_pull_request_maintainer_edits(detail: &PullRequestDetail) -> bool {
    detail.state == PullRequestState::Open
        && detail.capabilities.edit.as_ref().and_then(|e| e.maintainer_edits) == Some(true)
        && detail.viewer_permissions.maintainer_edits == Some(true)
        && detail.maintainer_can_modify.is_some()
}

/// Whether the viewer may edit a comment of `kind` by `author`. Hosts do not support rewriting
/// review summaries through comment mutations.
pub fn can_edit_pull_request_comment(
    detail: &PullRequestDetail,
    kind: PullRequestCommentKind,
    author: Option<&str>,
    can_edit: Option<bool>,
) -> bool {
    if detail.capabilities.edit.as_ref().map(|e| e.comment) != Some(true) {
        return false;
    }
    if !matches!(kind, PullRequestCommentKind::IssueComment | PullRequestCommentKind::ReviewComment) {
        return false;
    }
    can_edit.unwrap_or_else(|| same_login(detail.viewer.as_deref(), author))
}

/// Whether the viewer may resolve or unresolve a review thread.
pub fn can_resolve_pull_request_thread(detail: &PullRequestDetail, thread: &PullRequestReviewThread) -> bool {
    detail.viewer_permissions.resolve && thread.can_resolve != Some(false)
}

/// A saved choice or description, tied to the pull request it was written for and the host
/// reading it was made against.
#[derive(Debug, Clone, PartialEq)]
pub struct DescriptionDraft {
    pub key: String,
    pub observed_at: Option<f64>,
    /// `None` once the last save failed: until a host reading, nobody knows which saves landed.
    pub body: Option<String>,
    /// Saves still running. The host applies them in order, so the last one is `body`.
    pub saves: u32,
}

/// A saved choice ends at the first host reading delivered after the save. Readers refresh once
/// it succeeds, which drops reads still in flight, so that reading started after the write.
/// Returns the observation the choice holds after this check, or `None` when it no longer applies.
pub fn choice_in_force(
    choice_key: &str,
    choice_observed_at: Option<f64>,
    key: &str,
    observed_at: Option<f64>,
    saving: bool,
) -> Option<Option<f64>> {
    if choice_key != key {
        return None;
    }
    if saving {
        return Some(observed_at);
    }
    (observed_at.is_some() && choice_observed_at == observed_at).then_some(choice_observed_at)
}

/// The draft shown in place of the host's description, if it still applies.
pub fn description_draft_in_force(draft: Option<&DescriptionDraft>, key: &str, observed_at: Option<f64>) -> Option<DescriptionDraft> {
    let draft = draft?;
    let observed = choice_in_force(&draft.key, draft.observed_at, key, observed_at, draft.saves > 0)?;
    Some(DescriptionDraft { observed_at: observed, ..draft.clone() })
}

/// The draft after starting a save of `body`.
pub fn start_description_save(draft: Option<&DescriptionDraft>, url: &str, observed_at: Option<f64>, body: &str) -> DescriptionDraft {
    let saves = draft.filter(|d| d.key == url).map_or(0, |d| d.saves);
    DescriptionDraft { key: url.to_owned(), observed_at, body: Some(body.to_owned()), saves: saves + 1 }
}

/// Each save writes the whole body, so a failure only loses `body` when no later save is running.
pub fn settle_description_save(draft: Option<&DescriptionDraft>, key: &str, saved: bool) -> Option<DescriptionDraft> {
    let draft = draft?;
    if draft.key != key || draft.saves == 0 {
        return Some(draft.clone());
    }
    let saves = draft.saves - 1;
    Some(DescriptionDraft {
        saves,
        body: if saved || saves > 0 { draft.body.clone() } else { None },
        ..draft.clone()
    })
}

/// What a reply or a shown row says about a pull request's state, for staleness checks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservedPullRequest<'a> {
    pub state: PullRequestState,
    pub updated_at: Option<&'a str>,
    pub observed_at: Option<f64>,
}

/// `Date.parse` for the ISO timestamps hosts send. `None` where JavaScript gives `NaN`.
pub fn parse_instant_ms(value: &str) -> Option<f64> {
    if let Ok(ts) = value.parse::<jiff::Timestamp>() {
        return Some(ts.as_millisecond() as f64);
    }
    // A date-time with no offset is local time in JavaScript; a bare date is UTC.
    if let Ok(date) = value.parse::<jiff::civil::Date>() {
        let midnight = date.to_datetime(jiff::civil::Time::midnight());
        return jiff::tz::TimeZone::UTC.to_timestamp(midnight).ok().map(|ts| ts.as_millisecond() as f64);
    }
    value
        .parse::<jiff::civil::DateTime>()
        .ok()
        .and_then(|dt| jiff::tz::TimeZone::system().to_timestamp(dt).ok())
        .map(|ts| ts.as_millisecond() as f64)
}

/// Older than what is shown by host update time, then server read time, or a merge undone.
pub fn is_stale_pull_request_reply(shown: &ObservedPullRequest<'_>, reply: &ObservedPullRequest<'_>) -> bool {
    if shown.state == PullRequestState::Merged && reply.state != PullRequestState::Merged {
        return true;
    }
    if let (Some(shown_at), Some(reply_at)) = (shown.updated_at, reply.updated_at)
        && let (Some(a), Some(b)) = (parse_instant_ms(reply_at), parse_instant_ms(shown_at))
    {
        let updated = a - b;
        if updated != 0.0 {
            return updated < 0.0;
        }
    }
    reply.observed_at.unwrap_or(f64::NEG_INFINITY) < shown.observed_at.unwrap_or(f64::NEG_INFINITY)
}
