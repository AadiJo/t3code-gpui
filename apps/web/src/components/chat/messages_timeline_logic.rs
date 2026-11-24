//! Timeline rows derived from a thread's entries. Ports the row model and
//! `deriveMessagesTimelineRows` of `MessagesTimeline.logic.ts`: consecutive
//! tool calls fold into a group header, a finished turn's work folds behind
//! "Worked for …", the running turn gets live rows (working, thinking, the
//! active tool), the terminal assistant message carries the meta row, and
//! adjacent user messages group.
//!
//! Not ported: tool-call visibility modes (zen), thread-created summaries,
//! deferred work loading, and the delegation-row filter, which depend on
//! runtime data the fixtures do not carry.

use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
};

use gpui_kit::{Pixels, SharedString, px};

use super::{
    timeline_entries::{
        AttemptId, ChatMessage, CompactionStatus, EntryKind, InputIntent, MessageRole, ProposedPlan, QueuedMessage,
        RunId, RunStatus, Subagent, Timestamp, TimelineEntry, TimelineEvent, TimelineInput, ToolStatus,
        TurnDiffSummary, WorkItem, WorkLogEntry, WorktreeSetup,
    },
    work_log_presentation::{
        SummaryKind, format_duration, indicates_failure, is_tool_like, is_visible_in_group, summarize_tool_group,
        summary_kind, tool_action, ToolAction,
    },
};

/// Which groups, folds and attempts the reader expanded.
#[derive(Clone, Debug, Default)]
pub struct TimelineExpansion {
    pub runs: HashSet<RunId>,
    pub attempts: HashSet<AttemptId>,
    pub work_groups: HashSet<SharedString>,
}

/// One timeline row.
#[derive(Clone, Debug)]
pub struct TimelineRow {
    pub id: SharedString,
    /// Hash of `id`: the virtual list's key for measured heights and scroll anchoring.
    pub key: u64,
    pub created_at: Option<Timestamp>,
    /// The next row is also work-log content, so this row adds no bottom space.
    pub continues_work_log: bool,
    pub kind: RowKind,
}

/// What a row shows. The first seventeen mirror the web client's row kinds;
/// the tool group's expanded list, pending approvals and the queued-message
/// footer are rows here too so one virtual list owns all scrolling.
#[derive(Clone, Debug)]
pub enum RowKind {
    WorktreeSetup(WorktreeSetup),
    /// A standalone work-log line: a single tool call, error, notice, todo list or answer.
    Work { entry: WorkLogEntry, display_label: Option<SharedString> },
    /// The expanded list under a tool group header (a nested virtual list).
    WorkGroupDetails { group_id: SharedString, entries: Arc<[WorkLogEntry]> },
    WorkLive {
        entry: WorkLogEntry,
        entries: Arc<[WorkLogEntry]>,
        group_id: SharedString,
        expanded: bool,
        active: bool,
        thought: Option<WorkLogEntry>,
    },
    Working { started_at: Option<Timestamp>, compacting: bool },
    Thinking { label: Option<SharedString> },
    WorkToggle {
        group_id: SharedString,
        run_id: Option<RunId>,
        count: usize,
        expanded: bool,
        summary: SharedString,
        summary_kind: SummaryKind,
        has_failure: bool,
    },
    ViewedImages { paths: Arc<[SharedString]> },
    TurnFold { run_id: RunId, label: SharedString, expanded: bool },
    AttemptFold { attempt_id: AttemptId, label: SharedString, expanded: bool },
    ContextCompaction { label: SharedString, tokens: Option<(u64, u64)>, active: bool, failed: bool },
    Message(MessageRow),
    AssistantMeta { message: ChatMessage },
    Event { event: TimelineEvent, subagents: Arc<[Subagent]> },
    ProposedPlan(ProposedPlan),
    HtmlRender { title: SharedString },
    SentMessage { text: SharedString },
    McpApp { title: SharedString },
    PendingApproval { entry: WorkLogEntry },
    QueuedMessages { messages: Arc<[QueuedMessage]> },
}

/// A user or assistant message row.
#[derive(Clone, Debug)]
pub struct MessageRow {
    pub message: ChatMessage,
    /// The assistant reply ends its turn: show timestamp and actions under it.
    pub show_meta: bool,
    /// Copy waits until the reply finished streaming.
    pub copy_streaming: bool,
    pub diff_summary: Option<TurnDiffSummary>,
    /// Turns "Edit from here" reverts, when rollback is available.
    pub revert_turn_count: Option<usize>,
    pub grouped_with_next: bool,
    pub grouped_with_previous: bool,
    pub continues_steer: bool,
}

impl RowKind {
    /// Size estimate for a row that has not been measured
    /// (`MessagesTimeline.tsx:1636-1644`: by kind, messages by text length).
    pub fn estimate(&self) -> Pixels {
        match self {
            Self::Message(row) => {
                let lines = (row.message.text.len() as f32 / 90.).ceil().max(1.);
                let base = if row.message.role == MessageRole::User { 56. } else { 16. };
                px(base + lines * 22.75 + if row.show_meta { 30. } else { 0. })
            }
            Self::Work { .. } | Self::WorkToggle { .. } | Self::WorkLive { .. } | Self::Thinking { .. } => px(28.),
            Self::WorkGroupDetails { entries, .. } => px((entries.len() as f32 * 24.).min(288.) + 8.),
            Self::Working { .. } | Self::TurnFold { .. } => px(39.),
            Self::AssistantMeta { .. } => px(26.),
            Self::ContextCompaction { .. } => px(26.),
            Self::Event { .. } => px(44.),
            Self::ProposedPlan(_) => px(240.),
            Self::QueuedMessages { messages } => px(messages.len() as f32 * 56. + 16.),
            _ => px(90.),
        }
    }

