//! What a work row can open (`itemDetail.ts`): its output text, whether it has detail at all,
//! and whether that detail must be fetched because the stream withheld it.
//!
//! Differs from the TypeScript: image blocks always read `[image]` (the row has no image
//! previews yet), and MCP app envelopes are read like any other output.

use serde_json::Value;
use t3code_contracts::{OrchestrationV2TurnItem as Item, OrchestrationV2TurnItemStatus as Status};

const MAX_TEXT_BLOCK_DEPTH: usize = 4;

fn is_summarized_value(value: &Value) -> bool {
    value.get("truncated") == Some(&Value::Bool(true)) && value.get("summary").is_some_and(Value::is_string)
}

fn text_from_blocks(value: &Value, depth: usize) -> Option<String> {
    if depth > MAX_TEXT_BLOCK_DEPTH {
        return None;
    }
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let parts: Option<Vec<String>> = blocks.iter().map(|block| text_from_blocks(block, depth + 1)).collect();
            parts.map(|parts| parts.into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n"))
        }
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str);
            if kind == Some("text")
                && let Some(text) = map.get("text").and_then(Value::as_str)
            {
                return Some(text.to_owned());
            }
            if kind == Some("image") {
                return Some("[image]".to_owned());
            }
            if kind == Some("resource_link")
                && let Some(uri) = map.get("uri").and_then(Value::as_str)
            {
                return Some(uri.to_owned());
            }
            if kind == Some("resource")
                && let Some(resource) = map.get("resource").and_then(Value::as_object)
            {
                if let Some(text) = resource.get("text").and_then(Value::as_str) {
                    return Some(text.to_owned());
                }
                if let Some(uri) = resource.get("uri").and_then(Value::as_str) {
                    return Some(uri.to_owned());
                }
            }
            let keys: Vec<&str> = map.keys().map(String::as_str).filter(|key| *key != "isError" && *key != "is_error").collect();
            if keys == ["content"] {
                return text_from_blocks(&map["content"], depth + 1);
            }
            if keys.len() == 2 && keys.contains(&"content") && keys.contains(&"structuredContent") {
                let text = text_from_blocks(&map["content"], depth + 1);
                let images: Vec<Value> = map["content"]
                    .as_array()
                    .map(|blocks| blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("image")).cloned().collect())
                    .unwrap_or_default();
                let structured = format_json_value(&map["structuredContent"]);
                let blank = text.as_deref().is_none_or(|text| text.trim().is_empty());
                if let Some(structured) = structured
                    && (!images.is_empty() || blank)
                {
                    let markers = text_from_blocks(&Value::Array(images), depth + 1).unwrap_or_default();
                    return Some([structured, markers].into_iter().filter(|part| !part.is_empty()).collect::<Vec<_>>().join("\n"));
                }
                return text.filter(|text| !text.trim().is_empty());
            }
            None
        }
        _ => None,
    }
}

/// Two-space JSON like `JSON.stringify(value, null, 2)`.
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

fn has_lossy_json_number(json: &str) -> bool {
    // serde_json without `arbitrary_precision` reads huge integers as floats; a float that
    // prints differently from the source would change the value.
    let mut chars = json.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c == '"' {
            let mut escaped = false;
            for (_, c) in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    break;
                }
            }
            continue;
        }
        if c == '-' || c.is_ascii_digit() {
            let mut end = start + c.len_utf8();
            while let Some(&(i, c)) = chars.peek() {
                if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-') {
                    end = i + c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let token = &json[start..end];
            if !token.contains(['.', 'e', 'E']) {
                let Ok(value) = token.parse::<i128>() else { return true };
                if value.unsigned_abs() > (1u128 << 53) - 1 {
                    return true;
                }
            } else if !token.parse::<f64>().is_ok_and(f64::is_finite) {
                return true;
            }
        }
    }
    false
}

fn pretty_json_text(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with(['[', '{']) {
        return text.to_owned();
    }
    if let Ok(whole) = serde_json::from_str::<Value>(trimmed) {
        return if has_lossy_json_number(trimmed) { text.to_owned() } else { pretty(&whole) };
    }
    let documents: Option<Vec<Value>> =
        trimmed.lines().filter(|line| !line.trim().is_empty()).map(|line| serde_json::from_str(line.trim()).ok()).collect();
    match documents {
        Some(documents) if !has_lossy_json_number(trimmed) => documents.iter().map(pretty).collect::<Vec<_>>().join("\n\n"),
        _ => text.to_owned(),
    }
}

fn format_json_value(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) if text.trim().is_empty() => None,
        value => {
            let json = pretty(value);
            (json != "{}" && json != "[]").then_some(json)
        }
    }
}

fn format_tool_value(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    match text_from_blocks(value, 0) {
        Some(text) if text.trim().is_empty() => None,
        Some(text) => Some(pretty_json_text(&text)),
        None => format_json_value(value),
    }
}

/// A tool call's arguments as lines: a command, key/value pairs, or formatted text
/// (`toolCallLines`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCallLines {
    pub command: Option<String>,
    pub args: Option<Vec<(String, String)>>,
    pub args_text: Option<String>,
}

