//! One open thread on the GPUI side (`threadSync.ts`): follows a runtime [`ThreadHandle`] and
//! keeps the timeline's view model in step.
//!
//! Updates arrive at most once per frame. While only assistant or reasoning text streams
//! ([`QuietThread`]), the thread emits [`LiveThreadEvent::Text`] with the changed messages so
//! the timeline patches one row; any other change emits [`LiveThreadEvent::Structure`] and the
//! timeline takes the whole input.

use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{Context, EventEmitter, SharedString, Task};
use t3code_client_runtime::state::models::now_ms;
use t3code_client_runtime::state::optimistic_messages::{OptimisticMessage, OptimisticMessages};
use t3code_client_runtime::state::thread_detail::QuietThread;
use t3code_client_runtime::state::threads::LoadEarlierResult;
use t3code_client_runtime::state::{ThreadHandle, ThreadState, ThreadStatus};
use t3code_contracts::{MessageId as RuntimeMessageId, TurnItemId};
use t3code_shared::thread_projection::ThreadProjection;

use t3code_contracts::{WorktreeSetupPhase as Phase, WorktreeSetupSnapshot, WorktreeSetupStageId as StageId, WorktreeSetupStageStatus as StageStatus};

use crate::components::chat::timeline_entries::{EntryKind, MessageId, TimelineInput, Timestamp, ToolStatus, WorktreeSetup};
use crate::i18n::t;
use crate::components::sidebar::model::ThreadKey;
use crate::session_logic::{TimelineMapper, changed_message_text, timeline_input};
use crate::state::follow_watch;

/// What changed in a [`LiveThread`].
#[derive(Clone, Debug)]
pub enum LiveThreadEvent {
    /// The rows changed; read [`LiveThread::input`].
    Structure,
    /// Only these messages' text (or streaming flag) changed.
    Text(Arc<[(MessageId, SharedString, bool)]>),
    /// The sync status changed (cached, live, deleted, error) without new rows.
    Status,
}

/// The mapped view model of a thread, kept while the thread is closed so a revisit does not
/// map again.
pub struct MappedThread {
    pub projection: Option<Arc<ThreadProjection>>,
    pub input: TimelineInput,
    pub mapper: TimelineMapper,
}

/// Counters for measurements.
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadUpdateStats {
    pub structure: u64,
    pub text: u64,
    pub status: u64,
    /// Time spent mapping, in microseconds.
    pub map_micros: u64,
}

/// An open thread. Dropping it unmounts the runtime handle (the runtime keeps the state in its
/// resume cache).
pub struct LiveThread {
    key: ThreadKey,
    handle: Arc<ThreadHandle>,
    state: Arc<ThreadState>,
    quiet: QuietThread,
    mapped: MappedThread,
    optimistic: OptimisticMessages,
    stats: ThreadUpdateStats,
    /// Attachment images already asked for, so each downloads once.
    requested_images: std::collections::HashSet<String>,
    /// The newest worktree setup snapshot the server streamed for this thread (`heldSnapshot`).
    worktree: Option<Arc<WorktreeSetupSnapshot>>,
    /// The `subscribeWorktreeSetup` stream, once the thread has a worktree or prepares one.
    worktree_task: Option<Task<()>>,
    _follow: Task<()>,
}

impl EventEmitter<LiveThreadEvent> for LiveThread {}

impl LiveThread {
    /// Opens `handle`. The first value is mapped now (or taken from `cached` when it still
    /// describes the same projection), so the caller can paint it in this frame.
    pub fn new(key: ThreadKey, handle: ThreadHandle, cached: Option<MappedThread>, cx: &mut Context<Self>) -> Self {
        let state = handle.state();
        let mut mapped = cached.unwrap_or_else(|| MappedThread { projection: None, input: TimelineInput::default(), mapper: TimelineMapper::default() });
        let mut quiet = QuietThread::default();
        let mut stats = ThreadUpdateStats::default();
        let same = match (&mapped.projection, &state.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            let started = Instant::now();
            mapped.input = state.data.as_deref().map(|data| timeline_input(&mut mapped.mapper, data, &[])).unwrap_or_default();
            mapped.projection = state.data.clone();
            stats.map_micros += started.elapsed().as_micros() as u64;
        }
        if let Some(data) = &state.data {
            quiet.update(data);
        }
        let handle = Arc::new(handle);
        let weak = cx.weak_entity();
        let follow = follow_watch(handle.receiver(), cx, move |state, cx| weak.update(cx, |this, cx| this.apply(state, cx)).is_ok());
        let mut this = Self {
            key,
            handle,
            state,
            quiet,
            mapped,
            optimistic: OptimisticMessages::default(),
            stats,
            requested_images: Default::default(),
            worktree: None,
            worktree_task: None,
            _follow: follow,
        };
        this.fetch_images(cx);
        this.follow_worktree_setup(cx);
        this
    }

