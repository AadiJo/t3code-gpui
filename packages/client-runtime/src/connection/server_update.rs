//! Server self-updates and what each environment's update is doing. Ports the `updateServer`
//! command and `ServerUpdateState` of `state/server.ts`.
//!
//! [`ServerUpdates`] holds one [`ServerUpdateState`] per environment. [`update_server`] runs one
//! update on the environment's live session: `server.updateServerWithProgress` when the host
//! reports progress (else `server.updateServer`), `server.commitDesktopUpdate` for a prepared
//! desktop app, then waits for the host's `ready` on the version it was asked for, nudging the
//! supervisor to reconnect while the host restarts. The state lives here, not in a view, so an
//! update keeps going and stays visible while the UI that started it goes away.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt as _;
use t3code_contracts::rpc::methods::{
    ServerCommitDesktopUpdate, ServerUpdateServer, ServerUpdateServerWithProgress, SubscribeServerLifecycle,
};
use t3code_contracts::{
    DesktopUpdateCommitInput, ServerConfig, ServerLifecycleStreamEvent, ServerLifecycleStreamReadyEvent,
    ServerSelfUpdateCapability, ServerSelfUpdateInput, ServerSelfUpdateMethod, ServerSelfUpdateOutcomeStatus,
    ServerSelfUpdateProgressEvent, ServerSelfUpdateProgressStage, ServerSelfUpdateResult,
};
use tokio::sync::watch;

use super::model::{BlockedReason, ConnectionErrorKind, ConnectionPhase};
use super::supervisor::{SupervisorHandle, SupervisorSignal};
use crate::environment::EnvironmentId;
use crate::rpc::client::RpcError;
use crate::rpc::RpcSession;

/// Covers the service launcher's 30-minute trial and a final restart of the previous version
/// when the trial rolls back.
const RESUME_TIMEOUT: Duration = Duration::from_secs(32 * 60);
/// A live host replays its `ready` on subscribe within a round trip; past this, update without
/// a baseline.
const ACCEPTING_READY_TIMEOUT: Duration = Duration::from_secs(5);
/// The pause before each reconnect nudge while the host restarts.
const NUDGE_INTERVAL: Duration = Duration::from_secs(1);
/// Readies a desktop host may answer on the old version before the client gives up.
const DESKTOP_COMMIT_ATTEMPTS: usize = 3;

/// The part of an update the host is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerUpdateStage {
    Downloading,
    Installing,
    Resuming,
}

/// Why an update failed, kept typed so the UI can word it in its language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerUpdateFailure {
    /// The host's own reason (`ServerSelfUpdateError.reason`) or a transport error, in English.
    Reason(String),
    /// "Could not confirm that the server resumed on t3@{version}. Reconnect to check the update outcome."
    ResumeTimeout { version: String },
    /// "The t3@{version} update ended before the server accepted the restart."
    ProgressIncomplete { version: String },
    /// "The t3@{version} update {status}." (`committed`, `rolled-back` or `failed`).
    Terminal { version: String, status: ServerSelfUpdateOutcomeStatus },
    /// "The desktop app resumed without installing the prepared update."
    DesktopResumedWithoutUpdate,
    /// "Update T3 Code on {host} manually; it cannot update itself."
    CannotSelfUpdate { host: String },
    /// "{host} did not come back on a compatible T3 Code version."
    DidNotComeBack { host: String },
    /// "Server update failed."
    Unknown,
}

/// What an environment's server update is doing (`ServerUpdateState`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum ServerUpdateState {
    #[default]
    Idle,
    Running { stage: ServerUpdateStage, from_version: String, target_version: String },
    Failed { stage: ServerUpdateStage, from_version: String, target_version: String, failure: ServerUpdateFailure },
}

impl ServerUpdateState {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// The state as views show it for a host now on `server_version`
    /// (`serverUpdateStateForServerVersion`): a failure from an older version is over once the
    /// host runs something else, because someone updated it another way.
    pub fn for_server_version(&self, server_version: Option<&str>) -> Self {
        match (self, server_version) {
            (Self::Failed { from_version, .. }, Some(version)) if from_version != version => Self::Idle,
            _ => self.clone(),
        }
    }
}

