//! The thread details card's Lineage and Side chats rows (`ThreadRelationshipsPanel` in
//! `ThreadRelationshipsControl.tsx`): the relationship graph of the shown thread grouped into
//! related threads, active and previous agents and side chats, with the merge-back target, the
//! disconnect action and the section headings' running counts.

use std::sync::Arc;

use gpui_kit::SharedString;
use t3code_client_runtime::state::subagent_runtime::{
    RuntimeSubagent, WorkflowPhaseGroup, WorkflowPhaseState, derive_workflow_groups, format_subagent_display_title, is_active_subagent_status,
    projected_subagents_to_runtime, workflow_member_model_selection,
};
use t3code_client_runtime::state::thread_relationships::{
    RelationshipStatus, ThreadRelationshipKind, ThreadRelationshipWalkRow, derive_thread_relationship_graph,
    immediate_thread_relationships, is_parent_thread_relationship, order_web_thread_lineage_rows,
    resolve_merge_back_target_thread_id, thread_relationship_row_status,
};
use t3code_client_runtime::state::thread_workflows::{can_detach_thread_provider_session, resolve_latest_merge_back_run};
use t3code_contracts::{
    EnvironmentId, ModelSelection, NodeId, OrchestrationProjectShell, OrchestrationV2DelegatedFrom, OrchestrationV2ShellThreadStatus as ShellStatus,
    OrchestrationV2Subagent, OrchestrationV2SubagentOrigin as SubagentOrigin, OrchestrationV2ThreadShell, OrchestrationV2TurnItemStatus as ItemStatus,
    ProjectId, RunId, ThreadId,
};
use t3code_shared::thread_projection::ThreadProjection;
use t3code_shared::usage_format::parse_instant_ms;

use crate::provider_instances::{ProviderInstanceEntry, ServerProvider, has_instance_badge, should_show_instance_badge};
use crate::provider_models::resolve_selectable_model;
use crate::i18n::t;

/// The row's fallback icon (`RelationshipIcon`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationshipIcon {
    /// `CornerLeftUpIcon`: the parent.
    Parent,
    /// `BotIcon`: a subagent.
    Agent,
    /// `MessagesSquareIcon`: a side chat.
    SideChat,
    /// `GitForkIcon`: a fork or transfer.
    Fork,
}

/// One related thread.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipRow {
    pub thread_id: ThreadId,
    pub title: SharedString,
    pub icon: RelationshipIcon,
    pub status: Option<RelationshipStatus>,
    /// What a click does, under the title in the tooltip (`relationshipHint`).
    pub hint: SharedString,
    /// The fork's merge-back target: the row splits into open, merge and its status.
    pub merge_target: bool,
    /// A child agent's driver mark, shown instead of the fallback icon.
    pub mark: Option<ProviderMark>,
    /// The projected subagent behind a child agent row: its elapsed time, hover card and stop
    /// button.
    pub agent: Option<RowAgent>,
    /// A workflow coordinator's row expands into its phases and members.
    pub workflow: Option<WorkflowRow>,
    /// A task delegated to a linked environment (`RemoteSubagentRow`); `thread_id` is the
    /// thread there.
    pub remote: Option<RemoteRow>,
}

/// A subagent's start and end, for `AgentElapsed` (`deriveSubagentElapsedMs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentClock {
    /// Pending, running or waiting: the clock reads the wall clock.
    pub live: bool,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

impl AgentClock {
    fn of(status: ItemStatus, started_at: Option<&str>, completed_at: Option<&str>) -> Self {
        Self { live: is_active_subagent_status(status), started_at: started_at.and_then(parse_instant_ms), completed_at: completed_at.and_then(parse_instant_ms) }
    }

    /// Elapsed milliseconds at `now`, or `None` before it starts or when a finished agent has no end.
    pub fn elapsed_ms(&self, now: i64) -> Option<i64> {
        let start = self.started_at?;
        let end = if self.live { now } else { self.completed_at? };
        Some((end - start).max(0))
    }
}

/// The driver mark a subagent row shows instead of the fallback icon (`ProviderInstanceIcon`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderMark {
    pub driver: SharedString,
    pub display_name: SharedString,
    pub accent: Option<SharedString>,
    pub show_badge: bool,
}

/// `SubagentTooltipContent`: the hover card of an agent row.
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentCard {
    pub title: SharedString,
    pub mark: Option<ProviderMark>,
    /// The model line: its label, with the instance name when the badge shows.
    pub model: SharedString,
    pub effort: Option<SharedString>,
    pub status: ItemStatus,
    pub clock: AgentClock,
    /// Project and branch, worktree or workspace rows; `branch` picks the branch icon.
    pub metadata: Vec<(SharedString, bool)>,
    pub preview: Option<SharedString>,
}

/// A child agent row's projected subagent.
#[derive(Clone, Debug, PartialEq)]
pub struct RowAgent {
    pub subagent_id: NodeId,
    /// The thread that owns the subagent record (the interrupt goes there).
    pub owner_thread_id: ThreadId,
    pub clock: AgentClock,
    /// `canStop`: app-owned, or a Claude native task with a strong reference, started and active.
    pub can_stop: bool,
    pub card: SubagentCard,
}

