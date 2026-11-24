//! Every thread and project command as a typed method (`operations/commands.ts`,
//! `state/threadCommands.ts`). Each carries a fresh client `commandId`, so the server dedupes a
//! resend; replies carry the sequence that confirms them.
//!
//! Commands on one thread run one at a time. Settle, snooze, pin and the other row actions show
//! at once through the shell's optimistic layer, which retires once a snapshot reaches the
//! reply's sequence.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use t3code_contracts::rpc::methods::{
    OrchestrationDispatchCommand, OrchestrationGetThreadProjection, OrchestrationLaunchThread, ProjectsMutate,
};
use t3code_contracts::{
    CheckpointId, CheckpointScopeId, ChatAttachment, CommandId, GuestMessageId, MessageId, ModelSelection, NodeId,
    OrchestrationMessageContext, OrchestrationV2Actor, OrchestrationV2Command as Command, OrchestrationV2CommandMessageDispatch,
    OrchestrationV2CommandMessageDispatchDeliveryIntent as DeliveryIntent, OrchestrationV2CommandMessageDispatchDispatchMode as DispatchMode,
    OrchestrationV2CommandMessageDispatchDispatchModeQueueAfterActive, OrchestrationV2CommandMessageDispatchDispatchModeStartImmediately,
    OrchestrationV2CommandThreadUnsettleReason, OrchestrationV2CommandThreadUnsnoozeReason, OrchestrationV2CreationSource,
    OrchestrationV2GetThreadProjectionInput, OrchestrationV2PendingRuntimeRequestSummaryKind, OrchestrationV2RunStatus,
    OrchestrationV2ShellThreadStatus as ShellStatus, OrchestrationV2ThreadForkSourcePoint, OrchestrationV2ThreadLaunchInput,
    OrchestrationV2ThreadLaunchResult, OrchestrationV2ThreadProjection, OrchestrationV2ThreadShell,
    OrchestrationV2ThreadShellSettledOverride as Settled, Project, ProjectId, ProjectMutation, ProviderApprovalDecision,
    ProviderInteractionMode, ProviderUserInputAnswers, RunId, RuntimeMode, RuntimeRequestId, ThreadId,
};
use tokio::sync::watch;

use crate::connection::{ConnectionPhase, ConnectionStatus};
use crate::rpc::{RpcError, RpcSession};
use crate::state::environment::EnvironmentStore;
use crate::state::models::{format_ms, now_ms, parse_ms};
use crate::state::thread_lifecycle::ApplyUpdate;

/// The creation source this client sends. The contract has no desktop literal; the web and
/// Electron desktop send `"web"` too.
pub const CREATION_SOURCE: OrchestrationV2CreationSource = OrchestrationV2CreationSource::Web;

/// A sent message is resent on a new socket at most this many times.
const MAX_LOST_TRANSPORT_RESENDS: u32 = 2;
/// A live socket can still carry a command the host never answers.
const MESSAGE_DISPATCH_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a command waits for a session when none is connected.
const SESSION_WAIT: Duration = Duration::from_secs(30);

/// How a command failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CommandError {
    #[error("the environment is not connected")]
    NotConnected,
    #[error("the host did not confirm the command within 60 seconds")]
    TimedOut,
    #[error("the connection was lost before the host answered")]
    TransportLost,
    #[error("the host refused the command: {0}")]
    Refused(String),
}

fn transport_lost<E: std::fmt::Debug>(error: &RpcError<E>) -> bool {
    matches!(error, RpcError::Closed(_) | RpcError::Interrupted)
}

/// A refusal carries the server error's own `message` when it has one, as the web shows it.
fn command_error<E: std::fmt::Debug + serde::Serialize>(error: RpcError<E>) -> CommandError {
    if transport_lost(&error) {
        return CommandError::TransportLost;
    }
    let message = match &error {
        RpcError::Failure(failure) => serde_json::to_value(failure).ok().as_ref().and_then(server_message),
        _ => None,
    };
    CommandError::Refused(message.unwrap_or_else(|| error.to_string()))
}

/// The first non-empty `message` in a server error body.
fn server_message(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => map
            .get("message")
            .and_then(|m| m.as_str())
            .filter(|m| !m.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| map.values().find_map(server_message)),
        serde_json::Value::Array(items) => items.iter().find_map(server_message),
        _ => None,
    }
}

/// A fresh client command id (UUIDv4).
pub fn new_command_id() -> CommandId {
    CommandId::new(uuid::Uuid::new_v4().to_string())
}

/// How a message enters a thread (`deliveryIntent` plus `dispatchMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// The server picks: start when idle, steer or queue when running.
    Auto,
    Steer,
    Restart,
    /// Wait for the active run.
    Queue,
}