/// How an update ended for the caller (`ServerUpdateOutcome`).
#[derive(Clone, Debug, PartialEq)]
pub enum ServerUpdateOutcome {
    Updated(ServerSelfUpdateResult),
    /// Another update for the environment is already running, or it was interrupted.
    Skipped,
    Failed(ServerUpdateFailure),
}

/// Every environment's update state, shared by the runtime and the UI.
#[derive(Default)]
pub struct ServerUpdates {
    states: Mutex<HashMap<EnvironmentId, watch::Sender<ServerUpdateState>>>,
    in_flight: Mutex<HashSet<EnvironmentId>>,
}

impl ServerUpdates {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Follows one environment's state; starts `Idle`.
    pub fn watch(&self, environment_id: &EnvironmentId) -> watch::Receiver<ServerUpdateState> {
        self.sender(environment_id).subscribe()
    }

    pub fn state(&self, environment_id: &EnvironmentId) -> ServerUpdateState {
        self.sender(environment_id).borrow().clone()
    }

    pub fn is_pending(&self, environment_id: &EnvironmentId) -> bool {
        self.in_flight.lock().expect("server updates lock").contains(environment_id)
    }

    pub(crate) fn set(&self, environment_id: &EnvironmentId, state: ServerUpdateState) {
        self.sender(environment_id).send_replace(state);
    }

    fn sender(&self, environment_id: &EnvironmentId) -> watch::Sender<ServerUpdateState> {
        self.states
            .lock()
            .expect("server updates lock")
            .entry(environment_id.clone())
            .or_insert_with(|| watch::channel(ServerUpdateState::Idle).0)
            .clone()
    }

    /// Claims the environment for one update; `None` when one is already running.
    pub(crate) fn claim(self: &Arc<Self>, environment_id: &EnvironmentId) -> Option<UpdateClaim> {
        let fresh = self.in_flight.lock().expect("server updates lock").insert(environment_id.clone());
        fresh.then(|| UpdateClaim { updates: Arc::clone(self), environment_id: environment_id.clone(), settled: false })
    }
}

/// One update's hold on its environment. Dropped unsettled (the task was cancelled), the state
/// returns to `Idle`, as an interrupted command does in the web.
pub(crate) struct UpdateClaim {
    updates: Arc<ServerUpdates>,
    environment_id: EnvironmentId,
    settled: bool,
}

impl UpdateClaim {
    pub(crate) fn set(&self, state: ServerUpdateState) {
        self.updates.set(&self.environment_id, state);
    }

    pub(crate) fn settle(mut self, state: ServerUpdateState) {
        self.settled = true;
        self.updates.set(&self.environment_id, state);
    }
}

impl Drop for UpdateClaim {
    fn drop(&mut self) {
        self.updates.in_flight.lock().expect("server updates lock").remove(&self.environment_id);
        if !self.settled {
            self.updates.set(&self.environment_id, ServerUpdateState::Idle);
        }
    }
}

/// Whether an RPC failed because the socket went away, which a restarting host does on purpose
/// (`isLegacyUpdateHandoffLoss`).
pub(crate) fn is_handoff_loss<E: std::fmt::Debug>(error: &RpcError<E>) -> bool {
    matches!(error, RpcError::Closed(_) | RpcError::Interrupted)
}

/// The host's reason for a typed self-update failure, else the transport error.
pub(crate) fn rpc_failure<E: std::fmt::Debug + ServerSelfUpdateReason>(error: &RpcError<E>) -> ServerUpdateFailure {
    match error {
        RpcError::Failure(error) => error.reason().map_or(ServerUpdateFailure::Unknown, ServerUpdateFailure::Reason),
        other => ServerUpdateFailure::Reason(other.to_string()),
    }
}

/// The `ServerSelfUpdateError.reason` inside a method's error union.
pub(crate) trait ServerSelfUpdateReason {
    fn reason(&self) -> Option<String>;
}

macro_rules! self_update_reason {
    ($($error:ty => $variant:ident),* $(,)?) => {$(
        impl ServerSelfUpdateReason for $error {
            fn reason(&self) -> Option<String> {
                match self {
                    Self::$variant(error) => Some(error.reason.clone()),
                    Self::EnvironmentAuthorization(error) => Some(format!("{error:?}")),
                    Self::Unknown(_) => None,
                }
            }
        }
    )*};
}
self_update_reason! {
    t3code_contracts::ServerUpdateServerError => SelfUpdate,
    t3code_contracts::ServerUpdateServerWithProgressError => SelfUpdate,
    t3code_contracts::ServerCommitDesktopUpdateError => ServerSelfUpdateError,
}

