//! Label and assignee pickers on an issue. An issue write replaces the whole
//! set, so a picker's picks become one write when it closes. Ports
//! `issueSelection.ts`.

use t3code_contracts::{IssueAssigneeCandidate, IssueDetail, IssueLabelCandidate};

use crate::pull_request_selection::SelectionDraft;

/// The label set after the picks, or `None` when they leave the issue as the tracker has it.
pub fn issue_labels_after_picks(detail: &IssueDetail, draft: &SelectionDraft<IssueLabelCandidate>) -> Option<Vec<String>> {
    let names: Vec<&str> = detail.labels.iter().map(|l| l.name.as_str()).collect();
    let changes = draft.changes(|name, _| names.contains(&name));
    if changes.add.is_empty() && changes.remove.is_empty() {
        return None;
    }
    let removed: Vec<&str> = changes.remove.iter().map(|l| l.name.as_str()).collect();
    Some(
        names
            .iter()
            .filter(|n| !removed.contains(n))
            .map(|n| (*n).to_owned())
            .chain(changes.add.iter().map(|l| l.name.clone()))
            .collect(),
    )
}

/// Whether the issue already has `candidate` assigned, by login.
pub fn issue_assignee_held(detail: &IssueDetail, candidate: &IssueAssigneeCandidate) -> bool {
    let login = candidate.login.to_lowercase();
    detail.assignees.iter().any(|a| a.login.to_lowercase() == login)
}

/// People already assigned are written by login, the newly picked by the tracker's own id.
pub fn issue_assignees_after_picks(detail: &IssueDetail, draft: &SelectionDraft<IssueAssigneeCandidate>) -> Option<Vec<String>> {
    let changes = draft.changes(|_, candidate| issue_assignee_held(detail, candidate));
    if changes.add.is_empty() && changes.remove.is_empty() {
        return None;
    }
    let removed: Vec<String> = changes.remove.iter().map(|c| c.login.to_lowercase()).collect();
    Some(
        detail
            .assignees
            .iter()
            .map(|a| a.login.clone())
            .filter(|login| !removed.contains(&login.to_lowercase()))
            .chain(changes.add.iter().map(|c| c.id.clone()))
            .collect(),
    )
}

/// The set a closing picker writes: `None` when the tracker has it or was already sent it.
pub fn issue_write_after_picks(after: Option<&[String]>, held: &[String], written: Option<&[String]>) -> Option<Vec<String>> {
    let target = after.unwrap_or(held);
    let sent = written.unwrap_or(held);
    let same = target.len() == sent.len() && target.iter().all(|v| sent.contains(v));
    (!same).then(|| target.to_vec())
}