    /// Whether this row is work-log content (`timelineRowIsWorkLog`).
    fn is_work_log(&self) -> bool {
        matches!(self, Self::Work { .. } | Self::WorkToggle { .. } | Self::WorkLive { .. } | Self::Thinking { .. })
    }
}

fn row(id: impl Into<SharedString>, created_at: Option<Timestamp>, kind: RowKind) -> TimelineRow {
    let id = id.into();
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    TimelineRow { key: hasher.finish(), id, created_at, continues_work_log: false, kind }
}

fn group_id(entry_id: &str) -> SharedString {
    format!("work-group:{entry_id}").into()
}

fn is_steer(entry: &TimelineEntry) -> bool {
    matches!(&entry.kind, EntryKind::Message(m) if m.role == MessageRole::User && m.input_intent != InputIntent::Prompt)
}

/// A user prompt (not a steer) or a notification starts a response (`timelineEntryStartsResponse`).
fn starts_response(entry: &TimelineEntry) -> bool {
    match &entry.kind {
        EntryKind::Message(m) => m.role == MessageRole::User && !is_steer(entry),
        EntryKind::Work(w) => matches!(w.item, WorkItem::Notification { .. }),
        _ => false,
    }
}

fn is_compaction(entry: &TimelineEntry) -> bool {
    matches!(&entry.kind, EntryKind::Work(w) if matches!(w.item, WorkItem::Compaction { .. }))
}

/// Work entries that always stand alone instead of joining a tool group.
fn stands_alone(work: &WorkLogEntry, pending: &HashSet<&str>) -> bool {
    if let Some(source) = &work.source {
        use crate::work_entry_labels::{SourceActivityKind, Tone};
        use t3code_contracts::OrchestrationV2TurnItem as Item;
        let facts = &source.facts;
        return is_pending_approval(work, pending)
            || matches!(&*facts.item, Item::Error(_)) && work.status == ToolStatus::Failed
            || facts.tone == Tone::Error
            || facts.source_activity_kind == Some(SourceActivityKind::RuntimeError)
            || matches!(&*facts.item, Item::SystemNotice(_) | Item::Notification(_) | Item::TodoList(_))
            || has_question_answer(work);
    }
    matches!(
        work.item,
        WorkItem::Error { .. }
            | WorkItem::SystemNotice { .. }
            | WorkItem::Notification { .. }
            | WorkItem::TodoList { .. }
            | WorkItem::QuestionAnswer { .. }
    ) || is_pending_approval(work, pending)
}

fn is_pending_approval(work: &WorkLogEntry, pending: &HashSet<&str>) -> bool {
    matches!(&work.item, WorkItem::Approval { request_id, .. } if pending.contains(request_id.as_str()))
}

/// The last assistant message of each run (`deriveTerminalAssistantMessageIds`).
fn terminal_assistant_ids(entries: &[TimelineEntry]) -> HashSet<SharedString> {
    let mut last_by_run: HashMap<Option<RunId>, SharedString> = HashMap::new();
    for entry in entries {
        if let EntryKind::Message(m) = &entry.kind
            && m.role == MessageRole::Assistant
        {
            last_by_run.insert(m.run_id.clone(), m.id.0.clone());
        }
    }
    last_by_run.into_values().collect()
}

/// The run still in flight (`deriveUnsettledRunId`).
fn unsettled_run_id(latest: Option<&crate::components::chat::timeline_entries::LatestRun>) -> Option<RunId> {
    let latest = latest?;
    let settled = latest.completed_at.is_some()
        && !matches!(latest.status, RunStatus::Running | RunStatus::Starting | RunStatus::Waiting);
    (!settled).then(|| latest.run_id.clone())
}

/// A completed turn's hidden work, folded behind one row.
struct TurnFold {
    run_id: RunId,
    created_at: Timestamp,
    hidden: HashSet<SharedString>,
    label: SharedString,
}

/// Whether a question row carries its answer (`entry.questionAnswer !== undefined`).
fn has_question_answer(entry: &WorkLogEntry) -> bool {
    match &entry.source {
        Some(source) => matches!(&*source.facts.item, t3code_contracts::OrchestrationV2TurnItem::UserInputRequest(r) if r.question_answer.is_some()),
        None => matches!(&entry.item, WorkItem::QuestionAnswer { pairs } if !pairs.is_empty()),
    }
}

/// Fork cards stay answerable and visible inside folds (`timelineEntryIsPersistentResourceCard`).
fn is_persistent_card(entry: &TimelineEntry) -> bool {
    matches!(&entry.kind, EntryKind::Event(TimelineEvent::Divider(d)) if d.kind == super::timeline_entries::DividerKind::Forked)
}

