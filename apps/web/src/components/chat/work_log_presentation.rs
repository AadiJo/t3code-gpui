//! How work-log entries read in the timeline: their action, icon, labels and
//! group summaries. Ports the parts of `client-runtime/src/work-log/presentation.ts`
//! and `TimelineWorkEntryRow.logic.ts` the timeline rows use.

use crate::components::icons::IconName;
use gpui_kit::SharedString;

use super::{
    timeline_entries::{DynamicToolKind, TodoState, ToolStatus, WorkItem, WorkLogEntry},
};

/// What a tool call did, for grouping and summaries (`ToolGroupAction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolAction {
    Read,
    Edit,
    Command,
    ThreadCreate,
    Browser,
    Device,
    CodeSearch,
    Search,
    PullRequest,
    Other,
    Update,
}

/// The icon a group header shows (`ToolGroupSummaryKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryKind {
    Action(ToolAction),
    DynamicTool,
    Reasoning,
    AgentTool,
    ToneTool,
    JsRepl,
    Todo,
    Approval,
    Mixed,
}

impl SummaryKind {
    /// Icon per kind (`TimelineWorkEntryRow.logic.ts:14-55`).
    pub fn icon(self) -> IconName {
        match self {
            Self::Action(ToolAction::Read) => IconName::Eye,
            Self::Action(ToolAction::Edit) => IconName::SquarePen,
            Self::Action(ToolAction::Command) => IconName::Terminal,
            Self::Action(ToolAction::Search) => IconName::Globe,
            Self::Action(ToolAction::CodeSearch) => IconName::Search,
            Self::Action(ToolAction::Browser) => IconName::Globe,
            Self::Action(ToolAction::Device) => IconName::Smartphone,
            Self::Action(ToolAction::PullRequest) => IconName::GitPullRequest,
            Self::Action(ToolAction::ThreadCreate) => IconName::MessageCircle,
            Self::Action(ToolAction::Update) | Self::DynamicTool | Self::Mixed => IconName::Hammer,
            Self::Action(ToolAction::Other) => IconName::Wrench,
            Self::Reasoning => IconName::Brain,
            Self::AgentTool => IconName::Bot,
            Self::ToneTool => IconName::Zap,
            Self::JsRepl => IconName::CodeXml,
            Self::Todo => IconName::ListTodo,
            Self::Approval => IconName::MessageCircle,
        }
    }
}

/// The action of one entry (`toolGroupAction`).
pub fn tool_action(entry: &WorkLogEntry) -> ToolAction {
    match &entry.item {
        WorkItem::Command { .. } => ToolAction::Command,
        WorkItem::FileChange { .. } => ToolAction::Edit,
        WorkItem::FileRead { .. } | WorkItem::ViewedImage { .. } => ToolAction::Read,
        WorkItem::FileSearch { .. } => ToolAction::CodeSearch,
        WorkItem::WebSearch { .. } => ToolAction::Search,
        WorkItem::DynamicTool { kind, .. } => match kind {
            DynamicToolKind::Browser => ToolAction::Browser,
            DynamicToolKind::Device => ToolAction::Device,
            DynamicToolKind::PullRequest => ToolAction::PullRequest,
            DynamicToolKind::ThreadCreate => ToolAction::ThreadCreate,
            DynamicToolKind::Mcp | DynamicToolKind::JsRepl | DynamicToolKind::Agent => ToolAction::Other,
        },
        WorkItem::Approval { .. } => ToolAction::Other,
        _ => ToolAction::Update,
    }
}

/// Whether an entry reads as a tool call or a thought rather than a notice
/// (`workLogEntryIsToolLike`: tone tool or thinking).
pub fn is_tool_like(entry: &WorkLogEntry) -> bool {
    !matches!(
        entry.item,
        WorkItem::SystemNotice { .. }
            | WorkItem::Notification { .. }
            | WorkItem::Error { .. }
            | WorkItem::Compaction { .. }
            | WorkItem::TodoList { .. }
            | WorkItem::QuestionAnswer { .. }
    )
}