/// A workflow member under its coordinator (`WorkflowMemberRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowMember {
    pub id: SharedString,
    pub title: SharedString,
    pub status: ItemStatus,
    pub thread_id: Option<ThreadId>,
    pub clock: AgentClock,
    pub card: SubagentCard,
}

/// A workflow phase's status word (`phaseStatusLabel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseStatus {
    Running,
    Failed,
    Stopped,
    Done,
    NotStarted,
}

/// One phase with members (`group.phases`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowPhaseRow {
    pub index: u64,
    pub title: SharedString,
    pub running: bool,
    pub status: PhaseStatus,
    pub settled: usize,
    pub members: Vec<WorkflowMember>,
    /// `phaseElapsed`: first start to last end, live while running.
    pub clock: AgentClock,
}

/// `ThreadLineageWorkflowRow`'s data.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowRow {
    pub coordinator: NodeId,
    pub label: SharedString,
    pub mark: Option<ProviderMark>,
    pub can_stop: bool,
    pub phases: Vec<WorkflowPhaseRow>,
    pub unphased: Vec<WorkflowMember>,
}

/// A task delegated to a linked environment.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteRow {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    /// This client is connected to that environment.
    pub connected: bool,
    pub raw_status: ItemStatus,
}

/// A collapsible group of agent rows (`ThreadLineageGroup`); `label` is `None` for the
/// headingless groups.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipGroup {
    pub id: &'static str,
    pub label: Option<SharedString>,
    pub rows: Vec<RelationshipRow>,
    pub expanded: bool,
}

/// Merge back from the shown fork into its source.
#[derive(Clone, Debug, PartialEq)]
pub struct MergeBack {
    pub target: ThreadId,
    /// The target's title, unless it is hidden or unknown.
    pub parent_title: Option<SharedString>,
    /// The run merged; `None` until a run in this fork completes.
    pub run_id: Option<RunId>,
}

/// The thread on a linked environment whose agent delegated this one.
#[derive(Clone, Debug, PartialEq)]
pub struct DelegatedFromRow {
    pub from: OrchestrationV2DelegatedFrom,
    pub title: SharedString,
    /// The environment's label (connected, or as recorded at link time).
    pub label: Option<SharedString>,
    /// This client is connected to that environment.
    pub connected: bool,
}

/// What the Lineage and Side chats sections show; `None` hides both.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationshipsInput {
    pub lineage_visible: bool,
    pub side_chats_visible: bool,
    pub groups: Vec<RelationshipGroup>,
    pub side_chats: Vec<RelationshipRow>,
    pub running_count: usize,
    pub running_side_chats: usize,
    /// "Lineage" heading shows (a lone "Previous agents" group names itself).
    pub show_heading: bool,
    pub can_detach: bool,
    pub merge: Option<MergeBack>,
    pub delegated_from: Option<DelegatedFromRow>,
    /// A rel row's status dot sits above the icon when any provider instance shows a badge.
    pub any_instance_badge: bool,
    /// The subagent or workflow whose stop is waiting on the server (`stoppingId`).
    pub stopping: Option<SharedString>,
    /// Rows the shell does not list and the card asks `orchestration.getThreadTitles` about.
    pub unlisted: Vec<ThreadId>,
}

/// What `orchestration.getThreadTitles` said about an unlisted thread (`threadTitles`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TitleLookup {
    /// Not asked yet, in flight, or the read failed: the row stays, with what the shell knows.
    Unsettled,
    Found(SharedString),
    /// A settled lookup without it: the thread is gone.
    Absent,
}

/// What the derivation reads.
pub struct RelationshipSources<'a> {
    pub thread_id: &'a ThreadId,
    /// Live shells first, then archived ones.
    pub shells: Vec<&'a Arc<OrchestrationV2ThreadShell>>,
    /// How many of `shells` are live (`useThreadRelationshipShells`); the rest are archived.
    pub live_shells: usize,
    pub projection: Option<&'a ThreadProjection>,
    /// The environment's projects, for the hover card's project and workspace rows.
    pub projects: Vec<&'a OrchestrationProjectShell>,
    pub providers: &'a [Arc<ServerProvider>],
    pub provider_entries: &'a [ProviderInstanceEntry],
    /// A provider instance in another connected environment (remote subagents run there).
    pub remote_provider: &'a dyn Fn(&EnvironmentId, &str) -> Option<Arc<ServerProvider>>,
    /// Threads hidden by the environment's hiding rules.
    pub is_hidden: &'a dyn Fn(&ThreadId) -> bool,
    pub hiding_anywhere: bool,
    pub show_lineage: bool,
    pub show_side_chats: bool,
    /// `orchestration.stopWorkflow` is allowed here.
    pub can_operate_workflow: bool,
    /// The delegating environment's label when this client is connected to it.
    pub connected_label: &'a dyn Fn(&EnvironmentId) -> Option<SharedString>,
    pub titles: &'a dyn Fn(&ThreadId) -> TitleLookup,
}

