//! The run and session facts the thread details card's Lineage section acts on
//! (`threadWorkflows.ts`): which run "Merge back" sends, and whether "Disconnect agent session"
//! has a session to stop. The queue's derivations live in [`super::thread_detail`].

use std::sync::Arc;

use t3code_contracts::{
    OrchestrationV2ProviderSession, OrchestrationV2ProviderSessionStatus as SessionStatus, OrchestrationV2Run,
    OrchestrationV2RunStatus as RunStatus,
};
use t3code_shared::thread_projection::ThreadProjection;

use super::thread_detail::active_run;

/// The run a fork merges back (`resolveLatestMergeBackRun`): the newest run the provider
/// finished (`waiting` while its checkpoint is captured, or `completed`), unless a newer run is
/// still preparing, starting or running.
pub fn resolve_latest_merge_back_run(runs: &[Arc<OrchestrationV2Run>]) -> Option<&Arc<OrchestrationV2Run>> {
    let latest = runs
        .iter()
        .filter(|run| matches!(run.status, RunStatus::Waiting | RunStatus::Completed))
        .fold(None::<&Arc<OrchestrationV2Run>>, |latest, run| match latest {
            Some(latest) if run.ordinal <= latest.ordinal => Some(latest),
            _ => Some(run),
        })?;
    let newer_active = runs
        .iter()
        .any(|run| run.ordinal > latest.ordinal && matches!(run.status, RunStatus::Preparing | RunStatus::Starting | RunStatus::Running));
    (!newer_active).then_some(latest)
}

/// The provider session the thread talks through (`resolveThreadProviderSession`): the active
/// run's provider thread's, else any provider thread of this thread with a session, else the
/// newest session that is neither stopped nor failed.
pub fn resolve_thread_provider_session(projection: &ThreadProjection) -> Option<&Arc<OrchestrationV2ProviderSession>> {
    let provider_thread_id = active_run(projection)
        .and_then(|run| run.provider_thread_id.as_ref())
        .or(projection.thread.active_provider_thread_id.as_ref());
    let active = provider_thread_id.and_then(|id| projection.provider_threads.iter().find(|thread| &thread.id == id));
    let attached = active.or_else(|| {
        projection
            .provider_threads
            .iter()
            .find(|thread| thread.app_thread_id.as_ref() == Some(&projection.thread.id) && thread.provider_session_id.is_some())
    });
    if let Some(session_id) = attached.and_then(|thread| thread.provider_session_id.as_ref()) {
        return projection.provider_sessions.iter().find(|session| &session.id == session_id);
    }
    projection.provider_sessions.iter().rev().find(|session| !matches!(session.status, SessionStatus::Stopped | SessionStatus::Error))
}

/// Whether "Disconnect agent session" has a live session to stop (`canDetachThreadProviderSession`).
pub fn can_detach_thread_provider_session(projection: &ThreadProjection) -> bool {
    resolve_thread_provider_session(projection).is_some_and(|session| !matches!(session.status, SessionStatus::Stopped | SessionStatus::Error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_fixtures::{app_thread, projection, run};
    use serde_json::json;

    fn runs(values: &[(&str, &str, u64)]) -> ThreadProjection {
        let runs: Vec<_> = values.iter().map(|(id, status, ordinal)| run(json!({"id": id, "status": status, "ordinal": ordinal}))).collect();
        projection(app_thread(json!({})), json!({ "runs": runs }))
    }

    #[test]
    fn merges_the_newest_provider_finished_run_while_checkpoint_capture_is_pending() {
        let projection = runs(&[("newest-queued", "queued", 3), ("older-completed", "completed", 1), ("newest-finished", "waiting", 2)]);
        assert_eq!(resolve_latest_merge_back_run(&projection.runs).map(|r| r.id.as_str()), Some("newest-finished"));
    }

    #[test]
    fn does_not_merge_older_history_while_a_newer_run_is_active() {
        for status in ["preparing", "starting", "running"] {
            let projection = runs(&[("older-completed", "completed", 1), ("newer-active", status, 2)]);
            assert!(resolve_latest_merge_back_run(&projection.runs).is_none(), "{status}");
        }
    }
}
