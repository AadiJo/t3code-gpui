//! Typed records for the state tests, built from a complete wire object with the fields a test
//! names merged over it (the fork's tests cast partial objects; these decode real shapes).

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use t3code_shared::thread_projection::ThreadProjection;

/// Shallow-merges `overrides` into `base` and decodes the result.
fn merged<T: DeserializeOwned>(mut base: Value, overrides: Value) -> T {
    if let (Value::Object(base), Value::Object(overrides)) = (&mut base, overrides) {
        base.extend(overrides);
    }
    serde_json::from_value(base).expect("fixture decodes")
}

/// A thread shell row with `overrides` applied.
pub fn shell(overrides: Value) -> t3code_contracts::OrchestrationV2ThreadShell {
    merged(
        json!({
            "createdBy": "user", "creationSource": "web", "id": "thread", "projectId": "project", "title": "Thread",
            "providerInstanceId": "codex", "modelSelection": {"instanceId": "codex", "model": "gpt-5"},
            "runtimeMode": "full-access", "interactionMode": "default", "branch": null, "worktreePath": null,
            "lineage": {"parentThreadId": null, "relationshipToParent": null, "rootThreadId": "thread"},
            "forkedFrom": null, "activeProviderThreadId": null, "latestRunId": null, "activeRunId": null,
            "status": "completed", "pendingRuntimeRequest": null, "latestVisibleMessage": null,
            "latestUserMessageAt": null, "hasActionableProposedPlan": false, "itemCount": 0, "visibleItemCount": 0,
            "createdAt": "2026-06-01T00:00:00.000Z", "updatedAt": "2026-06-20T00:00:00.000Z", "archivedAt": null,
            "settledOverride": null, "settledAt": null, "deletedAt": null
        }),
        overrides,
    )
}

/// The app thread a projection carries.
pub fn app_thread(overrides: Value) -> Value {
    let mut base = json!({
        "createdBy": "user", "creationSource": "web", "id": "thread", "projectId": "project", "title": "Thread",
        "providerInstanceId": "codex", "modelSelection": {"instanceId": "codex", "model": "gpt-5"},
        "runtimeMode": "full-access", "interactionMode": "default", "branch": null, "worktreePath": null,
        "activeProviderThreadId": null,
        "lineage": {"parentThreadId": null, "relationshipToParent": null, "rootThreadId": "thread"},
        "forkedFrom": null, "createdAt": "2026-06-01T00:00:00.000Z", "updatedAt": "2026-06-01T00:00:00.000Z",
        "archivedAt": null, "settledOverride": null, "settledAt": null, "snoozedUntil": null, "snoozedAt": null,
        "lastVisitedAt": null, "deletedAt": null
    });
    if let (Value::Object(base), Value::Object(overrides)) = (&mut base, overrides) {
        base.extend(overrides);
    }
    base
}

/// A run of `thread`.
pub fn run(overrides: Value) -> Value {
    let mut base = json!({
        "id": "run", "threadId": "thread", "ordinal": 1, "providerInstanceId": "codex",
        "modelSelection": {"instanceId": "codex", "model": "gpt-5"}, "providerThreadId": null,
        "userMessageId": "message", "rootNodeId": "node", "activeAttemptId": null, "status": "completed",
        "queuePosition": null, "requestedAt": "2026-06-01T00:00:00.000Z", "startedAt": null, "completedAt": null,
        "checkpointId": null, "contextHandoffId": null
    });
    if let (Value::Object(base), Value::Object(overrides)) = (&mut base, overrides) {
        base.extend(overrides);
    }
    base
}

/// A subagent row of `thread`.
pub fn subagent(overrides: Value) -> Value {
    let mut base = json!({
        "id": "subagent", "threadId": "thread", "runId": null, "parentNodeId": "node", "origin": "app_owned",
        "createdBy": "agent", "driver": "codex", "providerInstanceId": "codex", "providerThreadId": null,
        "childThreadId": null, "nativeTaskRef": null, "prompt": "Do the thing", "title": null, "model": null,
        "status": "completed", "result": null, "startedAt": null, "completedAt": null,
        "updatedAt": "2026-06-01T00:00:00.000Z"
    });
    if let (Value::Object(base), Value::Object(overrides)) = (&mut base, overrides) {
        base.extend(overrides);
    }
    base
}

/// A context transfer row.
pub fn transfer(source: &str, target: &str, status: &str) -> Value {
    json!({
        "id": format!("transfer-{source}-{target}"), "type": "fork", "sourceThreadId": source, "targetThreadId": target,
        "sourcePoint": {"threadId": source}, "basePoint": null, "sourceProviderInstanceId": null,
        "targetProviderInstanceId": null, "targetRunId": null, "status": status, "resolution": null,
        "createdBy": "user", "error": null, "createdAt": "2026-06-01T00:00:00.000Z",
        "updatedAt": "2026-06-01T00:00:00.000Z", "consumedAt": null
    })
}

/// A projection of `thread` with the collections `overrides` names; the rest are empty.
pub fn projection(thread: Value, overrides: Value) -> ThreadProjection {
    merged(
        json!({
            "thread": thread, "runs": [], "attempts": [], "nodes": [], "subagents": [], "providerSessions": [],
            "providerThreads": [], "providerTurns": [], "runtimeRequests": [], "messages": [], "plans": [],
            "turnItems": [], "checkpointScopes": [], "checkpoints": [], "contextHandoffs": [],
            "contextTransfers": [], "visibleTurnItems": [], "updatedAt": "2026-06-01T00:00:00.000Z"
        }),
        overrides,
    )
}
