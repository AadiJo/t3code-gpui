//! The text and grouping of work-log rows, ported from the web: the fields `projectedWorkEntry`
//! (`session-logic.ts`) keeps on a `WorkLogEntry`, and the labels, actions, failure checks and
//! group summaries `MessagesTimeline.logic.ts`, `work-log/presentation.ts`,
//! `toolActivityCopy.ts` and `t3ToolSummary.ts` derive from them.
//!
//! [`WorkSource::new`] runs once per mapped turn item and keeps every label the rows read, so
//! row derivation stays cheap; the mapper remaps when the locale changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::SharedString;
use serde_json::{Map, Value, json};
use t3code_client_runtime::change_request_links::{SourceControlHostKind, parse_change_request_url};
use t3code_client_runtime::work_log::command_label::{command_display_text, command_program_name};
use t3code_client_runtime::work_log::presentation::tool_output_indicates_failure;
use t3code_client_runtime::work_log::t3_tools::{
    T3SummaryAction, T3ToolDefinition, T3ToolIcon, read_t3_tool_input, read_t3_tool_result, t3_summary_action, t3_tool_definition,
    t3_tool_result_indicates_failure,
};
use t3code_client_runtime::work_log::tool_activity::{
    DynamicToolTitle, ToolActivityAction, classify_tool_activity, collect_tool_file_paths, describe_dynamic_tool_title,
    search_tool_label_parts,
};
use t3code_contracts::{OrchestrationV2TurnItem as Item, ToolActivitySource, ToolActivitySourceKind};
use t3code_shared::turn_item::TurnItemExt;

use crate::file_path_display::format_workspace_relative_path;
use crate::i18n::t;

/// `WorkLogEntry.tone`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Thinking,
    Tool,
    Info,
    Error,
}

/// `WorkLogToolLifecycleStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    InProgress,
    Completed,
    Failed,
    Declined,
    Stopped,
    Idle,
}

/// `WorkLogEntry.sourceActivityKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceActivityKind {
    ContextCompaction,
    RuntimeWarning,
    RuntimeError,
}

/// `ToolGroupAction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupAction {
    LinkPr,
    UnlinkPr,
    ListPrs,
    WatchPr,
    UnwatchPr,
    Read,
    Edit,
    Command,
    ThreadCreate,
    Browser,
    Device,
    CodeSearch,
    Search,
    Other,
    Update,
}

/// `ToolGroupSummaryKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    PullRequest,
    Action(GroupAction),
    DynamicTool,
    Reasoning,
    AgentTool,
    ToneTool,
    Other,
    Mixed,
}

/// The fields of the web's `WorkLogEntry` the labels read.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkFacts {
    pub item: Arc<Item>,
    pub tone: Tone,
    /// `None` for notifications, which the web maps without a lifecycle.
    pub lifecycle: Option<Lifecycle>,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// The raw command of a command execution.
    pub command: Option<SharedString>,
    pub changed_files: Arc<[SharedString]>,
    pub tool_title: Option<SharedString>,
    pub source_activity_kind: Option<SourceActivityKind>,
    pub tool_source: Option<ToolActivitySource>,
    pub viewed_image_path: Option<SharedString>,
}

impl WorkFacts {
    fn item_type(&self) -> &str {
        self.item.type_name()
    }

    fn is_reasoning(&self) -> bool {
        matches!(*self.item, Item::Reasoning(_))
    }

    fn dynamic(&self) -> Option<&t3code_contracts::OrchestrationV2TurnItemDynamicTool> {
        match &*self.item {
            Item::DynamicTool(tool) => Some(tool),
            _ => None,
        }
    }

    /// `toolData`: input and output for a dynamic tool, the item itself otherwise.
    fn tool_data(&self) -> Value {
        match self.dynamic() {
            Some(tool) => json!({ "input": tool.input, "output": tool.output }),
            None => serde_json::to_value(&*self.item).unwrap_or(Value::Null),
        }
    }

    /// `workEntryToolName`.
    fn tool_name(&self) -> Option<&str> {
        if let Some(name) = self.dynamic().and_then(|tool| tool.tool_name.as_deref()).filter(|n| !n.is_empty()) {
            return Some(name);
        }
        match &self.tool_title {
            Some(title) if t3_tool_definition(Some(title), None).is_some() => Some(title),
            _ => Some(&self.label),
        }
    }

    /// `workEntryToolInput`: only a dynamic tool's input can name an action or a pull request.
    fn tool_input(&self) -> Option<&Value> {
        self.dynamic().map(|tool| &tool.input)
    }

    /// `workEntryToolOutput`.
    fn tool_output(&self) -> Option<Value> {
        match &*self.item {
            Item::DynamicTool(tool) => tool.output.clone(),
            Item::CommandExecution(command) => command.output.clone().map(Value::String),
            _ => None,
        }
    }

    fn collapsed_detail(&self) -> Option<String> {
        self.detail.as_deref().map(|d| d.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|d| !d.is_empty())
    }
}

fn record(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// The PR action a T3 tool performs (`ToolGroupAction` link/unlink/list/watch/unwatch).
fn pr_action(definition: &T3ToolDefinition) -> Option<GroupAction> {
    Some(match definition.summary_action {
        T3SummaryAction::LinkPr => GroupAction::LinkPr,
        T3SummaryAction::UnlinkPr => GroupAction::UnlinkPr,
        T3SummaryAction::ListPrs => GroupAction::ListPrs,
        T3SummaryAction::WatchPr => GroupAction::WatchPr,
        T3SummaryAction::UnwatchPr => GroupAction::UnwatchPr,
        _ => return None,
    })
}

/// A T3 tool row's sentence and icon (`resolveT3McpToolPresentation`).
#[derive(Clone, Debug)]
pub struct ToolPresentation {
    pub display_name: SharedString,
    pub icon: T3ToolIcon,
    pub action: Option<GroupAction>,
}

fn status_key(status: Option<Lifecycle>) -> &'static str {
    match status {
        Some(Lifecycle::InProgress) => "inProgress",
        Some(Lifecycle::Completed) => "completed",
        Some(Lifecycle::Failed) => "failed",
        Some(Lifecycle::Declined) => "declined",
        Some(Lifecycle::Stopped) => "stopped",
        _ => "other",
    }
}

fn change_request_short_label(kind: Option<SourceControlHostKind>) -> &'static str {
    match kind {
        Some(SourceControlHostKind::GitLab) => "MR",
        _ => "PR",
    }
}

/// `resolveWorkEntryToolPresentation`.
pub fn tool_presentation(facts: &WorkFacts, fallback: Option<Lifecycle>) -> Option<ToolPresentation> {
    let definition = t3_tool_definition(facts.tool_name(), facts.tool_input())?;
    let mut status = facts.lifecycle.or(fallback);
    if facts.tool_output().is_some_and(|output| t3_tool_result_indicates_failure(&output)) {
        status = Some(Lifecycle::Failed);
    }
    let action = pr_action(definition);
    let input = facts.tool_input().and_then(record);
    let url = input.and_then(|i| i.get("url")).and_then(Value::as_str).and_then(parse_change_request_url);
    let number = url
        .as_ref()
        .map(|(_, link)| link.number as f64)
        .or_else(|| input.and_then(|i| i.get("number")).and_then(Value::as_f64));
    let pull_request_number = number.filter(|n| {
        action.is_some_and(|a| a != GroupAction::ListPrs) && n.fract() == 0.0 && *n > 0.0 && *n <= 9_007_199_254_740_991.0
    });
    let short_label = change_request_short_label(url.as_ref().map(|(kind, _)| *kind));
    let key = status_key(status);
    let display_name = match pull_request_number {
        Some(number) => t3_change_request_sentence(definition.name, key, short_label, &format!("{number}")),
        None => None,
    }
    .or_else(|| t3_sentence(definition.name, key))
    .unwrap_or_else(|| {
        // A tool added to the shared table after the sentences were generated.
        let [action, running, completed, detail] = definition.labels;
        let verb = match status {
            Some(Lifecycle::InProgress) => running.to_owned(),
            Some(Lifecycle::Completed) => completed.to_owned(),
            Some(Lifecycle::Failed) => format!("Failed to {}", action.to_lowercase()),
            Some(Lifecycle::Declined) => format!("Declined to {}", action.to_lowercase()),
            Some(Lifecycle::Stopped) => format!("Stopped {}", running.to_lowercase()),
            _ => running.to_owned(),
        };
        let object = pull_request_number.map_or_else(|| detail.to_owned(), |n| format!("{short_label} #{n}"));
        format!("{verb} {object}").into()
    });
    Some(ToolPresentation { display_name, icon: definition.icon, action })
}

