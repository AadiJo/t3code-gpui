//! Which run a thread last executed, and whether its worktree is still pending
//! (`orchestrationV2ThreadError.ts`: `latestExecutedRun`, `runRanAfter`, `orchestrationV2WorktreePending`).

use t3code_contracts::{OrchestrationV2Run, OrchestrationV2RunStatus as Status, OrchestrationV2ThreadLaunchWorkspaceStrategy};

use super::models::parse_ms;

/// Whether `run` ran after `other`: held queues can execute out of ordinal order, and an
/// unfinished run counts as latest.
pub fn run_ran_after(run: &OrchestrationV2Run, other: &OrchestrationV2Run) -> bool {
    let end = |r: &OrchestrationV2Run| r.completed_at.as_deref().and_then(parse_ms).map_or(f64::INFINITY, |ms| ms as f64);
    let (a, b) = (end(run), end(other));
    if a == b { run.ordinal > other.ordinal } else { a > b }
}

/// The last run that executed: queued runs and runs cancelled before starting do not count.
pub fn latest_executed_run<'a>(runs: impl IntoIterator<Item = &'a OrchestrationV2Run>) -> Option<&'a OrchestrationV2Run> {
    let mut latest: Option<&OrchestrationV2Run> = None;
    for run in runs {
        if run.status == Status::Queued || (run.status == Status::Cancelled && run.started_at.is_none()) {
            continue;
        }
        if latest.is_none_or(|l| run_ran_after(run, l)) {
            latest = Some(run);
        }
    }
    latest
}

/// Work started now would run in the project checkout, not the worktree the thread asked for:
/// no worktree yet and the latest run is a worktree setup that is preparing, failed or was
/// interrupted.
pub fn worktree_pending<'a>(worktree_path: Option<&str>, runs: impl IntoIterator<Item = &'a OrchestrationV2Run>) -> bool {
    worktree_path.is_none()
        && latest_executed_run(runs).is_some_and(|run| {
            matches!(run.status, Status::Preparing | Status::Failed | Status::Interrupted)
                && matches!(run.workspace_preparation, Some(OrchestrationV2ThreadLaunchWorkspaceStrategy::Worktree(_)))
        })
}