/// Subagent cards of a batch with a live child stay visible (`liveSubagentCardEntryIds`).
fn live_subagent_entry_ids(entries: &[TimelineEntry]) -> HashSet<SharedString> {
    let mut visible = HashSet::new();
    let mut batch: Vec<(&TimelineEntry, &super::timeline_entries::Subagent)> = Vec::new();
    let mut flush = |batch: &mut Vec<(&TimelineEntry, &super::timeline_entries::Subagent)>| {
        if batch.iter().any(|(_, s)| s.status.is_live()) {
            visible.extend(batch.iter().map(|(e, _)| e.id.0.clone()));
        }
        batch.clear();
    };
    for entry in entries {
        let EntryKind::Event(TimelineEvent::Subagent(subagent)) = &entry.kind else {
            flush(&mut batch);
            continue;
        };
        if let Some((_, previous)) = batch.last()
            && (previous.run_id != subagent.run_id || previous.provider_turn_id != subagent.provider_turn_id)
        {
            flush(&mut batch);
        }
        batch.push((entry, subagent));
    }
    flush(&mut batch);
    visible
}

/// Final answers always stay in view (`retainedAssistantReplyEntryIds`, the phase rule).
fn retained_reply_ids(input: &TimelineInput) -> HashSet<SharedString> {
    use t3code_contracts::{OrchestrationV2TurnItem as Item, OrchestrationV2TurnItemAssistantMessagePhase as Phase};
    input
        .entries
        .iter()
        .filter(|entry| {
            input.item_sources.get(entry.id.as_str()).is_some_and(|item| matches!(&**item, Item::AssistantMessage(m) if m.phase == Some(Phase::FinalAnswer)))
        })
        .map(|entry| entry.id.0.clone())
        .collect()
}

/// The run an entry folds under, or the runless key of its prompt (`timelineEntryFoldRunId`).
fn fold_run_id(entry: &TimelineEntry, runless: Option<&RunId>) -> Option<RunId> {
    match &entry.kind {
        EntryKind::Work(w) if matches!(w.item, WorkItem::SystemNotice { .. }) => None,
        EntryKind::Message(m) if m.role == MessageRole::Assistant => m.run_id.clone().or_else(|| runless.cloned()),
        EntryKind::Work(w) => w.run_id.clone().or_else(|| runless.cloned()),
        EntryKind::Event(TimelineEvent::Subagent(s)) => s.run_id.clone().or_else(|| runless.cloned()),
        EntryKind::Event(event @ TimelineEvent::Divider(_)) if is_persistent_card(entry) => event.run_id().cloned().or_else(|| runless.cloned()),
        _ => None,
    }
}

/// `responseBoundaryRunId`.
fn boundary_run_id(entry: &TimelineEntry) -> Option<RunId> {
    match &entry.kind {
        EntryKind::Message(m) => m.run_id.clone(),
        EntryKind::Work(w) => w.run_id.clone(),
        _ => None,
    }
}

/// A top-level failed provider error (`timelineEntryFailedItem`).
fn is_failed_error(entry: &TimelineEntry) -> bool {
    match &entry.kind {
        EntryKind::Work(w) => match &w.source {
            Some(source) => matches!(&*source.facts.item, t3code_contracts::OrchestrationV2TurnItem::Error(e)
                if e.status == t3code_contracts::OrchestrationV2TurnItemStatus::Failed && e.parent_item_id.is_none()),
            None => matches!(w.item, WorkItem::Error { .. }) && w.status == ToolStatus::Failed,
        },
        _ => false,
    }
}