/// `normalizeCompactToolLabel`.
fn normalize_compact_tool_label(value: &str) -> &str {
    let trimmed = value.trim_end();
    let lower = trimmed.to_lowercase();
    for suffix in ["completed", "complete"] {
        if lower.ends_with(suffix) {
            let head = &trimmed[..trimmed.len() - suffix.len()];
            if head.ends_with(char::is_whitespace) {
                return head.trim();
            }
        }
    }
    value.trim()
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

/// `dynamicToolTitle` (`toolActivityCopy.ts`).
fn dynamic_tool_title(facts: &WorkFacts) -> Option<SharedString> {
    let tool = facts.dynamic()?;
    Some(match describe_dynamic_tool_title(tool.tool_name.as_deref(), &tool.input, tool.title.as_deref())? {
        DynamicToolTitle::Title(title) => title.to_owned().into(),
        DynamicToolTitle::ComputerUse => t!("Computer use"),
        DynamicToolTitle::RunJavaScript => t!("Run JavaScript"),
        DynamicToolTitle::Skill(name) => t!("Skill: {name}", name = name),
    })
}

/// `formatSearchToolLabel`.
pub fn format_search_tool_label(data: Option<&Map<String, Value>>) -> Option<SharedString> {
    let parts = search_tool_label_parts(data);
    Some(match (parts.query, parts.glob, parts.target) {
        (Some(query), _, Some(target)) => t!("Searched {query} in {target}", query = query, target = target),
        (None, Some(glob), Some(target)) => t!("Searched files {glob} in {target}", glob = glob, target = target),
        (_, Some(glob), None) if parts.query.is_none() => t!("Searched files {glob}", glob = glob),
        (Some(query), _, None) => t!("Searched {query}", query = query),
        (None, None, Some(target)) => t!("Searched in {target}", target = target),
        _ => return None,
    })
}

/// `formatReadToolLabel`.
pub fn format_read_tool_label(path: &str, extra: usize) -> SharedString {
    let path = path.trim();
    match (path.is_empty(), extra) {
        (true, 0) => t!("Read file"),
        (true, count) => t!("Read file +{count} more", count = count),
        (false, 0) => t!("Read {path}", path = path),
        (false, count) => t!("Read {path} +{count} more", path = path, count = count),
    }
}

/// `isToolLifecycleItemType` over turn item type names.
fn is_tool_lifecycle_item_type(item_type: &str) -> bool {
    matches!(item_type, "command_execution" | "file_change" | "web_search")
}

/// `workLogEntryIsToolLike` in `presentation.ts`.
fn presentation_tool_like(facts: &WorkFacts) -> bool {
    matches!(facts.tone, Tone::Tool | Tone::Thinking | Tone::Error)
        || facts.command.as_deref().is_some_and(|c| !c.trim().is_empty())
        || is_tool_lifecycle_item_type(facts.item_type())
}

/// `workLogEntryIsToolLike` in `session-logic.ts`.
fn session_tool_like(facts: &WorkFacts) -> bool {
    matches!(facts.tone, Tone::Tool | Tone::Thinking | Tone::Error) || facts.command.is_some()
}

fn is_local_code_search(facts: &WorkFacts) -> bool {
    match &*facts.item {
        Item::FileSearch(_) => true,
        Item::WebSearch(_) => {
            let label = normalize_compact_tool_label(facts.tool_title.as_deref().unwrap_or(&facts.label)).to_lowercase();
            label.match_indices("grep").any(|(index, _)| {
                let before = label[..index].chars().next_back();
                let after = label[index + 4..].chars().next();
                let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                !word(before) && !word(after)
            })
        }
        _ => false,
    }
}

/// `computeToolGroupAction`.
fn compute_group_action(facts: &WorkFacts, presentation: Option<&ToolPresentation>) -> GroupAction {
    if matches!(*facts.item, Item::ThreadCreated(_)) {
        return GroupAction::ThreadCreate;
    }
    if let Some(presentation) = presentation {
        if let Some(action) = presentation.action {
            return action;
        }
        match presentation.icon {
            T3ToolIcon::Browser => return GroupAction::Browser,
            T3ToolIcon::Device => return GroupAction::Device,
            _ => {}
        }
    }
    if facts.viewed_image_path.is_some() {
        return GroupAction::Read;
    }
    if matches!(*facts.item, Item::ApprovalRequest(_) | Item::UserInputRequest(_)) {
        return if presentation_tool_like(facts) { GroupAction::Other } else { GroupAction::Update };
    }
    let data = facts.tool_data();
    let mut data = data.as_object().cloned().unwrap_or_default();
    let tool_name = match facts.dynamic() {
        Some(tool) => tool.tool_name.clone(),
        None => data.get("toolName").and_then(Value::as_str).map(str::to_owned).or_else(|| facts.tool_title.as_ref().map(|t| t.to_string())),
    };
    if let Some(name) = tool_name.filter(|n| !n.is_empty()) {
        data.insert("toolName".into(), Value::String(name));
    }
    let item_type = match facts.item_type() {
        kind @ ("command_execution" | "file_change" | "web_search") => Some(kind),
        "dynamic_tool" => Some("dynamic_tool_call"),
        _ => None,
    };
    let classified = classify_tool_activity(item_type, None, Some(&data));
    let item_type = facts.item_type();
    if classified == ToolActivityAction::Read {
        return GroupAction::Read;
    }
    if classified == ToolActivityAction::FileChange || item_type == "file_change" {
        return GroupAction::Edit;
    }
    if classified == ToolActivityAction::Command || item_type == "command_execution" || facts.command.as_deref().is_some_and(|c| !c.is_empty()) {
        return GroupAction::Command;
    }
    if classified == ToolActivityAction::Search {
        return if item_type == "web_search" && !is_local_code_search(facts) { GroupAction::Search } else { GroupAction::CodeSearch };
    }
    if is_local_code_search(facts) {
        return GroupAction::CodeSearch;
    }
    if item_type == "web_search" {
        return GroupAction::Search;
    }
    if !facts.changed_files.is_empty() {
        return GroupAction::Edit;
    }
    if presentation_tool_like(facts) { GroupAction::Other } else { GroupAction::Update }
}

/// `workEntryIndicatesToolFailureFromOutput`.
fn failure_from_output(facts: &WorkFacts, include_command: bool) -> bool {
    if facts.tone == Tone::Error || matches!(facts.lifecycle, Some(Lifecycle::Failed | Lifecycle::Declined)) {
        return true;
    }
    // Reasoning is the model's prose; phrases like "exit code 2 is expected" are not failures.
    if !presentation_tool_like(facts) || facts.is_reasoning() {
        return false;
    }
    if t3_tool_definition(facts.tool_name(), None).is_some() && facts.tool_output().is_some_and(|o| t3_tool_result_indicates_failure(&o)) {
        return true;
    }
    if let Item::CommandExecution(command) = &*facts.item {
        if let Some(code) = command.exit_code {
            return code != 0;
        }
        if command.output_indicates_failure == Some(true) {
            return true;
        }
        if let Some(output) = &command.output {
            let end = output.char_indices().nth(32_768).map_or(output.len(), |(i, _)| i);
            if tool_output_indicates_failure(&output[..end]) {
                return true;
            }
        }
    }
    let output = if include_command {
        [facts.detail.as_deref(), facts.command.as_deref()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
    } else {
        facts.detail.as_deref().unwrap_or_default().to_owned()
    };
    !output.is_empty() && tool_output_indicates_failure(&output)
}

/// The raw paths a read names (`workEntryReadRawPaths`).
fn read_raw_paths(facts: &WorkFacts) -> Vec<String> {
    if !facts.changed_files.is_empty() {
        return facts.changed_files.iter().map(|p| p.to_string()).collect();
    }
    if let Some(tool) = facts.dynamic() {
        let paths = collect_tool_file_paths(&tool.input);
        if !paths.is_empty() {
            return paths;
        }
    }
    collect_tool_file_paths(&facts.tool_data())
}

fn display_label(facts: &WorkFacts, action: GroupAction, presentation: Option<&ToolPresentation>, root: Option<&str>) -> SharedString {
    if matches!(*facts.item, Item::SystemNotice(_)) {
        return facts.label.clone();
    }
    if facts.is_reasoning() || facts.tone == Tone::Thinking {
        return facts.collapsed_detail().map_or_else(|| facts.label.clone(), SharedString::from);
    }
    if let Some(presentation) = presentation {
        return presentation.display_name.clone();
    }
    if let Some(command) = facts.command.as_deref().filter(|c| !c.trim().is_empty()) {
        return command_display_text(command).into();
    }
    let searching = matches!(action, GroupAction::CodeSearch | GroupAction::Search);
    if searching {
        let label = if matches!(*facts.item, Item::FileSearch(_)) {
            Some(facts.label.clone())
        } else {
            format_search_tool_label(facts.tool_data().as_object())
        };
        if let Some(label) = label {
            return label;
        }
    }
    let read_paths: Vec<String> =
        if action == GroupAction::Read { read_raw_paths(facts).iter().map(|p| format_workspace_relative_path(p, root)).collect() } else { Vec::new() };
    if let Some(first) = read_paths.first() {
        return format_read_tool_label(first, read_paths.len() - 1);
    }
    let provider_retry = matches!(&*facts.item, Item::Error(error) if error.retry.is_some());
    if let Some(title) = dynamic_tool_title(facts) {
        return title;
    }
    let detail = facts.detail.as_deref().map(str::trim).filter(|d| !d.is_empty());
    let detail_is_search_output = searching && detail.is_some_and(|d| d.contains(['\r', '\n']));
    if let Some(detail) = detail
        && !provider_retry
        && action != GroupAction::Read
        && !detail_is_search_output
    {
        return detail.to_owned().into();
    }
    if let Some(first) = facts.changed_files.first() {
        let path = format_workspace_relative_path(first, root);
        return if facts.changed_files.len() == 1 {
            path.into()
        } else {
            t!("{count, plural, one {{path} +{count} more} other {{path} +{count} more}}", path = path, count = facts.changed_files.len() - 1)
        };
    }
    if action == GroupAction::Read && facts.viewed_image_path.is_none() {
        return t!("Read file");
    }
    let heading = normalize_compact_tool_label(facts.tool_title.as_deref().filter(|t| !t.is_empty()).unwrap_or(&facts.label));
    capitalize_first(heading).into()
}

/// `liveActivityToolStatus`.
fn live_status(lifecycle: Option<Lifecycle>, present_tense: bool) -> Lifecycle {
    match lifecycle {
        Some(status @ (Lifecycle::Failed | Lifecycle::Declined | Lifecycle::Stopped | Lifecycle::Idle)) => status,
        _ if present_tense || lifecycle == Some(Lifecycle::InProgress) => Lifecycle::InProgress,
        _ => Lifecycle::Completed,
    }
}

fn live_label(facts: &WorkFacts, action: GroupAction, display: &SharedString, active: bool) -> SharedString {
    let status = live_status(facts.lifecycle, active);
    if facts.is_reasoning() {
        return facts.collapsed_detail().map(SharedString::from).unwrap_or_else(|| {
            if status == Lifecycle::InProgress { t!("Thinking") } else { t!("Thought") }
        });
    }
    let live = WorkFacts { lifecycle: Some(status), ..facts.clone() };
    if let Some(presentation) = tool_presentation(&live, None) {
        return presentation.display_name;
    }
    if let Some(command) = facts.command.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        return match command_program_name(command) {
            None => match status {
                Lifecycle::InProgress => t!("Running command"),
                Lifecycle::Failed => t!("Failed command"),
                Lifecycle::Declined => t!("Declined command"),
                Lifecycle::Stopped => t!("Stopped command"),
                _ => t!("Ran command"),
            },
            Some(program) => match status {
                Lifecycle::InProgress => t!("Running {program}", program = program),
                Lifecycle::Failed => t!("Failed {program}", program = program),
                Lifecycle::Declined => t!("Declined {program}", program = program),
                Lifecycle::Stopped => t!("Stopped {program}", program = program),
                _ => t!("Ran {program}", program = program),
            },
        };
    }
    let _ = action;
    display.clone()
}

/// The per-entry presentation the timeline rows read, computed once per mapped item.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkSource {
    pub facts: WorkFacts,
    pub action: GroupAction,
    /// `workEntryDisplayLabel` (no workspace root yet).
    pub display_label: SharedString,
    /// `liveWorkEntryLabel` while the run is active, and once it settled.
    pub live_label_active: SharedString,
    pub live_label_settled: SharedString,
    /// `singleToolCallLabel`.
    pub single_call_label: SharedString,
    /// `workEntryIndicatesToolFailure` and `workEntryDisplayIndicatesToolFailure`.
    pub failure: bool,
    pub display_failure: bool,
    /// `workEntryIndicatesToolNeutralStatus`.
    pub neutral: bool,
    /// `workLogEntryIsToolLike` (session-logic).
    pub tool_like: bool,
    pub pull_request_icon: bool,
    /// The T3 tool action a group of these calls summarizes as.
    pub t3_summary: Option<T3SummaryAction>,
}

impl WorkSource {
    pub fn new(facts: WorkFacts) -> Self {
        let presentation = tool_presentation(&facts, None);
        let action = compute_group_action(&facts, presentation.as_ref());
        let display_label = display_label(&facts, action, presentation.as_ref(), None);
        let single_call_label = if facts.is_reasoning() {
            facts.collapsed_detail().map(SharedString::from).unwrap_or_else(|| t!("Thought"))
        } else if let Some(presentation) = tool_presentation(&facts, Some(Lifecycle::Completed)) {
            presentation.display_name
        } else if let Some(title) = dynamic_tool_title(&facts) {
            title
        } else if matches!(*facts.item, Item::WebSearch(_)) {
            facts.tool_title.clone().unwrap_or_else(|| t!("Web search"))
        } else {
            display_label.clone()
        };
        let failure = failure_from_output(&facts, true);
        let display_failure = failure_from_output(&facts, false);
        let tool_like = session_tool_like(&facts);
        let success = tool_like
            && !failure
            && !(facts.tone == Tone::Thinking && !facts.is_reasoning())
            && !matches!(
                facts.lifecycle,
                Some(Lifecycle::Failed | Lifecycle::Declined | Lifecycle::InProgress | Lifecycle::Stopped | Lifecycle::Idle)
            );
        let t3_summary = t3_summary_action(facts.tool_name(), facts.tool_input());
        Self {
            live_label_active: live_label(&facts, action, &display_label, true),
            live_label_settled: live_label(&facts, action, &display_label, false),
            pull_request_icon: presentation.as_ref().is_some_and(|p| p.icon == T3ToolIcon::PullRequest),
            neutral: tool_like && !failure && !success,
            facts,
            action,
            display_label,
            single_call_label,
            failure,
            display_failure,
            tool_like,
            t3_summary,
        }
    }

    /// `workEntryIsVisibleInGroup`.
    pub fn visible_in_group(&self, expanded_tool_group: bool) -> bool {
        if self.facts.is_reasoning() {
            return self.facts.detail.as_deref().is_some_and(|d| !d.trim().is_empty());
        }
        (expanded_tool_group && self.facts.lifecycle == Some(Lifecycle::InProgress))
            || self.facts.lifecycle == Some(Lifecycle::Stopped)
            || !self.neutral
    }

    /// `liveWorkEntryLabel`.
    pub fn live_label(&self, active: bool) -> SharedString {
        if active { self.live_label_active.clone() } else { self.live_label_settled.clone() }
    }
}

fn action_label(action: GroupAction, count: usize) -> SharedString {
    match action {
        GroupAction::LinkPr => t!("{count, plural, one {Linked {count} pull request} other {Linked {count} pull requests}}", count = count),
        GroupAction::UnlinkPr => {
            t!("{count, plural, one {Unlinked {count} pull request} other {Unlinked {count} pull requests}}", count = count)
        }
        GroupAction::WatchPr => {
            t!("{count, plural, one {Watching {count} pull request} other {Watching {count} pull requests}}", count = count)
        }
        GroupAction::UnwatchPr => t!(
            "{count, plural, one {Stopped watching {count} pull request} other {Stopped watching {count} pull requests}}",
            count = count
        ),
        GroupAction::ListPrs => t!(
            "{count, plural, =1 {Checked linked pull requests} other {Checked linked pull requests {count} times}}",
            count = count
        ),
        GroupAction::Read => t!("{count, plural, one {Read {count} file} other {Read {count} files}}", count = count),
        GroupAction::Edit => t!("{count, plural, one {Changed {count} file} other {Changed {count} files}}", count = count),
        GroupAction::Command => t!("{count, plural, one {Ran {count} command} other {Ran {count} commands}}", count = count),
        GroupAction::ThreadCreate => t!("{count, plural, one {Created {count} thread} other {Created {count} threads}}", count = count),
        GroupAction::Device => t!(
            "{count, plural, one {Used device controls {count} time} other {Used device controls {count} times}}",
            count = count
        ),
        GroupAction::Browser => t!("{count, plural, one {Used browser {count} time} other {Used browser {count} times}}", count = count),
        GroupAction::Search => {
            t!("{count, plural, one {Searched the web {count} time} other {Searched the web {count} times}}", count = count)
        }
        GroupAction::CodeSearch => {
            t!("{count, plural, one {Searched code {count} time} other {Searched code {count} times}}", count = count)
        }
        GroupAction::Other => t!("{count, plural, one {Used {count} tool} other {Used {count} tools}}", count = count),
        GroupAction::Update => t!("{count, plural, one {Received {count} update} other {Received {count} updates}}", count = count),
    }
}

