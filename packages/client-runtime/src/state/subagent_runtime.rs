//! Subagents and workflows as the thread details card's Lineage rows read them
//! (`subagentRuntime.ts`, `subagentDisplay.ts`): projected subagent rows become runtime
//! subagents, a workflow coordinator expands into its member agents, and members group under
//! their phases.

use std::collections::BTreeMap;

use t3code_contracts::{
    ModelSelection, OrchestrationV2Subagent, ProviderOptionSelection, ProviderOptionSelectionValue, ProviderOptionSelections, OrchestrationV2SubagentWorkflow,
    OrchestrationV2TurnItemStatus as ItemStatus, OrchestrationV2WorkflowAgentState as AgentState,
    OrchestrationV2WorkflowRunHandles, ProviderInstanceId, ThreadId,
};

/// A subagent's status (`RuntimeSubagentStatus`).
pub type RuntimeSubagentStatus = ItemStatus;

/// Finished work (`isTerminalSubagentStatus`).
pub fn is_terminal_subagent_status(status: RuntimeSubagentStatus) -> bool {
    matches!(status, ItemStatus::Completed | ItemStatus::Failed | ItemStatus::Cancelled | ItemStatus::Interrupted)
}

/// Pending, running or waiting (`isOrchestrationV2WorkActive`).
pub fn is_active_subagent_status(status: RuntimeSubagentStatus) -> bool {
    matches!(status, ItemStatus::Pending | ItemStatus::Running | ItemStatus::Waiting)
}

/// What a runtime subagent row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeSubagentKind {
    Subagent,
    SubagentBatch,
    Workflow,
    WorkflowAgent,
}

/// Token, tool and time totals (`SubagentUsage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentUsage {
    pub total_tokens: u64,
    pub tool_uses: Option<u64>,
    pub duration_ms: Option<u64>,
}

/// A declared workflow phase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentWorkflowPhase {
    pub index: u64,
    pub title: String,
}

/// One subagent, workflow coordinator or workflow member (`RuntimeSubagent`). Timestamps are
/// ISO strings, as the web compares them.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeSubagent {
    pub id: String,
    pub kind: RuntimeSubagentKind,
    pub title: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub status: RuntimeSubagentStatus,
    pub activation_count: u64,
    pub usage: Option<SubagentUsage>,
    pub progress: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub parent_agent_id: Option<String>,
    pub agent_index: Option<u64>,
    pub phase_index: Option<u64>,
    pub phase_title: Option<String>,
    pub attempt: Option<u64>,
    pub workflow_name: Option<String>,
    pub phases: Vec<SubagentWorkflowPhase>,
    pub run_handles: Option<OrchestrationV2WorkflowRunHandles>,
    pub child_thread_id: Option<ThreadId>,
    pub first_seen_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub updated_at: String,
}

/// How a phase stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowPhaseState {
    Pending,
    Running,
    Done,
}

/// A phase with its members.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowPhaseGroup {
    pub index: u64,
    pub title: String,
    pub members: Vec<RuntimeSubagent>,
    pub state: WorkflowPhaseState,
    pub settled_count: usize,
}

/// A workflow coordinator with its members by phase (`SubagentWorkflowGroup`).
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentWorkflowGroup {
    pub workflow: RuntimeSubagent,
    pub phases: Vec<WorkflowPhaseGroup>,
    pub unphased_members: Vec<RuntimeSubagent>,
}

fn workflow_usage(total_tokens: Option<u64>, tool_calls: Option<u64>, duration_ms: Option<u64>) -> Option<SubagentUsage> {
    Some(SubagentUsage { total_tokens: total_tokens?, tool_uses: tool_calls, duration_ms })
}

