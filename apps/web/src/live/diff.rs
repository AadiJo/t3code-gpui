//! The Diff tab's input on a live thread (`DiffPanel.tsx` queries): a turn's checkpoint diff
//! from `orchestration.getTurnDiff`, the whole thread from `orchestration.getFullThreadDiff`
//! when no checkout is known, and the branch, working tree or one commit from
//! `review.getDiffPreview`, with the branch's commits from `review.getBranchCommits`.

use std::sync::Arc;

use gpui_kit::{App, SharedString, Task};
use t3code_contracts::rpc::methods::{OrchestrationGetFullThreadDiff, OrchestrationGetTurnDiff, ReviewGetBranchCommits, ReviewGetDiffPreview};
use t3code_contracts::{
    OrchestrationGetFullThreadDiffInput, OrchestrationGetTurnDiffInput, ReviewBranchCommitsInput, ReviewDiffPreviewInput,
    ReviewDiffPreviewInputSourceKind,
};

use super::LiveData;
use super::rpc::RpcLink;
use crate::components::diff_panel::{CommitSummary, CommitsStatus, DiffPanelContent, DiffPanelInput, TurnSummary};
use crate::diff_panel_store::{DiffPanelSelection, DiffPanelStore};
use crate::diff_patch::renderable_patch;
use crate::i18n::t;
use crate::router::ThreadTarget;
use crate::session_logic::checkpoint_summaries;
use crate::state::app_data::AppData as _;
use crate::state::shell::runtime_ids;
use crate::timestamp_format::{format_relative_time_label, format_short_timestamp, now_ms};

/// The panel content for a unified diff.
fn content(diff: &str) -> DiffPanelContent {
    match renderable_patch(diff) {
        Some(patch) => DiffPanelContent::Patch { patch: Arc::new(patch), notices: Vec::new(), stats: None },
        None => DiffPanelContent::Empty { no_net_changes: true },
    }
}

