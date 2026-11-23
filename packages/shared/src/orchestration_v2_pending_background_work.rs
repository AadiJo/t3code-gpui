//! The parts of `orchestrationV2PendingBackgroundWork.ts` the client runtime needs: which
//! background tasks hold a run's completion, and which turn items are still doing background work.

use std::collections::HashSet;
use std::sync::Arc;

use t3code_contracts::{
    OrchestrationV2Run, OrchestrationV2RunStatus, OrchestrationV2ThreadShellPendingBackgroundTasksItem as Task,
    OrchestrationV2TurnItem,
};

use crate::turn_item::TurnItemExt;

/// Whether any pending task holds the run's completion (`backgroundWorkHoldsCompletion`).
/// Commands and monitors are as often a dev server as a wait, so they hold only when awaited.
pub fn background_work_holds_completion(tasks: &[Task]) -> bool {
    tasks.iter().any(|task| match task {
        Task::Subagent(_) | Task::BackgroundTask(_) => true,
        Task::Command(task) => task.awaited.is_some(),
        Task::Monitor(task) => task.awaited.is_some(),
        Task::Unknown(task) => task.awaited.is_some(),
    })
}

fn is_persistent_dynamic_tool(item: &OrchestrationV2TurnItem) -> bool {
    matches!(item, OrchestrationV2TurnItem::DynamicTool(tool) if tool.input.get("persistent").and_then(serde_json::Value::as_bool) == Some(true))
}

/// Command, dynamic tool and subagent items still working outside rolled-back runs
/// (`pendingBackgroundTurnItems`). Stop reconciliation asks the server about each one.
pub fn pending_background_turn_items<'a>(
    turn_items: &'a [Arc<OrchestrationV2TurnItem>],
    runs: &[Arc<OrchestrationV2Run>],
) -> Vec<&'a Arc<OrchestrationV2TurnItem>> {
    let rolled_back: HashSet<_> =
        runs.iter().filter(|run| run.status == OrchestrationV2RunStatus::RolledBack).map(|run| &run.id).collect();
    turn_items
        .iter()
        .filter(|item| {
            matches!(
                &***item,
                OrchestrationV2TurnItem::CommandExecution(_) | OrchestrationV2TurnItem::DynamicTool(_) | OrchestrationV2TurnItem::Subagent(_)
            ) && item.is_work_active()
                && !is_persistent_dynamic_tool(item)
                && item.item_run_id().is_none_or(|id| !rolled_back.contains(id))
        })
        .collect()
}