/// A message to send (`message.dispatch`).
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub thread_id: ThreadId,
    pub message_id: MessageId,
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
    pub context: Option<OrchestrationMessageContext>,
    pub mode: DeliveryMode,
    pub model_selection: Option<ModelSelection>,
    pub title_seed: Option<String>,
    pub fresh_context: bool,
    /// Explicit continuation of a run; empty text becomes "Continue where you left off."
    pub manual_continuation_of_run_id: Option<RunId>,
}

impl SendMessage {
    /// A plain text message with the server choosing delivery.
    pub fn text(thread_id: ThreadId, text: impl Into<String>) -> Self {
        Self {
            thread_id,
            message_id: MessageId::new(uuid::Uuid::new_v4().to_string()),
            text: text.into(),
            attachments: Vec::new(),
            context: None,
            mode: DeliveryMode::Auto,
            model_selection: None,
            title_seed: None,
            fresh_context: false,
            manual_continuation_of_run_id: None,
        }
    }
}

/// A new empty thread (`thread.create`).
#[derive(Debug, Clone)]
pub struct CreateThread {
    pub thread_id: ThreadId,
    pub project_id: ProjectId,
    pub title: String,
    pub model_selection: ModelSelection,
    pub runtime_mode: RuntimeMode,
    pub interaction_mode: ProviderInteractionMode,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
}

/// A command's reply: the sequence whose events confirm it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted {
    pub sequence: u64,
}

/// The commands of one environment. Cheap to clone.
#[derive(Clone)]
pub struct EnvironmentCommands {
    store: EnvironmentStore,
    serial: Arc<Mutex<HashMap<ThreadId, Arc<tokio::sync::Mutex<()>>>>>,
}

impl EnvironmentCommands {
    pub fn new(store: EnvironmentStore) -> Self {
        Self { store, serial: Arc::default() }
    }

    pub fn store(&self) -> &EnvironmentStore {
        &self.store
    }

    /// The live session, waiting a while for one if the environment is reconnecting.
    pub(crate) async fn session(&self) -> Result<Arc<RpcSession>, CommandError> {
        let mut sessions = self.store.context().supervisor.session.clone();
        let wait = async {
            loop {
                if let Some(session) = sessions.borrow_and_update().clone() {
                    return Ok(session);
                }
                if sessions.changed().await.is_err() {
                    return Err(CommandError::NotConnected);
                }
            }
        };
        tokio::time::timeout(SESSION_WAIT, wait).await.unwrap_or(Err(CommandError::NotConnected))
    }

    fn lock(&self, thread_id: &ThreadId) -> Arc<tokio::sync::Mutex<()>> {
        let mut serial = self.serial.lock().expect("command lock");
        Arc::clone(serial.entry(thread_id.clone()).or_default())
    }

    /// Sends one command and returns the sequence of its confirming events.
    pub async fn dispatch(&self, command: Command) -> Result<Accepted, CommandError> {
        let session = self.session().await?;
        let result = session.client.call::<OrchestrationDispatchCommand>(command).await.map_err(command_error)?;
        Ok(Accepted { sequence: result.sequence })
    }

    async fn on_thread(&self, thread_id: &ThreadId, command: Command) -> Result<Accepted, CommandError> {
        let lock = self.lock(thread_id);
        let _serial = lock.lock().await;
        self.dispatch(command).await
    }

    /// Sends `command` with an optimistic row update shown until the shell confirms it.
    async fn optimistic(&self, thread_id: &ThreadId, apply: ApplyUpdate, command: Command) -> Result<Accepted, CommandError> {
        let lock = self.lock(thread_id);
        let _serial = lock.lock().await;
        let shell = self.store.shell();
        let update = shell.edit_pending(|pending| pending.push(thread_id.clone(), apply));
        let result = self.dispatch(command).await;
        shell.edit_pending(|pending| match &result {
            Ok(accepted) => pending.confirm(update, accepted.sequence),
            Err(_) => pending.remove(update),
        });
        result
    }