fn restarts_in_place(method: Option<&ServerSelfUpdateCapability>) -> Option<ServerSelfUpdateMethod> {
    match method? {
        ServerSelfUpdateCapability::BootService => Some(ServerSelfUpdateMethod::BootService),
        ServerSelfUpdateCapability::Respawn => Some(ServerSelfUpdateMethod::Respawn),
        _ => None,
    }
}

fn stage_of(stage: &ServerSelfUpdateProgressStage) -> ServerUpdateStage {
    match stage {
        ServerSelfUpdateProgressStage::Installing => ServerUpdateStage::Installing,
        _ => ServerUpdateStage::Downloading,
    }
}

fn commits_desktop(result: &ServerSelfUpdateResult) -> bool {
    result.method == ServerSelfUpdateMethod::DesktopApp && result.desktop_update_token.is_some()
}

/// Whether `ready` is the host the update asked for (`matchesServerUpdateResumeEvent`).
fn resumes(result: &ServerSelfUpdateResult, ready: &ServerLifecycleStreamReadyEvent, accepted_at: Option<&str>) -> bool {
    let payload = &ready.payload;
    commits_desktop(result)
        || match &result.update_id {
            None => payload.environment.server_version == result.target_version,
            Some(id) => payload.update_outcome.as_ref().is_some_and(|outcome| &outcome.id == id),
        }
        || (result.update_id.is_some() && accepted_at.is_some_and(|at| payload.at != at))
}

/// Rejects a launcher host that came back without committing (`validateServerUpdateReadyEvent`).
fn validate(result: &ServerSelfUpdateResult, ready: &ServerLifecycleStreamReadyEvent) -> Result<(), ServerUpdateFailure> {
    let Some(id) = &result.update_id else { return Ok(()) };
    let outcome = ready.payload.update_outcome.as_ref().filter(|outcome| &outcome.id == id);
    let committed = outcome.is_some_and(|outcome| {
        outcome.status == ServerSelfUpdateOutcomeStatus::Committed && outcome.target_version == result.target_version
    }) && ready.payload.environment.server_version == result.target_version;
    if committed {
        return Ok(());
    }
    Err(match outcome.and_then(|outcome| outcome.reason.clone()) {
        Some(reason) => ServerUpdateFailure::Reason(reason),
        None if outcome.is_none() => ServerUpdateFailure::Reason(
            "The service launcher resumed without committing the requested server version.".into(),
        ),
        None => ServerUpdateFailure::Terminal {
            version: result.target_version.clone(),
            status: outcome.map_or(ServerSelfUpdateOutcomeStatus::Failed, |outcome| outcome.status.clone()),
        },
    })
}

/// The first `ready` on `session`'s lifecycle stream.
async fn first_ready(session: &RpcSession) -> Option<ServerLifecycleStreamReadyEvent> {
    let mut stream = session.client.subscribe::<SubscribeServerLifecycle>(t3code_contracts::wire::Empty);
    while let Some(Ok(events)) = stream.next().await {
        for event in events {
            if let ServerLifecycleStreamEvent::Ready(ready) = event {
                return Some(ready);
            }
        }
    }
    None
}