/// `toolGroupActionCount`: edits count distinct files, plus edits that name none.
fn action_count(action: GroupAction, entries: &[&WorkSource]) -> usize {
    if action != GroupAction::Edit {
        return entries.len();
    }
    let mut files = HashSet::new();
    let mut without = 0;
    for entry in entries {
        if entry.facts.changed_files.is_empty() {
            without += 1;
        }
        files.extend(entry.facts.changed_files.iter().cloned());
    }
    files.len() + without
}

fn join_clauses(items: &[SharedString]) -> SharedString {
    match items {
        [] => SharedString::default(),
        [one] => one.clone(),
        [first, second] => t!("{first} and {second}", first = first, second = second),
        [rest @ .., last] => {
            let items = rest.iter().map(|s| s.as_ref()).collect::<Vec<_>>().join(", ");
            t!("{items}, and {last}", items = items, last = last)
        }
    }
}

/// `summaryActionPriority`: lower sorts first.
fn summary_priority(action: GroupAction, t3: Option<T3SummaryAction>) -> u8 {
    use T3SummaryAction as A;
    match t3 {
        Some(
            A::Delegate
            | A::TaskCancel
            | A::ThreadCreate
            | A::ThreadSend
            | A::ThreadInterrupt
            | A::WorkflowStop
            | A::BackgroundWait
            | A::ScheduleCreate
            | A::ScheduleUpdate
            | A::ScheduleDelete
            | A::ScheduleRun
            | A::ThreadConfigure
            | A::ThreadFork
            | A::ThreadMerge
            | A::ThreadOrganize
            | A::ThreadUpdate
            | A::QueueEdit
            | A::QueueCancel
            | A::QueueReorder
            | A::QueueSteer
            | A::QuestionRespond
            | A::WorktreeHandoff
            | A::ProjectCreate
            | A::ProjectUpdate
            | A::ProjectDelete
            | A::ProjectClone
            | A::EnvironmentUpdate
            | A::ThemeSet
            | A::AttachmentPrepare
            | A::AttachmentDiscard
            | A::AttachmentSend,
        ) => 0,
        Some(_) => 1,
        None => match action {
            GroupAction::Command | GroupAction::Edit | GroupAction::ThreadCreate => 0,
            GroupAction::Other | GroupAction::Update => 2,
            _ => 1,
        },
    }
}

/// `summarizeToolGroup`: "Read 3 files and ran 2 commands", and whether any call failed.
pub fn summarize_tool_group(entries: &[&WorkSource]) -> (SharedString, bool) {
    let tools: Vec<&WorkSource> = entries.iter().copied().filter(|e| !e.facts.is_reasoning()).collect();
    if !entries.is_empty() && tools.is_empty() {
        let summary = if entries.len() == 1 {
            t!("Thought", context = "noun, a reasoning step")
        } else {
            t!("Thought (×{count})", count = entries.len())
        };
        return (summary, false);
    }
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum Key {
        Action(GroupAction),
        T3(T3SummaryAction),
    }
    let mut groups: Vec<(Key, GroupAction, Option<T3SummaryAction>, Vec<&WorkSource>)> = Vec::new();
    let mut sources: Vec<&ToolActivitySource> = Vec::new();
    for entry in &tools {
        let t3 = entry.t3_summary;
        if let (Some(source), None) = (&entry.facts.tool_source, t3) {
            match sources.iter().position(|s| s.key == source.key) {
                Some(index) => sources[index] = source,
                None => sources.push(source),
            }
            continue;
        }
        let key = t3.map_or(Key::Action(entry.action), Key::T3);
        match groups.iter_mut().find(|group| group.0 == key) {
            Some(group) => group.3.push(entry),
            None => groups.push((key, entry.action, t3, vec![entry])),
        }
    }
    let summaries: Vec<(usize, usize, u8, SharedString, usize)> = groups
        .iter()
        .enumerate()
        .map(|(index, (_, action, t3, group))| {
            let (label, failed) = match t3 {
                Some(t3) => summarize_t3_calls(*t3, group),
                None => (action_label(*action, action_count(*action, group)), group.iter().filter(|e| e.display_failure).count()),
            };
            (index, group.len(), summary_priority(*action, *t3), label, failed)
        })
        .collect();
    let mut selected: Vec<_> = summaries.iter().collect();
    selected.sort_by_key(|s| (s.2, s.0));
    selected.truncate(2);
    selected.sort_by_key(|s| s.0);
    let mut labels: Vec<SharedString> = selected.iter().map(|s| s.3.clone()).collect();
    if !sources.is_empty() {
        let mut names: Vec<&str> = Vec::new();
        for source in &sources {
            if !names.contains(&source.name.as_str()) {
                names.push(&source.name);
            }
        }
        let names_text = join_clauses(&names.iter().map(|n| SharedString::from(n.to_string())).collect::<Vec<_>>());
        let all_integrations = sources.iter().all(|s| s.kind == ToolActivitySourceKind::Integration);
        labels.insert(
            0,
            if all_integrations {
                t!(
                    "{count, plural, =1 {Used {names} integration} other {Used {names} integrations}}",
                    count = names.len(),
                    names = names_text
                )
            } else {
                t!("Used {names}", names = names_text)
            },
        );
    }
    let sourced = tools
        .iter()
        .filter(|e| e.facts.tool_source.is_some() && t3_tool_definition(e.facts.tool_name(), None).is_none())
        .count();
    let shown: usize = selected.iter().map(|s| s.1).sum();
    let remaining = tools.len().saturating_sub(sourced + shown);
    if remaining > 0 {
        labels.push(t!(
            "{count, plural, one {Performed {count} other action} other {Performed {count} other actions}}",
            count = remaining
        ));
    }
    let sentence: Vec<SharedString> =
        labels.iter().enumerate().map(|(i, l)| if i == 0 { l.clone() } else { lowercase_first(l).into() }).collect();
    (join_clauses(&sentence), summaries.iter().any(|s| s.4 > 0))
}

/// `toolGroupSummaryKind`.
pub fn group_kind(entries: &[&WorkSource]) -> GroupKind {
    let tools: Vec<&WorkSource> = entries.iter().copied().filter(|e| !e.facts.is_reasoning()).collect();
    if !entries.is_empty() && tools.is_empty() {
        return GroupKind::Reasoning;
    }
    if !tools.is_empty() && tools.iter().all(|e| e.pull_request_icon) {
        return GroupKind::PullRequest;
    }
    let Some(first) = tools.first().map(|e| e.action) else { return GroupKind::Mixed };
    if tools.iter().any(|e| e.action != first) {
        return GroupKind::Mixed;
    }
    if first != GroupAction::Other {
        return GroupKind::Action(first);
    }
    let kind = |e: &WorkSource| match (&*e.facts.item, e.facts.tone) {
        (Item::DynamicTool(_), _) => GroupKind::DynamicTool,
        (Item::Subagent(_), _) | (_, Tone::Thinking) => GroupKind::AgentTool,
        (_, Tone::Tool) => GroupKind::ToneTool,
        _ => GroupKind::Other,
    };
    let first_kind = kind(tools[0]);
    if tools.iter().all(|e| kind(e) == first_kind) { first_kind } else { GroupKind::Mixed }
}

/// One call as its group summary reads it (`T3ToolSummaryCall` after `readResult`).
struct SummaryCall {
    input: Option<Map<String, Value>>,
    output: Option<Map<String, Value>>,
    completed: bool,
    failed: bool,
}

