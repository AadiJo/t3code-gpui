//! What timeline buttons, the branch toolbar and the right panel do to a
//! fixture thread: approvals, fork, "Edit from here", the queue, the
//! thread's branch, and the diff and workspace each thread shows.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{Context, SharedString};

use super::timelines::TimelineBuilder;
use super::workspace;
use super::{FixtureEvent, FixtureStore, ThreadRecord};
use crate::components::chat::timeline_entries::*;
use crate::components::diff_panel::DiffPanelInput;
use crate::components::sidebar::model::{SidebarThread, ThreadId, ThreadKey, TimestampMs};
use crate::diff_panel_store::DiffPanelSelection;
use crate::router::ThreadTarget;

/// A project's workspace as the Files tab reads it.
#[derive(Clone, Debug)]
pub struct FixtureWorkspace {
    /// Keys one Files panel per project.
    pub project_key: SharedString,
    pub name: SharedString,
    pub root: PathBuf,
}

impl FixtureStore {
    fn changed(&mut self, key: &ThreadKey, cx: &mut Context<Self>) {
        cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
        cx.emit(FixtureEvent::SnapshotChanged);
    }

    /// Approves or declines the pending approval `request`. Approving lets
    /// the turn continue with a reply; declining ends it.
    pub fn respond_to_approval(&mut self, key: &ThreadKey, request: &RequestId, approve: bool, cx: &mut Context<Self>) {
        let Some(timeline) = self.threads.get_mut(key).and_then(|r| r.timeline.as_mut()) else { return };
        if !timeline.pending_approval_ids.contains(request) {
            return;
        }
        timeline.pending_approval_ids.retain(|id| id != request);
        for entry in Arc::make_mut(&mut timeline.entries).iter_mut().rev() {
            if let EntryKind::Work(work) = &mut entry.kind
                && matches!(&work.item, WorkItem::Approval { request_id, .. } if request_id == request)
            {
                work.status = if approve { ToolStatus::Completed } else { ToolStatus::Declined };
                break;
            }
        }
        let waiting = !timeline.pending_approval_ids.is_empty();
        if !waiting {
            timeline.latest_run = None;
        }
        if let Some(record) = self.threads.get_mut(key) {
            record.pending_approval = waiting;
        }
        self.set_flags(key, |t| t.has_pending_approvals = waiting);
        self.clock += 1_000;
        if approve && !waiting {
            self.start_reply(key, "Approved: run the router tests", cx);
        }
        self.changed(key, cx);
    }

    /// Adds a pending command approval to the end of `key`, as an agent
    /// asking to run a command does.
    pub fn request_approval(&mut self, key: &ThreadKey, cx: &mut Context<Self>) {
        let clock = self.clock;
        let Some(record) = self.threads.get_mut(key) else { return };
        if record.timeline.is_none() {
            record.timeline = Some(super::build_timeline(key, record));
        }
        let Some(timeline) = record.timeline.as_mut() else { return };
        let entries = std::mem::take(Arc::make_mut(&mut timeline.entries));
        let mut b = TimelineBuilder::resume(key.thread_id.0.clone(), entries, record.next_id);
        let (run, request) = (b.run_id(), b.request_id());
        b.work(
            Timestamp(clock),
            &run,
            ToolStatus::InProgress,
            "Run command",
            WorkItem::Approval {
                request_id: request.clone(),
                kind: ApprovalKind::Command,
                detail: "pnpm vitest run apps/web/src/router.test.ts".into(),
                reason: Some("The agent wants to run the router tests.".into()),
            },
        );
        record.next_id = b.next_index();
        timeline.entries = Arc::new(b.entries);
        timeline.pending_approval_ids.push(request);
        timeline.latest_run =
            Some(LatestRun { run_id: run, status: RunStatus::Waiting, started_at: Some(Timestamp(clock)), completed_at: None, turn_ended: false });
        record.pending_approval = true;
        self.set_flags(key, |t| t.has_pending_approvals = true);
        self.changed(key, cx);
    }

