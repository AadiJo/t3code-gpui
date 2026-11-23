//! Checks on a pull request: laying a newer checks read over the detail, the
//! poll interval, and the rollup the summary and list show. Ports
//! `pullRequestChecks.ts` and the rollup in `pullRequestPresentation.tsx`.

use t3code_contracts::{PullRequestCheck, PullRequestCheckStatus, PullRequestChecks, PullRequestDetail, PullRequestState};

/// The detail with a separately read checks list laid over it when that read is newer.
pub fn resolve_pull_request_detail_checks(
    detail: Option<&PullRequestDetail>,
    checks: Option<&PullRequestChecks>,
    checks_are_newer: bool,
) -> Option<PullRequestDetail> {
    let detail = detail?;
    let Some(checks) = checks else { return Some(detail.clone()) };
    let newer = match (detail.observed_at, checks.observed_at) {
        (Some(d), Some(c)) => c >= d,
        _ => checks_are_newer,
    };
    if !newer {
        return Some(detail.clone());
    }
    Some(PullRequestDetail {
        state: if detail.state == PullRequestState::Merged { PullRequestState::Merged } else { checks.state },
        checks: checks.checks.clone(),
        ..detail.clone()
    })
}

/// Checks still running are worth a tighter poll than a settled head.
pub fn pull_request_checks_refresh_interval_ms(checks: &[PullRequestCheck]) -> u64 {
    if checks.is_empty()
        || checks.iter().any(|c| matches!(c.status, PullRequestCheckStatus::Pending | PullRequestCheckStatus::ActionRequired))
    {
        45_000
    } else {
        60_000
    }
}

/// The one state a set of checks reads as. Ports `classifyPullRequestChecks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksClass {
    None,
    Pending,
    Failing,
    Passing,
}

impl ChecksClass {
    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Failing => "failing",
            Self::Passing => "passing",
        }
    }
}

fn is_failed(status: PullRequestCheckStatus) -> bool {
    matches!(status, PullRequestCheckStatus::Failure | PullRequestCheckStatus::Cancelled)
}

/// Failing wins over pending, which wins over passing.
pub fn classify_pull_request_checks(checks: &[PullRequestCheck]) -> ChecksClass {
    if checks.is_empty() {
        ChecksClass::None
    } else if checks.iter().any(|c| is_failed(c.status)) {
        ChecksClass::Failing
    } else if checks.iter().any(|c| matches!(c.status, PullRequestCheckStatus::Pending | PullRequestCheckStatus::ActionRequired)) {
        ChecksClass::Pending
    } else {
        ChecksClass::Passing
    }
}

/// Checks split the way the checks popover lists them. Ports `groupPullRequestChecks`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupedChecks<'a> {
    pub attention: Vec<&'a PullRequestCheck>,
    pub running: Vec<&'a PullRequestCheck>,
    pub completed: Vec<&'a PullRequestCheck>,
}

/// Groups checks into needs-attention, running and completed.
pub fn group_pull_request_checks(checks: &[PullRequestCheck]) -> GroupedChecks<'_> {
    use PullRequestCheckStatus as S;
    let mut groups = GroupedChecks::default();
    for check in checks {
        match check.status {
            S::Failure | S::Cancelled | S::ActionRequired => groups.attention.push(check),
            S::Pending => groups.running.push(check),
            S::Success | S::Skipped | S::Neutral => groups.completed.push(check),
        }
    }
    groups
}

/// Counts that the checks summaries are written from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChecksCounts {
    pub total: usize,
    pub failed: usize,
    pub pending: usize,
    pub action_required: usize,
    /// Action-required checks that are a workflow run waiting for approval.
    pub workflow_approvals: usize,
    pub passed: usize,
}

/// Counts `checks` by status. A workflow approval is an action-required check whose URL is a
/// GitHub Actions run, as `isWorkflowApprovalCheck` reads it.
pub fn count_pull_request_checks(checks: &[PullRequestCheck]) -> ChecksCounts {
    let mut counts = ChecksCounts { total: checks.len(), ..ChecksCounts::default() };
    for check in checks {
        match check.status {
            s if is_failed(s) => counts.failed += 1,
            PullRequestCheckStatus::Pending => counts.pending += 1,
            PullRequestCheckStatus::ActionRequired => {
                counts.action_required += 1;
                if is_workflow_approval_check(check) {
                    counts.workflow_approvals += 1;
                }
            }
            PullRequestCheckStatus::Success => counts.passed += 1,
            _ => {}
        }
    }
    counts
}

/// An action-required check that links to a workflow run awaiting approval.
pub fn is_workflow_approval_check(check: &PullRequestCheck) -> bool {
    check.status == PullRequestCheckStatus::ActionRequired
        && check.url.as_deref().is_some_and(|url| WORKFLOW_RUN.is_match(url))
}

static WORKFLOW_RUN: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"/actions/runs/\d+(?:/|$)").unwrap());
