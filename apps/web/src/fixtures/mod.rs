//! One in-memory store that feeds the sidebar, the timeline and the composer
//! the same threads, so the row you click is the thread you see. It stands in
//! for the sync layer: sending a message appends it and streams a canned
//! reply, sends during a reply steer or queue, and pending questions and
//! approvals resolve when answered.

pub mod palette_source;
pub mod timelines;

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{Context, EventEmitter, SharedString, Task};

use crate::components::chat::composer::FollowUpBehavior;
use crate::components::chat::timeline_entries::*;
use crate::components::chat::timeline_fixtures::fixture_thread;
use crate::components::sidebar::fixtures as sidebar_fixtures;
use crate::components::sidebar::model::{
    self as sidebar, EnvironmentId, ProjectRef, SidebarSnapshot, SidebarThread, ThreadId, ThreadKey, TimestampMs,
};
use crate::desktop_notifications::{DesktopNotification, DesktopNotificationKind};
use crate::router::{DraftId, ThreadTarget};
use timelines::{NOW_MS, TimelineBuilder};

/// Delay before the reply starts, and between streamed chunks.
const REPLY_DELAY: Duration = Duration::from_millis(250);
const STREAM_TICK: Duration = Duration::from_millis(32);
const STREAM_CHUNK: usize = 18;

/// What the chat view needs to show one thread.
#[derive(Clone)]
pub struct ThreadView {
    pub key: SharedString,
    pub project: SharedString,
    pub title: SharedString,
    pub cwd: SharedString,
    pub timeline: TimelineInput,
    pub is_draft: bool,
    pub running: bool,
}

/// A thread's content. The timeline is built on first open.
struct ThreadRecord {
    project: SharedString,
    title: SharedString,
    cwd: SharedString,
    timeline: Option<TimelineInput>,
    turns: usize,
    pending_approval: bool,
    pending_question: bool,
    next_id: usize,
}

/// A reply being streamed into a thread.
struct Stream {
    message: MessageId,
    text: String,
    _task: Task<()>,
}

/// A new thread that has no messages yet (`/draft/$draftId`).
struct DraftRecord {
    project: Option<ProjectRef>,
}

/// Changes the views apply.
#[derive(Clone, Debug)]
pub enum FixtureEvent {
    /// The sidebar rows changed (a run started or finished, a thread appeared).
    SnapshotChanged,
    /// A thread's timeline changed; re-read it with [`FixtureStore::thread_view`].
    ThreadChanged(ThreadTarget),
    /// A streamed reply grew.
    Delta { thread: ThreadKey, message: MessageId, text: SharedString },
    /// A draft got its first message and is now a server thread.
    DraftPromoted { draft: DraftId, thread: ThreadKey },
}

/// The fixture data behind the whole window.
pub struct FixtureStore {
    snapshot: SidebarSnapshot,
    threads: HashMap<ThreadKey, ThreadRecord>,
    drafts: HashMap<DraftId, DraftRecord>,
    streams: HashMap<ThreadKey, Stream>,
    queued: HashMap<ThreadKey, VecDeque<QueuedMessage>>,
    next_draft: usize,
    next_thread: usize,
    /// Advances with every fixture action so new rows sort to the top.
    clock: i64,
}

impl EventEmitter<FixtureEvent> for FixtureStore {}

impl FixtureStore {
    /// The showcase sidebar: every row state, one thread with every timeline row kind.
    pub fn showcase() -> Self {
        Self::from_snapshot(sidebar_fixtures::showcase_snapshot(), |_| 6)
    }

    /// The world the visual-parity reference was captured in
    /// (`sidebar_fixtures::reference_snapshot`, `timelines::reference_thread`).
    pub fn reference() -> Self {
        Self::from_snapshot(sidebar_fixtures::reference_snapshot(), |_| 2)
    }