    /// Forks `key` after the run that ends with `message`: a new thread with
    /// the conversation up to there, titled "<title> fork". Returns its key.
    pub fn fork_from_message(&mut self, key: &ThreadKey, message: &MessageId, cx: &mut Context<Self>) -> Option<ThreadKey> {
        let record = self.threads.get(key)?;
        let timeline = record.timeline.as_ref()?;
        let end = timeline.entries.iter().position(|e| matches!(&e.kind, EntryKind::Message(m) if m.id == *message))?;
        let entries: Vec<TimelineEntry> = timeline.entries[..=end].to_vec();
        let (project, cwd, project_id, title) =
            (record.project.clone(), record.cwd.clone(), record.project_id.clone(), record.title.clone());
        self.next_thread += 1;
        self.clock += 1_000;
        let fork = ThreadKey { environment_id: key.environment_id.clone(), thread_id: ThreadId(format!("fork-{}", self.next_thread).into()) };
        let fork_title: SharedString = format!("{title} fork").into();
        // Ids keep the source's prefix, so the fork's divider and later rows get their own.
        let mut b = TimelineBuilder::resume(fork.thread_id.0.clone(), entries, end + 1_000);
        let at = Timestamp(self.clock);
        b.entries.push(TimelineEntry {
            id: EntryId::new(format!("{}:forked", fork.thread_id.0)),
            created_at: at,
            kind: EntryKind::Event(TimelineEvent::Divider(Divider {
                kind: DividerKind::Forked,
                run_id: None,
                label: "Forked from conversation".into(),
                detail: Some(title.clone()),
                action: false,
            })),
        });
        let next_id = b.next_index();
        let now = TimestampMs(self.clock);
        let row = SidebarThread {
            key: fork.clone(),
            project_id: project_id.clone(),
            title: fork_title.clone(),
            created_at: now,
            updated_at: now,
            last_visited_at: Some(now),
            ..super::blank_thread(&fork)
        };
        self.snapshot.threads.push(Arc::new(row));
        self.threads.insert(
            fork.clone(),
            ThreadRecord {
                project,
                title: fork_title,
                cwd,
                timeline: Some(TimelineInput { entries: Arc::new(b.entries), supports_rollback: true, ..TimelineInput::default() }),
                turns: 0,
                pending_approval: false,
                pending_question: false,
                questions: Arc::from([]),
                branch: record_branch(self, key),
                project_id,
                next_id,
                working: false,
            },
        );
        cx.emit(FixtureEvent::SnapshotChanged);
        Some(fork)
    }