/// `relationshipHint`.
fn hint(kind: ThreadRelationshipKind, source: &ThreadId, current: &ThreadId, side_chat: bool) -> SharedString {
    if side_chat {
        return t!("Open side chat in this chat");
    }
    match kind {
        ThreadRelationshipKind::Transfer => t!("Open context transfer in this chat"),
        ThreadRelationshipKind::Subagent if source == current => t!("Open subagent in this chat"),
        ThreadRelationshipKind::Subagent => t!("Open parent agent in this chat"),
        _ if source == current => t!("Open fork in this chat"),
        _ => t!("Open parent thread in this chat"),
    }
}


/// `currentSubagent`: a finished subagent whose child thread runs again shows that run.
fn current_status(agent: &OrchestrationV2Subagent, child: Option<&OrchestrationV2ThreadShell>) -> (ItemStatus, Option<String>, Option<String>, bool) {
    let same = (agent.status, agent.started_at.clone(), agent.completed_at.clone(), false);
    let Some(child) = child.filter(|_| !is_active_subagent_status(agent.status)) else { return same };
    let live = child.activity_run_status.map(|s| s.as_str()).or((child.status == ShellStatus::Queued).then_some("queued"));
    let agent_at = agent.completed_at.as_deref().or(agent.started_at.as_deref()).unwrap_or(&agent.updated_at);
    let newer = child.latest_run_requested_at.as_deref().and_then(parse_instant_ms).zip(parse_instant_ms(agent_at)).is_some_and(|(run, at)| run > at);
    if live.is_none() && !newer {
        return same;
    }
    let status = live.unwrap_or(child.status.as_str());
    let status = match status {
        "preparing" | "starting" | "queued" => ItemStatus::Pending,
        "rolled_back" => ItemStatus::Interrupted,
        other => ItemStatus::from_wire(other).unwrap_or(agent.status),
    };
    let started = if live.is_some() { child.activity_run_started_at.clone() } else { child.latest_run_started_at.clone() };
    let completed = if live.is_some() { None } else { child.latest_run_completed_at.clone() };
    (status, started, completed, true)
}

/// `formatModelSlugName`.
fn format_model_slug_name(slug: &str) -> String {
    let split = slug.rfind('/').map_or(0, |i| i + 1);
    let (prefix, name) = slug.split_at(split);
    let lower = name.to_ascii_lowercase();
    let digit_after = |p: &str| lower.strip_prefix(p).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    if digit_after("gpt-") {
        let mut out = String::from("GPT");
        let mut chars = name[3..].chars().peekable();
        while let Some(c) = chars.next() {
            out.push(c);
            if c == '-' && let Some(next) = chars.next_if(|n| n.is_ascii_lowercase()) {
                out.push(next.to_ascii_uppercase());
            }
        }
        return format!("{prefix}{out}");
    }
    let family = ["claude-opus-", "claude-sonnet-", "claude-haiku-", "claude-fable-", "gemini-", "grok-", "composer-"];
    if !family.iter().any(|p| digit_after(p)) {
        return slug.to_owned();
    }
    let mut parts: Vec<String> = name.split('-').map(str::to_owned).collect();
    // `claude-opus-4-5` reads "4.5": a one- or two-digit part after the version joins it.
    if lower.starts_with("claude-") && parts.len() > 3 && parts[2].chars().all(|c| c.is_ascii_digit()) {
        let minor = parts[3].split('[').next().unwrap_or("").to_owned();
        if (1..=2).contains(&minor.len()) && minor.chars().all(|c| c.is_ascii_digit()) {
            let rest = parts.remove(3);
            parts[2] = format!("{}.{rest}", parts[2]);
        }
    }
    let words: Vec<String> = parts.iter().map(|p| { let mut c = p.chars(); c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default() }).collect();
    format!("{prefix}{}", words.join(" "))
}

