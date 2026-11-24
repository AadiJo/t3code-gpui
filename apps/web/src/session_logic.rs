//! A thread projection as the timeline reads it. Ports `deriveTimelineEntriesFromVisibleTurnItems`
//! and `projectedWorkEntry` of `session-logic.ts`, plus the run, request and queue state of
//! `ChatView.logic.ts` that [`TimelineInput`] carries.
//!
//! [`TimelineMapper`] keeps each mapped entry with the `Arc` of the turn item it came from, so
//! a projection where one item changed maps one entry; the rest are refcount bumps.
//!
//! Differs from the TypeScript: sent messages and HTML renders are recognised by the t3 tool
//! name alone (no output envelope parsing); MCP app frames, turn diff summaries, worktree setup
//! and attachment previews are not mapped yet.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::SharedString;
use serde_json::Value;
use t3code_client_runtime::state::models::{EpochMs, parse_ms};
use t3code_client_runtime::state::optimistic_messages::OptimisticMessage;
use t3code_client_runtime::state::thread_detail::{PendingRequest, active_run, pending_requests, user_queued_runs};
use t3code_client_runtime::work_log::command_label::command_display_text;
use t3code_client_runtime::work_log::item_detail::{args_lines, turn_item_output_text};
use t3code_client_runtime::work_log::presentation::{command_indicates_failure, context_compaction_label, tool_output_indicates_failure};
use t3code_contracts::{
    ChatAttachment, OrchestrationV2Actor, OrchestrationV2CreationSource, OrchestrationV2PlanStepStatus as Step,
    OrchestrationV2ProviderFailureClass as FailureClass, OrchestrationV2RunAttemptStatus, OrchestrationV2RunStatus as Run,
    OrchestrationV2RuntimeRequestKind as RequestKind, OrchestrationV2TurnItem as Item, OrchestrationV2TurnItemStatus as Status,
    OrchestrationV2TurnItemSystemNoticeLevel, OrchestrationV2UserMessageInputIntent as Intent, ProviderRequestKind,
};
use t3code_shared::thread_projection::{ThreadProjection, VisibleTurnItem};
use t3code_shared::turn_item::TurnItemExt;

use crate::components::chat::timeline_entries::{
    AgentSender,
    ApprovalKind, AttemptId, Attachment, AttachmentKind, ChatMessage, CompactionStatus, Divider, DividerKind, DynamicToolKind,
    EntryId, EntryKind, FileChange, InputIntent, LatestRun, MessageId, MessageRole, PendingQuestion, ProposedPlan,
    QueuedMessage, RequestId, RunId, RunStatus, SearchMatch, Subagent, SubagentStatus, Timestamp, TimelineEntry,
    TimelineEvent, TimelineInput, TodoState, ToolStatus, TurnDiffSummary, WebResult, WorkItem, WorkLogEntry,
};
use crate::i18n::t;
use crate::work_entry_labels::{Lifecycle, SourceActivityKind, Tone, WorkFacts, WorkSource, format_read_tool_label, format_search_tool_label};
use t3code_client_runtime::work_log::tool_activity::{ToolActivityAction, classify_tool_activity, collect_tool_file_paths};

/// `WORKSPACE_PREPARATION_INPUT` and the failure code of `turnItemIsWorkspacePreparation`.
const WORKSPACE_PREPARATION_INPUT: &str = "Preparing workspace";
const WORKSPACE_PREPARATION_FAILURE_CODE: &str = "workspace_preparation_failed";

/// Base fields every turn item variant has but the shared accessor trait does not expose.
macro_rules! base {
    ($item:expr, $bound:ident => $body:expr, $fallback:expr) => {
        match $item {
            Item::Notification($bound) => $body,
            Item::UserMessage($bound) => $body,
            Item::AssistantMessage($bound) => $body,
            Item::Reasoning($bound) => $body,
            Item::ProposedPlan($bound) => $body,
            Item::TodoList($bound) => $body,
            Item::UserInputRequest($bound) => $body,
            Item::FileChange($bound) => $body,
            Item::CommandExecution($bound) => $body,
            Item::FileSearch($bound) => $body,
            Item::WebSearch($bound) => $body,
            Item::ApprovalRequest($bound) => $body,
            Item::Checkpoint($bound) => $body,
            Item::RunInterruptRequest($bound) => $body,
            Item::RunInterruptResult($bound) => $body,
            Item::SystemNotice($bound) => $body,
            Item::Error($bound) => $body,
            Item::Compaction($bound) => $body,
            Item::Handoff($bound) => $body,
            Item::Fork($bound) => $body,
            Item::ThreadCreated($bound) => $body,
            Item::SecretRequest($bound) => $body,
            Item::LinkRequest($bound) => $body,
            Item::Subagent($bound) => $body,
            Item::DynamicTool($bound) => $body,
            Item::Unknown(_) => $fallback,
        }
    };
}

fn item_title(item: &Item) -> Option<&str> {
    base!(item, i => i.title.as_deref().map(str::trim).filter(|t| !t.is_empty()), None)
}

fn item_updated_at(item: &Item) -> Option<EpochMs> {
    base!(item, i => parse_ms(&i.updated_at), None)
}

/// When the row starts (`projectedItemCreatedAt`): its start, else its last update.
fn item_created_at(item: &Item) -> EpochMs {
    base!(item, i => i.started_at.as_deref().and_then(parse_ms).or_else(|| parse_ms(&i.updated_at)), None).unwrap_or_default()
}

fn ms(value: Option<&str>) -> Option<Timestamp> {
    value.and_then(parse_ms).map(Timestamp)
}

fn shared(text: impl Into<String>) -> SharedString {
    SharedString::from(text.into())
}

fn id_of(id: &impl AsRef<str>) -> SharedString {
    SharedString::from(id.as_ref().to_owned())
}

/// `projectedWorkEntryStatus`.
fn tool_status(status: Option<Status>) -> ToolStatus {
    match status {
        Some(Status::Pending | Status::Running | Status::Waiting) => ToolStatus::InProgress,
        Some(Status::Failed) => ToolStatus::Failed,
        Some(Status::Cancelled | Status::Interrupted) => ToolStatus::Stopped,
        _ => ToolStatus::Completed,
    }
}