fn id(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

/// `countEntities`: distinct known ids plus each unknown one.
fn count_entities(ids: &[Option<&str>]) -> usize {
    ids.iter().flatten().collect::<HashSet<_>>().len() + ids.iter().filter(|i| i.is_none()).count()
}

/// `pullRequestCount`: `None` when a batch result lost its count.
fn pull_request_count(calls: &[&SummaryCall], key: &str) -> Option<usize> {
    let mut count = 0;
    for call in calls {
        if let Some(reported) = call.output.as_ref().and_then(|o| o.get(key)).and_then(Value::as_f64) {
            count += reported as usize;
        } else if call.input.as_ref().is_some_and(|i| i.contains_key("url") || i.contains_key("number")) {
            count += 1;
        } else {
            return None;
        }
    }
    Some(count)
}

/// `summarizeT3ToolCalls`: the clause for a group of calls to one T3 tool, and how many failed.
fn summarize_t3_calls(action: T3SummaryAction, entries: &[&WorkSource]) -> (SharedString, usize) {
    use T3SummaryAction as A;
    let calls: Vec<SummaryCall> = entries
        .iter()
        .map(|entry| {
            let facts = &entry.facts;
            let result = facts.tool_output().map(|o| read_t3_tool_result(&o)).unwrap_or_default();
            let outcome_failed = matches!(facts.lifecycle, Some(Lifecycle::Failed | Lifecycle::Declined)) || facts.tone == Tone::Error;
            let failed = result.failed || outcome_failed;
            SummaryCall {
                input: facts.tool_input().and_then(read_t3_tool_input),
                output: result.data,
                completed: !failed && facts.lifecycle == Some(Lifecycle::Completed),
                failed,
            }
        })
        .collect();
    let completed: Vec<&SummaryCall> = calls.iter().filter(|c| c.completed).collect();
    let mut failed_count = calls.iter().filter(|c| c.failed).count();
    let done = !completed.is_empty();
    let selected: Vec<&SummaryCall> = if done { completed } else { calls.iter().collect() };
    let field = |call: &&SummaryCall, key: &str| -> Option<String> {
        id(call.output.as_ref().and_then(|o| o.get(key))).or_else(|| id(call.input.as_ref().and_then(|i| i.get(key)))).map(str::to_owned)
    };
    let owned_ids = |key: &str| -> Vec<Option<String>> { selected.iter().map(|c| field(c, key)).collect() };
    fn as_refs(ids: &[Option<String>]) -> Vec<Option<&str>> {
        ids.iter().map(|i| i.as_deref()).collect()
    }
    let entity_ids_owned: HashMap<&str, Vec<Option<String>>> = [
        "taskId", "queuedRunId", "scheduledTaskId", "requestId", "attachmentId", "subagentId", "worktreePath", "cwd", "environmentId",
    ]
    .into_iter()
    .map(|key| (key, owned_ids(key)))
    .collect();
    let entity_ids = |key: &str| as_refs(&entity_ids_owned[key]);
    let project_ids_owned: Vec<Option<String>> = selected
        .iter()
        .map(|c| {
            id(c.output.as_ref().and_then(|o| o.get("id")))
                .or_else(|| id(c.output.as_ref().and_then(|o| o.get("projectId"))))
                .or_else(|| id(c.input.as_ref().and_then(|i| i.get("projectId"))))
                .map(str::to_owned)
        })
        .collect();
    let project_ids = as_refs(&project_ids_owned);
    let thread_ids_owned: Vec<Option<String>> = selected
        .iter()
        .map(|c| {
            id(c.output.as_ref().and_then(|o| o.get("threadId")))
                .or_else(|| id(c.output.as_ref().and_then(|o| o.get("thread")).and_then(|t| t.get("threadId"))))
                .or_else(|| id(c.input.as_ref().and_then(|i| i.get("threadId"))))
                .map(str::to_owned)
        })
        .collect();
    let thread_ids = as_refs(&thread_ids_owned);
    let all_threads_known = thread_ids.iter().all(Option::is_some);
    let distinct_threads = thread_ids.iter().collect::<HashSet<_>>().len();
    let output_of = |c: &&SummaryCall, key: &str| c.output.as_ref().and_then(|o| o.get(key)).cloned();
    let label = match action {
        A::Delegate => {
            let count = count_entities(&entity_ids("taskId"));
            if done { t!("{count, plural, one {Delegated {count} task} other {Delegated {count} tasks}}", count = count) } else { t!("{count, plural, one {Tried to delegate {count} task} other {Tried to delegate {count} tasks}}", count = count) }
        }
        A::ThreadInterrupt => {
            let count = count_entities(&thread_ids);
            if done { t!("{count, plural, one {Requested interrupts for {count} thread} other {Requested interrupts for {count} threads}}", count = count) } else { t!("{count, plural, one {Tried to interrupt {count} thread} other {Tried to interrupt {count} threads}}", count = count) }
        }
        A::WorkflowStop => {
            let count = count_entities(&entity_ids("subagentId"));
            if done { t!("{count, plural, one {Requested stops for {count} workflow} other {Requested stops for {count} workflows}}", count = count) } else { t!("{count, plural, one {Tried to stop {count} workflow} other {Tried to stop {count} workflows}}", count = count) }
        }
        A::TaskCancel => {
            let count = count_entities(&entity_ids("taskId"));
            if done { t!("{count, plural, one {Requested cancellation of {count} task} other {Requested cancellation of {count} tasks}}", count = count) } else { t!("{count, plural, one {Tried to cancel {count} task} other {Tried to cancel {count} tasks}}", count = count) }
        }
        A::ScheduleCreate => {
            let count = count_entities(&entity_ids("scheduledTaskId"));
            if done { t!("{count, plural, one {Scheduled {count} task} other {Scheduled {count} tasks}}", count = count) } else { t!("{count, plural, one {Tried to schedule {count} task} other {Tried to schedule {count} tasks}}", count = count) }
        }
        A::ScheduleUpdate => {
            let count = count_entities(&entity_ids("scheduledTaskId"));
            if done { t!("{count, plural, one {Updated {count} scheduled task} other {Updated {count} scheduled tasks}}", count = count) } else { t!("{count, plural, one {Tried to update {count} scheduled task} other {Tried to update {count} scheduled tasks}}", count = count) }
        }
        A::ScheduleDelete => {
            let count = count_entities(&entity_ids("scheduledTaskId"));
            if done { t!("{count, plural, one {Requested deletion of {count} scheduled task} other {Requested deletion of {count} scheduled tasks}}", count = count) } else { t!("{count, plural, one {Tried to delete {count} scheduled task} other {Tried to delete {count} scheduled tasks}}", count = count) }
        }
        A::ScheduleRun => {
            let count = selected.len();
            if done { t!("{count, plural, one {Requested {count} scheduled task run} other {Requested {count} scheduled task runs}}", count = count) } else { t!("{count, plural, one {Tried to request {count} scheduled task run} other {Tried to request {count} scheduled task runs}}", count = count) }
        }
        A::ThreadFork => {
            let count = selected.len();
            if done { t!("{count, plural, one {Requested {count} thread fork} other {Requested {count} thread forks}}", count = count) } else { t!("{count, plural, one {Tried to request {count} thread fork} other {Tried to request {count} thread forks}}", count = count) }
        }
        A::ThreadMerge => {
            let count = selected.len();
            if done { t!("{count, plural, one {Requested {count} context merge} other {Requested {count} context merges}}", count = count) } else { t!("{count, plural, one {Tried to request {count} context merge} other {Tried to request {count} context merges}}", count = count) }
        }
        A::ThreadUpdate => {
            let count = count_entities(&thread_ids);
            if done { t!("{count, plural, one {Updated {count} thread} other {Updated {count} threads}}", count = count) } else { t!("{count, plural, one {Tried to update {count} thread} other {Tried to update {count} threads}}", count = count) }
        }
        A::QueueRead => {
            let count = count_entities(&entity_ids("queuedRunId"));
            if done { t!("{count, plural, one {Read {count} queued message} other {Read {count} queued messages}}", count = count) } else { t!("{count, plural, one {Tried to read {count} queued message} other {Tried to read {count} queued messages}}", count = count) }
        }
        A::QueueEdit => {
            let count = count_entities(&entity_ids("queuedRunId"));
            if done { t!("{count, plural, one {Edited {count} queued message} other {Edited {count} queued messages}}", count = count) } else { t!("{count, plural, one {Tried to edit {count} queued message} other {Tried to edit {count} queued messages}}", count = count) }
        }
        A::QueueCancel => {
            let count = count_entities(&entity_ids("queuedRunId"));
            if done { t!("{count, plural, one {Requested cancellation of {count} queued run} other {Requested cancellation of {count} queued runs}}", count = count) } else { t!("{count, plural, one {Tried to cancel {count} queued run} other {Tried to cancel {count} queued runs}}", count = count) }
        }
        A::QueueReorder => {
            let count = count_entities(&entity_ids("queuedRunId"));
            if done { t!("{count, plural, one {Reordered {count} queued run} other {Reordered {count} queued runs}}", count = count) } else { t!("{count, plural, one {Tried to reorder {count} queued run} other {Tried to reorder {count} queued runs}}", count = count) }
        }
        A::QueueSteer => {
            let count = count_entities(&entity_ids("queuedRunId"));
            if done { t!("{count, plural, one {Requested steering with {count} queued message} other {Requested steering with {count} queued messages}}", count = count) } else { t!("{count, plural, one {Tried to steer with {count} queued message} other {Tried to steer with {count} queued messages}}", count = count) }
        }
        A::QuestionRead => {
            let count = count_entities(&entity_ids("requestId"));
            if done { t!("{count, plural, one {Read {count} pending question request} other {Read {count} pending question requests}}", count = count) } else { t!("{count, plural, one {Tried to read {count} pending question request} other {Tried to read {count} pending question requests}}", count = count) }
        }
        A::QuestionRespond => {
            let count = count_entities(&entity_ids("requestId"));
            if done { t!("{count, plural, one {Answered {count} pending question request} other {Answered {count} pending question requests}}", count = count) } else { t!("{count, plural, one {Tried to answer {count} pending question request} other {Tried to answer {count} pending question requests}}", count = count) }
        }
        A::SecretRequest => {
            let count = selected.len();
            if done { t!("{count, plural, one {Asked for {count} secret} other {Asked for {count} secrets}}", count = count) } else { t!("{count, plural, one {Tried to ask for {count} secret} other {Tried to ask for {count} secrets}}", count = count) }
        }
        A::WorktreeHandoff => {
            let count = count_entities(&entity_ids("worktreePath"));
            if done { t!("{count, plural, one {Handed off to {count} worktree} other {Handed off to {count} worktrees}}", count = count) } else { t!("{count, plural, one {Tried to hand off to {count} worktree} other {Tried to hand off to {count} worktrees}}", count = count) }
        }
        A::ProjectRead => {
            let count = count_entities(&project_ids);
            if done { t!("{count, plural, one {Read {count} project} other {Read {count} projects}}", count = count) } else { t!("{count, plural, one {Tried to read {count} project} other {Tried to read {count} projects}}", count = count) }
        }
        A::ProjectCreate => {
            let count = count_entities(&project_ids);
            if done { t!("{count, plural, one {Registered {count} project} other {Registered {count} projects}}", count = count) } else { t!("{count, plural, one {Tried to register {count} project} other {Tried to register {count} projects}}", count = count) }
        }
        A::ProjectUpdate => {
            let count = count_entities(&project_ids);
            if done { t!("{count, plural, one {Updated {count} project} other {Updated {count} projects}}", count = count) } else { t!("{count, plural, one {Tried to update {count} project} other {Tried to update {count} projects}}", count = count) }
        }
        A::ProjectDelete => {
            let count = count_entities(&project_ids);
            if done { t!("{count, plural, one {Deleted {count} project} other {Deleted {count} projects}}", count = count) } else { t!("{count, plural, one {Tried to delete {count} project} other {Tried to delete {count} projects}}", count = count) }
        }
        A::ProjectClone => {
            let count = count_entities(&entity_ids("cwd"));
            if done { t!("{count, plural, one {Cloned {count} repository} other {Cloned {count} repositories}}", count = count) } else { t!("{count, plural, one {Tried to clone {count} repository} other {Tried to clone {count} repositories}}", count = count) }
        }
        A::EnvironmentLink => {
            let count = selected.len();
            if done { t!("{count, plural, one {Asked to link {count} environment} other {Asked to link {count} environments}}", count = count) } else { t!("{count, plural, one {Tried to ask to link {count} environment} other {Tried to ask to link {count} environments}}", count = count) }
        }
        A::EnvironmentUnlink => {
            let count = count_entities(&entity_ids("environmentId"));
            if done { t!("{count, plural, one {Forgot {count} linked environment} other {Forgot {count} linked environments}}", count = count) } else { t!("{count, plural, one {Tried to forget {count} linked environment} other {Tried to forget {count} linked environments}}", count = count) }
        }
        A::AttachmentPrepare => {
            let count = count_entities(&entity_ids("attachmentId"));
            if done { t!("{count, plural, one {Prepared {count} attachment upload} other {Prepared {count} attachment uploads}}", count = count) } else { t!("{count, plural, one {Tried to prepare {count} attachment upload} other {Tried to prepare {count} attachment uploads}}", count = count) }
        }
        A::AttachmentDiscard => {
            let count = count_entities(&entity_ids("attachmentId"));
            if done { t!("{count, plural, one {Discarded {count} pending attachment} other {Discarded {count} pending attachments}}", count = count) } else { t!("{count, plural, one {Tried to discard {count} pending attachment} other {Tried to discard {count} pending attachments}}", count = count) }
        }
        A::WatchPr => {
            let count = selected.len();
            if done { t!("{count, plural, one {Watching {count} pull request} other {Watching {count} pull requests}}", count = count) } else { t!("{count, plural, one {Tried to watch {count} pull request} other {Tried to watch {count} pull requests}}", count = count) }
        }
        A::UnwatchPr => {
            let count = selected.len();
            if done { t!("{count, plural, one {Stopped watching {count} pull request} other {Stopped watching {count} pull requests}}", count = count) } else { t!("{count, plural, one {Tried to stop watching {count} pull request} other {Tried to stop watching {count} pull requests}}", count = count) }
        }
        A::ReadPrGuide => {
            let count = selected.len();
            if done { t!("{count, plural, one {Read {count} review guide} other {Read {count} review guides}}", count = count) } else { t!("{count, plural, one {Tried to read {count} review guide} other {Tried to read {count} review guides}}", count = count) }
        }
        A::ListIssues => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed issues {count} time} other {Listed issues {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list issues {count} time} other {Tried to list issues {count} times}}", count = count) }
        }
        A::ReadIssue => {
            let count = selected.len();
            if done { t!("{count, plural, one {Read {count} issue} other {Read {count} issues}}", count = count) } else { t!("{count, plural, one {Tried to read {count} issue} other {Tried to read {count} issues}}", count = count) }
        }
        A::CreateIssue => {
            let count = selected.len();
            if done { t!("{count, plural, one {Filed {count} issue} other {Filed {count} issues}}", count = count) } else { t!("{count, plural, one {Tried to file {count} issue} other {Tried to file {count} issues}}", count = count) }
        }
        A::CommentIssue => {
            let count = selected.len();
            if done { t!("{count, plural, one {Commented on {count} issue} other {Commented on {count} issues}}", count = count) } else { t!("{count, plural, one {Tried to comment on {count} issue} other {Tried to comment on {count} issues}}", count = count) }
        }
        A::SetIssueState => {
            let count = selected.len();
            if done { t!("{count, plural, one {Changed the state of {count} issue} other {Changed the state of {count} issues}}", count = count) } else { t!("{count, plural, one {Tried to change the state of {count} issue} other {Tried to change the state of {count} issues}}", count = count) }
        }
        A::LinkIssue => {
            let count = selected.len();
            if done { t!("{count, plural, one {Linked {count} issue} other {Linked {count} issues}}", count = count) } else { t!("{count, plural, one {Tried to link {count} issue} other {Tried to link {count} issues}}", count = count) }
        }
        A::UnlinkIssue => {
            let count = selected.len();
            if done { t!("{count, plural, one {Unlinked {count} issue} other {Unlinked {count} issues}}", count = count) } else { t!("{count, plural, one {Tried to unlink {count} issue} other {Tried to unlink {count} issues}}", count = count) }
        }
        A::HtmlPreview => {
            let count = selected.len();
            if done { t!("{count, plural, one {Previewed {count} HTML page} other {Previewed {count} HTML pages}}", count = count) } else { t!("{count, plural, one {Tried to preview {count} HTML page} other {Tried to preview {count} HTML pages}}", count = count) }
        }
        A::HtmlRender => {
            let count = selected.len();
            if done { t!("{count, plural, one {Rendered {count} HTML page} other {Rendered {count} HTML pages}}", count = count) } else { t!("{count, plural, one {Tried to render {count} HTML page} other {Tried to render {count} HTML pages}}", count = count) }
        }
        A::SendToUser => {
            let count = selected.len();
            if done { t!("{count, plural, one {Sent {count} message} other {Sent {count} messages}}", count = count) } else { t!("{count, plural, one {Tried to send {count} message} other {Tried to send {count} messages}}", count = count) }
        }
        A::ThreadList => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed threads {count} time} other {Listed threads {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list threads {count} time} other {Tried to list threads {count} times}}", count = count) }
        }
        A::TaskStatus => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked task status {count} time} other {Checked task status {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check task status {count} time} other {Tried to check task status {count} times}}", count = count) }
        }
        A::BackgroundWait => {
            let count = selected.len();
            if done { t!("{count, plural, one {Updated background waits {count} time} other {Updated background waits {count} times}}", count = count) } else { t!("{count, plural, one {Tried to update background waits {count} time} other {Tried to update background waits {count} times}}", count = count) }
        }
        A::ScheduleList => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed scheduled tasks {count} time} other {Listed scheduled tasks {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list scheduled tasks {count} time} other {Tried to list scheduled tasks {count} times}}", count = count) }
        }
        A::ThreadConfiguration => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked thread configuration {count} time} other {Checked thread configuration {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check thread configuration {count} time} other {Tried to check thread configuration {count} times}}", count = count) }
        }
        A::ThreadConfigure => {
            let count = selected.len();
            if done { t!("{count, plural, one {Set thread model {count} time} other {Set thread model {count} times}}", count = count) } else { t!("{count, plural, one {Tried to set thread model {count} time} other {Tried to set thread model {count} times}}", count = count) }
        }
        A::ThreadSearch => {
            let count = selected.len();
            if done { t!("{count, plural, one {Searched threads {count} time} other {Searched threads {count} times}}", count = count) } else { t!("{count, plural, one {Tried to search threads {count} time} other {Tried to search threads {count} times}}", count = count) }
        }
        A::ThreadTransfers => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked thread transfers {count} time} other {Checked thread transfers {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check thread transfers {count} time} other {Tried to check thread transfers {count} times}}", count = count) }
        }
        A::ThreadOrganize => {
            let count = selected.len();
            if done { t!("{count, plural, one {Organized threads {count} time} other {Organized threads {count} times}}", count = count) } else { t!("{count, plural, one {Tried to organize threads {count} time} other {Tried to organize threads {count} times}}", count = count) }
        }
        A::QueueList => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed queued messages {count} time} other {Listed queued messages {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list queued messages {count} time} other {Tried to list queued messages {count} times}}", count = count) }
        }
        A::QuestionList => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed pending questions {count} time} other {Listed pending questions {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list pending questions {count} time} other {Tried to list pending questions {count} times}}", count = count) }
        }
        A::WorktreeList => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed workspace branches {count} time} other {Listed workspace branches {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list workspace branches {count} time} other {Tried to list workspace branches {count} times}}", count = count) }
        }
        A::WorktreeStatus => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked worktree status {count} time} other {Checked worktree status {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check worktree status {count} time} other {Tried to check worktree status {count} times}}", count = count) }
        }
        A::ProjectList => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed projects {count} time} other {Listed projects {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list projects {count} time} other {Tried to list projects {count} times}}", count = count) }
        }
        A::EnvironmentRead => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked environment preferences {count} time} other {Checked environment preferences {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check environment preferences {count} time} other {Tried to check environment preferences {count} times}}", count = count) }
        }
        A::EnvironmentUpdate => {
            let count = selected.len();
            if done { t!("{count, plural, one {Updated environment preferences {count} time} other {Updated environment preferences {count} times}}", count = count) } else { t!("{count, plural, one {Tried to update environment preferences {count} time} other {Tried to update environment preferences {count} times}}", count = count) }
        }
        A::ThemeRead => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked themes {count} time} other {Checked themes {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check themes {count} time} other {Tried to check themes {count} times}}", count = count) }
        }
        A::ThemeSet => {
            let count = selected.len();
            if done { t!("{count, plural, one {Set the theme {count} time} other {Set the theme {count} times}}", count = count) } else { t!("{count, plural, one {Tried to set the theme {count} time} other {Tried to set the theme {count} times}}", count = count) }
        }
        A::EnvironmentLinks => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed linked environments {count} time} other {Listed linked environments {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list linked environments {count} time} other {Tried to list linked environments {count} times}}", count = count) }
        }
        A::Browser => {
            let count = selected.len();
            if done { t!("{count, plural, one {Used browser {count} time} other {Used browser {count} times}}", count = count) } else { t!("{count, plural, one {Tried to use browser {count} time} other {Tried to use browser {count} times}}", count = count) }
        }
        A::Device => {
            let count = selected.len();
            if done { t!("{count, plural, one {Used device controls {count} time} other {Used device controls {count} times}}", count = count) } else { t!("{count, plural, one {Tried to use device controls {count} time} other {Tried to use device controls {count} times}}", count = count) }
        }
        A::Capabilities => {
            let count = selected.len();
            if done { t!("{count, plural, one {Checked orchestration capabilities {count} time} other {Checked orchestration capabilities {count} times}}", count = count) } else { t!("{count, plural, one {Tried to check orchestration capabilities {count} time} other {Tried to check orchestration capabilities {count} times}}", count = count) }
        }
        A::ReadPr => {
            let count = selected.len();
            if done { t!("{count, plural, one {Read {count} pull request} other {Read {count} pull requests}}", count = count) } else { t!("{count, plural, one {Tried to read {count} pull request} other {Tried to read {count} pull requests}}", count = count) }
        }
        A::ListRepoPrs => {
            let count = selected.len();
            if done { t!("{count, plural, one {Listed pull requests {count} time} other {Listed pull requests {count} times}}", count = count) } else { t!("{count, plural, one {Tried to list pull requests {count} time} other {Tried to list pull requests {count} times}}", count = count) }
        }
        A::UpdatePr => {
            let count = selected.len();
            if done { t!("{count, plural, one {Updated {count} pull request} other {Updated {count} pull requests}}", count = count) } else { t!("{count, plural, one {Tried to update {count} pull request} other {Tried to update {count} pull requests}}", count = count) }
        }
        A::RespondPr => {
            let count = selected.len();
            if done { t!("{count, plural, one {Responded on pull requests {count} time} other {Responded on pull requests {count} times}}", count = count) } else { t!("{count, plural, one {Tried to respond on pull requests {count} time} other {Tried to respond on pull requests {count} times}}", count = count) }
        }
        A::CiLogs => {
            let count = selected.len();
            if done { t!("{count, plural, one {Read CI logs {count} time} other {Read CI logs {count} times}}", count = count) } else { t!("{count, plural, one {Tried to read CI logs {count} time} other {Tried to read CI logs {count} times}}", count = count) }
        }
        A::ReviewPr => {
            let count = selected.len();
            if done { t!("{count, plural, one {Reviewed {count} pull request} other {Reviewed {count} pull requests}}", count = count) } else { t!("{count, plural, one {Tried to review {count} pull request} other {Tried to review {count} pull requests}}", count = count) }
        }
        A::ListThreadIssues => {
            let count = selected.len();
            if count == 1 {
                if done { t!("Checked linked issues") } else { t!("Tried to check linked issues") }
            } else if done {
                t!("{count, plural, one {Checked linked issues {count} time} other {Checked linked issues {count} times}}", count = count)
            } else {
                t!(
                    "{count, plural, one {Tried to check linked issues {count} time} other {Tried to check linked issues {count} times}}",
                    count = count
                )
            }
        }
        A::ThreadRead => {
            if all_threads_known {
                let count = distinct_threads;
                if done {
                    t!("{count, plural, one {Read {count} thread} other {Read {count} threads}}", count = count)
                } else {
                    t!("{count, plural, one {Tried to read {count} thread} other {Tried to read {count} threads}}", count = count)
                }
            } else {
                let count = selected.len();
                if done {
                    t!("{count, plural, one {Read threads {count} time} other {Read threads {count} times}}", count = count)
                } else {
                    t!("{count, plural, one {Tried to read threads {count} time} other {Tried to read threads {count} times}}", count = count)
                }
            }
        }
        A::ThreadWait => {
            if all_threads_known {
                let count = distinct_threads;
                if done {
                    t!("{count, plural, one {Waited on {count} thread} other {Waited on {count} threads}}", count = count)
                } else {
                    t!("{count, plural, one {Tried to wait on {count} thread} other {Tried to wait on {count} threads}}", count = count)
                }
            } else {
                let count = selected.len();
                if done {
                    t!("{count, plural, one {Waited on threads {count} time} other {Waited on threads {count} times}}", count = count)
                } else {
                    t!(
                        "{count, plural, one {Tried to wait on threads {count} time} other {Tried to wait on threads {count} times}}",
                        count = count
                    )
                }
            }
        }
        A::ThreadSend => {
            let message_ids: Vec<Option<String>> =
                selected.iter().map(|c| id(c.output.as_ref().and_then(|o| o.get("messageId"))).map(str::to_owned)).collect();
            let messages = count_entities(&as_refs(&message_ids));
            let threads = distinct_threads;
            if !all_threads_known {
                let count = messages;
                if done {
                    t!("{count, plural, one {Sent {count} message} other {Sent {count} messages}}", count = count)
                } else {
                    t!("{count, plural, one {Tried to send {count} message} other {Tried to send {count} messages}}", count = count)
                }
            } else if messages == threads && messages > 1 {
                if done {
                    t!("{threads, plural, one {Sent messages to {threads} thread} other {Sent messages to {threads} threads}}", threads = threads)
                } else {
                    t!(
                        "{threads, plural, one {Tried to send messages to {threads} thread} other {Tried to send messages to {threads} threads}}",
                        threads = threads
                    )
                }
            } else if done {
                t!(
                    "{messages, plural, one {Sent {messages} message to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Sent {messages} messages to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                    messages = messages,
                    threads = threads
                )
            } else {
                t!(
                    "{messages, plural, one {Tried to send {messages} message to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Tried to send {messages} messages to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                    messages = messages,
                    threads = threads
                )
            }
        }
        A::ThreadCreate => {
            let mut created: Vec<String> = Vec::new();
            let known = done
                && selected.iter().all(|call| {
                    let threads: Vec<Value> = match output_of(call, "threads") {
                        Some(Value::Array(threads)) => threads,
                        _ => vec![call.output.clone().map(Value::Object).unwrap_or(Value::Null)],
                    };
                    threads.iter().all(|thread| {
                        if thread.get("status").and_then(Value::as_str) == Some("rolled_back") {
                            return true;
                        }
                        match id(thread.get("threadId")) {
                            Some(thread_id) => {
                                created.push(thread_id.to_owned());
                                true
                            }
                            None => false,
                        }
                    })
                });
            if known {
                let count = created.iter().collect::<HashSet<_>>().len();
                t!("{count, plural, one {Created {count} thread} other {Created {count} threads}}", count = count)
            } else {
                let count = selected.len();
                t!(
                    "{count, plural, one {Requested thread creation {count} time} other {Requested thread creation {count} times}}",
                    count = count
                )
            }
        }
        A::AttachmentSend => {
            let mut by_message: Vec<(String, &&SummaryCall)> = Vec::new();
            for (index, call) in selected.iter().enumerate() {
                let key = id(call.output.as_ref().and_then(|o| o.get("messageId"))).map_or_else(|| format!("call-{index}"), str::to_owned);
                match by_message.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = call,
                    None => by_message.push((key, call)),
                }
            }
            let attachment_list = |call: &SummaryCall| call.input.as_ref().and_then(|i| i.get("attachments")).and_then(Value::as_array).map(Vec::len);
            let attachments: usize = by_message.iter().map(|(_, c)| attachment_list(c).unwrap_or(0)).sum();
            let counts_known = by_message.iter().all(|(_, c)| attachment_list(c).is_some_and(|n| n > 0));
            let threads = distinct_threads;
            let count = selected.len();
            match (all_threads_known, counts_known, done) {
                (true, true, true) => t!(
                    "{attachments, plural, one {Sent {attachments} attachment to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Sent {attachments} attachments to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                    attachments = attachments,
                    threads = threads
                ),
                (true, true, false) => t!(
                    "{attachments, plural, one {Tried to send {attachments} attachment to {threads, plural, one {{threads} thread} other {{threads} threads}}} other {Tried to send {attachments} attachments to {threads, plural, one {{threads} thread} other {{threads} threads}}}}",
                    attachments = attachments,
                    threads = threads
                ),
                (true, false, true) => t!(
                    "{count, plural, one {Sent attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} time} other {Sent attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} times}}",
                    threads = threads,
                    count = count
                ),
                (true, false, false) => t!(
                    "{count, plural, one {Tried to send attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} time} other {Tried to send attachments to {threads, plural, one {{threads} thread} other {{threads} threads}} {count} times}}",
                    threads = threads,
                    count = count
                ),
                (false, true, true) => t!(
                    "{attachments, plural, one {Sent {attachments} attachment} other {Sent {attachments} attachments}}",
                    attachments = attachments
                ),
                (false, true, false) => t!(
                    "{attachments, plural, one {Tried to send {attachments} attachment} other {Tried to send {attachments} attachments}}",
                    attachments = attachments
                ),
                (false, false, true) => {
                    t!("{count, plural, one {Sent attachments {count} time} other {Sent attachments {count} times}}", count = count)
                }
                (false, false, false) => t!(
                    "{count, plural, one {Tried to send attachments {count} time} other {Tried to send attachments {count} times}}",
                    count = count
                ),
            }
        }
        A::LinkPr => match pull_request_count(&selected, "linkedCount") {
            None => {
                if done { t!("Linked pull requests") } else { t!("Tried to link pull requests") }
            }
            Some(count) if done => {
                t!("{count, plural, one {Linked {count} pull request} other {Linked {count} pull requests}}", count = count)
            }
            Some(count) => {
                t!("{count, plural, one {Tried to link {count} pull request} other {Tried to link {count} pull requests}}", count = count)
            }
        },
        A::UnlinkPr => match pull_request_count(&selected, "unlinkedCount") {
            None => {
                if done { t!("Unlinked pull requests") } else { t!("Tried to unlink pull requests") }
            }
            Some(count) if done => {
                t!("{count, plural, one {Unlinked {count} pull request} other {Unlinked {count} pull requests}}", count = count)
            }
            Some(count) => t!(
                "{count, plural, one {Tried to unlink {count} pull request} other {Tried to unlink {count} pull requests}}",
                count = count
            ),
        },
        A::ListPrs => {
            let count = selected.len();
            if count == 1 {
                if done { t!("Checked linked pull requests") } else { t!("Tried to check linked pull requests") }
            } else if done {
                t!(
                    "{count, plural, one {Checked linked pull requests {count} time} other {Checked linked pull requests {count} times}}",
                    count = count
                )
            } else {
                t!(
                    "{count, plural, one {Tried to check linked pull requests {count} time} other {Tried to check linked pull requests {count} times}}",
                    count = count
                )
            }
        }
        A::WritePrGuides => {
            // One call writes many guides and reports a refusal per guide rather than failing.
            let outcomes: Vec<Option<String>> = selected
                .iter()
                .flat_map(|call| match output_of(call, "guides") {
                    Some(Value::Array(guides)) => {
                        guides.iter().map(|g| g.get("outcome").and_then(Value::as_str).map(str::to_owned)).collect::<Vec<_>>()
                    }
                    _ => Vec::new(),
                })
                .collect();
            let started = outcomes.iter().filter(|o| o.as_deref() != Some("failed")).count();
            let failed = outcomes.len() - started;
            failed_count += failed;
            if outcomes.is_empty() {
                let count = selected.len();
                match (count == 1, done) {
                    (true, true) => t!("Started writing review guides"),
                    (true, false) => t!("Tried to start writing review guides"),
                    (false, true) => t!(
                        "{count, plural, one {Started writing review guides {count} time} other {Started writing review guides {count} times}}",
                        count = count
                    ),
                    (false, false) => t!(
                        "{count, plural, one {Tried to start writing review guides {count} time} other {Tried to start writing review guides {count} times}}",
                        count = count
                    ),
                }
            } else if started == 0 {
                t!("{failed, plural, one {Could not start {failed} review guide} other {Could not start {failed} review guides}}", failed = failed)
            } else if failed > 0 {
                if done {
                    t!(
                        "{started, plural, one {Started {started} review guide, {failed} failed} other {Started {started} review guides, {failed} failed}}",
                        started = started,
                        failed = failed
                    )
                } else {
                    t!(
                        "{started, plural, one {Tried to start {started} review guide, {failed} failed} other {Tried to start {started} review guides, {failed} failed}}",
                        started = started,
                        failed = failed
                    )
                }
            } else if done {
                t!("{started, plural, one {Started {started} review guide} other {Started {started} review guides}}", started = started)
            } else {
                t!(
                    "{started, plural, one {Tried to start {started} review guide} other {Tried to start {started} review guides}}",
                    started = started
                )
            }
        }
        A::CreatePr => {
            // An open PR for the branch is returned rather than duplicated.
            let existing = selected.iter().filter(|c| output_of(c, "created") == Some(Value::Bool(false))).count();
            let opened = selected.len() - existing;
            if !done {
                t!(
                    "{count, plural, one {Tried to open {count} pull request} other {Tried to open {count} pull requests}}",
                    count = selected.len()
                )
            } else if opened == 0 {
                t!("{count, plural, one {Found {count} open pull request} other {Found {count} open pull requests}}", count = existing)
            } else if existing == 0 {
                t!("{count, plural, one {Opened {count} pull request} other {Opened {count} pull requests}}", count = opened)
            } else {
                t!(
                    "{opened, plural, one {Opened {opened} pull request, found {existing} open} other {Opened {opened} pull requests, found {existing} open}}",
                    opened = opened,
                    existing = existing
                )
            }
        }
        A::MergePr => {
            // Auto-merge and merge queues leave the PR open until it lands.
            let queued = selected
                .iter()
                .filter(|c| match output_of(c, "state") {
                    None => c.input.as_ref().and_then(|i| i.get("auto")) == Some(&Value::Bool(true)),
                    Some(state) => state != Value::String("merged".into()),
                })
                .count();
            let merged = selected.len() - queued;
            if !done {
                t!(
                    "{count, plural, one {Tried to merge {count} pull request} other {Tried to merge {count} pull requests}}",
                    count = selected.len()
                )
            } else if merged == 0 {
                t!(
                    "{count, plural, one {Queued {count} pull request to merge} other {Queued {count} pull requests to merge}}",
                    count = queued
                )
            } else if queued == 0 {
                t!("{count, plural, one {Merged {count} pull request} other {Merged {count} pull requests}}", count = merged)
            } else {
                t!(
                    "{merged, plural, one {Merged {merged} pull request, queued {queued}} other {Merged {merged} pull requests, queued {queued}}}",
                    merged = merged,
                    queued = queued
                )
            }
        }
        A::CiRerun => {
            // A host can refuse a check it cannot re-run, such as one outside its CI.
            let mut any_checks = false;
            let outcomes: Vec<bool> = selected
                .iter()
                .flat_map(|call| match output_of(call, "checks") {
                    Some(Value::Array(checks)) => {
                        any_checks = true;
                        checks.iter().map(|c| c.get("rerun") == Some(&Value::Bool(true))).collect::<Vec<_>>()
                    }
                    _ => Vec::new(),
                })
                .collect();
            let rerun = outcomes.iter().filter(|r| **r).count();
            let refused = outcomes.len() - rerun;
            failed_count += refused;
            if !done {
                t!("{count, plural, one {Tried to re-run CI {count} time} other {Tried to re-run CI {count} times}}", count = selected.len())
            } else if outcomes.is_empty() {
                // An empty list means no check had failed, so nothing was asked of the host.
                if any_checks {
                    t!("Found no failed checks to re-run")
                } else {
                    t!("{count, plural, one {Re-ran CI {count} time} other {Re-ran CI {count} times}}", count = selected.len())
                }
            } else if rerun == 0 {
                t!("{count, plural, one {Could not re-run {count} check} other {Could not re-run {count} checks}}", count = refused)
            } else if refused == 0 {
                t!("{count, plural, one {Re-ran {count} check} other {Re-ran {count} checks}}", count = rerun)
            } else {
                t!(
                    "{rerun, plural, one {Re-ran {rerun} check, {refused} refused} other {Re-ran {rerun} checks, {refused} refused}}",
                    rerun = rerun,
                    refused = refused
                )
            }
        }
    };
    (label, failed_count)
}

