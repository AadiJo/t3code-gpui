//! What a tool call does, read from its JSON input (`packages/shared/src/toolActivity.ts`):
//! whether it ran a command, read, changed or searched files, the paths it names, the parts
//! of a search label, and the titles of JavaScript REPL and Claude skill calls.

use serde_json::{Map, Value};

type Record = Map<String, Value>;

fn as_record(value: Option<&Value>) -> Option<&Record> {
    value.and_then(Value::as_object)
}

fn trimmed(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// A Claude `Skill` call: the skill's name and arguments (`claudeSkillInvocation`).
pub fn claude_skill_invocation<'a>(tool_name: Option<&str>, input: &'a Value) -> Option<(&'a str, Option<&'a str>)> {
    if tool_name != Some("Skill") {
        return None;
    }
    let record = input.as_object()?;
    Some((trimmed(record.get("skill"))?, trimmed(record.get("args"))))
}

/// `isJavaScriptReplTool`.
pub fn is_javascript_repl_tool(tool_name: Option<&str>) -> bool {
    matches!(tool_name, Some("js" | "cua_repl.js" | "node_repl.js"))
}

/// Whether a REPL call drives the computer (`dynamicToolSurface`).
pub fn dynamic_tool_drives_computer(tool_name: Option<&str>, input: &Value) -> bool {
    if tool_name == Some("cua_repl.js") {
        return true;
    }
    let code = input.get("code").and_then(Value::as_str);
    is_javascript_repl_tool(tool_name)
        && code.is_some_and(|code| {
            let sky_call = code.match_indices("sky").any(|(index, _)| {
                let before_ok = code[..index].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                before_ok && code[index + 3..].trim_start().starts_with('.')
            });
            sky_call || code.contains("\"@oai/sky\"") || code.contains("'@oai/sky'")
        })
}

/// The title a dynamic tool's row takes from its tool rather than the provider (`describeDynamicToolTitle`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynamicToolTitle<'a> {
    Title(&'a str),
    ComputerUse,
    RunJavaScript,
    Skill(&'a str),
}

pub fn describe_dynamic_tool_title<'a>(tool_name: Option<&str>, input: &'a Value, retained_title: Option<&'a str>) -> Option<DynamicToolTitle<'a>> {
    if is_javascript_repl_tool(tool_name) {
        let title = retained_title.map(str::trim).filter(|t| !t.is_empty());
        let meaningful = title.filter(|t| {
            let lower = t.to_lowercase();
            lower != "js" && Some(lower.as_str()) != tool_name.map(str::to_lowercase).as_deref()
        });
        if let Some(shown) = trimmed(input.get("title")).or(meaningful) {
            return Some(DynamicToolTitle::Title(shown));
        }
        return Some(if dynamic_tool_drives_computer(tool_name, input) {
            DynamicToolTitle::ComputerUse
        } else {
            DynamicToolTitle::RunJavaScript
        });
    }
    claude_skill_invocation(tool_name, input).map(|(name, _)| DynamicToolTitle::Skill(name))
}

const PATH_KEYS: &[&str] = &["path", "filePath", "file_path", "relativePath", "filename", "fileName", "newPath", "oldPath"];
const NESTED_PATH_KEYS: &[&str] = &["locations", "item", "input", "result", "rawInput", "data", "changes"];

fn collect_paths(value: &Value, paths: &mut Vec<String>, depth: usize) {
    if depth > 4 || paths.len() >= 8 {
        return;
    }
    if let Value::Array(items) = value {
        for item in items {
            collect_paths(item, paths, depth + 1);
            if paths.len() >= 8 {
                return;
            }
        }
        return;
    }
    let Value::Object(record) = value else { return };
    for key in PATH_KEYS {
        let Some(candidate) = trimmed(record.get(*key)) else { continue };
        if paths.iter().any(|seen| seen == candidate) {
            continue;
        }
        paths.push(candidate.to_owned());
        if paths.len() >= 8 {
            return;
        }
    }
    for key in NESTED_PATH_KEYS {
        if let Some(nested) = record.get(*key) {
            collect_paths(nested, paths, depth + 1);
            if paths.len() >= 8 {
                return;
            }
        }
    }
}

/// Up to eight distinct file paths a tool's data names (`collectToolFilePaths`).
pub fn collect_tool_file_paths(data: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    collect_paths(data, &mut paths, 0);
    paths
}

/// What kind of action a tool call is (`ToolActivityAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolActivityAction {
    Command,
    Read,
    FileChange,
    Search,
    Other,
}