    /// Sends a message. If the socket drops before the reply, the same command (same
    /// `commandId`, which the server dedupes) is resent on the next session, at most twice,
    /// unless the environment stops trying to connect.
    pub async fn send_message(&self, message: SendMessage) -> Result<Accepted, CommandError> {
        let lock = self.lock(&message.thread_id);
        let _serial = lock.lock().await;
        let text = if message.text.trim().is_empty() && message.manual_continuation_of_run_id.is_some() {
            "Continue where you left off.".to_owned()
        } else {
            message.text
        };
        let (delivery_intent, dispatch_mode) = match message.mode {
            DeliveryMode::Queue => (None, DispatchMode::QueueAfterActive(OrchestrationV2CommandMessageDispatchDispatchModeQueueAfterActive {})),
            mode => (
                Some(match mode {
                    DeliveryMode::Steer => DeliveryIntent::Steer,
                    DeliveryMode::Restart => DeliveryIntent::Restart,
                    _ => DeliveryIntent::Auto,
                }),
                DispatchMode::StartImmediately(OrchestrationV2CommandMessageDispatchDispatchModeStartImmediately {}),
            ),
        };
        let command = Command::MessageDispatch(OrchestrationV2CommandMessageDispatch {
            notification: None,
            guest_message_id: None,
            created_by: OrchestrationV2Actor::User,
            creation_source: CREATION_SOURCE,
            scheduled_task_id: None,
            sender_thread_id: None,
            linked_sender: None,
            idle_compaction: None,
            fresh_context: message.fresh_context.then_some(true),
            command_id: new_command_id(),
            thread_id: message.thread_id,
            message_id: message.message_id,
            text,
            context: message.context,
            attachments: message.attachments,
            title_seed: message.title_seed,
            model_selection: message.model_selection,
            source_plan_ref: None,
            restart_continuation_of_run_id: None,
            usage_limit_continuation_of_run_id: None,
            manual_continuation_of_run_id: message.manual_continuation_of_run_id,
            steer_follow_up_of_run_id: None,
            steer_follow_up_after_run_ordinal: None,
            usage_limit_recovery_request_id: None,
            delivery_intent,
            delegated_completion: None,
            dispatch_mode,
        });
        self.dispatch_with_resend(command).await
    }

    async fn dispatch_with_resend(&self, command: Command) -> Result<Accepted, CommandError> {
        let supervisor = &self.store.context().supervisor;
        let mut sessions = supervisor.session.clone();
        let mut status = supervisor.status.clone();
        for resends in 0.. {
            let session = self.session().await?;
            let attempt = session.client.call::<OrchestrationDispatchCommand>(command.clone());
            let error = match tokio::time::timeout(MESSAGE_DISPATCH_TIMEOUT, attempt).await {
                Err(_) => return Err(CommandError::TimedOut),
                Ok(Ok(result)) => return Ok(Accepted { sequence: result.sequence }),
                Ok(Err(error)) => error,
            };
            if !transport_lost(&error) || resends >= MAX_LOST_TRANSPORT_RESENDS {
                return Err(command_error(error));
            }
            if !wait_for_replacement(&mut sessions, &mut status, &session).await {
                return Err(CommandError::TransportLost);
            }
        }
        unreachable!("the loop returns")
    }

    /// Starts a new thread with its first message, or prepares a worktree (`launchThread`).
    pub async fn launch_thread(&self, mut input: OrchestrationV2ThreadLaunchInput) -> Result<OrchestrationV2ThreadLaunchResult, CommandError> {
        input.creation_source = Some(CREATION_SOURCE);
        let session = self.session().await?;
        session.client.call::<OrchestrationLaunchThread>(input).await.map_err(command_error)
    }

    /// Creates an empty thread.
    pub async fn create_thread(&self, input: CreateThread) -> Result<Accepted, CommandError> {
        self.dispatch(Command::ThreadCreate(t3code_contracts::OrchestrationV2CommandThreadCreate {
            created_by: OrchestrationV2Actor::User,
            creation_source: CREATION_SOURCE,
            command_id: new_command_id(),
            thread_id: input.thread_id,
            project_id: input.project_id,
            title: input.title,
            model_selection: input.model_selection,
            runtime_mode: input.runtime_mode,
            interaction_mode: input.interaction_mode,
            branch: input.branch,
            worktree_path: input.worktree_path,
            link_origin: None,
            delegated_from: None,
            imported_native_thread: None,
            initial_message: None,
        }))
        .await
    }

    async fn projection(&self, thread_id: &ThreadId) -> Result<OrchestrationV2ThreadProjection, CommandError> {
        let session = self.session().await?;
        session
            .client
            .call::<OrchestrationGetThreadProjection>(OrchestrationV2GetThreadProjectionInput { thread_id: thread_id.clone() })
            .await
            .map_err(command_error)
    }

    /// Stops a thread's work (`interruptThreadTurn`): `thread.stop` when the server supports it
    /// and no run is named, else `run.interrupt` of the latest active run with the queue held;
    /// a provider-native subagent gets `subagent.stop`. Then asks the server which background
    /// items ended, if the thread is mounted.
    pub async fn interrupt(&self, thread_id: &ThreadId, run_id: Option<RunId>, subagent_id: Option<NodeId>) -> Result<Accepted, CommandError> {
        let result = self.interrupt_inner(thread_id, run_id, subagent_id).await;
        if result.is_ok() {
            self.store.reconcile_pending_work(thread_id).await;
        }
        result
    }