/// Whether the entry should read as failed (`workEntryDisplayIndicatesToolFailure`).
pub fn indicates_failure(entry: &WorkLogEntry) -> bool {
    match &entry.item {
        WorkItem::Command { exit_code: Some(code), .. } if *code != 0 => true,
        WorkItem::Error { .. } => true,
        _ => matches!(entry.status, ToolStatus::Failed),
    }
}

/// Completed calls with nothing to show stay out of groups unless live
/// (`workEntryIsVisibleInGroup`).
pub fn is_visible_in_group(entry: &WorkLogEntry) -> bool {
    match &entry.item {
        WorkItem::Reasoning { text } => !text.trim().is_empty(),
        _ => true,
    }
}

/// The icon of a single entry (`TimelineWorkEntryRow.logic.ts:154-194`).
pub fn entry_icon(entry: &WorkLogEntry) -> IconName {
    match &entry.item {
        WorkItem::Reasoning { .. } => IconName::Brain,
        WorkItem::DynamicTool { kind: DynamicToolKind::JsRepl, .. } => IconName::CodeXml,
        WorkItem::DynamicTool { kind: DynamicToolKind::Agent, .. } => IconName::Bot,
        WorkItem::DynamicTool { kind: DynamicToolKind::Mcp, .. } => IconName::Hammer,
        WorkItem::TodoList { .. } => IconName::ListTodo,
        WorkItem::Approval { .. } | WorkItem::QuestionAnswer { .. } => IconName::MessageCircle,
        WorkItem::SystemNotice { info: true, .. } => IconName::Zap,
        WorkItem::SystemNotice { info: false, .. } | WorkItem::Error { .. } => IconName::CircleAlert,
        WorkItem::Notification { .. } => IconName::Zap,
        WorkItem::Compaction { .. } => IconName::Minimize2,
        _ => SummaryKind::Action(tool_action(entry)).icon(),
    }
}