/// Loads the input for `target`'s `selection`. Resolves to `None` only when the thread is gone.
pub fn load(data: &LiveData, target: &ThreadTarget, selection: &DiffPanelSelection, cx: &mut App) -> Task<Option<DiffPanelInput>> {
    let ThreadTarget::Server(key) = target else {
        return Task::ready(Some(DiffPanelInput { content: DiffPanelContent::NoTurns, ..Default::default() }));
    };
    let Some(view) = data.thread_view(target, cx) else { return Task::ready(None) };
    let (environment_id, thread_id) = runtime_ids(key);
    let Some(link) = RpcLink::new(&environment_id, cx) else {
        return Task::ready(Some(DiffPanelInput {
            content: DiffPanelContent::Error(t!("This environment is not connected.")),
            ..Default::default()
        }));
    };
    let format = crate::hooks::settings_bridge::timestamp_format(cx);
    let summaries = data
        .active(key, cx)
        .and_then(|thread| thread.read(cx).projection().map(|p| checkpoint_summaries(p)))
        .unwrap_or_default();
    let turns: Vec<TurnSummary> = summaries
        .iter()
        .rev()
        .filter_map(|summary| {
            Some(TurnSummary {
                turn_id: summary.assistant_message_id.clone()?,
                turn_count: summary.turn_count as u32,
                completed_label: format_short_timestamp(&summary.completed_at, format).into(),
            })
        })
        .collect();
    let ignore_whitespace = Some(DiffPanelStore::global(cx).effective_ignore_whitespace());
    let cwd = view.cwd.to_string();
    let base = DiffPanelInput {
        commits_status: CommitsStatus::Loaded { truncated: false },
        base_ref_choices: Vec::new(),
        thread_key: Some(view.key.clone()),
        workspace_root: Some(view.cwd.clone()),
        turns,
        commits: Vec::new(),
        head_ref: view.branch.clone(),
        base_ref: None,
        can_refresh: true,
        refreshing: false,
        content: DiffPanelContent::NoTurns,
    };
    let selection = selection.clone();
    let future = link.run({
        let link = link.clone();
        async move {
            let session = link.session().await?;
            let client = &session.client;
            let mut input = base;
            let branch_commits = |base_ref: Option<String>| {
                client.call::<ReviewGetBranchCommits>(ReviewBranchCommitsInput { cwd: cwd.clone(), base_ref })
            };
            input.content = match &selection {
                DiffPanelSelection::Turn { turn_id, .. } => {
                    let Some(summary) = summaries.iter().find(|s| s.assistant_message_id.as_deref() == Some(turn_id.as_str())) else {
                        return Ok(DiffPanelInput { content: DiffPanelContent::NoTurns, ..input });
                    };
                    let range = OrchestrationGetTurnDiffInput {
                        from_turn_count: summary.turn_count.saturating_sub(1),
                        to_turn_count: summary.turn_count,
                        thread_id: thread_id.clone(),
                        ignore_whitespace,
                    };
                    content(&client.call::<OrchestrationGetTurnDiff>(range).await.map_err(|e| super::rpc::error_message(&e))?.diff)
                }
                DiffPanelSelection::ToolCall { .. } => DiffPanelContent::Error(t!("Not available in this build yet.")),
                _ if cwd.is_empty() => {
                    let Some(last) = summaries.last() else { return Ok(input) };
                    let full = OrchestrationGetFullThreadDiffInput { thread_id: thread_id.clone(), to_turn_count: last.turn_count, ignore_whitespace };
                    content(&client.call::<OrchestrationGetFullThreadDiff>(full).await.map_err(|e| super::rpc::error_message(&e))?.diff)
                }
                DiffPanelSelection::Branch { base_ref } | DiffPanelSelection::Commit { base_ref, .. } => {
                    let commit = match &selection {
                        DiffPanelSelection::Commit { sha, .. } => Some(sha.clone()),
                        _ => None,
                    };
                    if let Ok(commits) = branch_commits(base_ref.clone()).await {
                        let now = now_ms();
                        input.commits = commits
                            .commits
                            .iter()
                            .map(|c| CommitSummary {
                            url: None,
                                sha: c.sha.clone().into(),
                                subject: c.subject.clone().into(),
                                relative_label: format_relative_time_label(&c.committed_at, now),
                            })
                            .collect();
                    }
                    let preview = ReviewDiffPreviewInput {
                        cwd: cwd.clone(),
                        base_ref: base_ref.clone(),
                        commit_sha: commit.clone(),
                        ignore_whitespace,
                        source_kind: commit.is_none().then_some(ReviewDiffPreviewInputSourceKind::BranchRange),
                        tracked_only: None,
                        metadata_only: None,
                        file: None,
                    };
                    let result = client.call::<ReviewGetDiffPreview>(preview).await.map_err(|e| super::rpc::error_message(&e))?;
                    let source = result.sources.first();
                    input.base_ref = source.and_then(|s| s.base_ref.clone()).map(SharedString::from);
                    if let Some(head) = source.and_then(|s| s.head_ref.clone()) {
                        input.head_ref = Some(head.into());
                    }
                    source.map(|s| content(&s.diff)).unwrap_or(DiffPanelContent::Empty { no_net_changes: true })
                }
                DiffPanelSelection::Unstaged => {
                    let preview = ReviewDiffPreviewInput {
                        cwd: cwd.clone(),
                        base_ref: None,
                        commit_sha: None,
                        ignore_whitespace,
                        source_kind: Some(ReviewDiffPreviewInputSourceKind::WorkingTree),
                        tracked_only: None,
                        metadata_only: None,
                        file: None,
                    };
                    let result = client.call::<ReviewGetDiffPreview>(preview).await.map_err(|e| super::rpc::error_message(&e))?;
                    result.sources.first().map(|s| content(&s.diff)).unwrap_or(DiffPanelContent::Empty { no_net_changes: false })
                }
            };
            Ok(input)
        }
    });
    cx.background_executor().spawn(async move {
        Some(future.await.unwrap_or_else(|message| DiffPanelInput { content: DiffPanelContent::Error(message), ..Default::default() }))
    })
}