    async fn interrupt_inner(&self, thread_id: &ThreadId, run_id: Option<RunId>, subagent_id: Option<NodeId>) -> Result<Accepted, CommandError> {
        let mut target = thread_id.clone();
        if let Some(subagent_id) = subagent_id {
            let projection = self.projection(thread_id).await?;
            let subagent = projection.subagents.iter().find(|agent| agent.id == subagent_id);
            let stop = |run_id: RunId, subagent_id: NodeId| {
                Command::SubagentStop(t3code_contracts::OrchestrationV2CommandSubagentStop {
                    command_id: new_command_id(),
                    thread_id: thread_id.clone(),
                    run_id,
                    subagent_id,
                })
            };
            match subagent {
                Some(agent) if agent.origin == t3code_contracts::OrchestrationV2SubagentOrigin::AppOwned && agent.child_thread_id.is_some() => {
                    if let Some(run_id) = agent.run_id.clone() {
                        // Older servers reject this for app-owned tasks; the child stop still runs.
                        _ = self.on_thread(thread_id, stop(run_id, agent.id.clone())).await;
                    }
                    target = agent.child_thread_id.clone().expect("checked above");
                }
                Some(agent) => {
                    let Some(run_id) = agent.run_id.clone() else { return Ok(Accepted { sequence: 0 }) };
                    return self.on_thread(thread_id, stop(run_id, agent.id.clone())).await;
                }
                None => return Ok(Accepted { sequence: 0 }),
            }
        }
        let named = if &target == thread_id { run_id } else { None };
        if named.is_none() {
            let session = self.session().await?;
            if session.initial_config.environment.capabilities.thread_stop == Some(true) {
                return self
                    .on_thread(&target, Command::ThreadStop(t3code_contracts::OrchestrationV2CommandThreadStop {
                        command_id: new_command_id(),
                        thread_id: target.clone(),
                        reason: None,
                        stop_delegated_tasks: None,
                    }))
                    .await;
            }
        }
        let run_id = match named {
            Some(run_id) => run_id,
            None => {
                let projection = self.projection(&target).await?;
                let active = projection.runs.iter().rev().find(|run| {
                    matches!(
                        run.status,
                        OrchestrationV2RunStatus::Preparing | OrchestrationV2RunStatus::Starting | OrchestrationV2RunStatus::Running | OrchestrationV2RunStatus::Waiting
                    )
                });
                match active {
                    Some(run) => run.id.clone(),
                    None => return Ok(Accepted { sequence: 0 }),
                }
            }
        };
        self.on_thread(&target, Command::RunInterrupt(t3code_contracts::OrchestrationV2CommandRunInterrupt {
            command_id: new_command_id(),
            thread_id: target.clone(),
            run_id,
            reason: None,
            hold_queue: Some(true),
        }))
        .await
    }