/// The group header icon (`toolGroupSummaryKind`).
pub fn summary_kind(entries: &[WorkLogEntry]) -> SummaryKind {
    let tools: Vec<_> = entries.iter().filter(|e| !matches!(e.item, WorkItem::Reasoning { .. })).collect();
    if !entries.is_empty() && tools.is_empty() {
        return SummaryKind::Reasoning;
    }
    let first = tools.first().map(|e| tool_action(e));
    if tools.iter().any(|e| Some(tool_action(e)) != first) {
        return SummaryKind::Mixed;
    }
    match first {
        Some(ToolAction::Other) => {
            let kinds: Vec<_> = tools
                .iter()
                .map(|e| match &e.item {
                    WorkItem::DynamicTool { kind: DynamicToolKind::Agent, .. } => SummaryKind::AgentTool,
                    WorkItem::DynamicTool { kind: DynamicToolKind::JsRepl, .. } => SummaryKind::JsRepl,
                    WorkItem::DynamicTool { .. } => SummaryKind::DynamicTool,
                    WorkItem::Approval { .. } => SummaryKind::Approval,
                    _ => SummaryKind::Action(ToolAction::Other),
                })
                .collect();
            if kinds.windows(2).all(|w| w[0] == w[1]) { kinds[0] } else { SummaryKind::Mixed }
        }
        Some(action) => SummaryKind::Action(action),
        None => SummaryKind::Mixed,
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 { one.replace("{n}", "1") } else { many.replace("{n}", &count.to_string()) }
}

fn action_label(action: ToolAction, count: usize) -> String {
    match action {
        ToolAction::Read => plural(count, "Read {n} file", "Read {n} files"),
        ToolAction::Edit => plural(count, "Changed {n} file", "Changed {n} files"),
        ToolAction::Command => plural(count, "Ran {n} command", "Ran {n} commands"),
        ToolAction::ThreadCreate => plural(count, "Created {n} thread", "Created {n} threads"),
        ToolAction::Device => plural(count, "Used device controls {n} time", "Used device controls {n} times"),
        ToolAction::Browser => plural(count, "Used browser {n} time", "Used browser {n} times"),
        ToolAction::Search => plural(count, "Searched the web {n} time", "Searched the web {n} times"),
        ToolAction::CodeSearch => plural(count, "Searched code {n} time", "Searched code {n} times"),
        ToolAction::PullRequest => plural(count, "Checked linked pull requests", "Checked linked pull requests {n} times"),
        ToolAction::Other => plural(count, "Used {n} tool", "Used {n} tools"),
        ToolAction::Update => plural(count, "Received {n} update", "Received {n} updates"),
    }
}

/// Lower sorts first when picking the two clauses a summary shows.
fn action_priority(action: ToolAction) -> u8 {
    match action {
        ToolAction::Command | ToolAction::Edit | ToolAction::ThreadCreate => 0,
        ToolAction::Other | ToolAction::Update => 2,
        _ => 1,
    }
}

/// Edits count distinct files, other actions count calls (`toolGroupActionCount`).
fn action_count(action: ToolAction, entries: &[&WorkLogEntry]) -> usize {
    if action != ToolAction::Edit {
        return entries.len();
    }
    let mut paths: Vec<&str> = entries
        .iter()
        .flat_map(|e| match &e.item {
            WorkItem::FileChange { changes, .. } => changes.iter().map(|c| c.path.as_ref()).collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    paths.sort_unstable();
    paths.dedup();
    paths.len().max(1)
}

/// "a", "a and b", "a, b, and c".
fn join_clauses(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// "Read 3 files and ran 2 commands" (`summarizeToolGroup`).
pub fn summarize_tool_group(entries: &[WorkLogEntry]) -> SharedString {
    let tools: Vec<&WorkLogEntry> = entries.iter().filter(|e| !matches!(e.item, WorkItem::Reasoning { .. })).collect();
    if !entries.is_empty() && tools.is_empty() {
        return if entries.len() == 1 { "Thought".into() } else { format!("Thought (×{})", entries.len()).into() };
    }
    let mut groups: Vec<(ToolAction, Vec<&WorkLogEntry>)> = Vec::new();
    for entry in &tools {
        let action = tool_action(entry);
        match groups.iter_mut().find(|(a, _)| *a == action) {
            Some((_, list)) => list.push(entry),
            None => groups.push((action, vec![entry])),
        }
    }
    let mut ranked: Vec<(usize, &(ToolAction, Vec<&WorkLogEntry>))> = groups.iter().enumerate().collect();
    ranked.sort_by_key(|(index, (action, _))| (action_priority(*action), *index));
    let mut selected: Vec<_> = ranked.into_iter().take(2).collect();
    selected.sort_by_key(|(index, _)| *index);
    let mut labels: Vec<String> =
        selected.iter().map(|(_, (action, list))| action_label(*action, action_count(*action, list))).collect();
    let shown: usize = selected.iter().map(|(_, (_, list))| list.len()).sum();
    let remaining = tools.len().saturating_sub(shown);
    if remaining > 0 {
        labels.push(plural(remaining, "Performed {n} other action", "Performed {n} other actions"));
    }
    let labels: Vec<String> = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| if index == 0 { label } else { lowercase_first(&label) })
        .collect();
    join_clauses(&labels).into()
}

fn lowercase_first(label: &str) -> String {
    let mut chars = label.chars();
    chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

/// The program a shell command runs: `pnpm test` → `pnpm` (`commandProgramName`).
pub fn command_program(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|token| !token.contains('=') && !matches!(*token, "sudo" | "env" | "time" | "exec"))
        .map(|token| token.rsplit('/').next().unwrap_or(token))
        .filter(|program| program.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '.'))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The one-line label of an entry (`workEntryDisplayLabel`).
pub fn entry_label(entry: &WorkLogEntry) -> SharedString {
    match &entry.item {
        WorkItem::Reasoning { text } => {
            let thought = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if thought.is_empty() { entry.label.clone() } else { thought.into() }
        }
        WorkItem::Command { command, .. } => command.lines().next().unwrap_or("").to_owned().into(),
        WorkItem::FileChange { changes, .. } => match changes.as_ref() {
            [] => entry.label.clone(),
            [one] => one.path.clone(),
            [first, rest @ ..] => format!("{} +{} more", first.path, rest.len()).into(),
        },
        WorkItem::FileRead { paths, .. } => match paths.as_ref() {
            [] => "Read file".into(),
            [one] => format!("Read {}", basename(one)).into(),
            [first, rest @ ..] => format!("Read {} and {} more", basename(first), rest.len()).into(),
        },
        WorkItem::ViewedImage { path } => format!("Viewed {}", basename(path)).into(),
        WorkItem::FileSearch { pattern, .. } => format!("Searched for {pattern}").into(),
        WorkItem::WebSearch { queries, .. } => {
            queries.first().map(|q| SharedString::from(format!("Searched the web for {q}"))).unwrap_or_else(|| "Web search".into())
        }
        WorkItem::DynamicTool { title, .. } => title.clone(),
        WorkItem::TodoList { steps } => {
            let done = steps.iter().filter(|(state, _)| *state == TodoState::Completed).count();
            let current = steps
                .iter()
                .find(|(state, _)| *state == TodoState::Running)
                .or_else(|| steps.iter().find(|(state, _)| *state == TodoState::Pending));
            match current {
                Some((_, step)) => format!("Tasks {done}/{} · {step}", steps.len()).into(),
                None => format!("Tasks {done}/{}", steps.len()).into(),
            }
        }
        WorkItem::Approval { detail, .. } => detail.clone(),
        WorkItem::SystemNotice { text, .. } | WorkItem::Notification { text } => text.clone(),
        WorkItem::Error { message, .. } => message.clone(),
        _ => entry.label.clone(),
    }
}

/// The label of a live entry: "Running pnpm", "Thinking" (`liveWorkEntryLabel`).
pub fn live_entry_label(entry: &WorkLogEntry, active: bool) -> SharedString {
    let status = if active { entry.status } else if entry.status == ToolStatus::InProgress { ToolStatus::Completed } else { entry.status };
    match &entry.item {
        WorkItem::Reasoning { text } if text.trim().is_empty() => {
            if status == ToolStatus::InProgress { "Thinking".into() } else { "Thought".into() }
        }
        WorkItem::Command { command, .. } => {
            let verb = match status {
                ToolStatus::InProgress => "Running",
                ToolStatus::Failed => "Failed",
                ToolStatus::Declined => "Declined",
                ToolStatus::Stopped => "Stopped",
                ToolStatus::Completed => "Ran",
            };
            match command_program(command) {
                Some(program) => format!("{verb} {program}").into(),
                None => format!("{verb} command").into(),
            }
        }
        WorkItem::FileRead { paths, .. } if status == ToolStatus::InProgress => {
            paths.first().map(|p| SharedString::from(format!("Reading {}", basename(p)))).unwrap_or_else(|| "Reading file".into())
        }
        WorkItem::FileChange { changes, .. } if status == ToolStatus::InProgress => {
            changes.first().map(|c| SharedString::from(format!("Editing {}", basename(&c.path)))).unwrap_or_else(|| "Editing".into())
        }
        _ => entry_label(entry),
    }
}

/// `1.2s`, `42s`, `3m 05s`… (`formatDuration`).
pub fn format_duration(ms: i64) -> String {
    if ms < 1_000 {
        return format!("{}ms", ms.max(1));
    }
    if ms < 10_000 {
        let tenths = (ms as f64 / 100.).round() / 10.;
        return format!("{tenths}s");
    }
    let total = (ms as f64 / 1_000.).round() as i64;
    if total < 60 {
        return format!("{total}s");
    }
    let (hours, minutes, seconds) = (total / 3_600, (total % 3_600) / 60, total % 60);
    match (hours, minutes, seconds) {
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, s) if m > 0 => format!("{h}h {m}m {s}s"),
        (h, _, s) if s > 0 => format!("{h}h {s}s"),
        (h, _, _) => format!("{h}h"),
    }
}

/// `3:41 PM` from a UTC timestamp (the client formats in local time).
pub fn format_clock(millis: i64) -> String {
    let minutes_of_day = (millis / 60_000).rem_euclid(24 * 60);
    let (hour, minute) = (minutes_of_day / 60, minutes_of_day % 60);
    let suffix = if hour < 12 { "AM" } else { "PM" };
    let hour12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{hour12}:{minute:02} {suffix}")
}