/// The sentence a T3 tool row reads by status (`t3ToolSentences.ts`).
fn t3_sentence(name: &str, status: &str) -> Option<SharedString> {
    Some(match name {
        "comment_on_issue" => t!("{status, select, inProgress {Commenting on an issue} completed {Commented on an issue} failed {Failed to comment on an issue} declined {Declined to comment on an issue} stopped {Stopped commenting on an issue} other {Commenting on an issue}}", status = status),
        "create_issue" => t!("{status, select, inProgress {Filing an issue} completed {Filed an issue} failed {Failed to file an issue} declined {Declined to file an issue} stopped {Stopped filing an issue} other {Filing an issue}}", status = status),
        "create_threads" => t!("{status, select, inProgress {Creating T3 threads} completed {Created T3 threads} failed {Failed to create T3 threads} declined {Declined to create T3 threads} stopped {Stopped creating T3 threads} other {Creating T3 threads}}", status = status),
        "delegate_task" => t!("{status, select, inProgress {Delegating a child task} completed {Delegated a child task} failed {Failed to delegate a child task} declined {Declined to delegate a child task} stopped {Stopped delegating a child task} other {Delegating a child task}}", status = status),
        "delete_scheduled_task" => t!("{status, select, inProgress {Deleting a scheduled task} completed {Requested deletion of a scheduled task} failed {Failed to delete a scheduled task} declined {Declined to delete a scheduled task} stopped {Stopped deleting a scheduled task} other {Deleting a scheduled task}}", status = status),
        "device_close" => t!("{status, select, inProgress {Closing a device} completed {Closed a device} failed {Failed to close a device} declined {Declined to close a device} stopped {Stopped closing a device} other {Closing a device}}", status = status),
        "device_list" => t!("{status, select, inProgress {Listing simulators and emulators} completed {Listed simulators and emulators} failed {Failed to list simulators and emulators} declined {Declined to list simulators and emulators} stopped {Stopped listing simulators and emulators} other {Listing simulators and emulators}}", status = status),
        "device_open" => t!("{status, select, inProgress {Opening a device in the Device panel} completed {Opened a device in the Device panel} failed {Failed to open a device in the Device panel} declined {Declined to open a device in the Device panel} stopped {Stopped opening a device in the Device panel} other {Opening a device in the Device panel}}", status = status),
        "device_screenshot" => t!("{status, select, inProgress {Taking a screenshot of the device} completed {Took a screenshot of the device} failed {Failed to take a screenshot of the device} declined {Declined to take a screenshot of the device} stopped {Stopped taking a screenshot of the device} other {Taking a screenshot of the device}}", status = status),
        "html_preview" => t!("{status, select, inProgress {Previewing an HTML page} completed {Previewed an HTML page} failed {Failed to preview an HTML page} declined {Declined to preview an HTML page} stopped {Stopped previewing an HTML page} other {Previewing an HTML page}}", status = status),
        "html_render" => t!("{status, select, inProgress {Rendering an HTML page} completed {Rendered an HTML page} failed {Failed to render an HTML page} declined {Declined to render an HTML page} stopped {Stopped rendering an HTML page} other {Rendering an HTML page}}", status = status),
        "link_issue" => t!("{status, select, inProgress {Linking an issue} completed {Linked an issue} failed {Failed to link an issue} declined {Declined to link an issue} stopped {Stopped linking an issue} other {Linking an issue}}", status = status),
        "link_pull_request" => t!("{status, select, inProgress {Linking a pull request} completed {Linked a pull request} failed {Failed to link a pull request} declined {Declined to link a pull request} stopped {Stopped linking a pull request} other {Linking a pull request}}", status = status),
        "link_pull_requests" => t!("{status, select, inProgress {Linking pull requests} completed {Linked pull requests} failed {Failed to link pull requests} declined {Declined to link pull requests} stopped {Stopped linking pull requests} other {Linking pull requests}}", status = status),
        "list_issues" => t!("{status, select, inProgress {Listing issues} completed {Listed issues} failed {Failed to list issues} declined {Declined to list issues} stopped {Stopped listing issues} other {Listing issues}}", status = status),
        "list_scheduled_tasks" => t!("{status, select, inProgress {Listing scheduled tasks} completed {Listed scheduled tasks} failed {Failed to list scheduled tasks} declined {Declined to list scheduled tasks} stopped {Stopped listing scheduled tasks} other {Listing scheduled tasks}}", status = status),
        "list_thread_issues" => t!("{status, select, inProgress {Checking linked issues} completed {Checked linked issues} failed {Failed to check linked issues} declined {Declined to check linked issues} stopped {Stopped checking linked issues} other {Checking linked issues}}", status = status),
        "list_thread_pull_requests" => t!("{status, select, inProgress {Checking linked pull requests} completed {Checked linked pull requests} failed {Failed to check linked pull requests} declined {Declined to check linked pull requests} stopped {Stopped checking linked pull requests} other {Checking linked pull requests}}", status = status),
        "orchestrator_capabilities" => t!("{status, select, inProgress {Getting orchestration capabilities} completed {Got orchestration capabilities} failed {Failed to get orchestration capabilities} declined {Declined to get orchestration capabilities} stopped {Stopped getting orchestration capabilities} other {Getting orchestration capabilities}}", status = status),
        "preview_act" => t!("{status, select, inProgress {Using the preview browser} completed {Used the preview browser} failed {Failed to use the preview browser} declined {Declined to use the preview browser} stopped {Stopped using the preview browser} other {Using the preview browser}}", status = status),
        "preview_click" => t!("{status, select, inProgress {Clicking in the preview browser} completed {Clicked in the preview browser} failed {Failed to click in the preview browser} declined {Declined to click in the preview browser} stopped {Stopped clicking in the preview browser} other {Clicking in the preview browser}}", status = status),
        "preview_dialog" => t!("{status, select, inProgress {Responding to a preview browser dialog} completed {Responded to a preview browser dialog} failed {Failed to respond to a preview browser dialog} declined {Declined to respond to a preview browser dialog} stopped {Stopped responding to a preview browser dialog} other {Responding to a preview browser dialog}}", status = status),
        "preview_drag" => t!("{status, select, inProgress {Dragging in the preview browser} completed {Dragged in the preview browser} failed {Failed to drag in the preview browser} declined {Declined to drag in the preview browser} stopped {Stopped dragging in the preview browser} other {Dragging in the preview browser}}", status = status),
        "preview_evaluate" => t!("{status, select, inProgress {Evaluating script in the preview browser} completed {Evaluated script in the preview browser} failed {Failed to evaluate script in the preview browser} declined {Declined to evaluate script in the preview browser} stopped {Stopped evaluating script in the preview browser} other {Evaluating script in the preview browser}}", status = status),
        "preview_hover" => t!("{status, select, inProgress {Hovering in the preview browser} completed {Hovered in the preview browser} failed {Failed to hover in the preview browser} declined {Declined to hover in the preview browser} stopped {Stopped hovering in the preview browser} other {Hovering in the preview browser}}", status = status),
        "preview_navigate" => t!("{status, select, inProgress {Navigating the preview browser} completed {Navigated the preview browser} failed {Failed to navigate the preview browser} declined {Declined to navigate the preview browser} stopped {Stopped navigating the preview browser} other {Navigating the preview browser}}", status = status),
        "preview_open" => t!("{status, select, inProgress {Opening a page in the preview browser} completed {Opened a page in the preview browser} failed {Failed to open a page in the preview browser} declined {Declined to open a page in the preview browser} stopped {Stopped opening a page in the preview browser} other {Opening a page in the preview browser}}", status = status),
        "preview_press" => t!("{status, select, inProgress {Pressing a key in the preview browser} completed {Pressed a key in the preview browser} failed {Failed to press a key in the preview browser} declined {Declined to press a key in the preview browser} stopped {Stopped pressing a key in the preview browser} other {Pressing a key in the preview browser}}", status = status),
        "preview_recording_start" => t!("{status, select, inProgress {Starting recording the preview browser} completed {Started recording the preview browser} failed {Failed to start recording the preview browser} declined {Declined to start recording the preview browser} stopped {Stopped starting recording the preview browser} other {Starting recording the preview browser}}", status = status),
        "preview_recording_stop" => t!("{status, select, inProgress {Stopping recording the preview browser} completed {Stopped recording the preview browser} failed {Failed to stop recording the preview browser} declined {Declined to stop recording the preview browser} stopped {Stopped stopping recording the preview browser} other {Stopping recording the preview browser}}", status = status),
        "preview_resize" => t!("{status, select, inProgress {Resizing the preview browser} completed {Resized the preview browser} failed {Failed to resize the preview browser} declined {Declined to resize the preview browser} stopped {Stopped resizing the preview browser} other {Resizing the preview browser}}", status = status),
        "preview_scroll" => t!("{status, select, inProgress {Scrolling the preview browser} completed {Scrolled the preview browser} failed {Failed to scroll the preview browser} declined {Declined to scroll the preview browser} stopped {Stopped scrolling the preview browser} other {Scrolling the preview browser}}", status = status),
        "preview_select" => t!("{status, select, inProgress {Choosing an option in the preview browser} completed {Chose an option in the preview browser} failed {Failed to choose an option in the preview browser} declined {Declined to choose an option in the preview browser} stopped {Stopped choosing an option in the preview browser} other {Choosing an option in the preview browser}}", status = status),
        "preview_set_appearance" => t!("{status, select, inProgress {Setting preview browser appearance} completed {Set preview browser appearance} failed {Failed to set preview browser appearance} declined {Declined to set preview browser appearance} stopped {Stopped setting preview browser appearance} other {Setting preview browser appearance}}", status = status),
        "preview_snapshot" => t!("{status, select, inProgress {Taking a snapshot of the preview page} completed {Took a snapshot of the preview page} failed {Failed to take a snapshot of the preview page} declined {Declined to take a snapshot of the preview page} stopped {Stopped taking a snapshot of the preview page} other {Taking a snapshot of the preview page}}", status = status),
        "preview_status" => t!("{status, select, inProgress {Getting preview browser status} completed {Got preview browser status} failed {Failed to get preview browser status} declined {Declined to get preview browser status} stopped {Stopped getting preview browser status} other {Getting preview browser status}}", status = status),
        "preview_type" => t!("{status, select, inProgress {Typing in the preview browser} completed {Typed in the preview browser} failed {Failed to type in the preview browser} declined {Declined to type in the preview browser} stopped {Stopped typing in the preview browser} other {Typing in the preview browser}}", status = status),
        "preview_upload" => t!("{status, select, inProgress {Uploading files to the preview browser} completed {Uploaded files to the preview browser} failed {Failed to upload files to the preview browser} declined {Declined to upload files to the preview browser} stopped {Stopped uploading files to the preview browser} other {Uploading files to the preview browser}}", status = status),
        "preview_wait_for" => t!("{status, select, inProgress {Waiting for the preview page} completed {Waited for the preview page} failed {Failed to wait for the preview page} declined {Declined to wait for the preview page} stopped {Stopped waiting for the preview page} other {Waiting for the preview page}}", status = status),
        "read_issue" => t!("{status, select, inProgress {Reading an issue} completed {Read an issue} failed {Failed to read an issue} declined {Declined to read an issue} stopped {Stopped reading an issue} other {Reading an issue}}", status = status),
        "read_pull_request_guide" => t!("{status, select, inProgress {Reading a review guide} completed {Read a review guide} failed {Failed to read a review guide} declined {Declined to read a review guide} stopped {Stopped reading a review guide} other {Reading a review guide}}", status = status),
        "request_secret" => t!("{status, select, inProgress {Asking for a secret} completed {Asked for a secret} failed {Failed to ask for a secret} declined {Declined to ask for a secret} stopped {Stopped asking for a secret} other {Asking for a secret}}", status = status),
        "run_scheduled_task_now" => t!("{status, select, inProgress {Running a scheduled task} completed {Requested a run of a scheduled task} failed {Failed to run a scheduled task} declined {Declined to run a scheduled task} stopped {Stopped running a scheduled task} other {Running a scheduled task}}", status = status),
        "schedule_task" => t!("{status, select, inProgress {Scheduling a task} completed {Scheduled a task} failed {Failed to schedule a task} declined {Declined to schedule a task} stopped {Stopped scheduling a task} other {Scheduling a task}}", status = status),
        "send_to_user" => t!("{status, select, inProgress {Sending a message} completed {Sent a message} failed {Failed to send a message} declined {Declined to send a message} stopped {Stopped sending a message} other {Sending a message}}", status = status),
        "set_issue_state" => t!("{status, select, inProgress {Changing an issue's state} completed {Changed an issue's state} failed {Failed to change an issue's state} declined {Declined to change an issue's state} stopped {Stopped changing an issue's state} other {Changing an issue's state}}", status = status),
        "t3_attachment_discard" => t!("{status, select, inProgress {Discarding a pending attachment} completed {Discarded a pending attachment} failed {Failed to discard a pending attachment} declined {Declined to discard a pending attachment} stopped {Stopped discarding a pending attachment} other {Discarding a pending attachment}}", status = status),
        "t3_attachment_prepare_upload" => t!("{status, select, inProgress {Preparing an attachment upload} completed {Prepared an attachment upload} failed {Failed to prepare an attachment upload} declined {Declined to prepare an attachment upload} stopped {Stopped preparing an attachment upload} other {Preparing an attachment upload}}", status = status),
        "t3_background_wait" => t!("{status, select, inProgress {Updating background waits} completed {Updated background waits} failed {Failed to update background waits} declined {Declined to update background waits} stopped {Stopped updating background waits} other {Updating background waits}}", status = status),
        "t3_environment_link" => t!("{status, select, inProgress {Asking to link an environment} completed {Asked to link an environment} failed {Failed to ask to link an environment} declined {Declined to ask to link an environment} stopped {Stopped asking to link an environment} other {Asking to link an environment}}", status = status),
        "t3_environment_links" => t!("{status, select, inProgress {Listing linked environments} completed {Listed linked environments} failed {Failed to list linked environments} declined {Declined to list linked environments} stopped {Stopped listing linked environments} other {Listing linked environments}}", status = status),
        "t3_environment_preferences_update" => t!("{status, select, inProgress {Updating environment preferences} completed {Updated environment preferences} failed {Failed to update environment preferences} declined {Declined to update environment preferences} stopped {Stopped updating environment preferences} other {Updating environment preferences}}", status = status),
        "t3_environment_read" => t!("{status, select, inProgress {Reading environment preferences} completed {Read environment preferences} failed {Failed to read environment preferences} declined {Declined to read environment preferences} stopped {Stopped reading environment preferences} other {Reading environment preferences}}", status = status),
        "t3_environment_unlink" => t!("{status, select, inProgress {Forgetting a linked environment} completed {Forgot a linked environment} failed {Failed to forget a linked environment} declined {Declined to forget a linked environment} stopped {Stopped forgetting a linked environment} other {Forgetting a linked environment}}", status = status),
        "t3_pending_request" => t!("{status, select, inProgress {Handling pending questions} completed {Handled pending questions} failed {Failed to handle pending questions} declined {Declined to handle pending questions} stopped {Stopped handling pending questions} other {Handling pending questions}}", status = status),
        "t3_pending_request_list" => t!("{status, select, inProgress {Listing pending questions} completed {Listed pending questions} failed {Failed to list pending questions} declined {Declined to list pending questions} stopped {Stopped listing pending questions} other {Listing pending questions}}", status = status),
        "t3_pending_request_read" => t!("{status, select, inProgress {Reading pending questions} completed {Read pending questions} failed {Failed to read pending questions} declined {Declined to read pending questions} stopped {Stopped reading pending questions} other {Reading pending questions}}", status = status),
        "t3_pending_request_respond" => t!("{status, select, inProgress {Answering pending questions} completed {Answered pending questions} failed {Failed to answer pending questions} declined {Declined to answer pending questions} stopped {Stopped answering pending questions} other {Answering pending questions}}", status = status),
        "t3_preview_close" => t!("{status, select, inProgress {Closing a preview tab} completed {Closed a preview tab} failed {Failed to close a preview tab} declined {Declined to close a preview tab} stopped {Stopped closing a preview tab} other {Closing a preview tab}}", status = status),
        "t3_preview_list" => t!("{status, select, inProgress {Listing preview tabs} completed {Listed preview tabs} failed {Failed to list preview tabs} declined {Declined to list preview tabs} stopped {Stopped listing preview tabs} other {Listing preview tabs}}", status = status),
        "t3_project_clone" => t!("{status, select, inProgress {Cloning a repository} completed {Cloned a repository} failed {Failed to clone a repository} declined {Declined to clone a repository} stopped {Stopped cloning a repository} other {Cloning a repository}}", status = status),
        "t3_project_create" => t!("{status, select, inProgress {Registering a project} completed {Registered a project} failed {Failed to register a project} declined {Declined to register a project} stopped {Stopped registering a project} other {Registering a project}}", status = status),
        "t3_project_delete" => t!("{status, select, inProgress {Deleting a project} completed {Deleted a project} failed {Failed to delete a project} declined {Declined to delete a project} stopped {Stopped deleting a project} other {Deleting a project}}", status = status),
        "t3_project_list" => t!("{status, select, inProgress {Listing projects} completed {Listed projects} failed {Failed to list projects} declined {Declined to list projects} stopped {Stopped listing projects} other {Listing projects}}", status = status),
        "t3_project_read" => t!("{status, select, inProgress {Reading a project} completed {Read a project} failed {Failed to read a project} declined {Declined to read a project} stopped {Stopped reading a project} other {Reading a project}}", status = status),
        "t3_project_update" => t!("{status, select, inProgress {Updating a project} completed {Updated a project} failed {Failed to update a project} declined {Declined to update a project} stopped {Stopped updating a project} other {Updating a project}}", status = status),
        "t3_queue" => t!("{status, select, inProgress {Managing queued messages} completed {Managed queued messages} failed {Failed to manage queued messages} declined {Declined to manage queued messages} stopped {Stopped managing queued messages} other {Managing queued messages}}", status = status),
        "t3_queue_cancel" => t!("{status, select, inProgress {Canceling a queued run} completed {Requested cancellation of a queued run} failed {Failed to cancel a queued run} declined {Declined to cancel a queued run} stopped {Stopped canceling a queued run} other {Canceling a queued run}}", status = status),
        "t3_queue_edit" => t!("{status, select, inProgress {Editing a queued message} completed {Edited a queued message} failed {Failed to edit a queued message} declined {Declined to edit a queued message} stopped {Stopped editing a queued message} other {Editing a queued message}}", status = status),
        "t3_queue_list" => t!("{status, select, inProgress {Listing queued messages} completed {Listed queued messages} failed {Failed to list queued messages} declined {Declined to list queued messages} stopped {Stopped listing queued messages} other {Listing queued messages}}", status = status),
        "t3_queue_promote_to_steer" => t!("{status, select, inProgress {Steering with a queued message} completed {Requested steering with a queued message} failed {Failed to steer with a queued message} declined {Declined to steer with a queued message} stopped {Stopped steering with a queued message} other {Steering with a queued message}}", status = status),
        "t3_queue_read" => t!("{status, select, inProgress {Reading a queued message} completed {Read a queued message} failed {Failed to read a queued message} declined {Declined to read a queued message} stopped {Stopped reading a queued message} other {Reading a queued message}}", status = status),
        "t3_queue_reorder" => t!("{status, select, inProgress {Reordering a queued run} completed {Reordered a queued run} failed {Failed to reorder a queued run} declined {Declined to reorder a queued run} stopped {Stopped reordering a queued run} other {Reordering a queued run}}", status = status),
        "t3_theme_read" => t!("{status, select, inProgress {Reading themes} completed {Read themes} failed {Failed to read themes} declined {Declined to read themes} stopped {Stopped reading themes} other {Reading themes}}", status = status),
        "t3_theme_set" => t!("{status, select, inProgress {Setting the theme} completed {Set the theme} failed {Failed to set the theme} declined {Declined to set the theme} stopped {Stopped setting the theme} other {Setting the theme}}", status = status),
        "t3_thread_configuration" => t!("{status, select, inProgress {Reading thread configuration} completed {Read thread configuration} failed {Failed to read thread configuration} declined {Declined to read thread configuration} stopped {Stopped reading thread configuration} other {Reading thread configuration}}", status = status),
        "t3_thread_configure" => t!("{status, select, inProgress {Using thread model settings} completed {Used thread model settings} failed {Failed to use thread model settings} declined {Declined to use thread model settings} stopped {Stopped using thread model settings} other {Using thread model settings}}", status = status),
        "t3_thread_fork" => t!("{status, select, inProgress {Forking this thread} completed {Requested a fork of this thread} failed {Failed to fork this thread} declined {Declined to fork this thread} stopped {Stopped forking this thread} other {Forking this thread}}", status = status),
        "t3_thread_interrupt" => t!("{status, select, inProgress {Interrupting a T3 thread} completed {Requested an interrupt of a T3 thread} failed {Failed to interrupt a T3 thread} declined {Declined to interrupt a T3 thread} stopped {Stopped interrupting a T3 thread} other {Interrupting a T3 thread}}", status = status),
        "t3_thread_launch" => t!("{status, select, inProgress {Launching a project thread} completed {Launched a project thread} failed {Failed to launch a project thread} declined {Declined to launch a project thread} stopped {Stopped launching a project thread} other {Launching a project thread}}", status = status),
        "t3_thread_list" => t!("{status, select, inProgress {Listing T3 threads} completed {Listed T3 threads} failed {Failed to list T3 threads} declined {Declined to list T3 threads} stopped {Stopped listing T3 threads} other {Listing T3 threads}}", status = status),
        "t3_thread_merge_back" => t!("{status, select, inProgress {Merging thread context} completed {Requested a merge of thread context} failed {Failed to merge thread context} declined {Declined to merge thread context} stopped {Stopped merging thread context} other {Merging thread context}}", status = status),
        "t3_thread_organize" => t!("{status, select, inProgress {Organizing a thread} completed {Organized a thread} failed {Failed to organize a thread} declined {Declined to organize a thread} stopped {Stopped organizing a thread} other {Organizing a thread}}", status = status),
        "t3_thread_read" => t!("{status, select, inProgress {Reading a T3 thread} completed {Read a T3 thread} failed {Failed to read a T3 thread} declined {Declined to read a T3 thread} stopped {Stopped reading a T3 thread} other {Reading a T3 thread}}", status = status),
        "t3_thread_search" => t!("{status, select, inProgress {Searching thread content} completed {Searched thread content} failed {Failed to search thread content} declined {Declined to search thread content} stopped {Stopped searching thread content} other {Searching thread content}}", status = status),
        "t3_thread_send" => t!("{status, select, inProgress {Sending to a T3 thread} completed {Sent to a T3 thread} failed {Failed to send to a T3 thread} declined {Declined to send to a T3 thread} stopped {Stopped sending to a T3 thread} other {Sending to a T3 thread}}", status = status),
        "t3_thread_send_attachments" => t!("{status, select, inProgress {Sending attachments} completed {Sent attachments} failed {Failed to send attachments} declined {Declined to send attachments} stopped {Stopped sending attachments} other {Sending attachments}}", status = status),
        "t3_thread_start" => t!("{status, select, inProgress {Starting a T3 thread} completed {Started a T3 thread} failed {Failed to start a T3 thread} declined {Declined to start a T3 thread} stopped {Stopped starting a T3 thread} other {Starting a T3 thread}}", status = status),
        "t3_thread_transfer" => t!("{status, select, inProgress {Using thread transfers} completed {Used thread transfers} failed {Failed to use thread transfers} declined {Declined to use thread transfers} stopped {Stopped using thread transfers} other {Using thread transfers}}", status = status),
        "t3_thread_transfers" => t!("{status, select, inProgress {Reading thread transfers} completed {Read thread transfers} failed {Failed to read thread transfers} declined {Declined to read thread transfers} stopped {Stopped reading thread transfers} other {Reading thread transfers}}", status = status),
        "t3_thread_update" => t!("{status, select, inProgress {Updating T3 thread metadata} completed {Updated T3 thread metadata} failed {Failed to update T3 thread metadata} declined {Declined to update T3 thread metadata} stopped {Stopped updating T3 thread metadata} other {Updating T3 thread metadata}}", status = status),
        "t3_thread_wait" => t!("{status, select, inProgress {Waiting for a T3 thread} completed {Waited for a T3 thread} failed {Failed to wait for a T3 thread} declined {Declined to wait for a T3 thread} stopped {Stopped waiting for a T3 thread} other {Waiting for a T3 thread}}", status = status),
        "t3_workflow_stop" => t!("{status, select, inProgress {Stopping a workflow} completed {Requested a stop for a workflow} failed {Failed to stop a workflow} declined {Declined to stop a workflow} stopped {Stopped stopping a workflow} other {Stopping a workflow}}", status = status),
        "t3_worktree_handoff" => t!("{status, select, inProgress {Handing off thread to a git worktree} completed {Handed off thread to a git worktree} failed {Failed to hand off thread to a git worktree} declined {Declined to hand off thread to a git worktree} stopped {Stopped handing off thread to a git worktree} other {Handing off thread to a git worktree}}", status = status),
        "t3_worktree_list" => t!("{status, select, inProgress {Listing workspace branches} completed {Listed workspace branches} failed {Failed to list workspace branches} declined {Declined to list workspace branches} stopped {Stopped listing workspace branches} other {Listing workspace branches}}", status = status),
        "t3_worktree_status" => t!("{status, select, inProgress {Getting thread worktree status} completed {Got thread worktree status} failed {Failed to get thread worktree status} declined {Declined to get thread worktree status} stopped {Stopped getting thread worktree status} other {Getting thread worktree status}}", status = status),
        "task_cancel" => t!("{status, select, inProgress {Canceling a delegated task} completed {Requested cancellation of a delegated task} failed {Failed to cancel a delegated task} declined {Declined to cancel a delegated task} stopped {Stopped canceling a delegated task} other {Canceling a delegated task}}", status = status),
        "task_status" => t!("{status, select, inProgress {Getting delegated task status} completed {Got delegated task status} failed {Failed to get delegated task status} declined {Declined to get delegated task status} stopped {Stopped getting delegated task status} other {Getting delegated task status}}", status = status),
        "unlink_issue" => t!("{status, select, inProgress {Unlinking an issue} completed {Unlinked an issue} failed {Failed to unlink an issue} declined {Declined to unlink an issue} stopped {Stopped unlinking an issue} other {Unlinking an issue}}", status = status),
        "unlink_pull_request" => t!("{status, select, inProgress {Unlinking a pull request} completed {Unlinked a pull request} failed {Failed to unlink a pull request} declined {Declined to unlink a pull request} stopped {Stopped unlinking a pull request} other {Unlinking a pull request}}", status = status),
        "unlink_pull_requests" => t!("{status, select, inProgress {Unlinking pull requests by state} completed {Unlinked pull requests by state} failed {Failed to unlink pull requests by state} declined {Declined to unlink pull requests by state} stopped {Stopped unlinking pull requests by state} other {Unlinking pull requests by state}}", status = status),
        "unwatch_pull_request" => t!("{status, select, inProgress {Unwatching a pull request} completed {Stopped watching a pull request} failed {Failed to stop watching a pull request} declined {Declined to stop watching a pull request} stopped {Stopped unwatching a pull request} other {Unwatching a pull request}}", status = status),
        "update_scheduled_task" => t!("{status, select, inProgress {Updating a scheduled task} completed {Updated a scheduled task} failed {Failed to update a scheduled task} declined {Declined to update a scheduled task} stopped {Stopped updating a scheduled task} other {Updating a scheduled task}}", status = status),
        "watch_pull_request" => t!("{status, select, inProgress {Watching a pull request} completed {Watching a pull request} failed {Failed to watch a pull request} declined {Declined to watch a pull request} stopped {Stopped watching a pull request} other {Watching a pull request}}", status = status),
        "write_pull_request_guides" => t!("{status, select, inProgress {Writing review guides} completed {Started writing review guides} failed {Failed to write review guides} declined {Declined to write review guides} stopped {Stopped writing review guides} other {Writing review guides}}", status = status),
        _ => return None,
    })
}