fn attachments(list: &[ChatAttachment]) -> Arc<[Attachment]> {
    list.iter()
        .filter_map(|attachment| match attachment {
            ChatAttachment::Image(image) => {
                Some(Attachment { name: shared(image.name.clone()), kind: AttachmentKind::Image, source: SharedString::default() })
            }
            ChatAttachment::File(file) => {
                Some(Attachment { name: shared(file.name.clone()), kind: AttachmentKind::File, source: SharedString::default() })
            }
            ChatAttachment::Unknown(_) => None,
        })
        .collect()
}

/// The bare tool name of a t3 MCP tool (`t3-code.send_to_user`, `mcp__t3-code__send_to_user`).
fn t3_tool(tool_name: Option<&str>) -> Option<&str> {
    let name = tool_name?;
    if !name.contains("t3") {
        return None;
    }
    let tail = name.rsplit(['.', '/']).next()?;
    Some(tail.rsplit("__").next().unwrap_or(tail))
}

fn dynamic_tool_kind(tool_name: Option<&str>) -> DynamicToolKind {
    let lower = tool_name.unwrap_or_default().to_lowercase();
    let tool = t3_tool(Some(&lower)).unwrap_or(&lower).to_owned();
    if tool.starts_with("preview_") || tool.contains("browser") {
        DynamicToolKind::Browser
    } else if tool.starts_with("device_") {
        DynamicToolKind::Device
    } else if tool.starts_with("pull_request") {
        DynamicToolKind::PullRequest
    } else if matches!(tool.as_str(), "t3_thread_launch" | "create_threads" | "delegate_task") {
        DynamicToolKind::ThreadCreate
    } else if tool.contains("js_repl") {
        DynamicToolKind::JsRepl
    } else if tool == "agent" || tool == "task" {
        DynamicToolKind::Agent
    } else {
        DynamicToolKind::Mcp
    }
}