/// Folds each settled turn's work behind "Worked for …", and the work after each steer behind
/// "Continued for …" (`deriveTurnFolds`).
fn derive_turn_folds(
    entries: &[TimelineEntry],
    terminal_ids: &HashSet<SharedString>,
    input: &TimelineInput,
    unfolded: &HashSet<RunId>,
) -> HashMap<SharedString, TurnFold> {
    struct Group<'a> {
        entries: Vec<&'a TimelineEntry>,
        terminal: Option<&'a TimelineEntry>,
        streaming: bool,
        start: Option<Timestamp>,
        anchor: SharedString,
        steers: Vec<(usize, &'a TimelineEntry)>,
    }
    let mut interrupted: HashSet<RunId> = HashSet::new();
    let mut prompt_index: HashMap<RunId, usize> = HashMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if starts_response(entry)
            && let Some(run) = boundary_run_id(entry)
        {
            prompt_index.entry(run).or_insert(index);
        }
        if let EntryKind::Event(TimelineEvent::Divider(d)) = &entry.kind
            && d.kind == super::timeline_entries::DividerKind::RunInterrupted
            && let Some(run) = &d.run_id
        {
            interrupted.insert(run.clone());
        }
    }
    let live_subagents = live_subagent_entry_ids(entries);
    let retained = retained_reply_ids(input);
    let mut order: Vec<RunId> = Vec::new();
    let mut groups: HashMap<RunId, Group> = HashMap::new();
    let mut runless_failed: HashSet<RunId> = HashSet::new();
    let mut runless_key: Option<RunId> = None;
    let mut response_key: Option<RunId> = None;
    let mut pending_boundary: Option<(Timestamp, SharedString)> = None;
    for (index, entry) in entries.iter().enumerate() {
        if starts_response(entry) {
            pending_boundary = entries.get(index + 1).map(|next| (entry.created_at, next.id.0.clone()));
            let boundary = boundary_run_id(entry);
            runless_key = boundary.is_none().then(|| RunId::new(format!("runless:{}", entry.id.as_str())));
            response_key = boundary.or_else(|| runless_key.clone());
            continue;
        }
        if is_steer(entry) {
            let steer_run = match &entry.kind {
                EntryKind::Message(m) => m.run_id.clone().or_else(|| response_key.clone()),
                _ => None,
            };
            if let Some(group) = steer_run.and_then(|run| groups.get_mut(&run))
                && !group.entries.is_empty()
            {
                // Back-to-back steers split once, after the last of them.
                if group.steers.last().is_some_and(|(start, _)| *start == group.entries.len()) {
                    group.steers.pop();
                }
                group.steers.push((group.entries.len(), entry));
            }
            continue;
        }
        let Some(own) = fold_run_id(entry, runless_key.as_ref()) else { continue };
        // Resumed subagents keep their position and fold with their original turn.
        let run = match &response_key {
            Some(key) if prompt_index.get(&own).is_some_and(|&p| p > index) => key.clone(),
            _ => own,
        };
        if Some(&run) == runless_key.as_ref() && is_failed_error(entry) {
            runless_failed.insert(run.clone());
        }
        let group = groups.entry(run.clone()).or_insert_with(|| {
            order.push(run.clone());
            let (start, anchor) = pending_boundary.take().map_or((None, entry.id.0.clone()), |(t, a)| (Some(t), a));
            Group { entries: Vec::new(), terminal: None, streaming: false, start, anchor, steers: Vec::new() }
        });
        group.entries.push(entry);
        if let EntryKind::Message(m) = &entry.kind {
            if terminal_ids.contains(&m.id.0) {
                group.terminal = Some(entry);
            }
            group.streaming |= m.streaming;
        }
    }

    let mut folds = HashMap::new();
    for run in order {
        let group = &groups[&run];
        if unfolded.contains(&run) || interrupted.contains(&run) || runless_failed.contains(&run) || group.streaming {
            continue;
        }
        let terminal_index =
            group.terminal.and_then(|t| group.entries.iter().position(|e| e.id == t.id)).unwrap_or(group.entries.len());
        // Folded dividers lose their place in the turn, so only the latest stays visible.
        let visible_compaction = group.entries.iter().rev().find(|e| is_compaction(e)).map(|e| e.id.0.clone());
        let mut hidden = HashSet::new();
        for (index, entry) in group.entries.iter().enumerate() {
            if group.terminal.is_some_and(|t| t.id == entry.id) {
                continue;
            }
            let foldable_trailing =
                matches!(&entry.kind, EntryKind::Work(w) if w.status != ToolStatus::InProgress && !indicates_failure(w));
            if !is_compaction(entry) && index > terminal_index && !foldable_trailing {
                continue;
            }
            if Some(&entry.id.0) == visible_compaction.as_ref()
                || is_persistent_card(entry)
                || live_subagents.contains(&entry.id.0)
                || retained.contains(&entry.id.0)
            {
                continue;
            }
            // Notifications and answered questions stay in view; an open question folds.
            if matches!(&entry.kind, EntryKind::Work(w)
                if matches!(w.item, WorkItem::Notification { .. }) || has_question_answer(w))
            {
                continue;
            }
            hidden.insert(entry.id.0.clone());
        }
        let hides_work = |list: &[&TimelineEntry]| list.iter().any(|e| hidden.contains(&e.id.0) && !is_compaction(e));
        if !hides_work(&group.entries) {
            continue;
        }
        let first = group.entries[0];
        // A subagent card after the terminal reply was spawned once the turn settled; it adds no time.
        let late_subagent = |index: usize, entry: &TimelineEntry| {
            index > terminal_index && matches!(entry.kind, EntryKind::Event(TimelineEvent::Subagent(_)))
        };
        let last_turn = group.entries.iter().enumerate().rev().find(|(i, e)| !late_subagent(*i, e)).map_or(group.entries[group.entries.len() - 1], |(_, e)| *e);
        let start = group.start.unwrap_or(first.created_at);
        let end = group.terminal.map_or(last_turn.end(), |t| t.end().max(last_turn.end()));
        let elapsed = match &input.latest_run {
            Some(latest) if latest.run_id == run => match (latest.started_at, latest.completed_at) {
                (Some(s), Some(c)) => c.millis_since(s).max(0),
                _ => end.millis_since(start).max(0),
            },
            _ => end.millis_since(start).max(0),
        };
        let stopped = input.latest_run.as_ref().is_some_and(|l| l.run_id == run && l.status == RunStatus::Interrupted);
        let duration = format_duration(elapsed);
        let label = if stopped {
            crate::i18n::t!("You stopped after {duration}", duration = duration)
        } else {
            crate::i18n::t!("Worked for {duration}", duration = duration)
        };
        let segment_end = group.steers.first().map_or(group.entries.len(), |(start, _)| *start);
        let segment_hidden = |list: &[&TimelineEntry]| list.iter().filter(|e| hidden.contains(&e.id.0)).map(|e| e.id.0.clone()).collect();
        folds.insert(
            group.anchor.clone(),
            TurnFold { run_id: run.clone(), created_at: start, hidden: segment_hidden(&group.entries[..segment_end]), label },
        );
        // The top row keeps the whole turn's time; work after each steer folds on its own.
        for (index, (start_index, steer)) in group.steers.iter().enumerate() {
            let end_index = group.steers.get(index + 1).map_or(group.entries.len(), |(s, _)| *s);
            let segment = &group.entries[*start_index..end_index];
            let Some(anchor) = segment.first() else { continue };
            if !hides_work(segment) {
                continue;
            }
            let segment_end = segment
                .iter()
                .enumerate()
                .filter(|(i, e)| !late_subagent(start_index + i, e))
                .map(|(_, e)| e.end())
                .max()
                .unwrap_or(steer.created_at);
            let duration = format_duration(segment_end.millis_since(steer.created_at).max(0));
            folds.insert(
                anchor.id.0.clone(),
                TurnFold {
                    run_id: RunId::new(format!("{}:steer:{}", run.as_str(), steer.id.as_str())),
                    created_at: steer.created_at,
                    hidden: segment_hidden(segment),
                    label: crate::i18n::t!("Continued for {duration}", duration = duration),
                },
            );
        }
    }
    folds
}