    /// `threads` threads where every `long_every`th one has a 1,000-turn
    /// (3,000-row) timeline, for the navigation benchmark.
    pub fn large(threads: usize, long_every: usize) -> Self {
        let snapshot = sidebar_fixtures::large_snapshot(threads, 24);
        Self::from_snapshot(snapshot, move |index| if index % long_every == 0 { 1_000 } else { 4 + index % 5 })
    }

    /// No projects or threads: the app as it starts with no fixtures.
    pub fn empty() -> Self {
        Self::from_sidebar(sidebar_fixtures::empty_snapshot())
    }

    /// Whether the store has no projects and no threads.
    pub fn is_empty(&self) -> bool {
        self.snapshot.projects.is_empty() && self.snapshot.threads.is_empty()
    }

    /// Inbox entries for the store's threads that wait on an approval or an
    /// answer (unread), plus one read completion, as a desktop host would
    /// have saved them. Checks seed the inbox with these where no host runs.
    pub fn inbox_items(&self) -> Arc<[DesktopNotification]> {
        let project = |thread: &SidebarThread| {
            self.snapshot.projects.iter().find(|p| p.project_ref.project_id == thread.project_id).map(|p| p.title.clone())
        };
        let entry = |thread: &SidebarThread, kind, read| DesktopNotification {
            id: format!("fixture-{}-{kind:?}", thread.key.thread_id.0).into(),
            environment_id: thread.key.environment_id.0.clone(),
            thread_id: thread.key.thread_id.0.clone(),
            kind,
            thread_title: thread.title.clone(),
            project_title: project(thread).unwrap_or_default(),
            environment_title: "devbox".into(),
            created_at: "2026-11-23T10:00:00.000Z".into(),
            read,
            resolved: false,
        };
        let waiting = self.snapshot.threads.iter().filter_map(|thread| {
            if thread.has_pending_approvals {
                Some(entry(thread, DesktopNotificationKind::Approval, false))
            } else if thread.has_pending_user_input {
                Some(entry(thread, DesktopNotificationKind::Input, false))
            } else {
                None
            }
        });
        let done = self.snapshot.threads.iter().find(|t| !t.has_pending_approvals && !t.has_pending_user_input);
        waiting.chain(done.map(|t| entry(t, DesktopNotificationKind::Completed, true))).collect()
    }

    /// Any sidebar snapshot, each thread with a short timeline.
    pub fn from_sidebar(snapshot: SidebarSnapshot) -> Self {
        Self::from_snapshot(snapshot, |index| 4 + index % 5)
    }

    fn from_snapshot(snapshot: SidebarSnapshot, turns: impl Fn(usize) -> usize) -> Self {
        let titles: HashMap<_, _> =
            snapshot.projects.iter().map(|p| (p.project_ref.project_id.clone(), (p.title.clone(), p.workspace_root.clone()))).collect();
        let threads = snapshot
            .threads
            .iter()
            .enumerate()
            .map(|(index, thread)| {
                let (project, cwd) = titles.get(&thread.project_id).cloned().unwrap_or_default();
                let record = ThreadRecord {
                    project,
                    title: thread.title.clone(),
                    cwd,
                    timeline: None,
                    turns: turns(index),
                    pending_approval: thread.has_pending_approvals,
                    pending_question: thread.has_pending_user_input,
                    next_id: 0,
                };
                (thread.key.clone(), record)
            })
            .collect();
        Self {
            snapshot,
            threads,
            drafts: HashMap::new(),
            streams: HashMap::new(),
            queued: HashMap::new(),
            next_draft: 0,
            next_thread: 0,
            clock: NOW_MS,
        }
    }

    /// The sidebar's data, with `active` marked.
    pub fn snapshot(&self, active: Option<&ThreadKey>) -> SidebarSnapshot {
        SidebarSnapshot { active_thread: active.cloned(), ..self.snapshot.clone() }
    }