    /// "Edit from here": removes `message` (a user message) and everything
    /// after it, and returns its text for the composer. `None` while the
    /// thread runs or when the message is not there.
    pub fn revert_to_message(&mut self, key: &ThreadKey, message: &MessageId, cx: &mut Context<Self>) -> Option<SharedString> {
        if self.streams.contains_key(key) {
            return None;
        }
        let record = self.threads.get_mut(key)?;
        let timeline = record.timeline.as_mut()?;
        let at = timeline.entries.iter().position(
            |e| matches!(&e.kind, EntryKind::Message(m) if m.id == *message && m.role == MessageRole::User),
        )?;
        let text = match &timeline.entries[at].kind {
            EntryKind::Message(m) => m.text.clone(),
            _ => return None,
        };
        Arc::make_mut(&mut timeline.entries).truncate(at);
        let kept: Vec<MessageId> = timeline
            .entries
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::Message(m) => Some(m.id.clone()),
                _ => None,
            })
            .collect();
        timeline.turn_diff_summaries =
            timeline.turn_diff_summaries.iter().filter(|s| kept.contains(&s.assistant_message_id)).cloned().collect();
        timeline.pending_approval_ids.clear();
        timeline.pending_question = None;
        timeline.latest_run = None;
        record.pending_approval = false;
        record.pending_question = false;
        record.questions = Arc::from([]);
        self.set_flags(key, |t| {
            t.has_pending_approvals = false;
            t.has_pending_user_input = false;
        });
        self.changed(key, cx);
        Some(text)
    }

    /// "Send now": the queued message steers the running turn at once.
    pub fn send_queued_now(&mut self, key: &ThreadKey, id: &str, cx: &mut Context<Self>) {
        let Some(message) = self.take_queued(key, id) else { return };
        self.clock += 1_000;
        let intent = if self.streams.contains_key(key) { InputIntent::PromotedQueuedToSteer } else { InputIntent::Prompt };
        self.append_user(key, &message, intent);
        if intent == InputIntent::Prompt {
            self.start_reply(key, &message, cx);
        }
        self.changed(key, cx);
    }

    /// Takes a queued message out of the queue (edit puts its text in the
    /// composer; remove drops it). Returns its text.
    pub fn remove_queued(&mut self, key: &ThreadKey, id: &str, cx: &mut Context<Self>) -> Option<SharedString> {
        let text = self.take_queued(key, id)?;
        cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
        Some(text.into())
    }

    fn take_queued(&mut self, key: &ThreadKey, id: &str) -> Option<String> {
        let queue = self.queued.get_mut(key)?;
        let index = queue.iter().position(|q| q.id.as_ref() == id)?;
        queue.remove(index).map(|q| q.text.to_string())
    }

    /// The thread moved to `branch` (the branch toolbar or a git action).
    pub fn set_thread_branch(&mut self, key: &ThreadKey, branch: SharedString, cx: &mut Context<Self>) {
        if let Some(record) = self.threads.get_mut(key) {
            record.branch = Some(branch.clone());
        }
        self.set_flags(key, |t| t.branch = Some(branch));
        self.changed(key, cx);
    }

    /// The Diff tab's data for `target` and the scope it shows.
    pub fn diff_input(&mut self, target: &ThreadTarget, selection: &DiffPanelSelection) -> Option<DiffPanelInput> {
        let view = self.thread_view(target)?;
        let workspace = self.workspace(target)?;
        if let Some(checkout) = self.reference.clone() {
            return Some(workspace::reference_diff_input(view.key, workspace.root.to_string_lossy().to_string().into(), view.branch, &checkout.patch));
        }
        Some(workspace::diff_input(
            view.key,
            workspace.root.to_string_lossy().to_string().into(),
            view.branch,
            &view.timeline.turn_diff_summaries,
            selection,
        ))
    }

    /// The project directory the Files tab of `target` reads, written on first use.
    pub fn workspace(&self, target: &ThreadTarget) -> Option<FixtureWorkspace> {
        let project_id = match target {
            ThreadTarget::Server(key) => self.threads.get(key).map(|r| r.project_id.clone())?,
            ThreadTarget::Draft(id) => self.drafts.get(id)?.project.as_ref()?.project_id.clone(),
        };
        let project = self.snapshot.projects.iter().find(|p| p.project_ref.project_id == project_id)?;
        if let Some(checkout) = &self.reference {
            let root = std::env::temp_dir().join("t3code-reference-checkout");
            return Some(FixtureWorkspace {
                project_key: format!("{}:{}", project.project_ref.environment_id.0, project_id.0).into(),
                name: project.title.clone(),
                root: checkout.write(&root).ok()?,
            });
        }
        let mut changed: Vec<&str> = workspace::CHANGED_PATHS.to_vec();
        if let ThreadTarget::Server(key) = target
            && let Some(timeline) = self.threads.get(key).and_then(|r| r.timeline.as_ref())
        {
            changed.extend(timeline.turn_diff_summaries.iter().flat_map(|s| s.files.iter()).map(|f| f.path.as_ref()));
        }
        Some(FixtureWorkspace {
            project_key: format!("{}:{}", project.project_ref.environment_id.0, project_id.0).into(),
            name: project.title.clone(),
            root: workspace::project_directory(&project_id.0, &changed),
        })
    }
}

fn record_branch(store: &FixtureStore, key: &ThreadKey) -> Option<SharedString> {
    store.threads.get(key).and_then(|r| r.branch.clone())
}