impl ToolCallLines {
    pub fn is_empty(&self) -> bool {
        self.command.is_none() && self.args.is_none() && self.args_text.is_none()
    }
}

/// The lines of a command.
pub fn command_lines(command: &str) -> ToolCallLines {
    let command = command.trim();
    ToolCallLines { command: (!command.is_empty()).then(|| command.to_owned()), ..ToolCallLines::default() }
}

/// The lines of a dynamic tool's arguments.
pub fn args_lines(args: &Value) -> ToolCallLines {
    if let Value::Object(map) = args
        && !is_summarized_value(args)
    {
        let entries: Vec<(String, String)> = map
            .iter()
            .map(|(key, value)| {
                let shown = match value {
                    Value::String(text) if !text.is_empty() => text.clone(),
                    value => serde_json::to_string(value).unwrap_or_default(),
                };
                (key.clone(), shown)
            })
            .collect();
        return ToolCallLines { args: (!entries.is_empty()).then_some(entries), ..ToolCallLines::default() };
    }
    ToolCallLines { args_text: format_tool_value(Some(args)), ..ToolCallLines::default() }
}

fn is_live(status: Status) -> bool {
    matches!(status, Status::Idle | Status::Pending | Status::Running | Status::Waiting)
}

/// A key that changes when a fetched body may differ: `live` while the item runs, then its
/// update time (`turnItemDetailRevision`).
pub fn turn_item_detail_revision(status: Status, updated_at: &str) -> String {
    if is_live(status) { "live".to_owned() } else { updated_at.to_owned() }
}

/// Whether the stream withheld this item's body (`turnItemNeedsDetailFetch`).
pub fn turn_item_needs_detail_fetch(item: &Item) -> bool {
    match item {
        Item::CommandExecution(command) => command.output_omitted == Some(true),
        Item::DynamicTool(tool) => tool.output_omitted == Some(true) || is_summarized_value(&tool.input),
        _ => false,
    }
}

/// The output text a row opens to (`turnItemOutputText`).
pub fn turn_item_output_text(item: &Item) -> Option<String> {
    match item {
        Item::CommandExecution(command) => command.output.clone().filter(|output| !output.trim().is_empty()),
        Item::DynamicTool(tool) if tool.output_omitted == Some(true) => None,
        Item::DynamicTool(tool) => format_tool_value(tool.output.as_ref()),
        Item::FileSearch(search) => search.results.as_ref().filter(|results| !results.is_empty()).map(|results| {
            results
                .iter()
                .map(|result| {
                    let head = match result.line {
                        Some(line) => format!("{}:{line}", result.file_name),
                        None => result.file_name.clone(),
                    };
                    match result.preview.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                        Some(preview) => format!("{head}\n{preview}"),
                        None => head,
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }),
        Item::WebSearch(search) => search.results.as_ref().filter(|results| !results.is_empty()).map(|results| {
            results
                .iter()
                .map(|result| {
                    let title = result.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
                    let url = result.url.clone().unwrap_or_default();
                    let mut parts = vec![title.map_or_else(|| url.clone(), str::to_owned)];
                    if result.title.is_some() && !url.is_empty() {
                        parts.push(url);
                    }
                    parts.retain(|part| !part.is_empty());
                    if let Some(snippet) = result.snippet.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        parts.push(snippet.to_owned());
                    }
                    parts.join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        }),
        _ => None,
    }
}

/// Whether a row has anything to open (`turnItemHasDetail`).
pub fn turn_item_has_detail(item: &Item) -> bool {
    match item {
        Item::Reasoning(reasoning) => !reasoning.text.trim().is_empty(),
        Item::CommandExecution(command) => {
            command_lines(&command.input).command.is_some()
                || command.output_omitted == Some(true)
                || command.output.as_deref().is_some_and(|output| !output.trim().is_empty())
                || command.exit_code.is_some_and(|code| code != 0)
        }
        Item::FileChange(_) | Item::Checkpoint(_) | Item::Fork(_) | Item::Handoff(_) => true,
        Item::FileSearch(search) => {
            search.results.as_ref().is_some_and(|r| !r.is_empty()) || search.pattern.as_deref().is_some_and(|p| !p.trim().is_empty())
        }
        Item::WebSearch(search) => {
            search.results.as_ref().is_some_and(|r| !r.is_empty()) || search.patterns.as_ref().is_some_and(|p| !p.is_empty())
        }
        Item::DynamicTool(tool) => tool.output_omitted == Some(true) || !args_lines(&tool.input).is_empty(),
        Item::ApprovalRequest(request) => request.prompt.as_deref().is_some_and(|p| !p.trim().is_empty()),
        Item::UserInputRequest(request) => !request.questions.is_empty(),
        Item::Notification(notification) => notification.detail.as_deref().is_some_and(|d| !d.trim().is_empty()),
        Item::SystemNotice(notice) => !notice.message.trim().is_empty(),
        Item::Error(error) => !error.failure.message.trim().is_empty(),
        Item::ProposedPlan(plan) => !plan.markdown.trim().is_empty(),
        Item::TodoList(todo) => !todo.steps.is_empty(),
        Item::Subagent(subagent) => subagent.child_thread_id.is_some(),
        _ => false,
    }
}