/// Derives the rows of a thread timeline.
pub fn derive_timeline_rows(input: &TimelineInput, expansion: &TimelineExpansion) -> Vec<TimelineRow> {
    let entries: &[TimelineEntry] = &input.entries;
    let pending: HashSet<&str> = input.pending_approval_ids.iter().map(|id| id.as_str()).collect();
    let unsettled = unsettled_run_id(input.latest_run.as_ref());
    let terminal_ids = terminal_assistant_ids(entries);
    let diff_by_message: HashMap<&str, &TurnDiffSummary> =
        input.turn_diff_summaries.iter().map(|s| (s.assistant_message_id.as_str(), s)).collect();

    let response_boundary = entries.iter().rposition(starts_response);
    let mut active_runs: HashSet<RunId> = HashSet::new();
    if let Some(run) = &unsettled {
        active_runs.insert(run.clone());
        if input.is_working {
            let from = response_boundary.map_or(0, |i| i + 1);
            active_runs.extend(entries[from..].iter().filter_map(|e| e.run_id().cloned()));
        }
    }
    let mut failed_runs: HashSet<RunId> = HashSet::new();
    if let Some(latest) = &input.latest_run
        && latest.status == RunStatus::Failed
    {
        failed_runs.insert(latest.run_id.clone());
    }
    // A run with a failed error item stays unfolded (`failedTimelineRunIds`).
    for entry in entries {
        if let EntryKind::Work(w) = &entry.kind
            && matches!(w.item, WorkItem::Error { .. })
            && let Some(run) = &w.run_id
        {
            failed_runs.insert(run.clone());
        }
    }
    let unfolded: HashSet<RunId> = active_runs.union(&failed_runs).cloned().collect();
    let folds = derive_turn_folds(entries, &terminal_ids, input, &unfolded);
    let mut collapsed: HashSet<SharedString> = HashSet::new();
    for fold in folds.values() {
        if !expansion.runs.contains(&fold.run_id) {
            collapsed.extend(fold.hidden.iter().cloned());
        }
    }
    // Superseded attempts fold behind their own row.
    let mut attempt_anchor: HashMap<SharedString, (AttemptId, Timestamp)> = HashMap::new();
    let mut attempt_hidden: HashSet<SharedString> = HashSet::new();
    let mut seen_attempts: HashSet<AttemptId> = HashSet::new();
    for entry in entries {
        if let EntryKind::Work(w) = &entry.kind
            && let Some(attempt) = &w.superseded_attempt
        {
            if seen_attempts.insert(attempt.clone()) {
                attempt_anchor.insert(entry.id.0.clone(), (attempt.clone(), entry.created_at));
            }
            if !expansion.attempts.contains(attempt) {
                attempt_hidden.insert(entry.id.0.clone());
            }
        }
    }

    let work_in_active_run = |w: &WorkLogEntry| {
        input.is_working && w.status == ToolStatus::InProgress && w.run_id.is_some() && w.run_id == unsettled
    };
    let active_header_index = if input.is_working { response_boundary.map_or(0, |i| i + 1) } else { entries.len() };

    // The running turn's trailing tool calls become one live row.
    let mut active_tools: Vec<&TimelineEntry> = Vec::new();
    if input.is_working && unsettled.is_some() {
        for entry in entries[active_header_index.min(entries.len())..].iter().rev() {
            let EntryKind::Work(w) = &entry.kind else { break };
            if stands_alone(w, &pending)
                || is_compaction(entry)
                || w.run_id.as_ref().is_none_or(|r| !active_runs.contains(r))
                || collapsed.contains(&entry.id.0)
            {
                break;
            }
            active_tools.push(entry);
        }
        active_tools.reverse();
    }
    let visible_active: Vec<&WorkLogEntry> = active_tools
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Work(w) if is_visible_in_group(w) => Some(w),
            _ => None,
        })
        .collect();
    let latest_running = visible_active.iter().rev().find(|w| w.status == ToolStatus::InProgress).copied();
    let latest_visible = visible_active.last().copied();
    let latest_failed = latest_running.is_none()
        && latest_visible.is_some_and(|w| w.status != ToolStatus::Declined && indicates_failure(w));
    let keeps_live = latest_running.is_some()
        || (latest_visible.is_some_and(|w| w.status == ToolStatus::Completed) && pending.is_empty());
    let active_anchor = active_tools.first().copied();
    let active_row = match (active_anchor, latest_visible) {
        (Some(anchor), Some(latest)) if !latest_failed => {
            let group = group_id(anchor.id.as_str());
            let thought = visible_active
                .iter()
                .rev()
                .find(|w| matches!(&w.item, WorkItem::Reasoning { text } if !text.trim().is_empty()))
                .map(|w| (*w).clone());
            Some(row(
                if keeps_live { "live-activity-row".to_owned() } else { format!("work-live:{}", anchor.id.as_str()) },
                Some(anchor.created_at),
                RowKind::WorkLive {
                    entry: latest_running.unwrap_or(latest).clone(),
                    entries: visible_active.iter().map(|w| (*w).clone()).collect(),
                    expanded: expansion.work_groups.contains(&group),
                    group_id: group,
                    active: keeps_live,
                    thought,
                },
            ))
        }
        _ => None,
    };
    let active_ids: HashSet<&str> = if active_row.is_some() || latest_failed {
        active_tools.iter().map(|e| e.id.as_str()).collect()
    } else {
        HashSet::new()
    };
    let active_placement = active_tools
        .iter()
        .rev()
        .find(|e| matches!(&e.kind, EntryKind::Work(w) if is_visible_in_group(w)))
        .map(|e| e.id.as_str());

    let mut rows: Vec<TimelineRow> = Vec::new();
    let mut has_activity_row = false;
    let push_working = |rows: &mut Vec<TimelineRow>| {
        rows.push(row(
            "working-indicator-row",
            input.active_turn_started_at,
            RowKind::Working { started_at: input.active_turn_started_at, compacting: false },
        ));
    };

    let mut index = 0;
    while index < entries.len() {
        let entry = &entries[index];
        if input.is_working && index == active_header_index {
            push_working(&mut rows);
        }
        if Some(entry.id.as_str()) == active_placement
            && let Some(live) = &active_row
        {
            rows.push(live.clone());
            if let RowKind::WorkLive { expanded, group_id, entries: group_entries, active, .. } = &live.kind {
                has_activity_row |= *active;
                if *expanded {
                    rows.push(row(
                        format!("{group_id}:details"),
                        live.created_at,
                        RowKind::WorkGroupDetails { group_id: group_id.clone(), entries: group_entries.clone() },
                    ));
                }
            }
        }
        if let Some(fold) = folds.get(entry.id.as_str()) {
            rows.push(row(
                format!("turn-fold:{}", fold.run_id.as_str()),
                Some(fold.created_at),
                RowKind::TurnFold {
                    run_id: fold.run_id.clone(),
                    label: fold.label.clone(),
                    expanded: expansion.runs.contains(&fold.run_id),
                },
            ));
        }
        if collapsed.contains(entry.id.as_str()) {
            index += 1;
            continue;
        }
        if let Some((attempt, created_at)) = attempt_anchor.get(entry.id.as_str()) {
            rows.push(row(
                format!("attempt-fold:{}", attempt.as_str()),
                Some(*created_at),
                RowKind::AttemptFold {
                    attempt_id: attempt.clone(),
                    label: "Superseded attempt".into(),
                    expanded: expansion.attempts.contains(attempt),
                },
            ));
        }
        if attempt_hidden.contains(entry.id.as_str()) || active_ids.contains(entry.id.as_str()) {
            index += 1;
            continue;
        }

        match &entry.kind {
            EntryKind::Work(work) => {
                if let WorkItem::Compaction { status, tokens_before, tokens_after } = &work.item {
                    let active = work_in_active_run(work);
                    rows.push(row(
                        entry.id.0.clone(),
                        Some(entry.created_at),
                        RowKind::ContextCompaction {
                            label: if active { "Compacting context".into() } else { work.label.clone() },
                            tokens: tokens_before.zip(*tokens_after),
                            active,
                            failed: *status == CompactionStatus::Failed,
                        },
                    ));
                    index += 1;
                    continue;
                }
                if is_pending_approval(work, &pending) {
                    rows.push(row(entry.id.0.clone(), Some(entry.created_at), RowKind::PendingApproval { entry: work.clone() }));
                    index += 1;
                    continue;
                }
                if stands_alone(work, &pending) {
                    rows.push(row(
                        entry.id.0.clone(),
                        Some(entry.created_at),
                        RowKind::Work { entry: work.clone(), display_label: None },
                    ));
                    index += 1;
                    continue;
                }
                // Gather the run of consecutive groupable calls.
                let mut grouped = vec![work.clone()];
                let mut cursor = index + 1;
                while let Some(next) = entries.get(cursor) {
                    let EntryKind::Work(next_work) = &next.kind else { break };
                    if is_compaction(next)
                        || stands_alone(next_work, &pending)
                        || active_ids.contains(next.id.as_str())
                        || collapsed.contains(next.id.as_str())
                        || attempt_hidden.contains(next.id.as_str())
                        || folds.contains_key(next.id.as_str())
                        || attempt_anchor.contains_key(next.id.as_str())
                        || next_work.run_id != work.run_id
                    {
                        break;
                    }
                    grouped.push(next_work.clone());
                    cursor += 1;
                }
                let visible: Vec<WorkLogEntry> = grouped.into_iter().filter(is_visible_in_group).collect();
                let group = group_id(entry.id.as_str());
                let live: Vec<&WorkLogEntry> = visible.iter().filter(|e| work_in_active_run(e)).collect();
                if let Some(latest) = live.last() {
                    let expanded = expansion.work_groups.contains(&group);
                    rows.push(row(
                        format!("work-live:{}", entry.id.as_str()),
                        Some(entry.created_at),
                        RowKind::WorkLive {
                            entry: (*latest).clone(),
                            entries: visible.clone().into(),
                            group_id: group.clone(),
                            expanded,
                            active: true,
                            thought: None,
                        },
                    ));
                    has_activity_row = true;
                    if expanded {
                        rows.push(row(
                            format!("{group}:details"),
                            Some(entry.created_at),
                            RowKind::WorkGroupDetails { group_id: group.clone(), entries: visible.clone().into() },
                        ));
                    }
                } else if visible.len() == 1 && is_tool_like(&visible[0]) {
                    let only = visible[0].clone();
                    // An edit reads as its group summary, anything else as `singleToolCallLabel`.
                    let display_label = if tool_action(&only) == ToolAction::Edit {
                        Some(summarize_tool_group(&visible))
                    } else {
                        only.source.as_ref().map(|source| source.single_call_label.clone())
                    };
                    rows.push(row(entry.id.0.clone(), Some(entry.created_at), RowKind::Work { entry: only, display_label }));
                } else if !visible.is_empty() {
                    let expanded = expansion.work_groups.contains(&group);
                    let has_failure = visible.iter().rev().find(|e| is_tool_like(e)).is_some_and(indicates_failure);
                    rows.push(row(
                        format!("work-toggle:{}", entry.id.as_str()),
                        Some(entry.created_at),
                        RowKind::WorkToggle {
                            group_id: group.clone(),
                            run_id: work.run_id.clone(),
                            count: visible.len(),
                            expanded,
                            summary: match visible.as_slice() {
                                [single] if !is_tool_like(single) => single.label.clone(),
                                _ => summarize_tool_group(&visible),
                            },
                            summary_kind: summary_kind(&visible),
                            has_failure,
                        },
                    ));
                    if expanded {
                        rows.push(row(
                            format!("{group}:details"),
                            Some(entry.created_at),
                            RowKind::WorkGroupDetails { group_id: group.clone(), entries: visible.clone().into() },
                        ));
                    }
                }
                let images: Vec<SharedString> = visible
                    .iter()
                    .filter_map(|e| match &e.item {
                        WorkItem::ViewedImage { path } => Some(path.clone()),
                        _ => None,
                    })
                    .collect();
                if !images.is_empty() {
                    rows.push(row(format!("{group}:viewed-images"), Some(entry.created_at), RowKind::ViewedImages { paths: images.into() }));
                }
                index = cursor;
                continue;
            }
            EntryKind::ProposedPlan(plan) => {
                rows.push(row(entry.id.0.clone(), Some(entry.created_at), RowKind::ProposedPlan(plan.clone())));
            }
            EntryKind::HtmlRender { title, .. } => {
                rows.push(row(entry.id.0.clone(), Some(entry.created_at), RowKind::HtmlRender { title: title.clone() }));
            }
            EntryKind::McpApp { title, .. } => {
                rows.push(row(entry.id.0.clone(), Some(entry.created_at), RowKind::McpApp { title: title.clone() }));
            }
            EntryKind::SentMessage { text, .. } => {
                rows.push(row(entry.id.0.clone(), Some(entry.created_at), RowKind::SentMessage { text: text.clone() }));
            }
            EntryKind::Event(event) => {
                // Subagents launched in one provider turn share a card.
                if let TimelineEvent::Subagent(sub) = event
                    && let Some(TimelineRow { kind: RowKind::Event { event: TimelineEvent::Subagent(first), subagents }, .. }) =
                        rows.last_mut()
                    && first.run_id == sub.run_id
                    && first.provider_turn_id == sub.provider_turn_id
                {
                    let mut all: Vec<Subagent> =
                        if subagents.is_empty() { vec![first.clone()] } else { subagents.to_vec() };
                    all.push(sub.clone());
                    *subagents = all.into();
                } else {
                    rows.push(row(
                        entry.id.0.clone(),
                        Some(entry.created_at),
                        RowKind::Event { event: event.clone(), subagents: Arc::from([]) },
                    ));
                }
            }
            EntryKind::Message(message) => {
                let still_answering = message.role == MessageRole::Assistant
                    && message.run_id.as_ref().is_some_and(|r| active_runs.contains(r));
                let show_meta =
                    message.role == MessageRole::Assistant && terminal_ids.contains(&message.id.0) && !still_answering;
                rows.push(row(
                    entry.id.0.clone(),
                    Some(entry.created_at),
                    RowKind::Message(MessageRow {
                        message: message.clone(),
                        show_meta,
                        copy_streaming: message.streaming || still_answering,
                        diff_summary: (message.role == MessageRole::Assistant)
                            .then(|| diff_by_message.get(message.id.as_str()).map(|s| (*s).clone()))
                            .flatten(),
                        revert_turn_count: None,
                        grouped_with_next: false,
                        grouped_with_previous: false,
                        continues_steer: false,
                    }),
                ));
            }
        }
        index += 1;
    }

    if let Some(setup) = &input.worktree_setup {
        let first_user = rows
            .iter()
            .position(|r| matches!(&r.kind, RowKind::Message(m) if m.message.role == MessageRole::User));
        let at = first_user.map_or(rows.len(), |i| i + 1);
        rows.insert(at, row("worktree-setup-row", Some(setup.started_at), RowKind::WorktreeSetup(setup.clone())));
    }
    let has_working = rows.iter().any(|r| matches!(r.kind, RowKind::Working { .. }));
    if input.is_working && !has_working && active_header_index >= entries.len() {
        push_working(&mut rows);
    }
    let turn_ended = input.latest_run.as_ref().is_some_and(|l| l.turn_ended);
    if input.is_working && !turn_ended && (latest_failed || (!has_activity_row && pending.is_empty())) {
        rows.push(row("live-activity-row", input.active_turn_started_at, RowKind::Thinking { label: None }));
    }
    if !input.queued.is_empty() {
        rows.push(row("queued-messages", None, RowKind::QueuedMessages { messages: input.queued.clone() }));
    }

    let mut rows = attach_trailing_meta(rows);
    assign_revert_counts(&mut rows, input.supports_rollback);
    finish_grouping(&mut rows);
    rows
}