/// `modelShortName` with the driver's native families (`resolveModelShortName`).
fn model_short_name(model: &crate::provider_instances::ServerProviderModel, driver: &str) -> Option<String> {
    if let Some(short) = model.short_name.clone().filter(|s| !s.is_empty()) {
        return Some(short);
    }
    const FAMILIES: [&str; 6] = ["Claude ", "GPT-", "GPT ", "Gemini ", "Muse ", "Grok "];
    const VARIANTS: [&str; 33] = [
        "pro", "flash", "mini", "max", "lite", "nano", "ultra", "plus", "fast", "turbo", "preview", "latest", "thinking", "reasoning", "build",
        "code", "image", "vision", "audio", "live", "custom", "tools", "exp", "experimental", "chat", "tts", "stt", "non", "transcribe", "", "", "", "",
    ];
    let is_version = |w: &str| {
        let w = w.strip_prefix(['v', 'V']).unwrap_or(w);
        let w = w.strip_suffix(|c: char| c.is_ascii_alphabetic()).unwrap_or(w);
        !w.is_empty() && w.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    };
    let words = |text: &str| -> Vec<String> {
        let mut clean = String::new();
        let mut depth = 0;
        for c in text.chars() {
            match c {
                '(' => depth += 1,
                ')' if depth > 0 => { depth -= 1; clean.push(' '); continue }
                _ => {}
            }
            clean.push(if depth > 0 { ' ' } else { c });
        }
        clean.split(|c: char| c.is_whitespace() || c == '-').filter(|w| !w.is_empty() && !is_version(w)).map(str::to_owned).collect()
    };
    let names_model = |text: &str| words(text).iter().any(|w| !VARIANTS.contains(&w.to_lowercase().as_str()));
    let natives = crate::components::settings::provider_driver_meta::driver_meta(driver).map(|m| m.model_families).unwrap_or(&[]);
    let name = model.name.as_str();
    // `VENDOR_PREFIX`: "Anthropic: Claude …".
    let unprefixed = name.split_once(": ").filter(|(vendor, rest)| {
        vendor.len() <= 25 && vendor.starts_with(|c: char| c.is_ascii_uppercase()) && !rest.starts_with(char::is_whitespace)
    }).map_or(name, |(_, rest)| rest);
    let without_family = FAMILIES.iter().find(|p| unprefixed.starts_with(*p)).and_then(|prefix| {
        let rest = unprefixed[prefix.len()..].trim();
        let keep = if natives.contains(prefix) { !words(rest).is_empty() } else { names_model(rest) };
        keep.then(|| if rest.contains(char::is_whitespace) { rest.to_owned() } else { rest.replace('-', " ") })
    });
    let short = without_family.unwrap_or_else(|| if names_model(unprefixed) { unprefixed.to_owned() } else { name.to_owned() });
    (short != name).then_some(short)
}

/// The option value of the first of `ids` set on a selection.
fn option_text(selection: &ModelSelection, ids: &[&str]) -> Option<String> {
    use t3code_contracts::{ProviderOptionSelectionValue as V, ProviderOptionSelections as O};
    match selection.options.as_ref()? {
        O::List(list) => ids.iter().find_map(|id| list.iter().find(|o| o.id == *id)).and_then(|o| match &o.value {
            V::Text(text) if !text.is_empty() => Some(text.clone()),
            _ => None,
        }),
        O::Map(map) => ids.iter().find_map(|id| map.get(*id)?.as_str().filter(|t| !t.is_empty()).map(str::to_owned)),
    }
}

/// `REASONING_EFFORT_OPTION_IDS`.
const EFFORT_OPTION_IDS: [&str; 6] = ["reasoningEffort", "effort", "reasoning", "variant", "reasoning_effort", "thinking_level"];

/// The row's driver mark (`ThreadRelationshipIcon` with a driver).
fn mark(driver: &str, provider: Option<&ServerProvider>, entries: &[ProviderInstanceEntry]) -> ProviderMark {
    let entry = provider.and_then(|p| entries.iter().find(|e| e.instance_id == p.instance_id));
    ProviderMark {
        driver: SharedString::from(driver.to_owned()),
        display_name: entry
            .map(|e| e.display_name.clone())
            .or_else(|| provider.and_then(|p| p.display_name.clone()))
            .unwrap_or_else(|| driver.to_owned())
            .into(),
        accent: entry.and_then(|e| e.accent_color.clone()).map(Into::into),
        show_badge: entry.is_some_and(|e| should_show_instance_badge(e, entries)),
    }
}

/// What `SubagentTooltipContent` reads from the subagent and its threads.
struct CardSource<'a> {
    title: SharedString,
    model: Option<&'a str>,
    instance_id: &'a str,
    selection: Option<ModelSelection>,
    provider: Option<&'a ServerProvider>,
    driver: Option<&'a str>,
    status: ItemStatus,
    clock: AgentClock,
    result: Option<&'a str>,
    progress: Option<&'a str>,
    parent: Option<(&'a ProjectId, Option<&'a str>)>,
    child: Option<&'a OrchestrationV2ThreadShell>,
    parent_project: Option<&'a OrchestrationProjectShell>,
    child_project: Option<&'a OrchestrationProjectShell>,
}