/// `toolNameToken`: a bare tool name, lowercased without separators; `None` for namespaced names.
fn tool_name_token(value: Option<&str>) -> Option<String> {
    let trimmed = value.map(str::trim).filter(|v| !v.is_empty())?;
    if trimmed.contains("__") || trimmed.contains(['.', '/']) {
        return None;
    }
    Some(trimmed.chars().filter(|c| !matches!(c, '_' | '-') && !c.is_whitespace()).collect::<String>().to_lowercase())
}

/// `classifyToolActivity`. `item_type` is the lifecycle item type (`dynamic_tool_call` for dynamic tools).
pub fn classify_tool_activity(item_type: Option<&str>, request_kind: Option<&str>, data: Option<&Record>) -> ToolActivityAction {
    let request_kind = request_kind.map(str::trim).filter(|v| !v.is_empty()).map(str::to_lowercase);
    let kind = data.and_then(|d| trimmed(d.get("kind"))).map(str::to_lowercase);
    let tool_name = tool_name_token(
        data.and_then(|d| trimmed(d.get("toolName"))).or_else(|| data.and_then(|d| as_record(d.get("item"))).and_then(|i| trimmed(i.get("tool")))),
    );
    let request_kind = request_kind.as_deref();
    let kind = kind.as_deref();
    let tool = tool_name.as_deref();
    match item_type {
        Some("command_execution") => return ToolActivityAction::Command,
        Some("image_view") => return ToolActivityAction::Read,
        Some("file_change") => return ToolActivityAction::FileChange,
        Some("web_search") => return ToolActivityAction::Search,
        _ => {}
    }
    if request_kind == Some("command") || kind == Some("execute") {
        return ToolActivityAction::Command;
    }
    if request_kind == Some("file-change") || matches!(kind, Some("edit" | "move" | "delete" | "write")) {
        return ToolActivityAction::FileChange;
    }
    if kind == Some("search") || matches!(tool, Some("find" | "grep" | "glob" | "rg" | "ls")) {
        return ToolActivityAction::Search;
    }
    if request_kind == Some("file-read") || kind == Some("read") {
        return ToolActivityAction::Read;
    }
    if matches!(tool, Some("terminal" | "bash" | "shell")) {
        return ToolActivityAction::Command;
    }
    if matches!(tool, Some("read" | "readfile")) {
        return ToolActivityAction::Read;
    }
    ToolActivityAction::Other
}

const SEARCH_QUERY_KEYS: &[&str] = &["pattern", "query", "searchTerm", "regex", "grep", "needle"];
const SEARCH_GLOB_KEYS: &[&str] = &["glob", "globPattern", "glob_pattern", "include", "filePattern", "file_pattern"];
const SEARCH_TARGET_KEYS: &[&str] = &["path", "target_directory", "targetDirectory", "directory", "cwd", "root"];

fn first_string<'a>(record: Option<&'a Record>, keys: &[&str]) -> Option<&'a str> {
    let record = record?;
    keys.iter().find_map(|key| trimmed(record.get(*key)))
}

/// The parts of a search label (`searchToolLabelParts`): query, glob and the target's last segment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchLabelParts<'a> {
    pub query: Option<&'a str>,
    pub glob: Option<&'a str>,
    pub target: Option<&'a str>,
}

pub fn search_tool_label_parts(data: Option<&Record>) -> SearchLabelParts<'_> {
    let input = data.and_then(|data| {
        [data.get("rawInput"), data.get("input"), as_record(data.get("item")).and_then(|i| i.get("input"))]
            .into_iter()
            .map(as_record)
            .find(|record| record.is_some_and(|r| !r.is_empty()))
            .flatten()
    });
    let input = input.or(data);
    let target = first_string(input, SEARCH_TARGET_KEYS).and_then(|value| value.rsplit(['/', '\\']).find(|part| !part.is_empty() && *part != "."));
    SearchLabelParts { query: first_string(input, SEARCH_QUERY_KEYS), glob: first_string(input, SEARCH_GLOB_KEYS), target }
}