/// Moves an assistant reply's meta row below the tool groups that trail it
/// (`attachTrailingToolGroupsToAssistant`).
fn attach_trailing_meta(rows: Vec<TimelineRow>) -> Vec<TimelineRow> {
    let mut meta_after: HashMap<usize, TimelineRow> = HashMap::new();
    let mut without_meta: HashSet<usize> = HashSet::new();
    for (message_index, candidate) in rows.iter().enumerate() {
        let RowKind::Message(message) = &candidate.kind else { continue };
        let Some(run) = message.message.run_id.as_ref().filter(|_| message.show_meta && message.message.role == MessageRole::Assistant) else {
            continue;
        };
        let mut last_trailing = None;
        let mut has_group = false;
        for (index, next) in rows.iter().enumerate().skip(message_index + 1) {
            match &next.kind {
                RowKind::Message(_) => break,
                RowKind::WorkToggle { run_id: Some(r), .. } if r == run => {
                    has_group = true;
                    last_trailing = Some(index);
                }
                RowKind::Work { entry, .. } if entry.run_id.as_ref() == Some(run) => {
                    has_group |= is_tool_like(entry) || matches!(entry.item, WorkItem::Error { .. });
                    if has_group {
                        last_trailing = Some(index);
                    }
                }
                _ => {}
            }
        }
        if let Some(last) = last_trailing {
            without_meta.insert(message_index);
            meta_after.insert(
                last,
                row(
                    format!("assistant-meta:{}", message.message.id.as_str()),
                    rows[last].created_at,
                    RowKind::AssistantMeta { message: message.message.clone() },
                ),
            );
        }
    }
    let mut out = Vec::with_capacity(rows.len() + meta_after.len());
    for (index, mut current) in rows.into_iter().enumerate() {
        if without_meta.contains(&index)
            && let RowKind::Message(message) = &mut current.kind
        {
            message.show_meta = false;
        }
        out.push(current);
        if let Some(meta) = meta_after.remove(&index) {
            out.push(meta);
        }
    }
    out
}