/// `SubagentTooltipContent` with `resolveSubagentMetadata` and `subagentDetailPreview`.
fn card(source: CardSource<'_>, entries: &[ProviderInstanceEntry]) -> SubagentCard {
    let selection = source.selection.as_ref();
    let model = source.model.map(str::trim).filter(|m| !m.is_empty()).or_else(|| selection.map(|s| s.model.trim()).filter(|m| !m.is_empty()));
    let models = source.provider.map(|p| p.models.as_slice()).unwrap_or_default();
    let slug = match source.provider {
        Some(provider) => resolve_selectable_model(&provider.driver, model, models),
        None => model,
    };
    let catalog = models.iter().find(|m| Some(m.slug.as_str()) == slug);
    let reported = match catalog {
        Some(m) => model_short_name(m, source.provider.map_or("", |p| p.driver.as_str())).unwrap_or_else(|| m.name.clone()),
        None => model.map(format_model_slug_name).unwrap_or_else(|| t!("Not reported").to_string()),
    };
    let label = match catalog.and_then(|m| m.sub_provider.as_deref()).map(str::trim).filter(|q| !q.is_empty()) {
        Some(qualifier) if reported.len() > qualifier.len() && reported[..qualifier.len()].eq_ignore_ascii_case(qualifier) => {
            let rest = reported[qualifier.len()..].trim_start_matches(|c: char| c.is_whitespace() || ".:/-".contains(c));
            let stripped = rest.len() < reported.len() - qualifier.len();
            if stripped && !rest.trim().is_empty() { rest.trim().to_owned() } else { reported.clone() }
        }
        _ => reported.clone(),
    };
    let child_model = selection.map(|s| match source.provider {
        Some(p) => resolve_selectable_model(&p.driver, Some(&s.model), models).unwrap_or(s.model.trim()).to_owned(),
        None => s.model.trim().to_owned(),
    });
    let matching = selection.filter(|s| child_model.as_deref() == slug.or(model) && s.instance_id == source.instance_id);
    let effort = matching.and_then(|s| option_text(s, &EFFORT_OPTION_IDS)).map(SharedString::from);
    let driver = source.provider.map(|p| p.driver.as_str()).or(source.driver);
    let mark = driver.map(|d| mark(d, source.provider, entries));
    let model_line = match mark.as_ref().filter(|m| m.show_badge) {
        Some(m) => format!("{label} · {}", m.display_name),
        None => label,
    };
    let mut metadata = Vec::new();
    let project_row = source.parent.zip(source.child_project).filter(|((parent, _), child)| &child.id != *parent);
    if let Some((_, child)) = project_row {
        metadata.push((SharedString::from(child.title.clone()), false));
    }
    let parent_ws = source.parent.and_then(|(_, w)| w).or(source.parent_project.map(|p| p.workspace_root.as_str()));
    let child_ws = source.child.and_then(|c| c.worktree_path.as_deref()).or(source.child_project.map(|p| p.workspace_root.as_str()));
    if let (Some(parent_ws), Some(child_ws)) = (parent_ws, child_ws)
        && parent_ws != child_ws
    {
        let branch = source.child.and_then(|c| c.branch.clone());
        let value = branch.clone().unwrap_or_else(|| crate::file_path_display::file_basename(child_ws).to_owned());
        metadata.push((value.into(), false));
    }
    // A branch child marks every row but a leading project row with the branch icon.
    let has_branch = source.child.is_some_and(|c| c.branch.is_some());
    for (index, row) in metadata.iter_mut().enumerate() {
        row.1 = has_branch && !(project_row.is_some() && index == 0);
    }
    SubagentCard {
        title: source.title,
        mark,
        model: model_line.into(),
        effort,
        status: source.status,
        clock: source.clock,
        metadata,
        preview: detail_preview(source.status, source.result, source.progress).map(Into::into),
    }
}

/// `subagentDetailPreview`: the result once finished (else progress), whitespace folded, 280 chars.
fn detail_preview(status: ItemStatus, result: Option<&str>, progress: Option<&str>) -> Option<String> {
    let terminal = t3code_client_runtime::state::subagent_runtime::is_terminal_subagent_status(status);
    let (preferred, fallback) = if terminal { (result, progress) } else { (progress, result) };
    let detail = preferred.map(str::trim_start).filter(|d| !d.is_empty()).or(fallback.map(str::trim_start)).unwrap_or("");
    let compact = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return None;
    }
    if compact.chars().count() <= 280 {
        return Some(compact);
    }
    let cut: String = compact.chars().take(280).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// `phaseElapsed`.
fn phase_clock(phase: &WorkflowPhaseGroup) -> AgentClock {
    let running = phase.state == WorkflowPhaseState::Running;
    let mut starts: Vec<i64> = phase.members.iter().filter_map(|m| m.started_at.as_deref().and_then(parse_instant_ms)).collect();
    let mut ends: Vec<i64> = phase.members.iter().filter_map(|m| m.completed_at.as_deref().and_then(parse_instant_ms)).collect();
    starts.sort_unstable();
    ends.sort_unstable();
    let completed = if running { None } else { ends.last().copied() };
    AgentClock { live: running, started_at: (running || completed.is_some()).then(|| starts.first().copied()).flatten(), completed_at: completed }
}

/// `phaseStatusLabel`.
fn phase_status(phase: &WorkflowPhaseGroup) -> PhaseStatus {
    if phase.state == WorkflowPhaseState::Running {
        PhaseStatus::Running
    } else if phase.members.iter().any(|m| m.status == ItemStatus::Failed) {
        PhaseStatus::Failed
    } else if phase.members.iter().any(|m| matches!(m.status, ItemStatus::Cancelled | ItemStatus::Interrupted)) {
        PhaseStatus::Stopped
    } else if phase.state == WorkflowPhaseState::Done {
        PhaseStatus::Done
    } else {
        PhaseStatus::NotStarted
    }
}