    /// The most recently updated project (`_chat.index.tsx` starts a draft there).
    pub fn most_recent_project(&self) -> Option<ProjectRef> {
        self.snapshot
            .projects
            .iter()
            .filter(|p| !p.archived)
            .max_by_key(|p| {
                self.snapshot
                    .threads
                    .iter()
                    .filter(|t| t.project_id == p.project_ref.project_id)
                    .map(|t| t.updated_at)
                    .max()
                    .unwrap_or(p.updated_at)
            })
            .map(|p| p.project_ref.clone())
    }

    pub fn thread_exists(&self, target: &ThreadTarget) -> bool {
        match target {
            ThreadTarget::Server(key) => self.threads.contains_key(key),
            ThreadTarget::Draft(id) => self.drafts.contains_key(id),
        }
    }

    /// Every thread key, for pruning per-thread state.
    pub fn thread_keys(&self) -> impl Iterator<Item = &ThreadKey> {
        self.threads.keys()
    }

    /// Everything the chat view shows for `target`, building the timeline on first open.
    pub fn thread_view(&mut self, target: &ThreadTarget) -> Option<ThreadView> {
        match target {
            ThreadTarget::Server(key) => {
                let record = self.threads.get_mut(key)?;
                if record.timeline.is_none() {
                    record.timeline = Some(build_timeline(key, record));
                }
                let mut timeline = record.timeline.clone().unwrap_or_default();
                if let Some(stream) = self.streams.get(key) {
                    set_message_text(&mut timeline, &stream.message, stream.text.as_str().into(), true);
                }
                if let Some(queue) = self.queued.get(key) {
                    timeline.queued = queue.iter().cloned().collect::<Vec<_>>().into();
                }
                Some(ThreadView {
                    key: scoped_key(key),
                    project: record.project.clone(),
                    title: record.title.clone(),
                    cwd: record.cwd.clone(),
                    running: self.streams.contains_key(key),
                    timeline,
                    is_draft: false,
                })
            }
            ThreadTarget::Draft(id) => {
                let draft = self.drafts.get(id)?;
                let project = draft.project.as_ref().and_then(|p| self.snapshot.projects.iter().find(|x| x.project_ref == *p));
                Some(ThreadView {
                    key: format!("draft:{}", id.0).into(),
                    project: project.map(|p| p.title.clone()).unwrap_or_default(),
                    title: "New thread".into(),
                    cwd: project.map(|p| p.workspace_root.clone()).unwrap_or_default(),
                    timeline: TimelineInput::default(),
                    is_draft: true,
                    running: false,
                })
            }
        }
    }

    /// Starts a draft thread in `project`.
    pub fn create_draft(&mut self, project: Option<ProjectRef>) -> DraftId {
        self.next_draft += 1;
        let id = DraftId(format!("draft-{}", self.next_draft).into());
        self.drafts.insert(id.clone(), DraftRecord { project });
        id
    }

    pub fn discard_draft(&mut self, id: &DraftId) {
        self.drafts.remove(id);
    }

    /// Sends `prompt` in `target`. A draft becomes a server thread first.
    /// While a reply streams, `Queue` holds the message for later and `Steer`
    /// appends it to the running turn.
    pub fn submit(
        &mut self,
        target: &ThreadTarget,
        prompt: String,
        dispatch: Option<FollowUpBehavior>,
        cx: &mut Context<Self>,
    ) -> Option<ThreadKey> {
        let key = match target {
            ThreadTarget::Server(key) => key.clone(),
            ThreadTarget::Draft(id) => {
                let key = self.promote_draft(id, &prompt)?;
                cx.emit(FixtureEvent::DraftPromoted { draft: id.clone(), thread: key.clone() });
                key
            }
        };
        self.clock += 1_000;
        if self.streams.contains_key(&key) && dispatch == Some(FollowUpBehavior::Queue) {
            let queue = self.queued.entry(key.clone()).or_default();
            queue.push_back(QueuedMessage {
                id: format!("{}:queued-{}", key.thread_id.0, self.clock).into(),
                text: prompt.into(),
                created_at: Timestamp(self.clock),
                image_count: 0,
                guest: false,
                editing: false,
            });
            cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
            return Some(key);
        }
        let steering = self.streams.contains_key(&key);
        self.answer_pending(&key);
        self.append_user(&key, &prompt, if steering { InputIntent::Steer } else { InputIntent::Prompt });
        if !steering {
            self.start_reply(&key, &prompt, cx);
        }
        cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
        Some(key)
    }