/// The label of a provider error row (`providerErrorPresentation`).
fn provider_error_label(error: &t3code_contracts::OrchestrationV2TurnItemError) -> SharedString {
    let Some(retry) = &error.retry else {
        return if error.failure.class == FailureClass::UsageLimit {
            t!("Usage limit reached")
        } else {
            error.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(shared).unwrap_or_else(|| t!("Provider error"))
        };
    };
    let progress = match retry.max_attempts {
        Some(max) => format!("{}/{max}", retry.attempt),
        None => retry.attempt.to_string(),
    };
    let attempt = retry.attempt;
    match error.status {
        Status::Running => t!("Retrying provider ({progress})", progress = progress),
        Status::Completed => t!(
            "{attempt, plural, one {Provider recovered ({progress} retries)} other {Provider recovered ({progress} retries)}}",
            attempt = attempt,
            progress = progress
        ),
        Status::Failed if error.failure.class == FailureClass::UsageLimit => t!(
            "{attempt, plural, one {Usage limit reached after {progress} retries} other {Usage limit reached after {progress} retries}}",
            attempt = attempt,
            progress = progress
        ),
        Status::Failed => t!(
            "{attempt, plural, one {Provider error after {progress} retries} other {Provider error after {progress} retries}}",
            attempt = attempt,
            progress = progress
        ),
        _ => t!("Provider retry stopped ({progress})", progress = progress),
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn compaction_label(compaction: &t3code_contracts::OrchestrationV2TurnItemCompaction) -> SharedString {
    match context_compaction_label(compaction) {
        ("Context compaction failed", _) => t!("Context compaction failed"),
        ("Context compaction stopped", _) => t!("Context compaction stopped"),
        ("Compacting context", _) => t!("Compacting context"),
        (_, Some((before, after))) => t!("Context compacted {before} → {after} tokens", before = before, after = after),
        _ => t!("Context compacted"),
    }
}

fn todo_state(status: Step) -> TodoState {
    match status {
        Step::Pending => TodoState::Pending,
        Step::Running => TodoState::Running,
        Step::Completed => TodoState::Completed,
    }
}

/// The lifecycle the web gives an item (`projectedWorkEntryStatus`).
fn lifecycle(status: Status) -> Lifecycle {
    match status {
        Status::Pending | Status::Running | Status::Waiting => Lifecycle::InProgress,
        Status::Completed => Lifecycle::Completed,
        Status::Idle => Lifecycle::Idle,
        Status::Failed => Lifecycle::Failed,
        Status::Cancelled | Status::Interrupted => Lifecycle::Stopped,
    }
}

/// `projectedWorkEntryTone`.
fn tone(item: &Item) -> Tone {
    match item {
        Item::Reasoning(_) => Tone::Thinking,
        Item::CommandExecution(_)
        | Item::FileChange(_)
        | Item::FileSearch(_)
        | Item::WebSearch(_)
        | Item::DynamicTool(_)
        | Item::Subagent(_)
        | Item::ThreadCreated(_)
        | Item::UserInputRequest(_)
        | Item::ApprovalRequest(_) => Tone::Tool,
        _ => Tone::Info,
    }
}

/// `providerErrorPresentation`'s detail: the failure, then the retry delay while retrying.
fn provider_error_detail(error: &t3code_contracts::OrchestrationV2TurnItemError) -> String {
    let delay = error.retry.as_ref().filter(|_| error.status == Status::Running).and_then(|retry| retry.retry_delay_ms).filter(|ms| *ms > 0);
    match delay {
        Some(ms) if ms < 1_000 => format!("{} {}", error.failure.message, t!("Retrying in {delay}ms.", delay = ms)),
        Some(ms) => {
            let seconds = t3code_client_runtime::work_log::presentation::to_fixed(ms as f64 / 1_000.0, 1);
            let seconds = seconds.strip_suffix(".0").unwrap_or(&seconds).to_owned();
            format!("{} {}", error.failure.message, t!("Retrying in {delay}s.", delay = seconds))
        }
        None => error.failure.message.clone(),
    }
}

fn is_image_preview_path(path: &str) -> bool {
    const EXTENSIONS: &[&str] =
        &[".avif", ".bmp", ".gif", ".ico", ".heic", ".heif", ".jpeg", ".jpg", ".png", ".svg", ".tif", ".tiff", ".webp"];
    let lower = path.to_lowercase();
    EXTENSIONS.iter().any(|extension| lower.ends_with(extension))
}

/// The fields `projectedWorkEntry` keeps for one item: label, detail, command, changed files,
/// tool title, tone, lifecycle and the tool's source (`extractToolActivityPresentation`).
fn work_facts(item: &Arc<Item>) -> WorkFacts {
    let title = item_title(item).map(shared);
    let mut facts = WorkFacts {
        item: Arc::clone(item),
        tone: tone(item),
        lifecycle: item.item_status().map(lifecycle),
        label: SharedString::default(),
        detail: None,
        command: None,
        changed_files: Arc::from([]),
        tool_title: None,
        source_activity_kind: None,
        tool_source: None,
        viewed_image_path: None,
    };
    facts.label = match &**item {
        Item::ThreadCreated(_) => t!("Created thread"),
        Item::Compaction(compaction) => {
            facts.source_activity_kind = Some(SourceActivityKind::ContextCompaction);
            facts.detail = compaction.summary.clone().filter(|s| !s.is_empty()).map(shared);
            compaction_label(compaction)
        }
        Item::Reasoning(reasoning) => {
            facts.detail = (!reasoning.text.is_empty()).then(|| shared(reasoning.text.clone()));
            title.clone().unwrap_or_else(|| t!("Thinking", context = "status of an agent reasoning"))
        }
        Item::CommandExecution(command) => {
            facts.command = Some(shared(command.input.clone()));
            facts.tool_title = Some(title.clone().unwrap_or_else(|| t!("Command", context = "noun, a shell command tool")));
            title.clone().unwrap_or_else(|| t!("Ran command"))
        }
        Item::FileChange(change) => {
            let changes = change.changes.as_deref().unwrap_or_default();
            facts.changed_files = if changes.is_empty() {
                Arc::from([shared(change.file_name.clone())])
            } else {
                changes.iter().map(|c| shared(c.path.clone())).collect()
            };
            facts.tool_title = Some(title.clone().unwrap_or_else(|| t!("File change", context = "noun, a tool title")));
            title.clone().unwrap_or_else(|| {
                if changes.len() > 1 {
                    t!("{count, plural, one {Changed {count} file} other {Changed {count} files}}", count = changes.len())
                } else {
                    t!("Changed {fileName}", fileName = change.file_name.clone())
                }
            })
        }
        Item::FileSearch(search) => {
            facts.detail = search.pattern.clone().filter(|p| !p.is_empty()).map(shared);
            facts.tool_title = Some(title.clone().unwrap_or_else(|| t!("File search", context = "noun, a tool title")));
            let record = serde_json::to_value(&**item).unwrap_or(Value::Null);
            title.clone().or_else(|| format_search_tool_label(record.as_object())).unwrap_or_else(|| t!("Searched files"))
        }
        Item::WebSearch(search) => {
            facts.detail = search.patterns.as_ref().filter(|p| !p.is_empty()).map(|p| shared(p.join(", ")));
            facts.tool_title = Some(title.clone().unwrap_or_else(|| t!("Web search", context = "noun, a tool title")));
            title.clone().unwrap_or_else(|| t!("Searched the web"))
        }
        Item::SystemNotice(notice) => {
            if notice.level != Some(OrchestrationV2TurnItemSystemNoticeLevel::Info) {
                facts.source_activity_kind = Some(SourceActivityKind::RuntimeWarning);
            }
            shared(notice.message.clone())
        }
        Item::Error(error) => {
            facts.detail = Some(shared(provider_error_detail(error)));
            facts.source_activity_kind = if error.failure.class == FailureClass::UsageLimit && error.status != Status::Completed {
                Some(SourceActivityKind::RuntimeWarning)
            } else if error.retry.is_none() {
                Some(SourceActivityKind::RuntimeError)
            } else {
                None
            };
            provider_error_label(error)
        }
        Item::DynamicTool(tool) => {
            let tool_name = tool.tool_name.as_deref().filter(|n| !n.is_empty());
            let mut data = serde_json::Map::new();
            if let Some(name) = tool_name {
                data.insert("toolName".into(), Value::String(name.to_owned()));
            }
            data.insert("input".into(), tool.input.clone());
            let classified = classify_tool_activity(Some("dynamic_tool_call"), None, Some(&data));
            let read_path = collect_tool_file_paths(&serde_json::json!({ "input": tool.input })).into_iter().next();
            facts.tool_title = Some(title.clone().or_else(|| tool_name.map(shared)).unwrap_or_else(|| t!("Tool", context = "noun, a tool title")));
            let fallback = || tool_name.map(shared).unwrap_or_else(|| t!("Tool call", context = "noun"));
            title.clone().unwrap_or_else(|| match classified {
                ToolActivityAction::Read => format_read_tool_label(read_path.as_deref().unwrap_or_default(), 0),
                ToolActivityAction::Search => {
                    let record = serde_json::json!({ "input": tool.input });
                    format_search_tool_label(record.as_object()).unwrap_or_else(fallback)
                }
                _ => fallback(),
            })
        }
        Item::TodoList(todo) => {
            let done = todo.steps.iter().filter(|step| step.status == Step::Completed).count();
            let total = todo.steps.len();
            let current = todo.steps.iter().find(|step| step.status == Step::Running).or_else(|| todo.steps.iter().find(|step| step.status == Step::Pending));
            match current {
                Some(current) => t!("Tasks {done}/{total} · {current}", done = done, total = total, current = current.text.clone()),
                None => t!("Tasks {done}/{total}", done = done, total = total),
            }
        }
        Item::ApprovalRequest(request) => {
            facts.detail = Some(shared(request.prompt.clone().unwrap_or_else(|| request.request_kind.as_str().to_owned())));
            title.clone().unwrap_or_else(|| t!("Approval requested"))
        }
        Item::UserInputRequest(request) => title.clone().unwrap_or_else(|| {
            if request.question_answer.is_some() { t!("Answered questions") } else { t!("Input requested") }
        }),
        Item::Notification(notification) => {
            // Notifications map without a lifecycle or tool fields.
            facts.lifecycle = None;
            return WorkFacts { label: shared(notification.summary.clone()), ..facts };
        }
        other => title.clone().unwrap_or_else(|| shared(other.type_name().replace('_', " "))),
    };
    if let Item::DynamicTool(tool) = &**item {
        let computer = tool.tool_surface.as_ref().is_some_and(|s| s.as_str() == "computer")
            || tool.tool_source.as_ref().is_some_and(|s| s.kind.as_str() == "computer")
            || (tool.tool_surface.is_none()
                && tool.tool_source.is_none()
                && t3code_client_runtime::work_log::tool_activity::dynamic_tool_drives_computer(tool.tool_name.as_deref(), &tool.input));
        facts.tool_source = if computer && tool.tool_source.as_ref().is_none_or(|s| s.kind.as_str() != "computer") {
            Some(t3code_contracts::ToolActivitySource {
                key: "computer-use".into(),
                name: "Computer Use".into(),
                kind: t3code_contracts::ToolActivitySourceKind::Computer,
                icon: None,
            })
        } else {
            tool.tool_source.clone()
        };
        facts.viewed_image_path = tool
            .viewed_image_path
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty() && !p.contains(['\r', '\n']) && is_image_preview_path(p))
            .map(|p| shared(p.to_owned()));
    }
    facts
}

/// The work-log entry of one item (`projectedWorkEntry`).
fn work_entry(source_item: &Arc<Item>, created_at: Timestamp, superseded_attempt: Option<AttemptId>) -> WorkLogEntry {
    let item: &Item = source_item;
    let source = Arc::new(WorkSource::new(work_facts(source_item)));
    let title = item_title(item);
    let mut status = tool_status(item.item_status());
    let (label, work) = match item {
        Item::ThreadCreated(_) => (
            t!("Created thread"),
            WorkItem::DynamicTool {
                kind: DynamicToolKind::ThreadCreate,
                tool: SharedString::default(),
                title: t!("Created thread"),
                args: Arc::from([]),
                output: None,
            },
        ),
        Item::Compaction(compaction) => {
            let compaction_status = match compaction.status {
                Status::Failed | Status::Cancelled | Status::Interrupted => CompactionStatus::Failed,
                Status::Pending | Status::Running | Status::Waiting => CompactionStatus::Running,
                _ => CompactionStatus::Completed,
            };
            (
                compaction_label(compaction),
                WorkItem::Compaction {
                    status: compaction_status,
                    tokens_before: compaction.before_token_count,
                    tokens_after: compaction.after_token_count,
                },
            )
        }
        Item::Reasoning(reasoning) => (
            title.map(shared).unwrap_or_else(|| t!("Thinking", context = "status of an agent reasoning")),
            WorkItem::Reasoning { text: shared(reasoning.text.clone()) },
        ),
        Item::CommandExecution(command) => {
            if status == ToolStatus::Completed
                && command_indicates_failure(command.exit_code, command.output_indicates_failure, command.output.as_deref())
            {
                status = ToolStatus::Failed;
            }
            (
                title.map(shared).unwrap_or_else(|| t!("Ran command")),
                WorkItem::Command {
                    command: shared(command_display_text(&command.input)),
                    output: turn_item_output_text(item).map(shared),
                    exit_code: command.exit_code.and_then(|code| i32::try_from(code).ok()),
                },
            )
        }
        Item::FileChange(change) => {
            let count = change.changes.as_ref().map_or(0, Vec::len);
            let label = match title {
                Some(title) => shared(title),
                None if count > 1 => t!("{count, plural, one {Changed {count} file} other {Changed {count} files}}", count = count),
                None => t!("Changed {fileName}", fileName = change.file_name.clone()),
            };
            let changes: Arc<[FileChange]> = match &change.changes {
                Some(changes) if !changes.is_empty() => changes
                    .iter()
                    .map(|detail| FileChange {
                        path: shared(detail.path.clone()),
                        additions: 0,
                        deletions: 0,
                        operation: shared(detail.operation.clone()),
                    })
                    .collect(),
                _ => Arc::from([FileChange {
                    path: shared(change.file_name.clone()),
                    additions: change.additions.unwrap_or(0) as u32,
                    deletions: change.deletions.unwrap_or(0) as u32,
                    operation: "update".into(),
                }]),
            };
            (label, WorkItem::FileChange { changes, raw_diff: change.diff_str.clone().map(shared) })
        }
        Item::FileSearch(search) => (
            title.map(shared).unwrap_or_else(|| t!("Searched files")),
            WorkItem::FileSearch {
                pattern: shared(search.pattern.clone().unwrap_or_default()),
                matches: search
                    .results
                    .iter()
                    .flatten()
                    .map(|result| SearchMatch {
                        path: shared(result.file_name.clone()),
                        line: result.line.unwrap_or(0) as u32,
                        column: result.column.unwrap_or(0) as u32,
                        preview: shared(result.preview.clone().unwrap_or_default()),
                    })
                    .collect(),
            },
        ),
        Item::WebSearch(search) => (
            title.map(shared).unwrap_or_else(|| t!("Searched the web")),
            WorkItem::WebSearch {
                queries: search.patterns.iter().flatten().map(|query| shared(query.clone())).collect(),
                results: search
                    .results
                    .iter()
                    .flatten()
                    .map(|result| WebResult {
                        title: shared(result.title.clone().unwrap_or_default()),
                        url: shared(result.url.clone().unwrap_or_default()),
                        snippet: shared(result.snippet.clone().unwrap_or_default()),
                    })
                    .collect(),
            },
        ),
        Item::SystemNotice(notice) => (
            shared(notice.message.clone()),
            WorkItem::SystemNotice {
                info: notice.level == Some(OrchestrationV2TurnItemSystemNoticeLevel::Info),
                text: shared(notice.message.clone()),
            },
        ),
        Item::Error(error) => (
            provider_error_label(error),
            WorkItem::Error {
                message: shared(error.failure.message.clone()),
                usage_limit_retry: (error.failure.class == FailureClass::UsageLimit).then(|| error.failure.reset_at.clone().map(shared)).flatten(),
                retryable: error.failure.retryable == Some(true),
            },
        ),
        Item::DynamicTool(tool) => {
            let tool_name = tool.tool_name.as_deref();
            let output = turn_item_output_text(item);
            if status == ToolStatus::Completed && output.as_deref().is_some_and(tool_output_indicates_failure) {
                status = ToolStatus::Failed;
            }
            let label = title.map(shared).or_else(|| tool_name.map(shared)).unwrap_or_else(|| t!("Tool call", context = "noun"));
            let lines = args_lines(&tool.input);
            let args: Arc<[(SharedString, SharedString)]> = match (lines.args, lines.args_text) {
                (Some(pairs), _) => pairs.into_iter().map(|(key, value)| (shared(key), shared(value))).collect(),
                (None, Some(text)) => Arc::from([(SharedString::default(), shared(text))]),
                _ => Arc::from([]),
            };
            let work = match &tool.viewed_image_path {
                Some(path) => WorkItem::ViewedImage { path: shared(path.clone()) },
                None => WorkItem::DynamicTool {
                    kind: dynamic_tool_kind(tool_name),
                    tool: shared(tool_name.unwrap_or_default()),
                    title: label.clone(),
                    args,
                    output: output.map(shared),
                },
            };
            (label, work)
        }
        Item::TodoList(todo) => {
            let done = todo.steps.iter().filter(|step| step.status == Step::Completed).count();
            let total = todo.steps.len();
            let current = todo
                .steps
                .iter()
                .find(|step| step.status == Step::Running)
                .or_else(|| todo.steps.iter().find(|step| step.status == Step::Pending));
            let label = match current {
                Some(current) => t!("Tasks {done}/{total} · {current}", done = done, total = total, current = current.text.clone()),
                None => t!("Tasks {done}/{total}", done = done, total = total),
            };
            (label, WorkItem::TodoList { steps: todo.steps.iter().map(|step| (todo_state(step.status), shared(step.text.clone()))).collect() })
        }
        Item::ApprovalRequest(request) => {
            let kind = match request.request_kind {
                ProviderRequestKind::Command => ApprovalKind::Command,
                ProviderRequestKind::FileRead => ApprovalKind::FileRead,
                ProviderRequestKind::FileChange => ApprovalKind::FileChange,
                ProviderRequestKind::McpElicitation => ApprovalKind::AppAccess,
                ProviderRequestKind::Permission => ApprovalKind::Permission,
            };
            (
                title.map(shared).unwrap_or_else(|| t!("Approval requested")),
                WorkItem::Approval {
                    request_id: RequestId(id_of(&request.request_id)),
                    kind,
                    detail: shared(request.prompt.clone().unwrap_or_else(|| request.request_kind.as_str().to_owned())),
                    reason: request.reason.clone().map(shared),
                },
            )
        }
        Item::UserInputRequest(request) => {
            let pairs: Arc<[(SharedString, SharedString)]> = match &request.question_answer {
                Some(answer) => request
                    .questions
                    .iter()
                    .map(|question| {
                        let text = answer.answers.get(&question.id).map(answer_text).unwrap_or_default();
                        (shared(question.question.clone()), shared(text))
                    })
                    .collect(),
                None => Arc::from([]),
            };
            let label = match (title, request.question_answer.is_some()) {
                (Some(title), _) => shared(title),
                (None, true) => t!("Answered questions"),
                (None, false) => t!("Input requested"),
            };
            (label, WorkItem::QuestionAnswer { pairs })
        }
        Item::Notification(notification) => {
            (shared(notification.summary.clone()), WorkItem::Notification { text: shared(notification.summary.clone()) })
        }
        other => {
            let label = title.map(shared).unwrap_or_else(|| shared(other.type_name().replace('_', " ")));
            (label.clone(), WorkItem::Notification { text: label })
        }
    };
    let _ = label;
    WorkLogEntry {
        id: EntryId(id_of(&item.item_id())),
        run_id: item.item_run_id().map(|run| RunId(id_of(run))),
        created_at,
        status,
        label: source.facts.label.clone(),
        item: work,
        superseded_attempt,
        source: Some(source),
    }
}

fn answer_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(answer_text).collect::<Vec<_>>().join(", "),
        Value::Object(_) => str_field(value, "text").or_else(|| str_field(value, "label")).unwrap_or_default().to_owned(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn subagent_status(status: Status) -> SubagentStatus {
    match status {
        Status::Pending => SubagentStatus::Queued,
        Status::Running => SubagentStatus::Running,
        Status::Waiting => SubagentStatus::Waiting,
        Status::Completed => SubagentStatus::Completed,
        Status::Failed => SubagentStatus::Failed,
        Status::Idle => SubagentStatus::Idle,
        Status::Cancelled | Status::Interrupted => SubagentStatus::Stopped,
    }
}

/// `turnItemIsWorkspacePreparation`.
fn is_workspace_preparation(item: &Item) -> bool {
    match item {
        Item::CommandExecution(command) => command.input == WORKSPACE_PREPARATION_INPUT,
        Item::Error(error) => error.status == Status::Cancelled && error.failure.code.as_deref() == Some(WORKSPACE_PREPARATION_FAILURE_CODE),
        _ => false,
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().chars().take(160).collect()
}

fn message(id: MessageId, role: MessageRole, text: &str, created_at: Timestamp, updated_at: Timestamp) -> ChatMessage {
    ChatMessage {
        id,
        role,
        text: shared(text),
        created_at,
        updated_at,
        streaming: false,
        run_id: None,
        attachments: Arc::from([]),
        input_intent: InputIntent::Prompt,
        sent_by_app: false,
        guest: None,
        from_agent: None,
    }
}

/// The entry of one visible row, or `None` when the timeline leaves it out.
fn map_row(row: &VisibleTurnItem, superseded_attempt: Option<AttemptId>, folded_answer: bool) -> Option<TimelineEntry> {
    let item = &*row.item;
    if is_workspace_preparation(item) && item.deferred_work().is_none() {
        return None;
    }
    let created_at = Timestamp(item_created_at(item));
    let updated_at = Timestamp(item_updated_at(item).unwrap_or(created_at.0));
    let id = EntryId(id_of(&item.item_id()));
    let run_id = item.item_run_id().map(|run| RunId(id_of(run)));
    let kind = match item {
        Item::Checkpoint(_) | Item::RunInterruptRequest(_) => return None,
        Item::UserMessage(user) if user.input_intent == Intent::QueuedTurn && user.status == Status::Pending => return None,
        Item::UserMessage(_) if folded_answer => return None,
        Item::UserMessage(user) => EntryKind::Message(ChatMessage {
            run_id,
            attachments: attachments(&user.attachments),
            input_intent: match user.input_intent {
                Intent::Steer => InputIntent::Steer,
                Intent::PromotedQueuedToSteer => InputIntent::PromotedQueuedToSteer,
                _ => InputIntent::Prompt,
            },
            // `resolveUserMessagePresentation`: an agent's message folds; one the
            // server made shows "Sent by T3 Code".
            from_agent: (user.created_by == OrchestrationV2Actor::Agent
                && user.creation_source != OrchestrationV2CreationSource::Server
                && user.scheduled_task_id.is_none())
            .then(|| AgentSender { title: user.linked_sender.as_ref().map(|s| shared(s.title.trim())).filter(|t| !t.is_empty()) }),
            sent_by_app: (user.created_by != OrchestrationV2Actor::User
                || matches!(user.creation_source, OrchestrationV2CreationSource::Server | OrchestrationV2CreationSource::Mcp)
                || user.scheduled_task_id.is_some())
                && !(user.created_by == OrchestrationV2Actor::Agent && user.creation_source != OrchestrationV2CreationSource::Server),
            guest: (user.created_by != OrchestrationV2Actor::Agent).then_some(()).and(user.linked_sender.as_ref()).map(|_| t!("Guest")),
            ..message(MessageId(id_of(&user.message_id)), MessageRole::User, &user.text, created_at, updated_at)
        }),
        Item::AssistantMessage(reply) => EntryKind::Message(ChatMessage {
            streaming: reply.streaming,
            run_id,
            attachments: reply.attachments.as_deref().map(attachments).unwrap_or_else(|| Arc::from([])),
            ..message(MessageId(id_of(&reply.message_id)), MessageRole::Assistant, &reply.text, created_at, updated_at)
        }),
        Item::ProposedPlan(plan) => EntryKind::ProposedPlan(ProposedPlan {
            run_id,
            title: plan
                .markdown
                .lines()
                .find_map(|line| line.trim().strip_prefix('#').map(|title| title.trim_start_matches('#').trim().to_owned()))
                .filter(|title| !title.is_empty())
                .map(shared)
                .unwrap_or_else(|| t!("Proposed plan")),
            markdown: shared(plan.markdown.clone()),
        }),
        Item::DynamicTool(tool) if tool.status == Status::Completed && t3_tool(tool.tool_name.as_deref()) == Some("send_to_user") => {
            let text = str_field(&tool.input, "message").or_else(|| str_field(&tool.input, "text")).unwrap_or_default();
            EntryKind::SentMessage { run_id, text: shared(text) }
        }
        Item::DynamicTool(tool) if tool.status == Status::Completed && t3_tool(tool.tool_name.as_deref()) == Some("html_render") => {
            EntryKind::HtmlRender { run_id, title: shared(str_field(&tool.input, "title").unwrap_or_default()) }
        }
        Item::Subagent(subagent) => EntryKind::Event(TimelineEvent::Subagent(Subagent {
            id: id_of(&subagent.subagent_id),
            run_id,
            provider_turn_id: subagent.provider_turn_id.as_ref().map(id_of),
            title: item_title(item).map(shared).unwrap_or_else(|| shared(first_line(&subagent.prompt))),
            detail: subagent.progress.clone().or_else(|| subagent.result.clone()).map(|text| shared(first_line(&text))),
            status: subagent_status(subagent.status),
            model: shared(subagent.driver.as_str()),
            elapsed_ms: match (ms(subagent.started_at.as_deref()), ms(subagent.completed_at.as_deref())) {
                (Some(start), Some(end)) => Some(end.millis_since(start)),
                _ => None,
            },
        })),
        Item::RunInterruptResult(result) => EntryKind::Event(TimelineEvent::Divider(Divider {
            kind: DividerKind::RunInterrupted,
            run_id,
            label: shared(result.message.clone()),
            detail: None,
            action: false,
        })),
        Item::Fork(_) => EntryKind::Event(TimelineEvent::Divider(Divider {
            kind: DividerKind::Forked,
            run_id,
            label: item_title(item).map(shared).unwrap_or_else(|| t!("Forked from conversation")),
            detail: None,
            action: true,
        })),
        Item::Handoff(handoff) => EntryKind::Event(TimelineEvent::Divider(Divider {
            kind: DividerKind::Handoff,
            run_id,
            label: item_title(item).map(shared).unwrap_or_else(|| shared(handoff.to_provider_instance_id.to_string())),
            detail: handoff.summary.clone().map(shared),
            action: false,
        })),
        _ => EntryKind::Work(work_entry(&row.item, created_at, superseded_attempt)),
    };
    // A message entry is keyed by its message id, as in the TypeScript.
    let id = match &kind {
        EntryKind::Message(message) => EntryId(message.id.0.clone()),
        _ => id,
    };
    Some(TimelineEntry { id, created_at, kind })
}

fn run_key(run: Option<&t3code_contracts::RunId>) -> &str {
    run.map_or("", |run| run.as_str())
}

/// Items of superseded attempts: each item's node resolved up to its attempt (`resolveAttempt`).
fn superseded_attempts(projection: &ThreadProjection) -> HashMap<t3code_contracts::TurnItemId, AttemptId> {
    if !projection.attempts.iter().any(|attempt| attempt.status == OrchestrationV2RunAttemptStatus::Superseded) {
        return HashMap::new();
    }
    let by_root: HashMap<_, _> = projection.attempts.iter().map(|attempt| (&attempt.root_node_id, attempt)).collect();
    let nodes: HashMap<_, _> = projection.nodes.iter().map(|node| (&node.id, node)).collect();
    let mut result = HashMap::new();
    for row in projection.visible_turn_items.iter() {
        let (Some(start), Some(run_id)) = (row.item.item_node_id(), row.item.item_run_id()) else { continue };
        let mut node_id = start;
        let mut visited = HashSet::new();
        let attempt = loop {
            if !visited.insert(node_id) {
                break None;
            }
            if let Some(attempt) = by_root.get(node_id).filter(|attempt| &attempt.run_id == run_id) {
                break Some(*attempt);
            }
            let Some(node) = nodes.get(node_id) else { break None };
            if let Some(attempt) = by_root.get(&node.root_node_id).filter(|attempt| &attempt.run_id == run_id) {
                break Some(*attempt);
            }
            match &node.parent_node_id {
                Some(parent) => node_id = parent,
                None => break None,
            }
        };
        if let Some(attempt) = attempt.filter(|attempt| attempt.status == OrchestrationV2RunAttemptStatus::Superseded) {
            result.insert(row.item.item_id(), AttemptId(id_of(&attempt.id)));
        }
    }
    result
}

type Memo = (Arc<Item>, Option<AttemptId>, bool, Option<TimelineEntry>);

/// Maps projections to timeline entries, reusing entries whose item did not change.
#[derive(Default)]
pub struct TimelineMapper {
    memo: HashMap<t3code_contracts::TurnItemId, Memo>,
    /// Entries mapped (not reused) by the last call, for measurements.
    pub last_mapped: usize,
}

impl TimelineMapper {
    /// The timeline entries of `projection`, then local messages the server has not
    /// confirmed (`deriveTimelineEntriesFromVisibleTurnItems`).
    pub fn entries(&mut self, projection: &ThreadProjection, optimistic: &[OptimisticMessage]) -> Vec<TimelineEntry> {
        let superseded = superseded_attempts(projection);
        // An answer that started a later run stays a normal message at that run's position.
        let folded: HashSet<String> = projection
            .visible_turn_items
            .iter()
            .filter_map(|row| match &*row.item {
                Item::UserInputRequest(request) => request
                    .question_answer
                    .as_ref()
                    .map(|answer| format!("async-answer:{}|{}", answer.request_id, run_key(row.item.item_run_id()))),
                _ => None,
            })
            .collect();
        let mut next = HashMap::with_capacity(projection.visible_turn_items.len());
        let mut entries = Vec::with_capacity(projection.visible_turn_items.len() + optimistic.len());
        let mut committed = HashSet::new();
        self.last_mapped = 0;
        for row in projection.visible_turn_items.iter() {
            let item_id = row.item.item_id();
            let attempt = superseded.get(&item_id).cloned();
            let folded_answer = match &*row.item {
                Item::UserMessage(user) => folded.contains(&format!("{}|{}", user.message_id, run_key(row.item.item_run_id()))),
                _ => false,
            };
            let entry = match self.memo.remove(&item_id) {
                Some((item, memo_attempt, memo_folded, entry))
                    if Arc::ptr_eq(&item, &row.item) && memo_attempt == attempt && memo_folded == folded_answer =>
                {
                    entry
                }
                _ => {
                    self.last_mapped += 1;
                    map_row(row, attempt.clone(), folded_answer)
                }
            };
            if let Some(entry) = &entry {
                if let EntryKind::Message(message) = &entry.kind {
                    committed.insert(message.id.clone());
                }
                entries.push(entry.clone());
            }
            next.insert(item_id, (Arc::clone(&row.item), attempt, folded_answer, entry));
        }
        self.memo = next;
        for local in optimistic {
            let id = MessageId(id_of(&local.message_id));
            if committed.contains(&id) {
                continue;
            }
            let at = Timestamp(local.created_at);
            entries.push(TimelineEntry {
                id: EntryId(id.0.clone()),
                created_at: at,
                kind: EntryKind::Message(ChatMessage {
                    attachments: attachments(&local.attachments),
                    ..message(id, MessageRole::User, &local.text, at, at)
                }),
            });
        }
        entries
    }
}

fn run_status(status: Run) -> RunStatus {
    match status {
        Run::Preparing | Run::Queued | Run::Starting => RunStatus::Starting,
        Run::Running => RunStatus::Running,
        Run::Waiting => RunStatus::Waiting,
        Run::Completed | Run::RolledBack => RunStatus::Completed,
        Run::Failed => RunStatus::Failed,
        Run::Interrupted | Run::Cancelled => RunStatus::Interrupted,
    }
}

/// The drawer for the first pending question (`derivePendingUserInputs`).
fn pending_question(requests: &[PendingRequest]) -> Option<PendingQuestion> {
    let pending = requests.iter().find(|request| request.request.kind == RequestKind::UserInput)?;
    let Some(Item::UserInputRequest(card)) = pending.card.as_deref() else { return None };
    let question = card.questions.first()?;
    Some(PendingQuestion {
        request_id: RequestId(id_of(&pending.request.id)),
        header: shared(question.header.clone()),
        question: shared(question.question.clone()),
        options: question.options.iter().map(|option| (shared(option.label.clone()), shared(option.description.clone()))).collect(),
        multi_select: question.multi_select == Some(true),
        index: 0,
        count: card.questions.len(),
    })
}

/// One run's checkpoint: the turn count its diff is fetched by and the reply it follows.
#[derive(Clone, Debug)]
pub struct CheckpointSummary {
    /// The run's last assistant message; the timeline's changed-files card and the diff panel's
    /// turn chips key turns by it.
    pub assistant_message_id: Option<SharedString>,
    pub turn_count: u64,
    pub completed_at: String,
    pub files: Arc<[FileChange]>,
}

/// `deriveThreadCheckpointSummaries`: one summary per run checkpoint, oldest first.
pub fn checkpoint_summaries(projection: &ThreadProjection) -> Vec<CheckpointSummary> {
    let mut last_assistant: HashMap<&str, &str> = HashMap::new();
    for message in projection.messages.iter() {
        if message.role == t3code_contracts::OrchestrationV2ConversationMessageRole::Assistant
            && let Some(run) = message.run_id.as_ref()
        {
            last_assistant.insert(run.as_ref(), message.id.as_ref());
        }
    }
    projection
        .checkpoints
        .iter()
        .filter_map(|checkpoint| {
            let (turn_count, run) = (checkpoint.app_run_ordinal?, checkpoint.run_id.as_ref()?);
            Some(CheckpointSummary {
                assistant_message_id: last_assistant.get(run.as_ref()).map(|id| SharedString::from(id.to_string())),
                turn_count,
                completed_at: checkpoint.captured_at.clone(),
                files: checkpoint
                    .files
                    .iter()
                    .map(|file| FileChange {
                        path: shared(file.path.clone()),
                        additions: file.additions as u32,
                        deletions: file.deletions as u32,
                        operation: shared(file.kind.clone()),
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Every question of the first pending user-input request, with each question's id
/// (`derivePendingUserInputs`); the answers map is keyed by those ids.
pub fn pending_questions(projection: &ThreadProjection) -> Vec<(PendingQuestion, String)> {
    let requests = pending_requests(projection);
    let Some(pending) = requests.iter().find(|request| request.request.kind == RequestKind::UserInput) else { return Vec::new() };
    let Some(Item::UserInputRequest(card)) = pending.card.as_deref() else { return Vec::new() };
    let count = card.questions.len();
    card.questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let pending = PendingQuestion {
                request_id: RequestId(id_of(&pending.request.id)),
                header: shared(question.header.clone()),
                question: shared(question.question.clone()),
                options: question.options.iter().map(|option| (shared(option.label.clone()), shared(option.description.clone()))).collect(),
                multi_select: question.multi_select == Some(true),
                index,
                count,
            };
            (pending, question.id.clone())
        })
        .collect()
}

/// Everything the timeline derives its rows from, for one projection.
pub fn timeline_input(mapper: &mut TimelineMapper, projection: &ThreadProjection, optimistic: &[OptimisticMessage]) -> TimelineInput {
    let entries = mapper.entries(projection, optimistic);
    // Entries are keyed by message id for messages and item id otherwise.
    let item_sources: HashMap<SharedString, Arc<Item>> = projection
        .visible_turn_items
        .iter()
        .map(|row| {
            let key = match &*row.item {
                Item::UserMessage(user) => id_of(&user.message_id),
                Item::AssistantMessage(reply) => id_of(&reply.message_id),
                other => id_of(&other.item_id()),
            };
            (key, Arc::clone(&row.item))
        })
        .collect();
    let requests = pending_requests(projection);
    let active = active_run(projection);
    let latest = projection.runs.iter().filter(|run| run.status != Run::Queued).max_by_key(|run| run.ordinal);
    TimelineInput {
        entries: Arc::new(entries),
        pending_approval_ids: requests
            .iter()
            .filter(|request| !matches!(request.request.kind, RequestKind::UserInput | RequestKind::AuthRefresh))
            .map(|request| RequestId(id_of(&request.request.id)))
            .collect(),
        latest_run: latest.map(|run| LatestRun {
            run_id: RunId(id_of(&run.id)),
            status: run_status(run.status),
            started_at: ms(run.started_at.as_deref()),
            completed_at: ms(run.completed_at.as_deref()),
            turn_ended: run.completed_at.is_some(),
        }),
        is_working: active.is_some(),
        active_turn_started_at: active.and_then(|run| ms(Some(run.started_at.as_deref().unwrap_or(&run.requested_at)))),
        turn_diff_summaries: checkpoint_summaries(projection)
            .into_iter()
            .filter(|summary| !summary.files.is_empty())
            .filter_map(|summary| Some(TurnDiffSummary { assistant_message_id: MessageId(summary.assistant_message_id?), files: summary.files }))
            .collect(),
        supports_rollback: false,
        worktree_setup: None,
        queued: user_queued_runs(projection)
            .into_iter()
            .map(|(run, message)| QueuedMessage {
                id: id_of(&run.id),
                text: message.as_ref().map(|m| shared(m.text.clone())).unwrap_or_default(),
                created_at: Timestamp(parse_ms(&run.requested_at).unwrap_or_default()),
                image_count: message
                    .as_ref()
                    .map_or(0, |m| m.attachments.iter().filter(|a| matches!(a, ChatAttachment::Image(_))).count()),
                guest: message.as_ref().is_some_and(|m| m.linked_sender.is_some()),
                editing: false,
            })
            .collect(),
        pending_question: pending_question(&requests),
        item_sources: Arc::new(item_sources),
    }
}

/// Messages whose text or streaming flag differs between two entry lists of the same
/// structure, as `(id, text, streaming)` for `MessagesTimeline::update_message_text`.
pub fn changed_message_text(previous: &[TimelineEntry], next: &[TimelineEntry]) -> Vec<(MessageId, SharedString, bool)> {
    previous
        .iter()
        .zip(next)
        .filter_map(|(before, after)| match (&before.kind, &after.kind) {
            (EntryKind::Message(a), EntryKind::Message(b)) if a.id == b.id && (a.text != b.text || a.streaming != b.streaming) => {
                Some((b.id.clone(), b.text.clone(), b.streaming))
            }
            _ => None,
        })
        .collect()
}