/// Derives the sections' rows, or `None` when neither section has anything to show.
pub fn derive(sources: RelationshipSources<'_>) -> Option<RelationshipsInput> {
    let current = sources.thread_id;
    let projection = sources.projection;
    let entries = sources.provider_entries;
    let graph = derive_thread_relationship_graph(sources.shells.iter().copied(), projection);
    let current_shell = sources.shells.iter().find(|s| &s.id == current).copied();
    let listed = |thread: &ThreadId| sources.shells[..sources.live_shells.min(sources.shells.len())].iter().any(|s| &s.id == thread);
    let merge_target = resolve_merge_back_target_thread_id(projection);
    let latest_merge_run = projection.and_then(|p| resolve_latest_merge_back_run(&p.runs)).map(|r| r.id.clone());
    let runtime = projection.map(|p| projected_subagents_to_runtime(p.subagents.iter().map(|a| &**a))).unwrap_or_default();
    let workflows = derive_workflow_groups(&runtime, &|n| t!("Phase {number}", number = n as i64).to_string());
    let agent_by_thread = |thread: &ThreadId| projection.and_then(|p| p.subagents.iter().find(|s| s.child_thread_id.as_ref() == Some(thread)));
    let side_chat_of = |thread: &ThreadId| graph.nodes.get(thread).and_then(|n| n.thread.as_ref()).is_some_and(|t| t.lineage.side_chat == Some(true));
    let provider_by_instance = |instance: &str| sources.providers.iter().find(|p| p.instance_id == instance);
    let project_by_id = |id: &ProjectId| sources.projects.iter().copied().find(|p| &p.id == id);
    let parent_thread = projection.map(|p| (&p.thread.project_id, p.thread.worktree_path.as_deref()));
    let parent_project = projection.and_then(|p| project_by_id(&p.thread.project_id));

    let ordered = order_web_thread_lineage_rows(&graph, &immediate_thread_relationships(&graph, current), current, merge_target.as_ref());
    let rows: Vec<ThreadRelationshipWalkRow> = ordered
        .into_iter()
        // The graph rebuilds missing parents from lineage, so hidden ones are filtered here.
        .filter(|row| !(sources.is_hidden)(&row.thread_id))
        .filter(|row| {
            let missing = graph.nodes.get(&row.thread_id).is_some_and(|n| n.missing);
            let unloaded_child = row.edge.kind == ThreadRelationshipKind::Subagent && !is_parent_thread_relationship(&row.edge, current);
            !(sources.hiding_anywhere && missing && !unloaded_child)
        })
        .map(|mut row| {
            // A workflow coordinator's row shows the workflow's status.
            if row.edge.kind == ThreadRelationshipKind::Subagent && !is_parent_thread_relationship(&row.edge, current)
                && let Some(agent) = agent_by_thread(&row.thread_id)
                && let Some(group) = workflows.iter().find(|g| g.workflow.id == agent.id.as_str())
            {
                row.edge.status = Some(RelationshipStatus::from_wire(group.workflow.status.as_str()));
            }
            row
        })
        .collect();
    // Omitted and archived threads stay reachable; only a settled lookup proves one is gone.
    let unlisted: Vec<ThreadId> = rows
        .iter()
        .filter(|row| !listed(&row.thread_id) && (row.edge.kind != ThreadRelationshipKind::Subagent || agent_by_thread(&row.thread_id).and_then(|a| a.title.as_ref()).is_none()))
        .map(|row| row.thread_id.clone())
        .collect();
    let rows: Vec<ThreadRelationshipWalkRow> = rows
        .into_iter()
        .filter(|row| row.edge.kind == ThreadRelationshipKind::Subagent || !unlisted.contains(&row.thread_id) || (sources.titles)(&row.thread_id) != TitleLookup::Absent)
        .collect();

    let to_row = |row: &ThreadRelationshipWalkRow| {
        let node = graph.nodes.get(&row.thread_id);
        let shell = node.and_then(|n| n.thread.as_deref());
        let is_parent = is_parent_thread_relationship(&row.edge, current);
        let is_subagent = row.edge.kind == ThreadRelationshipKind::Subagent;
        let side_chat = shell.is_some_and(|t| t.lineage.side_chat == Some(true));
        let agent = (is_subagent && !is_parent).then(|| agent_by_thread(&row.thread_id)).flatten();
        let workflow = agent.and_then(|a| workflows.iter().find(|g| g.workflow.id == a.id.as_str()));
        let resolved = match (sources.titles)(&row.thread_id) {
            TitleLookup::Found(title) => Some(title.to_string()),
            _ => None,
        };
        let title = workflow
            .and_then(|g| g.workflow.workflow_name.clone())
            .or(resolved)
            .or_else(|| shell.map(|t| t.title.clone()))
            .or_else(|| agent.and_then(|a| a.title.clone()))
            .unwrap_or_else(|| row.thread_id.to_string());
        let title: SharedString = if is_subagent { format_subagent_display_title(&title) } else { title }.into();
        let icon = if is_parent {
            RelationshipIcon::Parent
        } else if is_subagent {
            RelationshipIcon::Agent
        } else if side_chat {
            RelationshipIcon::SideChat
        } else {
            RelationshipIcon::Fork
        };
        let provider = provider_by_instance(agent.map_or_else(|| shell.map_or("", |t| t.provider_instance_id.as_str()), |a| a.provider_instance_id.as_str())).map(|p| &**p);
        let driver = agent.map(|a| a.driver.as_str()).or(provider.map(|p| p.driver.as_str()));
        let row_agent = agent.map(|agent| {
            let (status, started, completed, _) = if workflow.is_some() { (agent.status, agent.started_at.clone(), agent.completed_at.clone(), false) } else { current_status(agent, shell) };
            let reran = workflow.is_none() && (status, &started, &completed) != (agent.status, &agent.started_at, &agent.completed_at);
            let clock = AgentClock::of(status, started.as_deref(), completed.as_deref());
            let active = |s: ItemStatus| matches!(s, ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting);
            let native_stoppable = agent.origin == SubagentOrigin::ProviderNative
                && agent.driver.as_str() == "claudeAgent"
                && agent.run_id.is_some()
                && agent.native_task_ref.as_ref().is_some_and(|r| r.strength == t3code_contracts::OrchestrationV2NativeRefStrength::Strong && r.native_id.is_some())
                && active(agent.status);
            let can_stop = (agent.origin == SubagentOrigin::AppOwned || native_stoppable) && started.is_some() && active(status);
            let selection = agent.model_selection.clone().or_else(|| (agent.origin == SubagentOrigin::AppOwned).then(|| shell.map(|t| t.model_selection.clone())).flatten());
            let child_project = shell.and_then(|t| project_by_id(&t.project_id));
            RowAgent {
                subagent_id: agent.id.clone(),
                owner_thread_id: agent.thread_id.clone(),
                clock,
                can_stop,
                card: card(
                    CardSource {
                        title: title.clone(),
                        model: agent.model.as_deref(),
                        instance_id: agent.provider_instance_id.as_str(),
                        selection,
                        provider,
                        driver,
                        status,
                        clock,
                        result: if reran { None } else { agent.result.as_deref() },
                        progress: if reran { None } else { agent.progress.as_deref() },
                        parent: parent_thread,
                        child: shell,
                        parent_project,
                        child_project,
                    },
                    entries,
                ),
            }
        });
        let workflow_row = workflow.zip(agent).map(|(group, agent)| {
            let instance = agent.provider_instance_id.clone();
            let member = |m: &RuntimeSubagent| {
                let clock = AgentClock::of(m.status, m.started_at.as_deref(), m.completed_at.as_deref());
                WorkflowMember {
                    id: m.id.clone().into(),
                    title: m.title.clone().into(),
                    status: m.status,
                    thread_id: m.child_thread_id.clone(),
                    clock,
                    card: card(
                        CardSource {
                            title: m.title.clone().into(),
                            model: m.model.as_deref(),
                            instance_id: instance.as_str(),
                            selection: workflow_member_model_selection(m, instance.clone()),
                            provider,
                            driver,
                            status: m.status,
                            clock,
                            result: m.result.as_deref().or(m.error.as_deref()),
                            progress: m.progress.as_deref(),
                            parent: None,
                            child: None,
                            parent_project: None,
                            child_project: None,
                        },
                        entries,
                    ),
                }
            };
            WorkflowRow {
                coordinator: agent.id.clone(),
                label: group.workflow.workflow_name.clone().unwrap_or_else(|| group.workflow.title.clone()).into(),
                mark: driver.map(|d| mark(d, provider, entries)),
                can_stop: sources.can_operate_workflow && group.workflow.status == ItemStatus::Running && agent.origin == SubagentOrigin::ProviderNative && driver == Some("claudeAgent"),
                phases: group
                    .phases
                    .iter()
                    .filter(|p| !p.members.is_empty())
                    .map(|p| WorkflowPhaseRow {
                        index: p.index,
                        title: p.title.clone().into(),
                        running: p.state == WorkflowPhaseState::Running,
                        status: phase_status(p),
                        settled: p.settled_count,
                        members: p.members.iter().map(member).collect(),
                        clock: phase_clock(p),
                    })
                    .collect(),
                unphased: group.unphased_members.iter().map(member).collect(),
            }
        });
        // `ThreadRelationshipIcon` takes a driver only for child agents.
        let row_mark = (is_subagent && !is_parent).then(|| driver.map(|d| mark(d, provider, entries))).flatten();
        RelationshipRow {
            thread_id: row.thread_id.clone(),
            title,
            icon,
            status: thread_relationship_row_status(&graph, row),
            hint: hint(row.edge.kind, &row.edge.source_thread_id, current, side_chat && !is_parent),
            merge_target: Some(&row.thread_id) == merge_target.as_ref(),
            mark: row_mark,
            agent: row_agent,
            workflow: workflow_row,
            remote: None,
        }
    };

    let (mut related, mut active, mut previous, mut side_chats) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut running = 0;
    for row in &rows {
        let is_parent = is_parent_thread_relationship(&row.edge, current);
        if !is_parent && side_chat_of(&row.thread_id) {
            side_chats.push(to_row(row));
        } else if row.edge.kind != ThreadRelationshipKind::Subagent || is_parent {
            related.push(to_row(row));
        } else if row.edge.status.is_some_and(RelationshipStatus::is_finished_agent) {
            previous.push(to_row(row));
        } else {
            if row.edge.status == Some(RelationshipStatus::Running) {
                running += 1;
            }
            active.push(to_row(row));
        }
    }
    // Tasks on linked environments have no thread here.
    let (mut remote_active, mut remote_previous) = (Vec::new(), Vec::new());
    for agent in projection.map(|p| p.subagents.iter().filter(|a| a.remote_child.is_some()).collect::<Vec<_>>()).unwrap_or_default() {
        let remote = agent.remote_child.as_ref().expect("filtered above");
        let label: SharedString = remote.label.clone().into();
        let connected = (sources.connected_label)(&remote.environment_id).is_some();
        let provider = (sources.remote_provider)(&remote.environment_id, agent.provider_instance_id.as_str());
        let title: SharedString = format_subagent_display_title(agent.title.as_deref().unwrap_or(&t!("Subagent"))).into();
        let status = RelationshipStatus::from_wire(agent.status.as_str());
        let row = RelationshipRow {
            thread_id: remote.thread_id.clone(),
            hint: if connected { t!("Open on {label}", label = label.as_ref()) } else { t!("Runs on {label}. Connect to it to open the thread there.", label = label.as_ref()) },
            title,
            icon: RelationshipIcon::Agent,
            status: Some(status),
            merge_target: false,
            mark: provider.as_deref().map(|p| mark(&p.driver, Some(p), &[])),
            agent: None,
            workflow: None,
            remote: Some(RemoteRow { environment_id: remote.environment_id.clone(), label, connected, raw_status: agent.status }),
        };
        if status.is_finished_agent() { remote_previous.push(row) } else { remote_active.push(row) }
    }
    let running_count = running
        + projection.map_or(0, |p| {
            p.subagents.iter().filter(|a| a.child_thread_id.is_none() && a.status.as_str() == "running").count()
        });
    let can_detach = projection.is_some_and(can_detach_thread_provider_session);
    let delegated_from = current_shell.and_then(|s| s.delegated_from.clone()).map(|from| {
        let connected = (sources.connected_label)(&from.environment_id);
        let fallback = current_shell.and_then(|s| s.link_origin.as_ref()).map(|o| SharedString::from(o.label.clone()));
        DelegatedFromRow {
            // The title it had at delegation; the parent lives in the other environment's shell.
            title: from.title.clone().into(),
            label: connected.clone().or(fallback),
            connected: connected.is_some(),
            from,
        }
    });
    let lineage_visible = sources.show_lineage
        && (!related.is_empty()
            || !active.is_empty()
            || !previous.is_empty()
            || running_count > 0
            || !remote_active.is_empty()
            || !remote_previous.is_empty()
            || delegated_from.is_some()
            // Disconnect lives in this heading, so a thread with only side chats still shows it.
            || (can_detach && !side_chats.is_empty()));
    let side_chats_visible = sources.show_side_chats && !side_chats.is_empty();
    if !lineage_visible && !side_chats_visible {
        return None;
    }
    let running_side_chats = side_chats.iter().filter(|r| r.status == Some(RelationshipStatus::Running)).count();
    let show_heading = running_count > 0 || can_detach || !related.is_empty() || !active.is_empty() || !remote_active.is_empty() || delegated_from.is_some();
    let merge = merge_target.map(|target| MergeBack {
        parent_title: (!(sources.is_hidden)(&target))
            .then(|| graph.nodes.get(&target).and_then(|n| n.thread.as_ref()).map(|t| SharedString::from(t.title.clone())))
            .flatten(),
        target,
        run_id: latest_merge_run,
    });
    active.extend(remote_active);
    previous.extend(remote_previous);
    let groups = vec![
        RelationshipGroup { id: "related", label: None, rows: related, expanded: true },
        RelationshipGroup { id: "active", label: None, rows: active, expanded: true },
        RelationshipGroup { id: "previous", label: Some(t!("Previous agents")), rows: previous, expanded: false },
    ];
    Some(RelationshipsInput {
        lineage_visible,
        side_chats_visible,
        groups,
        running_side_chats,
        side_chats,
        running_count,
        show_heading,
        can_detach,
        merge,
        delegated_from,
        any_instance_badge: has_instance_badge(entries),
        stopping: None,
        unlisted,
    })
}