    /// Stops the streaming reply in `key`, keeping what arrived.
    pub fn stop(&mut self, key: &ThreadKey, cx: &mut Context<Self>) {
        if let Some(stream) = self.streams.remove(key) {
            self.finish_reply(key, stream.message, stream.text, RunStatus::Interrupted, cx);
        }
    }

    /// Answers the pending question in `key` with option `index`.
    pub fn answer_question(&mut self, key: &ThreadKey, index: usize, cx: &mut Context<Self>) {
        let Some(record) = self.threads.get_mut(key) else { return };
        let Some(timeline) = record.timeline.as_mut() else { return };
        let Some(question) = timeline.pending_question.take() else { return };
        let answer = question.options.get(index).map(|(label, _)| label.clone()).unwrap_or_default();
        record.pending_question = false;
        let entries = std::mem::take(Arc::make_mut(&mut timeline.entries));
        let mut b = TimelineBuilder::resume(key.thread_id.0.clone(), entries, record.next_id);
        let run = timeline.latest_run.as_ref().map(|r| r.run_id.clone()).unwrap_or_else(|| b.run_id());
        b.work(
            Timestamp(self.clock),
            &run,
            ToolStatus::Completed,
            "Answered",
            WorkItem::QuestionAnswer { pairs: Arc::from([(question.question.clone(), answer)]) },
        );
        record.next_id = b.next_index();
        timeline.entries = Arc::new(b.entries);
        timeline.latest_run = None;
        self.set_flags(key, |t| t.has_pending_user_input = false);
        cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
        cx.emit(FixtureEvent::SnapshotChanged);
    }

    fn promote_draft(&mut self, id: &DraftId, prompt: &str) -> Option<ThreadKey> {
        let draft = self.drafts.remove(id)?;
        let project_ref = draft.project.or_else(|| self.most_recent_project())?;
        let project = self.snapshot.projects.iter().find(|p| p.project_ref == project_ref)?;
        self.next_thread += 1;
        let key = ThreadKey {
            environment_id: project_ref.environment_id.clone(),
            thread_id: ThreadId(format!("new-{}", self.next_thread).into()),
        };
        let title: SharedString = prompt.lines().next().unwrap_or("New thread").chars().take(48).collect::<String>().into();
        let now = TimestampMs(self.clock);
        let thread = SidebarThread {
            key: key.clone(),
            project_id: project_ref.project_id.clone(),
            title: title.clone(),
            created_at: now,
            updated_at: now,
            latest_user_message_at: Some(now),
            latest_user_authored_message_at: Some(now),
            last_visited_at: Some(now),
            ..blank_thread(&key)
        };
        self.snapshot.threads.push(Arc::new(thread));
        self.threads.insert(
            key.clone(),
            ThreadRecord {
                project: project.title.clone(),
                title,
                cwd: project.workspace_root.clone(),
                timeline: Some(TimelineInput::default()),
                turns: 0,
                pending_approval: false,
                pending_question: false,
                next_id: 0,
            },
        );
        Some(key)
    }

    fn answer_pending(&mut self, key: &ThreadKey) {
        let Some(record) = self.threads.get_mut(key) else { return };
        let Some(timeline) = record.timeline.as_mut() else { return };
        // Sending while an approval waits declines it and redirects the agent.
        timeline.pending_approval_ids.clear();
        timeline.pending_question = None;
        record.pending_approval = false;
        record.pending_question = false;
        self.set_flags(key, |t| {
            t.has_pending_approvals = false;
            t.has_pending_user_input = false;
        });
    }