/// Every user message can revert the turns after it when rollback is supported.
fn assign_revert_counts(rows: &mut [TimelineRow], supports_rollback: bool) {
    if !supports_rollback {
        return;
    }
    let total = rows
        .iter()
        .filter(|r| matches!(&r.kind, RowKind::Message(m) if m.message.role == MessageRole::User))
        .count();
    let mut seen = 0;
    for current in rows.iter_mut() {
        if let RowKind::Message(message) = &mut current.kind
            && message.message.role == MessageRole::User
        {
            message.revert_turn_count = Some(total - seen);
            seen += 1;
        }
    }
}

fn is_own_user_message(kind: Option<&RowKind>) -> bool {
    matches!(kind, Some(RowKind::Message(m)) if m.message.role == MessageRole::User && m.message.guest.is_none() && !m.message.sent_by_app && m.message.from_agent.is_none())
}

/// Marks work-log continuation and groups adjacent user messages.
fn finish_grouping(rows: &mut [TimelineRow]) {
    for index in 0..rows.len() {
        let next_is_work = rows.get(index + 1).is_some_and(|r| r.kind.is_work_log());
        if rows[index].kind.is_work_log() && next_is_work {
            rows[index].continues_work_log = true;
            continue;
        }
        if !is_own_user_message(Some(&rows[index].kind)) {
            continue;
        }
        let with_next = is_own_user_message(rows.get(index + 1).map(|r| &r.kind));
        let with_previous = index > 0 && is_own_user_message(rows.get(index - 1).map(|r| &r.kind));
        let previous_steer = index > 0
            && matches!(&rows[index - 1].kind, RowKind::Message(m) if m.message.input_intent != InputIntent::Prompt);
        if let RowKind::Message(message) = &mut rows[index].kind {
            message.grouped_with_next = with_next;
            message.grouped_with_previous = with_previous;
            message.continues_steer = with_previous && previous_steer && message.message.input_intent != InputIntent::Prompt;
        }
    }
}