/// `DateTime.formatIso`: millisecond precision, always with three fraction digits.
fn iso_from_epoch_millis(ms: Option<u64>) -> Option<String> {
    let time = jiff::Timestamp::from_millisecond(i64::try_from(ms?).ok()?).ok()?;
    Some(time.strftime("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

fn workflow_members(coordinator: &RuntimeSubagent, workflow: &OrchestrationV2SubagentWorkflow) -> Vec<RuntimeSubagent> {
    workflow
        .agents
        .iter()
        .map(|agent| {
            let status = match agent.state {
                AgentState::Queued | AgentState::Running if is_terminal_subagent_status(coordinator.status) => coordinator.status,
                AgentState::Queued => ItemStatus::Pending,
                AgentState::Running => ItemStatus::Running,
                AgentState::Completed => ItemStatus::Completed,
                AgentState::Failed => ItemStatus::Failed,
            };
            let failed = status == ItemStatus::Failed;
            let started_at = iso_from_epoch_millis(agent.started_at);
            let completed_at = is_terminal_subagent_status(status).then(|| {
                iso_from_epoch_millis(agent.completed_at)
                    .or_else(|| iso_from_epoch_millis(agent.started_at.zip(agent.duration_ms).map(|(start, duration)| start + duration)))
                    .or_else(|| coordinator.completed_at.clone())
            });
            let completed_at = completed_at.flatten();
            RuntimeSubagent {
                id: format!("{}:agent:{}", coordinator.id, agent.index),
                kind: RuntimeSubagentKind::WorkflowAgent,
                title: agent.label.clone(),
                model: agent.model.clone(),
                effort: agent.effort.clone(),
                speed: agent.speed.clone(),
                status,
                activation_count: agent.attempt.unwrap_or(1),
                usage: workflow_usage(agent.total_tokens, agent.tool_calls, agent.duration_ms),
                progress: agent.prompt.clone(),
                result: if failed { None } else { agent.result.clone() },
                error: if failed { agent.result.clone() } else { None },
                parent_agent_id: Some(coordinator.id.clone()),
                agent_index: Some(agent.index),
                phase_index: agent.phase_index,
                phase_title: agent.phase_title.clone(),
                attempt: agent.attempt,
                workflow_name: coordinator.workflow_name.clone(),
                phases: Vec::new(),
                run_handles: coordinator.run_handles.clone(),
                child_thread_id: agent.child_thread_id.clone(),
                first_seen_at: iso_from_epoch_millis(agent.queued_at)
                    .or_else(|| started_at.clone())
                    .unwrap_or_else(|| coordinator.first_seen_at.clone()),
                updated_at: completed_at.clone().or_else(|| started_at.clone()).unwrap_or_else(|| coordinator.first_seen_at.clone()),
                started_at,
                completed_at,
            }
        })
        .collect()
}

/// An untitled subagent's title: its prompt, cut to 80 characters (UTF-16 units, as the web
/// counts) without splitting a surrogate pair (`promptTitle`).
fn prompt_title(prompt: &str) -> String {
    let units: Vec<u16> = prompt.encode_utf16().collect();
    if units.len() <= 80 {
        return prompt.to_owned();
    }
    let mut end = 77;
    if (0xd800..=0xdbff).contains(&units[end - 1]) && (0xdc00..=0xdfff).contains(&units[end]) {
        end -= 1;
    }
    format!("{}...", String::from_utf16_lossy(&units[..end]))
}

/// A workflow member's model pick for its tooltip (`workflowMemberModelSelection`).
pub fn workflow_member_model_selection(member: &RuntimeSubagent, instance_id: ProviderInstanceId) -> Option<ModelSelection> {
    let model = member.model.clone().filter(|m| !m.is_empty())?;
    let mut options = Vec::new();
    if let Some(effort) = member.effort.as_ref().filter(|e| !e.is_empty()) {
        options.push(ProviderOptionSelection { id: "effort".into(), value: ProviderOptionSelectionValue::Text(effort.clone()) });
    }
    if let Some(speed) = member.speed.as_ref().filter(|s| !s.is_empty()) {
        options.push(ProviderOptionSelection { id: "fastMode".into(), value: ProviderOptionSelectionValue::Bool(speed == "fast") });
    }
    Some(ModelSelection { instance_id: instance_id.to_string(), model, options: Some(ProviderOptionSelections::List(options)) })
}

/// Projected subagent rows as runtime subagents (`projectedSubagentsToRuntime`); a workflow
/// coordinator is followed by its members.
pub fn projected_subagents_to_runtime<'a>(subagents: impl IntoIterator<Item = &'a OrchestrationV2Subagent>) -> Vec<RuntimeSubagent> {
    subagents
        .into_iter()
        .flat_map(|subagent| {
            let workflow = subagent.workflow.as_ref();
            let first_seen_at = subagent.started_at.clone().unwrap_or_else(|| subagent.updated_at.clone());
            let coordinator = RuntimeSubagent {
                id: subagent.id.to_string(),
                kind: if workflow.is_some() { RuntimeSubagentKind::Workflow } else { RuntimeSubagentKind::Subagent },
                title: subagent.title.clone().unwrap_or_else(|| prompt_title(&subagent.prompt)),
                model: subagent.model.clone(),
                effort: None,
                speed: None,
                status: subagent.status,
                activation_count: 1,
                usage: workflow.and_then(|w| workflow_usage(w.total_tokens, w.tool_calls, w.duration_ms)),
                progress: subagent.progress.clone(),
                result: subagent.result.clone(),
                error: if subagent.status == ItemStatus::Failed { subagent.result.clone() } else { None },
                parent_agent_id: None,
                agent_index: None,
                phase_index: None,
                phase_title: None,
                attempt: None,
                workflow_name: workflow.and_then(|w| w.name.clone()),
                phases: workflow
                    .map(|w| w.phases.iter().map(|p| SubagentWorkflowPhase { index: p.index, title: p.title.clone() }).collect())
                    .unwrap_or_default(),
                run_handles: workflow.and_then(|w| w.run_handles.clone()),
                child_thread_id: subagent.child_thread_id.clone(),
                first_seen_at,
                started_at: subagent.started_at.clone(),
                completed_at: subagent.completed_at.clone(),
                updated_at: subagent.updated_at.clone(),
            };
            let members = workflow.map(|w| workflow_members(&coordinator, w)).unwrap_or_default();
            std::iter::once(coordinator).chain(members)
        })
        .collect()
}

/// Groups workflow members under their coordinators and phases (`deriveWorkflowGroups`).
/// `phase_fallback` names a phase the provider reported only by number (`Phase {number}`).
pub fn derive_workflow_groups(agents: &[RuntimeSubagent], phase_fallback: &dyn Fn(u64) -> String) -> Vec<SubagentWorkflowGroup> {
    let mut workflows: Vec<&RuntimeSubagent> = agents.iter().filter(|a| a.kind == RuntimeSubagentKind::Workflow).collect();
    workflows.sort_by(|a, b| a.first_seen_at.cmp(&b.first_seen_at).then_with(|| a.id.cmp(&b.id)));
    workflows
        .into_iter()
        .map(|workflow| {
            let members: Vec<&RuntimeSubagent> = agents
                .iter()
                .filter(|a| a.kind != RuntimeSubagentKind::Workflow && a.parent_agent_id.as_deref() == Some(workflow.id.as_str()))
                .collect();
            // The declared plan lags the members, so members seed the titles and the declared
            // titles then overwrite whatever they guessed.
            let mut titles: BTreeMap<u64, String> = BTreeMap::new();
            for member in &members {
                let Some(index) = member.phase_index else { continue };
                let title = member.phase_title.clone().or_else(|| titles.get(&index).cloned()).unwrap_or_else(|| phase_fallback(index + 1));
                titles.insert(index, title);
            }
            for phase in &workflow.phases {
                titles.insert(phase.index, phase.title.clone());
            }
            let by_agent = |a: &&RuntimeSubagent| a.agent_index.unwrap_or(0);
            let phases = titles
                .into_iter()
                .map(|(index, title)| {
                    let mut phase_members: Vec<&RuntimeSubagent> = members.iter().copied().filter(|m| m.phase_index == Some(index)).collect();
                    phase_members.sort_by_key(by_agent);
                    // Idle members keep a phase live: a resumable member has not finished it.
                    let active = phase_members.iter().any(|m| is_active_subagent_status(m.status) || m.status == ItemStatus::Idle);
                    let state = if phase_members.is_empty() {
                        WorkflowPhaseState::Pending
                    } else if active {
                        WorkflowPhaseState::Running
                    } else {
                        WorkflowPhaseState::Done
                    };
                    let settled_count = phase_members.iter().filter(|m| is_terminal_subagent_status(m.status)).count();
                    WorkflowPhaseGroup { index, title, members: phase_members.into_iter().cloned().collect(), state, settled_count }
                })
                .collect();
            let mut unphased: Vec<&RuntimeSubagent> = members.iter().copied().filter(|m| m.phase_index.is_none()).collect();
            unphased.sort_by_key(by_agent);
            SubagentWorkflowGroup { workflow: workflow.clone(), phases, unphased_members: unphased.into_iter().cloned().collect() }
        })
        .collect()
}

/// A subagent's display title (`formatSubagentDisplayTitle`): drops a `Subagent:` prefix and
/// names a `/root/.../name` path by its last segment in title case.
pub fn format_subagent_display_title(title: &str) -> String {
    let display = match title.get(..9) {
        Some(prefix) if prefix.eq_ignore_ascii_case("subagent:") => title[9..].trim_start(),
        _ => title,
    };
    let Some(rest) = display.strip_prefix("/root/") else { return display.to_owned() };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let Some(name) = rest.rsplit('/').next().filter(|n| !n.is_empty()) else { return display.to_owned() };
    let words: Vec<String> = name
        .split(|c: char| c == '_' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        })
        .collect();
    if words.is_empty() { display.to_owned() } else { words.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_fixtures::subagent as subagent_json;
    use serde_json::{Value, json};

    fn subagent(overrides: Value) -> OrchestrationV2Subagent {
        let mut base = json!({"id": "coordinator", "title": "probe", "prompt": "export const meta = …", "model": "claude-opus-5[1m]",
            "status": "running", "result": null, "startedAt": "2026-08-01T10:00:00.000Z", "completedAt": null,
            "updatedAt": "2026-08-01T10:00:05.000Z"});
        if let (Value::Object(base), Value::Object(overrides)) = (&mut base, overrides) {
            base.extend(overrides);
        }
        serde_json::from_value(subagent_json(base)).expect("subagent decodes")
    }

    fn member(overrides: Value) -> Value {
        let mut base = json!({"index": 1, "label": "alpha:one", "state": "running", "phaseIndex": 1, "phaseTitle": "Alpha"});
        if let (Value::Object(base), Value::Object(overrides)) = (&mut base, overrides) {
            base.extend(overrides);
        }
        base
    }

    fn phase(number: u64) -> String {
        format!("Phase {number}")
    }

    fn groups_of(subagents: &[OrchestrationV2Subagent]) -> Vec<SubagentWorkflowGroup> {
        derive_workflow_groups(&projected_subagents_to_runtime(subagents), &phase)
    }

    #[test]
    fn uses_a_settled_members_wall_clock_completion_separately_from_its_execution_duration() {
        let runtime = projected_subagents_to_runtime(&[subagent(json!({
            "status": "completed", "completedAt": "2026-08-01T10:00:09.000Z",
            "workflow": {"phases": [], "agents": [member(json!({
                "state": "completed", "model": "claude-opus-5[1m]", "effort": "xhigh", "speed": "standard",
                "startedAt": 1785578400000u64, "durationMs": 1500, "completedAt": 1785578404432u64,
                "totalTokens": 71141, "toolCalls": 3, "prompt": "Reply with exactly: A1", "result": "A1"
            }))]}
        }))]);
        let settled = &runtime[1];
        assert_eq!(settled.status, ItemStatus::Completed);
        assert_eq!(settled.completed_at.as_deref(), Some("2026-08-01T10:00:04.432Z"));
        assert_eq!(settled.result.as_deref(), Some("A1"));
        assert_eq!(settled.model.as_deref(), Some("claude-opus-5[1m]"));
        assert_eq!(settled.effort.as_deref(), Some("xhigh"));
        assert_eq!(settled.speed.as_deref(), Some("standard"));
        assert_eq!(settled.progress.as_deref(), Some("Reply with exactly: A1"));
        assert_eq!(settled.usage, Some(SubagentUsage { total_tokens: 71141, tool_uses: Some(3), duration_ms: Some(1500) }));
        let instance = ProviderInstanceId::new("claudeAgent");
        let selection = workflow_member_model_selection(settled, instance.clone()).unwrap();
        assert_eq!(selection.model, "claude-opus-5[1m]");
        let Some(ProviderOptionSelections::List(options)) = selection.options else { panic!("options are a list") };
        assert_eq!((options[0].id.as_str(), &options[0].value), ("effort", &ProviderOptionSelectionValue::Text("xhigh".into())));
        assert_eq!((options[1].id.as_str(), &options[1].value), ("fastMode", &ProviderOptionSelectionValue::Bool(false)));
        let unmodelled = RuntimeSubagent { model: None, ..settled.clone() };
        assert!(workflow_member_model_selection(&unmodelled, instance).is_none());
    }

    #[test]
    fn settles_unfinished_members_when_the_coordinator_settles() {
        for status in ["completed", "cancelled", "failed"] {
            let groups = groups_of(&[subagent(json!({
                "status": status, "completedAt": "2026-08-01T10:00:09.000Z",
                "workflow": {"phases": [], "agents": [
                    member(json!({})),
                    member(json!({"index": 2, "state": "queued"})),
                    member(json!({"index": 3, "state": "completed", "startedAt": 1785578400000u64, "durationMs": 1500})),
                ]}
            }))]);
            assert_eq!(groups[0].workflow.status.as_str(), status);
            let members: Vec<_> = groups[0].phases.iter().flat_map(|p| p.members.iter()).collect();
            assert_eq!(members.iter().map(|m| m.status.as_str()).collect::<Vec<_>>(), vec![status, status, "completed"]);
            assert_eq!(
                members.iter().map(|m| m.completed_at.clone().unwrap()).collect::<Vec<_>>(),
                vec!["2026-08-01T10:00:09.000Z", "2026-08-01T10:00:09.000Z", "2026-08-01T10:00:01.500Z"]
            );
            assert!(members[0].model.is_none() && members[0].effort.is_none() && members[0].speed.is_none());
        }
    }

    #[test]
    fn titles_an_untitled_subagent_from_its_prompt_without_splitting_an_emoji() {
        let prompt = format!("{}😀 and more work.", "a".repeat(76));
        let runtime = projected_subagents_to_runtime(&[subagent(json!({"title": null, "prompt": prompt}))]);
        assert_eq!(runtime[0].title, format!("{}...", "a".repeat(76)));
    }

    #[test]
    fn groups_members_under_their_phases_and_counts_the_run() {
        let groups = groups_of(&[subagent(json!({"workflow": {
            "name": "probe-wf",
            "phases": [{"index": 1, "title": "Alpha"}, {"index": 2, "title": "Beta"}],
            "agents": [
                member(json!({"state": "completed"})),
                member(json!({"index": 2, "label": "alpha:two", "state": "failed"})),
                member(json!({"index": 3, "label": "beta:one", "phaseIndex": 2, "phaseTitle": "Beta"})),
                member(json!({"index": 4, "label": "beta:two", "phaseIndex": 2, "phaseTitle": "Beta", "state": "queued"})),
            ]
        }}))]);
        let group = &groups[0];
        let phases: Vec<_> = group.phases.iter().map(|p| (p.title.as_str(), p.state)).collect();
        assert_eq!(phases, vec![("Alpha", WorkflowPhaseState::Done), ("Beta", WorkflowPhaseState::Running)]);
        assert_eq!(group.phases.iter().map(|p| p.settled_count).collect::<Vec<_>>(), vec![2, 0]);
        assert_eq!(group.phases.iter().map(|p| p.members.len()).sum::<usize>(), 4);
        assert!(group.unphased_members.is_empty());
    }

    #[test]
    fn keeps_a_reported_phase_title() {
        for missing_first in [false, true] {
            let mut agents = vec![member(json!({"phaseTitle": "Review"})), member(json!({"index": 2, "phaseTitle": null}))];
            if missing_first {
                agents.reverse();
            }
            let groups = groups_of(&[subagent(json!({"workflow": {"phases": [], "agents": agents}}))]);
            assert_eq!(groups[0].phases[0].title, "Review", "missing first: {missing_first}");
        }
    }

    #[test]
    fn keeps_unphased_members_with_their_workflow_and_excludes_unrelated_agents() {
        let runtime = projected_subagents_to_runtime(&[
            subagent(json!({"id": "ordinary"})),
            subagent(json!({"id": "__proto__", "workflow": {"phases": [], "agents": [member(json!({"phaseIndex": null, "phaseTitle": null}))]}})),
            subagent(json!({"id": "constructor", "workflow": {"phases": [], "agents": []}})),
            subagent(json!({"id": "orphan", "workflow": {"phases": [], "agents": [member(json!({}))]}})),
        ]);
        let kept: Vec<_> = runtime.into_iter().filter(|a| a.id != "orphan").collect();
        let groups = derive_workflow_groups(&kept, &phase);
        assert_eq!(groups.iter().map(|g| g.workflow.id.as_str()).collect::<Vec<_>>(), vec!["__proto__", "constructor"]);
        assert!(groups[0].phases.is_empty());
        assert_eq!(groups[0].unphased_members.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(), vec!["alpha:one"]);
        assert!(groups[1].phases.is_empty() && groups[1].unphased_members.is_empty());
    }

    #[test]
    fn formats_subagent_display_titles() {
        assert_eq!(format_subagent_display_title("Subagent: Explore the repo"), "Explore the repo");
        assert_eq!(format_subagent_display_title("/root/agents/code_reviewer/"), "Code Reviewer");
        assert_eq!(format_subagent_display_title("Plain"), "Plain");
    }
}