/// The sentence of a T3 tool row that names one change request.
fn t3_change_request_sentence(name: &str, status: &str, change_request: &str, number: &str) -> Option<SharedString> {
    Some(match name {
        "link_pull_request" => t!("{status, select, inProgress {Linking {changeRequest} #{number}} completed {Linked {changeRequest} #{number}} failed {Failed to link {changeRequest} #{number}} declined {Declined to link {changeRequest} #{number}} stopped {Stopped linking {changeRequest} #{number}} other {Linking {changeRequest} #{number}}}", status = status, changeRequest = change_request, number = number),
        "link_pull_requests" => t!("{status, select, inProgress {Linking {changeRequest} #{number}} completed {Linked {changeRequest} #{number}} failed {Failed to link {changeRequest} #{number}} declined {Declined to link {changeRequest} #{number}} stopped {Stopped linking {changeRequest} #{number}} other {Linking {changeRequest} #{number}}}", status = status, changeRequest = change_request, number = number),
        "unlink_pull_request" => t!("{status, select, inProgress {Unlinking {changeRequest} #{number}} completed {Unlinked {changeRequest} #{number}} failed {Failed to unlink {changeRequest} #{number}} declined {Declined to unlink {changeRequest} #{number}} stopped {Stopped unlinking {changeRequest} #{number}} other {Unlinking {changeRequest} #{number}}}", status = status, changeRequest = change_request, number = number),
        "unlink_pull_requests" => t!("{status, select, inProgress {Unlinking {changeRequest} #{number}} completed {Unlinked {changeRequest} #{number}} failed {Failed to unlink {changeRequest} #{number}} declined {Declined to unlink {changeRequest} #{number}} stopped {Stopped unlinking {changeRequest} #{number}} other {Unlinking {changeRequest} #{number}}}", status = status, changeRequest = change_request, number = number),
        "unwatch_pull_request" => t!("{status, select, inProgress {Unwatching {changeRequest} #{number}} completed {Stopped watching {changeRequest} #{number}} failed {Failed to stop watching {changeRequest} #{number}} declined {Declined to stop watching {changeRequest} #{number}} stopped {Stopped unwatching {changeRequest} #{number}} other {Unwatching {changeRequest} #{number}}}", status = status, changeRequest = change_request, number = number),
        "watch_pull_request" => t!("{status, select, inProgress {Watching {changeRequest} #{number}} completed {Watching {changeRequest} #{number}} failed {Failed to watch {changeRequest} #{number}} declined {Declined to watch {changeRequest} #{number}} stopped {Stopped watching {changeRequest} #{number}} other {Watching {changeRequest} #{number}}}", status = status, changeRequest = change_request, number = number),
        _ => return None,
    })
}