    /// Subscribes to the thread's worktree setup once it has a worktree or a run prepares one
    /// (`useWorktreeSetup`'s query condition). Snapshots of a lower sequence never replace a newer one.
    fn follow_worktree_setup(&mut self, cx: &mut Context<Self>) {
        use futures::StreamExt as _;
        use t3code_contracts::rpc::methods::SubscribeWorktreeSetup;
        if self.worktree_task.is_some() {
            return;
        }
        let Some(data) = &self.state.data else { return };
        let preparing = data.runs.iter().any(|run| run.status == t3code_contracts::OrchestrationV2RunStatus::Preparing);
        if data.thread.worktree_path.is_none() && !preparing {
            return;
        }
        let (environment_id, thread_id) = crate::state::shell::runtime_ids(&self.key);
        let Some(link) = crate::live::rpc::RpcLink::new(&environment_id, cx) else { return };
        let mut stream = link.stream::<SubscribeWorktreeSetup>(t3code_contracts::WorktreeSetupSubscribeInput { thread_id: thread_id.clone() });
        self.worktree_task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = stream.next().await {
                let Ok(Some(snapshot)) = event else { continue };
                if snapshot.thread_id != thread_id {
                    continue;
                }
                let kept = this.update(cx, |this, cx| {
                    if this.worktree.as_ref().is_some_and(|held| held.sequence > snapshot.sequence) {
                        return;
                    }
                    this.worktree = Some(Arc::new(snapshot));
                    this.remap();
                    this.stats.structure += 1;
                    cx.emit(LiveThreadEvent::Structure);
                    cx.notify();
                });
                if kept.is_err() {
                    return;
                }
            }
        }));
    }

    /// Downloads the images of sent messages that are not in the image cache yet
    /// (`assets.createUrl` for the attachment), then maps the rows again so they show.
    fn fetch_images(&mut self, cx: &mut Context<Self>) {
        use t3code_contracts::{ChatAttachment, OrchestrationV2TurnItem as Item};
        let Some(data) = &self.state.data else { return };
        let missing: Vec<(String, String)> = data
            .visible_turn_items
            .iter()
            .filter_map(|row| match &*row.item {
                Item::UserMessage(user) => Some(user.attachments.as_slice()),
                Item::AssistantMessage(reply) => reply.attachments.as_deref(),
                _ => None,
            })
            .flatten()
            .filter_map(|attachment| match attachment {
                ChatAttachment::Image(image) => Some((image.id.clone(), image.mime_type.clone())),
                _ => None,
            })
            .filter(|(id, mime)| !self.requested_images.contains(id) && crate::state::attachments::cached_image(id, mime).is_none())
            .collect();
        if missing.is_empty() {
            return;
        }
        let (environment_id, _) = crate::state::shell::runtime_ids(&self.key);
        let Some(link) = crate::live::rpc::RpcLink::new(&environment_id, cx) else { return };
        self.requested_images.extend(missing.iter().map(|(id, _)| id.clone()));
        let runner = link.clone();
        let task = runner.run(async move {
            use t3code_contracts::rpc::methods::AssetsCreateUrl;
            use t3code_contracts::{AssetCreateUrlInput, AssetResource, AssetResourceAttachment};
            let session = link.session().await?;
            let mut fetched = 0;
            for (attachment_id, mime) in missing {
                let resource = AssetResource::Attachment(AssetResourceAttachment {
                    attachment_id: attachment_id.clone(),
                    file_name: None,
                    mime_type: None,
                    disposition: None,
                    render_intent: None,
                });
                let Ok(url) = session.client.call::<AssetsCreateUrl>(AssetCreateUrlInput { resource }).await else { continue };
                if let Ok(bytes) = link.link.fetch_relative(reqwest::Method::GET, &url.relative_url, None).await {
                    crate::state::attachments::cache_image(&attachment_id, &mime, &bytes);
                    fetched += 1;
                }
            }
            Ok::<_, SharedString>(fetched)
        });
        cx.spawn(async move |this, cx| {
            if let Ok(fetched) = task.await
                && fetched > 0
            {
                _ = this.update(cx, |this, cx| {
                    // Rows are cached by the mapper; start over so the new sources show.
                    this.mapped.mapper = TimelineMapper::default();
                    this.remap();
                    this.stats.structure += 1;
                    cx.emit(LiveThreadEvent::Structure);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn key(&self) -> &ThreadKey {
        &self.key
    }

    /// The timeline's view model as of the last update.
    pub fn input(&self) -> &TimelineInput {
        &self.mapped.input
    }

    pub fn state(&self) -> &Arc<ThreadState> {
        &self.state
    }

    pub fn status(&self) -> ThreadStatus {
        self.state.status
    }

    pub fn projection(&self) -> Option<&Arc<ThreadProjection>> {
        self.state.data.as_ref()
    }

    pub fn stats(&self) -> ThreadUpdateStats {
        self.stats
    }

    pub fn handle(&self) -> &Arc<ThreadHandle> {
        &self.handle
    }

    /// Takes the mapped view model, leaving an empty one; for the cache on close.
    pub fn take_mapped(&mut self) -> MappedThread {
        std::mem::replace(
            &mut self.mapped,
            MappedThread { projection: None, input: TimelineInput::default(), mapper: TimelineMapper::default() },
        )
    }

    fn remap(&mut self) {
        let started = Instant::now();
        if let Some(data) = &self.state.data {
            self.mapped.input = timeline_input(&mut self.mapped.mapper, data, self.optimistic.messages());
        } else {
            self.mapped.input = TimelineInput::default();
        }
        self.mapped.input.worktree_setup = self.state.data.as_deref().and_then(|data| visible_worktree_setup(self.worktree.as_deref(), data, &self.mapped.input));
        self.mapped.projection = self.state.data.clone();
        self.stats.map_micros += started.elapsed().as_micros() as u64;
    }

    fn apply(&mut self, state: Arc<ThreadState>, cx: &mut Context<Self>) {
        if Arc::ptr_eq(&state, &self.state) {
            return;
        }
        let previous = std::mem::replace(&mut self.state, state);
        let same_data = match (&previous.data, &self.state.data) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same_data {
            self.stats.status += 1;
            cx.emit(LiveThreadEvent::Status);
            cx.notify();
            return;
        }
        let thread_id = self.handle.thread_id().clone();
        let reconciled = self.optimistic.reconcile(&thread_id, self.state.data.as_deref());
        let structure_changed = match &self.state.data {
            Some(data) => self.quiet.update(data),
            None => true,
        };
        let before = Arc::clone(&self.mapped.input.entries);
        self.remap();
        self.fetch_images(cx);
        self.follow_worktree_setup(cx);
        if structure_changed || reconciled {
            self.stats.structure += 1;
            cx.emit(LiveThreadEvent::Structure);
        } else {
            let changes = changed_message_text(&before, &self.mapped.input.entries);
            self.stats.text += 1;
            cx.emit(LiveThreadEvent::Text(changes.into()));
        }
        cx.notify();
    }

    /// Shows a sent message at once, until the server's copy arrives.
    pub fn push_optimistic(&mut self, message_id: RuntimeMessageId, text: String, attachments: Vec<t3code_contracts::ChatAttachment>, cx: &mut Context<Self>) {
        let thread_id = self.handle.thread_id().clone();
        self.optimistic.push(&thread_id, OptimisticMessage { message_id, text, attachments, created_at: now_ms() });
        self.remap();
        self.stats.structure += 1;
        cx.emit(LiveThreadEvent::Structure);
        cx.notify();
    }

    /// Drops a local message whose send failed.
    pub fn remove_optimistic(&mut self, message_id: &RuntimeMessageId, cx: &mut Context<Self>) {
        self.optimistic.remove(message_id);
        self.remap();
        cx.emit(LiveThreadEvent::Structure);
        cx.notify();
    }

    /// Pages older history (`loadEarlier`); the new rows arrive as a structure update.
    pub fn load_earlier(&self, cx: &mut Context<Self>) -> Task<LoadEarlierResult> {
        let handle = Arc::clone(&self.handle);
        cx.background_executor().spawn(async move { handle.load_earlier().await })
    }

    /// Loads the hidden rows of a collapsed work group.
    pub fn expand_work_group(&self, anchor: TurnItemId, cx: &mut Context<Self>) -> Task<LoadEarlierResult> {
        let handle = Arc::clone(&self.handle);
        cx.background_executor().spawn(async move { handle.expand_work_group(anchor).await })
    }
}

/// The setup card the timeline shows, if any (`resolveVisibleWorktreeSetup` with no recorded
/// snapshot): while running; until a follow-up is sent, when it did not finish cleanly or before
/// any run started; after that only when a stage failed.
fn visible_worktree_setup(snapshot: Option<&WorktreeSetupSnapshot>, data: &ThreadProjection, input: &TimelineInput) -> Option<WorktreeSetup> {
    let snapshot = snapshot?;
    let follow_up_sent = input.entries.iter().filter(|entry| matches!(&entry.kind, EntryKind::Message(message) if message.role == crate::components::chat::timeline_entries::MessageRole::User)).count() > 1;
    let shown = match snapshot.phase {
        Phase::Running => true,
        _ if follow_up_sent => false,
        Phase::Done => !data.runs.iter().any(|run| run.started_at.is_some()) || snapshot.stages.iter().any(|stage| stage.status == StageStatus::Failed),
        _ => true,
    };
    if !shown {
        return None;
    }
    let ms = |iso: Option<&str>| iso.and_then(t3code_client_runtime::state::models::parse_ms).map(Timestamp);
    let text = |value: &Option<String>| value.clone().map(SharedString::from);
    let script_name = snapshot.setup_script.as_ref().map(|script| SharedString::from(script.name.clone()));
    let stages = snapshot
        .stages
        .iter()
        .map(|stage| {
            use crate::components::chat::timeline_entries::{SetupStage, SetupStageId as Id, SetupStageStatus as Status};
            let id = match stage.id {
                StageId::Fetch => Id::Fetch,
                StageId::Checkout => Id::Checkout,
                StageId::Submodules => Id::Submodules,
                StageId::SetupScript => Id::SetupScript,
                _ => Id::Agent,
            };
            // `worktreeSetupStageLabel`, or the script's own name for its stage.
            let label = match (id, &script_name) {
                (Id::SetupScript, Some(name)) => name.clone(),
                (Id::Fetch, _) => t!("Fetch base branch"),
                (Id::Checkout, _) => t!("Check out files"),
                (Id::Submodules, _) => t!("Init submodules"),
                (Id::SetupScript, _) => t!("Run setup script"),
                (Id::Agent, _) => t!("Start agent"),
            };
            let status = match stage.status {
                StageStatus::Pending => Status::Pending,
                StageStatus::Running => Status::Running,
                StageStatus::Done => Status::Done,
                StageStatus::Skipped => Status::Skipped,
                StageStatus::Warning => Status::Warning,
                StageStatus::Failed => Status::Failed,
                // A status from a newer server reads as not started.
                _ => Status::Pending,
            };
            SetupStage {
                id,
                label,
                status,
                started_at: ms(stage.started_at.as_deref()),
                ended_at: ms(stage.ended_at.as_deref()),
                percent: stage.percent,
                detail: text(&stage.detail),
                tail: stage.tail.iter().cloned().map(SharedString::from).collect(),
            }
        })
        .collect();
    use crate::components::chat::timeline_entries::{SetupPhase, SetupScript};
    Some(WorktreeSetup {
        phase: match snapshot.phase {
            Phase::Running => SetupPhase::Running,
            Phase::Done => SetupPhase::Done,
            Phase::Failed => SetupPhase::Failed,
            _ => SetupPhase::Cancelled,
        },
        started_at: Timestamp(t3code_client_runtime::state::models::parse_ms(&snapshot.started_at).unwrap_or_default()),
        ended_at: ms(snapshot.ended_at.as_deref()),
        branch: text(&snapshot.branch),
        base_ref: text(&snapshot.base_ref),
        worktree_path: text(&snapshot.worktree_path),
        setup_script: snapshot.setup_script.as_ref().map(|script| SetupScript {
            name: script.name.clone().into(),
            command: script.command.clone().into(),
            terminal_id: script.terminal_id.clone().into(),
        }),
        stages,
        error: text(&snapshot.error),
        embedded: false,
    })
}