    /// Approves or denies a request.
    pub async fn respond_to_approval(
        &self,
        thread_id: &ThreadId,
        request_id: RuntimeRequestId,
        decision: ProviderApprovalDecision,
        feedback: Option<String>,
    ) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::RuntimeRequestRespond(t3code_contracts::OrchestrationV2CommandRuntimeRequestRespond {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            request_id,
            decision: Some(decision),
            feedback,
            answers: None,
            attachments_by_question_id: None,
        }))
        .await
    }

    /// Answers a question, keyed by question id.
    pub async fn answer_question(&self, thread_id: &ThreadId, request_id: RuntimeRequestId, answers: ProviderUserInputAnswers) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::RuntimeRequestRespond(t3code_contracts::OrchestrationV2CommandRuntimeRequestRespond {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            request_id,
            decision: None,
            feedback: None,
            answers: Some(answers),
            attachments_by_question_id: None,
        }))
        .await
    }

    /// Dismisses a question without answering.
    pub async fn dismiss_question(&self, thread_id: &ThreadId, request_id: RuntimeRequestId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadUserInputDismiss(t3code_contracts::OrchestrationV2CommandThreadUserInputDismiss {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            request_id,
        }))
        .await
    }

    fn metadata(thread_id: &ThreadId) -> t3code_contracts::OrchestrationV2CommandThreadMetadataUpdate {
        t3code_contracts::OrchestrationV2CommandThreadMetadataUpdate {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            title: None,
            regenerate_title: None,
            branch: None,
            worktree_path: None,
            expected_worktree_path: None,
            expected_branch: None,
            continuation: None,
            expected_empty: None,
            limit_recovery: None,
            linked_pull_request: None,
        }
    }

    pub async fn rename(&self, thread_id: &ThreadId, title: impl Into<String>) -> Result<Accepted, CommandError> {
        let mut update = Self::metadata(thread_id);
        update.title = Some(title.into());
        self.on_thread(thread_id, Command::ThreadMetadataUpdate(update)).await
    }

    /// Sets the branch and worktree a thread works in.
    pub async fn set_workspace(&self, thread_id: &ThreadId, branch: Option<String>, worktree_path: Option<String>) -> Result<Accepted, CommandError> {
        let mut update = Self::metadata(thread_id);
        update.branch = branch;
        update.worktree_path = worktree_path;
        self.on_thread(thread_id, Command::ThreadMetadataUpdate(update)).await
    }

    /// Asks the server for a new title. Watch the shell row's `lastTitleRegeneration.requestId`
    /// for the returned command id to learn the outcome.
    pub async fn regenerate_title(&self, thread_id: &ThreadId) -> Result<(CommandId, Accepted), CommandError> {
        let mut update = Self::metadata(thread_id);
        update.regenerate_title = Some(true);
        let command_id = update.command_id.clone();
        let accepted = self.on_thread(thread_id, Command::ThreadMetadataUpdate(update)).await?;
        Ok((command_id, accepted))
    }

    /// Changes the model; shown at once.
    pub async fn set_model_selection(&self, thread_id: &ThreadId, model_selection: ModelSelection) -> Result<Accepted, CommandError> {
        let shown = model_selection.clone();
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, _| thread.model_selection = shown.clone());
        let command = Command::ThreadModelSelectionSet(t3code_contracts::OrchestrationV2CommandThreadModelSelectionSet {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            model_selection,
        });
        self.optimistic(thread_id, apply, command).await
    }

    /// Moves the thread to another provider.
    pub async fn switch_provider(&self, thread_id: &ThreadId, model_selection: ModelSelection) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ProviderSwitch(t3code_contracts::OrchestrationV2CommandProviderSwitch {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            model_selection,
        }))
        .await
    }

    pub async fn set_runtime_mode(&self, thread_id: &ThreadId, runtime_mode: RuntimeMode) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadRuntimeModeSet(t3code_contracts::OrchestrationV2CommandThreadRuntimeModeSet {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            runtime_mode,
        }))
        .await
    }

    pub async fn set_interaction_mode(&self, thread_id: &ThreadId, interaction_mode: ProviderInteractionMode) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadInteractionModeSet(t3code_contracts::OrchestrationV2CommandThreadInteractionModeSet {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            interaction_mode,
        }))
        .await
    }

    /// Reverts to a checkpoint. `restore_files: false` keeps the working tree.
    pub async fn rollback_to_checkpoint(
        &self,
        thread_id: &ThreadId,
        scope_id: CheckpointScopeId,
        checkpoint_id: CheckpointId,
        restore_files: Option<bool>,
    ) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::CheckpointRollback(t3code_contracts::OrchestrationV2CommandCheckpointRollback {
            restore_files,
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            scope_id,
            checkpoint_id,
        }))
        .await
    }

    /// Reverts to the checkpoint of the run with ordinal `run_ordinal`, resolving its ids from
    /// the server projection. The checkpoint must be `ready`, or `missing` without file restore.
    pub async fn rollback_to_run(&self, thread_id: &ThreadId, run_ordinal: u64, restore_files: bool) -> Result<Accepted, CommandError> {
        use t3code_contracts::OrchestrationV2CheckpointStatus as Status;
        let projection = self.projection(thread_id).await?;
        let checkpoint = projection
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.app_run_ordinal == Some(run_ordinal))
            .filter(|checkpoint| checkpoint.status == Status::Ready || (checkpoint.status == Status::Missing && !restore_files))
            .ok_or_else(|| CommandError::Refused("No usable checkpoint for that turn.".to_owned()))?;
        self.rollback_to_checkpoint(thread_id, checkpoint.scope_id.clone(), checkpoint.id.clone(), Some(restore_files)).await
    }

    pub async fn archive(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadArchive(t3code_contracts::OrchestrationV2CommandThreadArchive { command_id: new_command_id(), thread_id: thread_id.clone() })).await
    }

    pub async fn unarchive(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadUnarchive(t3code_contracts::OrchestrationV2CommandThreadUnarchive { command_id: new_command_id(), thread_id: thread_id.clone() })).await
    }

    /// Deletes a thread. A thread missing from the shell is not proof it is gone; callers that
    /// delete on absence must ask the server (`getThreadFamily`) first.
    pub async fn delete(&self, thread_id: &ThreadId, only_if_unused: bool) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadDelete(t3code_contracts::OrchestrationV2CommandThreadDelete {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            only_if_unused: only_if_unused.then_some(true),
        }))
        .await
    }

    /// Settles a thread; shown at once unless a request is pending or a run is live.
    pub async fn settle(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        let now = format_ms(now_ms());
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, accepted| {
            let live = matches!(thread.status, ShellStatus::Preparing | ShellStatus::Queued | ShellStatus::Starting | ShellStatus::Running | ShellStatus::Waiting);
            if !accepted && (thread.pending_runtime_request.is_some() || live) {
                return;
            }
            thread.pending_runtime_request = None;
            thread.settled_at = if thread.settled_override == Some(Settled::Settled) {
                thread.settled_at.clone().or_else(|| Some(now.clone()))
            } else {
                Some(now.clone())
            };
            thread.settled_override = Some(Settled::Settled);
            thread.unsettled_at = None;
            thread.active_order_key = None;
            thread.pinned_at = None;
            thread.pin_order_key = None;
            thread.snoozed_at = None;
            thread.snoozed_until = None;
        });
        let command = Command::ThreadSettle(t3code_contracts::OrchestrationV2CommandThreadSettle {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            expected_latest_run_id: None,
            settled_at: None,
        });
        self.optimistic(thread_id, apply, command).await
    }

    /// Moves a settled thread back to active. `restore_unsettled_at` undoes a settle.
    pub async fn unsettle(&self, thread_id: &ThreadId, restore_unsettled_at: Option<Option<String>>) -> Result<Accepted, CommandError> {
        let now = format_ms(now_ms());
        let restore = restore_unsettled_at.clone();
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, _| {
            thread.unsettled_at = if thread.settled_override == Some(Settled::Active) {
                thread.unsettled_at.clone()
            } else {
                match &restore {
                    None => Some(now.clone()),
                    Some(restored) => restored.clone(),
                }
            };
            thread.settled_override = Some(Settled::Active);
            thread.settled_at = None;
        });
        let command = Command::ThreadUnsettle(t3code_contracts::OrchestrationV2CommandThreadUnsettle {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            reason: OrchestrationV2CommandThreadUnsettleReason::User,
            restore_unsettled_at: restore_unsettled_at.flatten(),
        });
        self.optimistic(thread_id, apply, command).await
    }

    pub async fn set_auto_settle(&self, thread_id: &ThreadId, enabled: bool) -> Result<Accepted, CommandError> {
        let now = format_ms(now_ms());
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, _| {
            thread.auto_settle_disabled_at = if enabled { None } else { thread.auto_settle_disabled_at.clone().or_else(|| Some(now.clone())) };
        });
        let command = Command::ThreadAutoSettleSet(t3code_contracts::OrchestrationV2CommandThreadAutoSettleSet {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            enabled,
        });
        self.optimistic(thread_id, apply, command).await
    }

    /// Snoozes until `snoozed_until` (ISO); shown at once unless an approval is pending or a run
    /// is starting.
    pub async fn snooze(&self, thread_id: &ThreadId, snoozed_until: String) -> Result<Accepted, CommandError> {
        let now_at = now_ms();
        let now = format_ms(now_at);
        let until = snoozed_until.clone();
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, accepted| {
            let blocked = thread
                .pending_runtime_request
                .as_ref()
                .is_some_and(|request| request.kind != OrchestrationV2PendingRuntimeRequestSummaryKind::UserInput)
                || matches!(thread.status, ShellStatus::Preparing | ShellStatus::Queued | ShellStatus::Starting);
            if (!accepted && blocked) || parse_ms(&until).is_none_or(|at| at <= now_at) {
                return;
            }
            thread.snoozed_until = Some(until.clone());
            thread.snoozed_at = Some(now.clone());
        });
        let command = Command::ThreadSnooze(t3code_contracts::OrchestrationV2CommandThreadSnooze {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            snoozed_until,
        });
        self.optimistic(thread_id, apply, command).await
    }

    pub async fn unsnooze(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        let apply: ApplyUpdate = Arc::new(|thread: &mut OrchestrationV2ThreadShell, _| {
            thread.snoozed_until = None;
            thread.snoozed_at = None;
        });
        let command = Command::ThreadUnsnooze(t3code_contracts::OrchestrationV2CommandThreadUnsnooze {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            reason: OrchestrationV2CommandThreadUnsnoozeReason::User,
        });
        self.optimistic(thread_id, apply, command).await
    }

    /// Pins a thread; a settled one becomes active.
    pub async fn pin(&self, thread_id: &ThreadId, order_key: Option<String>) -> Result<Accepted, CommandError> {
        let now = format_ms(now_ms());
        let key = order_key.clone();
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, _| {
            if thread.pinned_at.is_none() {
                thread.pin_order_key = key.clone();
            }
            thread.pinned_at = thread.pinned_at.clone().or_else(|| Some(now.clone()));
            if thread.settled_override == Some(Settled::Settled) {
                thread.settled_override = Some(Settled::Active);
                thread.settled_at = None;
                thread.unsettled_at = Some(now.clone());
            }
            thread.snoozed_until = None;
            thread.snoozed_at = None;
        });
        let command = Command::ThreadPin(t3code_contracts::OrchestrationV2CommandThreadPin { command_id: new_command_id(), thread_id: thread_id.clone(), order_key });
        self.optimistic(thread_id, apply, command).await
    }

    pub async fn unpin(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        let apply: ApplyUpdate = Arc::new(|thread: &mut OrchestrationV2ThreadShell, _| {
            thread.pinned_at = None;
            thread.pin_order_key = None;
        });
        let command = Command::ThreadUnpin(t3code_contracts::OrchestrationV2CommandThreadUnpin { command_id: new_command_id(), thread_id: thread_id.clone() });
        self.optimistic(thread_id, apply, command).await
    }

    /// Moves a pinned thread; take keys from `thread_sort::plan_pinned_reorder`.
    pub async fn reorder_pin(&self, thread_id: &ThreadId, order_key: String) -> Result<Accepted, CommandError> {
        let key = order_key.clone();
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, _| thread.pin_order_key = Some(key.clone()));
        let command = Command::ThreadPinReorder(t3code_contracts::OrchestrationV2CommandThreadPinReorder { command_id: new_command_id(), thread_id: thread_id.clone(), order_key });
        self.optimistic(thread_id, apply, command).await
    }

    pub async fn reorder_active(&self, thread_id: &ThreadId, order_key: String) -> Result<Accepted, CommandError> {
        let key = order_key.clone();
        let apply: ApplyUpdate = Arc::new(move |thread: &mut OrchestrationV2ThreadShell, _| thread.active_order_key = Some(key.clone()));
        let command = Command::ThreadActiveReorder(t3code_contracts::OrchestrationV2CommandThreadActiveReorder { command_id: new_command_id(), thread_id: thread_id.clone(), order_key });
        self.optimistic(thread_id, apply, command).await
    }

    /// Records that the user saw the thread up to `visited_at`; `opened` counts an open for
    /// search ranking. Driven by `thread_visits::VisitTracker`.
    pub async fn visit(&self, thread_id: &ThreadId, visited_at: String, opened: bool, search_query: Option<String>) -> Result<Accepted, CommandError> {
        self.dispatch(Command::ThreadVisit(t3code_contracts::OrchestrationV2CommandThreadVisit {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            visited_at,
            opened: opened.then_some(true),
            search_query,
        }))
        .await
    }

    /// Adds or removes one tag on a thread (`thread.tag.set`).
    pub async fn set_tag(&self, thread_id: &ThreadId, tag: impl Into<String>, tagged: bool) -> Result<Accepted, CommandError> {
        let command = Command::ThreadTagSet(t3code_contracts::OrchestrationV2CommandThreadTagSet { command_id: new_command_id(), thread_id: thread_id.clone(), tag: tag.into(), tagged });
        self.on_thread(thread_id, command).await
    }

    pub async fn mark_unread(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadMarkUnread(t3code_contracts::OrchestrationV2CommandThreadMarkUnread { command_id: new_command_id(), thread_id: thread_id.clone() })).await
    }

    /// Resumes a held queue.
    pub async fn resume_queue(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::QueueResume(t3code_contracts::OrchestrationV2CommandQueueResume { command_id: new_command_id(), thread_id: thread_id.clone() })).await
    }

    /// Starts one queued run now.
    pub async fn start_queued_run(&self, thread_id: &ThreadId, run_id: RunId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::QueuedRunStart(t3code_contracts::OrchestrationV2CommandQueuedRunStart { command_id: new_command_id(), thread_id: thread_id.clone(), run_id })).await
    }

    /// Turns a queued message into a steer of the running run.
    pub async fn promote_queued_to_steer(&self, thread_id: &ThreadId, queued_run_id: RunId, target_run_id: RunId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::QueuedMessagePromoteToSteer(t3code_contracts::OrchestrationV2CommandQueuedMessagePromoteToSteer {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            queued_run_id,
            target_run_id,
        }))
        .await
    }

    pub async fn cancel_queued_run(&self, thread_id: &ThreadId, run_id: RunId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::QueuedRunCancel(t3code_contracts::OrchestrationV2CommandQueuedRunCancel { command_id: new_command_id(), thread_id: thread_id.clone(), run_id })).await
    }

    /// Moves a queued run before another, or to the end.
    pub async fn reorder_queued_run(&self, thread_id: &ThreadId, run_id: RunId, before_run_id: Option<RunId>) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::QueuedRunReorder(t3code_contracts::OrchestrationV2CommandQueuedRunReorder {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            run_id,
            before_run_id,
        }))
        .await
    }

    /// Replaces a queued message's text and attachments.
    pub async fn edit_queued_run(
        &self,
        thread_id: &ThreadId,
        run_id: RunId,
        text: String,
        attachments: Option<Vec<ChatAttachment>>,
        context: Option<OrchestrationMessageContext>,
    ) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::QueuedRunEdit(t3code_contracts::OrchestrationV2CommandQueuedRunEdit {
            context,
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            run_id,
            text,
            attachments,
        }))
        .await
    }

    /// Retries a failed worktree preparation.
    pub async fn retry_prepared_run(&self, thread_id: &ThreadId, run_id: RunId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::PreparedRunRetry(t3code_contracts::OrchestrationV2CommandPreparedRunRetry { command_id: new_command_id(), thread_id: thread_id.clone(), run_id })).await
    }

    /// Detaches every provider session of the thread.
    pub async fn stop_sessions(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        let projection = self.projection(thread_id).await?;
        let mut last = Accepted { sequence: 0 };
        for session in &projection.provider_sessions {
            last = self
                .on_thread(thread_id, Command::ProviderSessionDetach(t3code_contracts::OrchestrationV2CommandProviderSessionDetach {
                    command_id: new_command_id(),
                    thread_id: thread_id.clone(),
                    provider_session_id: session.id.clone(),
                    reason: Some("client-requested".into()),
                }))
                .await?;
        }
        Ok(last)
    }

    /// Forks a thread, or opens a side chat, into `target_thread_id`.
    pub async fn fork(
        &self,
        source_thread_id: &ThreadId,
        target_thread_id: ThreadId,
        source_point: OrchestrationV2ThreadForkSourcePoint,
        title: Option<String>,
        side_chat: bool,
    ) -> Result<Accepted, CommandError> {
        self.on_thread(source_thread_id, Command::ThreadFork(t3code_contracts::OrchestrationV2CommandThreadFork {
            created_by: OrchestrationV2Actor::User,
            creation_source: CREATION_SOURCE,
            command_id: new_command_id(),
            source_thread_id: source_thread_id.clone(),
            target_thread_id,
            source_point,
            title,
            side_chat: side_chat.then_some(true),
            created_at: None,
        }))
        .await
    }

    /// Merges a side chat back into its source.
    pub async fn merge_back(&self, source_thread_id: &ThreadId, target_thread_id: ThreadId, source_point: OrchestrationV2ThreadForkSourcePoint) -> Result<Accepted, CommandError> {
        self.on_thread(source_thread_id, Command::ThreadMergeBack(t3code_contracts::OrchestrationV2CommandThreadMergeBack {
            created_by: OrchestrationV2Actor::User,
            creation_source: CREATION_SOURCE,
            command_id: new_command_id(),
            source_thread_id: source_thread_id.clone(),
            target_thread_id,
            source_point,
            created_at: None,
        }))
        .await
    }

    pub async fn close_guest_share(&self, thread_id: &ThreadId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadGuestShareClose(t3code_contracts::OrchestrationV2CommandThreadGuestShareClose { command_id: new_command_id(), thread_id: thread_id.clone() })).await
    }

    pub async fn dismiss_guest_message(&self, thread_id: &ThreadId, guest_message_id: GuestMessageId) -> Result<Accepted, CommandError> {
        self.on_thread(thread_id, Command::ThreadGuestMessageDismiss(t3code_contracts::OrchestrationV2CommandThreadGuestMessageDismiss {
            command_id: new_command_id(),
            thread_id: thread_id.clone(),
            guest_message_id,
        }))
        .await
    }

    /// Creates, updates or deletes a project (`projects.mutate`).
    pub async fn mutate_project(&self, mutation: ProjectMutation) -> Result<Project, CommandError> {
        let session = self.session().await?;
        session.client.call::<ProjectsMutate>(mutation).await.map_err(command_error)
    }
}

/// Waits for a session other than `lost`. False when the supervisor stops trying.
async fn wait_for_replacement(
    sessions: &mut watch::Receiver<Option<Arc<RpcSession>>>,
    status: &mut watch::Receiver<ConnectionStatus>,
    lost: &Arc<RpcSession>,
) -> bool {
    loop {
        if sessions.borrow_and_update().as_ref().is_some_and(|session| !Arc::ptr_eq(session, lost)) {
            return true;
        }
        if matches!(status.borrow_and_update().phase, ConnectionPhase::Available | ConnectionPhase::Blocked) {
            return false;
        }
        tokio::select! {
            changed = sessions.changed() => if changed.is_err() { return false },
            changed = status.changed() => if changed.is_err() { return false },
        }
    }
}