/// Follows the environment's sessions until one's lifecycle stream reports a `ready` for which
/// `wanted` holds. Each new session (the host restarted) is subscribed again.
async fn next_ready(
    supervisor: &SupervisorHandle,
    skip_generation: Option<u64>,
    wanted: impl Fn(&ServerLifecycleStreamReadyEvent) -> bool,
) -> ServerLifecycleStreamReadyEvent {
    let mut sessions = supervisor.session.clone();
    let mut status = supervisor.status.clone();
    loop {
        let session = sessions.borrow_and_update().clone();
        let generation = status.borrow_and_update().generation;
        if let Some(session) = session.filter(|_| skip_generation != Some(generation)) {
            let mut stream = session.client.subscribe::<SubscribeServerLifecycle>(t3code_contracts::wire::Empty);
            loop {
                tokio::select! {
                    item = stream.next() => match item {
                        Some(Ok(events)) => {
                            for event in events {
                                if let ServerLifecycleStreamEvent::Ready(ready) = event
                                    && wanted(&ready)
                                {
                                    return ready;
                                }
                            }
                        }
                        _ => break,
                    },
                    changed = sessions.changed() => {
                        if changed.is_err() {
                            return std::future::pending().await;
                        }
                        break;
                    }
                }
            }
        } else if sessions.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

/// Retries the connection whenever it backs off, or the restarted host does not yet know this
/// device, until dropped (`nudgeReconnectDuringUpdateRestart`). The pause before each nudge
/// paces a connection that fails at once.
async fn nudge_reconnect(supervisor: SupervisorHandle) {
    let mut status = supervisor.status.clone();
    loop {
        let waiting = {
            let current = status.borrow_and_update();
            current.phase == ConnectionPhase::Backoff
                || (current.phase == ConnectionPhase::Blocked
                    && current.last_failure.as_ref().is_some_and(|failure| failure.kind == ConnectionErrorKind::Blocked(BlockedReason::Authentication)))
        };
        if waiting {
            tokio::time::sleep(NUDGE_INTERVAL).await;
            supervisor.signal(SupervisorSignal::Retry);
        }
        if status.changed().await.is_err() {
            return;
        }
    }
}

/// Runs one update of `environment_id` to `input.target_version` and records its state in
/// `updates`. Never panics on a host failure; the outcome says what happened.
pub async fn update_server(
    updates: &Arc<ServerUpdates>,
    environment_id: &EnvironmentId,
    supervisor: SupervisorHandle,
    input: ServerSelfUpdateInput,
) -> ServerUpdateOutcome {
    let Some(claim) = updates.claim(environment_id) else { return ServerUpdateOutcome::Skipped };
    let Some(session) = supervisor.session.borrow().clone() else { return ServerUpdateOutcome::Skipped };
    let config: Arc<ServerConfig> = session.config().borrow().clone();
    let mut target_version = input.target_version.clone();
    let from_version = config.environment.server_version.clone();
    let mut stage = ServerUpdateStage::Downloading;
    let running = |stage, target: &str| ServerUpdateState::Running {
        stage,
        from_version: from_version.clone(),
        target_version: target.to_owned(),
    };
    claim.set(running(stage, &target_version));
    let result = run_update(&claim, &supervisor, &session, &config, &input, &mut stage, &mut target_version, &running).await;
    match result {
        Ok(result) => {
            claim.settle(ServerUpdateState::Idle);
            ServerUpdateOutcome::Updated(result)
        }
        Err(failure) => {
            claim.settle(ServerUpdateState::Failed {
                stage,
                from_version: from_version.clone(),
                target_version,
                failure: failure.clone(),
            });
            ServerUpdateOutcome::Failed(failure)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_update(
    claim: &UpdateClaim,
    supervisor: &SupervisorHandle,
    session: &Arc<RpcSession>,
    config: &ServerConfig,
    input: &ServerSelfUpdateInput,
    stage: &mut ServerUpdateStage,
    target_version: &mut String,
    running: &impl Fn(ServerUpdateStage, &str) -> ServerUpdateState,
) -> Result<ServerSelfUpdateResult, ServerUpdateFailure> {
    let capabilities = &config.environment.capabilities;
    let in_place = restarts_in_place(capabilities.server_self_update.as_ref());
    let generation = supervisor.status.borrow().generation;
    // Only launcher updates carry an update id; read the baseline before the request so a ready
    // from after it can never pose as the accepting process.
    let accepted_at = if capabilities.server_self_update == Some(ServerSelfUpdateCapability::BootService) {
        tokio::time::timeout(ACCEPTING_READY_TIMEOUT, first_ready(session))
            .await
            .ok()
            .flatten()
            .map(|ready| ready.payload.at)
    } else {
        None
    };

    let result = if capabilities.server_self_update_progress == Some(true) {
        let mut stream = session.client.subscribe::<ServerUpdateServerWithProgress>(input.clone());
        let mut terminal = None;
        let mut received_progress = false;
        let mut ended = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(events) => {
                    for event in events {
                        match event {
                            ServerSelfUpdateProgressEvent::Progress(progress) => {
                                received_progress = true;
                                *stage = stage_of(&progress.stage);
                            }
                            ServerSelfUpdateProgressEvent::Complete(complete) => {
                                *stage = if commits_desktop(&complete.result) {
                                    ServerUpdateStage::Installing
                                } else {
                                    ServerUpdateStage::Resuming
                                };
                                terminal = Some(complete.result);
                            }
                            ServerSelfUpdateProgressEvent::Unknown(_) => continue,
                        }
                        claim.set(running(*stage, target_version));
                    }
                }
                Err(error) => {
                    ended = Some(error);
                    break;
                }
            }
        }
        // `resolveServerUpdateProgressResult`.
        match (terminal, ended) {
            (Some(result), None) => result,
            (Some(result), Some(error)) if is_handoff_loss(&error) => result,
            (None, Some(error)) if is_handoff_loss(&error) && received_progress && in_place.is_some() => {
                ServerSelfUpdateResult {
                    target_version: target_version.clone(),
                    method: in_place.clone().expect("checked"),
                    update_id: None,
                    desktop_update_token: None,
                }
            }
            (_, Some(error)) => return Err(rpc_failure(&error)),
            (None, None) => return Err(ServerUpdateFailure::ProgressIncomplete { version: target_version.clone() }),
        }
    } else {
        match session.client.call::<ServerUpdateServer>(input.clone()).await {
            Ok(result) => result,
            // Older servers can tear the transport down before their acknowledgement arrives;
            // only that is a handoff, proven below by the target's ready.
            Err(error) if is_handoff_loss(&error) && in_place.is_some() => ServerSelfUpdateResult {
                target_version: target_version.clone(),
                method: in_place.clone().expect("checked"),
                update_id: None,
                desktop_update_token: None,
            },
            Err(error) => return Err(rpc_failure(&error)),
        }
    };

    if let Some(token) = result.desktop_update_token.clone().filter(|_| commits_desktop(&result)) {
        commit_desktop(session, &token).await?;
    }
    *target_version = result.target_version.clone();
    *stage = ServerUpdateStage::Resuming;
    claim.set(running(*stage, target_version));

    // A desktop host already on its track's newest build only saved the track.
    if result.method == ServerSelfUpdateMethod::DesktopApp
        && result.desktop_update_token.is_none()
        && result.target_version == config.environment.server_version
    {
        return Ok(result);
    }

    let nudger = tokio::spawn(nudge_reconnect(supervisor.clone()));
    let _stop_nudging = AbortOnDrop(nudger);
    let wait = async {
        if commits_desktop(&result) {
            let token = result.desktop_update_token.clone().expect("checked");
            for attempt in 1..=DESKTOP_COMMIT_ATTEMPTS {
                let ready = next_ready(supervisor, Some(generation), |_| true).await;
                if ready.payload.environment.server_version == result.target_version {
                    return Ok(ready);
                }
                if attempt == DESKTOP_COMMIT_ATTEMPTS {
                    break;
                }
                let session = supervisor.session.borrow().clone();
                if let Some(session) = session {
                    commit_desktop(&session, &token).await?;
                }
            }
            Err(ServerUpdateFailure::DesktopResumedWithoutUpdate)
        } else {
            Ok(next_ready(supervisor, None, |ready| resumes(&result, ready, accepted_at.as_deref())).await)
        }
    };
    let ready = tokio::time::timeout(RESUME_TIMEOUT, wait)
        .await
        .map_err(|_| ServerUpdateFailure::ResumeTimeout { version: target_version.clone() })??;
    validate(&result, &ready)?;
    Ok(result)
}

/// Commits a prepared desktop update. The commit relaunches the app, so a dropped socket is success.
async fn commit_desktop(session: &RpcSession, token: &str) -> Result<(), ServerUpdateFailure> {
    let input = DesktopUpdateCommitInput { request_id: token.to_owned() };
    match session.client.call::<ServerCommitDesktopUpdate>(input).await {
        Ok(_) => Ok(()),
        Err(error) if is_handoff_loss(&error) => Ok(()),
        Err(error) => Err(rpc_failure(&error)),
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