    fn append_user(&mut self, key: &ThreadKey, prompt: &str, intent: InputIntent) {
        let clock = self.clock;
        let Some(record) = self.threads.get_mut(key) else { return };
        let timeline = record.timeline.get_or_insert_with(TimelineInput::default);
        let entries = std::mem::take(Arc::make_mut(&mut timeline.entries));
        let mut b = TimelineBuilder::resume(key.thread_id.0.clone(), entries, record.next_id);
        b.user(Timestamp(clock), prompt.to_string(), intent);
        record.next_id = b.next_index();
        timeline.entries = Arc::new(b.entries);
    }

    fn start_reply(&mut self, key: &ThreadKey, prompt: &str, cx: &mut Context<Self>) {
        let clock = self.clock;
        let Some(record) = self.threads.get_mut(key) else { return };
        let timeline = record.timeline.get_or_insert_with(TimelineInput::default);
        let entries = std::mem::take(Arc::make_mut(&mut timeline.entries));
        let mut b = TimelineBuilder::resume(key.thread_id.0.clone(), entries, record.next_id);
        let run = b.run_id();
        let message = b.assistant(Timestamp(clock + 500), &run, "", true);
        record.next_id = b.next_index();
        timeline.entries = Arc::new(b.entries);
        timeline.latest_run =
            Some(LatestRun { run_id: run, status: RunStatus::Running, started_at: Some(Timestamp(clock)), completed_at: None, turn_ended: false });
        timeline.is_working = true;
        timeline.active_turn_started_at = Some(Timestamp(clock));

        let reply = timelines::canned_reply(prompt);
        let thread = key.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REPLY_DELAY).await;
            let mut sent = 0;
            while sent < reply.len() {
                let mut end = (sent + STREAM_CHUNK).min(reply.len());
                while !reply.is_char_boundary(end) {
                    end += 1;
                }
                sent = end;
                let text = reply[..sent].to_string();
                let alive = this
                    .update(cx, |this, cx| {
                        let Some(stream) = this.streams.get_mut(&thread) else { return false };
                        stream.text.clone_from(&text);
                        let message = stream.message.clone();
                        cx.emit(FixtureEvent::Delta { thread: thread.clone(), message, text: text.into() });
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    return;
                }
                cx.background_executor().timer(STREAM_TICK).await;
            }
            this.update(cx, |this, cx| {
                if let Some(stream) = this.streams.remove(&thread) {
                    this.finish_reply(&thread, stream.message, stream.text, RunStatus::Completed, cx);
                }
            })
            .ok();
        });
        self.streams.insert(key.clone(), Stream { message, text: String::new(), _task: task });
        self.set_flags(key, |t| {
            t.runtime = Some(sidebar::ThreadRuntime {
                status: sidebar::RuntimeStatus::Run(sidebar::RunStatus::Running),
                last_error_class: None,
            });
            t.latest_run = Some(sidebar::LatestRun {
                status: sidebar::RunStatus::Running,
                requested_at: Some(TimestampMs(clock)),
                started_at: Some(TimestampMs(clock)),
                completed_at: None,
            });
            t.updated_at = TimestampMs(clock);
            t.latest_user_message_at = Some(TimestampMs(clock));
        });
        cx.emit(FixtureEvent::SnapshotChanged);
    }

    fn finish_reply(&mut self, key: &ThreadKey, message: MessageId, text: String, status: RunStatus, cx: &mut Context<Self>) {
        self.clock += 2_000;
        let clock = self.clock;
        if let Some(timeline) = self.threads.get_mut(key).and_then(|r| r.timeline.as_mut()) {
            set_message_text(timeline, &message, text.into(), false);
            if let Some(run) = timeline.latest_run.as_mut() {
                run.status = status;
                run.completed_at = Some(Timestamp(clock));
                run.turn_ended = true;
            }
            timeline.is_working = false;
            timeline.active_turn_started_at = None;
        }
        let status = match status {
            RunStatus::Interrupted => sidebar::RunStatus::Interrupted,
            _ => sidebar::RunStatus::Completed,
        };
        self.set_flags(key, |t| {
            t.runtime = Some(sidebar::ThreadRuntime { status: sidebar::RuntimeStatus::Run(status), last_error_class: None });
            if let Some(run) = t.latest_run.as_mut() {
                run.status = status;
                run.completed_at = Some(TimestampMs(clock));
            }
            t.updated_at = TimestampMs(clock);
            t.last_visited_at = Some(TimestampMs(clock));
        });
        cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
        cx.emit(FixtureEvent::SnapshotChanged);
        // The next queued message sends once the turn ends.
        if let Some(next) = self.queued.get_mut(key).and_then(VecDeque::pop_front) {
            let prompt = next.text.to_string();
            self.append_user(key, &prompt, InputIntent::Prompt);
            self.start_reply(key, &prompt, cx);
            cx.emit(FixtureEvent::ThreadChanged(ThreadTarget::Server(key.clone())));
        }
    }

    /// Edits one sidebar row.
    fn set_flags(&mut self, key: &ThreadKey, edit: impl FnOnce(&mut SidebarThread)) {
        if let Some(thread) = self.snapshot.threads.iter_mut().find(|t| t.key == *key) {
            edit(Arc::make_mut(thread));
        }
    }
}

/// `environmentId:threadId`, as the client runtime keys threads.
pub fn scoped_key(key: &ThreadKey) -> SharedString {
    format!("{}:{}", key.environment_id.0, key.thread_id.0).into()
}

fn build_timeline(key: &ThreadKey, record: &mut ThreadRecord) -> TimelineInput {
    // The showcase's working thread carries every row kind.
    let mut input = if key.thread_id.0 == "a1" {
        fixture_thread()
    } else if key.thread_id.0 == "r-sync" {
        timelines::reference_thread("r-sync")
    } else {
        timelines::finished_thread(&key.thread_id.0, &record.title, record.turns)
    };
    if record.pending_approval {
        input = timelines::with_pending_approval(input, &key.thread_id.0);
    }
    if record.pending_question {
        input = timelines::with_pending_question(input, &key.thread_id.0);
    }
    record.next_id = input.entries.len() + 8;
    input
}

fn set_message_text(input: &mut TimelineInput, id: &MessageId, text: SharedString, streaming: bool) {
    let entries = Arc::make_mut(&mut input.entries);
    if let Some(message) = entries.iter_mut().rev().find_map(|entry| match &mut entry.kind {
        EntryKind::Message(m) if m.id == *id => Some(m),
        _ => None,
    }) {
        message.text = text;
        message.streaming = streaming;
    }
}

fn blank_thread(key: &ThreadKey) -> SidebarThread {
    SidebarThread {
        key: key.clone(),
        project_id: sidebar::ProjectId(SharedString::default()),
        title: SharedString::default(),
        branch: None,
        in_worktree: false,
        created_at: TimestampMs(0),
        updated_at: TimestampMs(0),
        latest_user_message_at: None,
        latest_user_authored_message_at: None,
        runtime: None,
        latest_run: None,
        blocked_on_thread_wait: false,
        has_pending_approvals: false,
        has_pending_user_input: false,
        pending_subagent_request: None,
        pending_guest_message_count: 0,
        goal_active: false,
        last_visited_at: None,
        has_unread_completion_notification: false,
        settled_override: None,
        settled_at: None,
        unsettled_at: None,
        snoozed_until: None,
        woke_at: None,
        pinned_at: None,
        pin_order_key: None,
        active_order_key: None,
        archived: false,
        has_unsent_draft: false,
        diff: None,
        pull_request: None,
        issue_key: None,
        running_terminal_count: 0,
        remote_machine: false,
        provider: None,
    }
}

/// The environment fixtures live in.
pub fn fixture_environment() -> EnvironmentId {
    EnvironmentId("env-local".into())
}
