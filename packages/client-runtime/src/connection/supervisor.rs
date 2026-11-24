//! One retry owner per environment: connects, keeps the session, backs off
//! on failure, and moves back to a better route when one returns (spec 01,
//! section 5.3; spec 02, section 3.5).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::catalog::{CatalogEntry, ConnectionRoute};
use super::driver::{self, ConnectionContext, RoutePreflight};
use super::model::{
    AttemptStage, ConnectionError, ConnectionPhase, ConnectionStatus, DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS,
    NetworkStatus,
};
use super::routes::{ReportedEndpoint, route_id};
use crate::authorization::HttpAuthorization;
use crate::authorization::remote::now_epoch_ms;
use crate::rpc::RpcSession;

const RETRY_BASE_DELAY_MS: f64 = 1_000.0;
const RETRY_MAX_DELAY_MS: f64 = 300_000.0;
/// The first loss of a live session retries almost at once.
const SESSION_LOSS_RETRY_DELAY_MS: f64 = 250.0;
/// Someone may be approving this device right now; check back soon.
const APPROVAL_PENDING_RETRY_DELAY: Duration = Duration::from_secs(4);
/// An approval request lives 10 minutes; after that, back off normally.
const APPROVAL_PENDING_FAST_RETRIES: u32 = 150;
/// A session that stayed up this long resets the retry ladder.
const BACKOFF_RESET_AFTER: Duration = Duration::from_secs(30);
/// While on a later route, earlier routes are checked this often.
const BETTER_ROUTE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
/// A better route that answered but then failed to connect rests this long.
const BETTER_ROUTE_COOLDOWN: Duration = Duration::from_secs(300);
/// A dead route's checks back off up to this.
const SILENT_ROUTE_BACKOFF_MAX: Duration = Duration::from_secs(1_800);

/// Retry delay for the `failure`th failure (0-based): exponential with
/// jitter in the upper half, capped at 5 minutes.
pub fn retry_delay(failure: u32, random: f64) -> Duration {
    let ceiling = RETRY_MAX_DELAY_MS.min(RETRY_BASE_DELAY_MS * 2f64.powi(failure as i32 + 1));
    Duration::from_millis((ceiling / 2.0 + ceiling / 2.0 * random).round() as u64)
}

/// Events that change what the supervisor wants or how soon it retries.
#[derive(Clone, Copy, Debug)]
pub enum SupervisorSignal {
    Connect,
    Disconnect,
    /// Retry now and reset the backoff ladder (a user's "Retry").
    Retry,
    NetworkChanged(NetworkStatus),
    /// The app came to the foreground or woke from sleep.
    Wakeup,
}

/// Persists routes the supervisor learned from the server's config.
pub type LearnRoutes = Arc<dyn Fn(&CatalogEntry, &ConnectionRoute, &[ReportedEndpoint]) + Send + Sync>;

/// What the UI and state layer observe about one environment.
#[derive(Clone)]
pub struct SupervisorHandle {
    pub status: watch::Receiver<ConnectionStatus>,
    /// The live session, or `None` between sessions.
    pub session: watch::Receiver<Option<Arc<RpcSession>>>,
    /// The live session's HTTP credential, updated as DPoP renews.
    pub http_authorization: watch::Receiver<Option<HttpAuthorization>>,
    signals: mpsc::UnboundedSender<SupervisorSignal>,
}

impl SupervisorHandle {
    pub fn signal(&self, signal: SupervisorSignal) {
        _ = self.signals.send(signal);
    }
}

/// Owns the supervisor task; dropping it stops the task and its session.
pub struct EnvironmentSupervisor {
    handle: SupervisorHandle,
    task: tokio::task::JoinHandle<()>,
    /// Cleared on drop: a watch keeps its last value alive for every
    /// receiver, which would hold the socket open.
    session: watch::Sender<Option<Arc<RpcSession>>>,
}

impl Drop for EnvironmentSupervisor {
    fn drop(&mut self) {
        self.task.abort();
        self.session.send_replace(None);
    }
}

impl EnvironmentSupervisor {
    /// Starts the loop. `entry` carries catalog edits (reordered or learned
    /// routes) while it runs.
    pub fn spawn(
        context: Arc<ConnectionContext>,
        entry: watch::Receiver<CatalogEntry>,
        desired: bool,
        learn_routes: Option<LearnRoutes>,
    ) -> Self {
        let (status_tx, status) = watch::channel(ConnectionStatus::available());
        let (session_tx, session) = watch::channel(None);
        let session_owner = session_tx.clone();
        let (auth_tx, http_authorization) = watch::channel(None);
        let (signals, signal_rx) = mpsc::unbounded_channel();
        let state = Supervisor {
            context,
            entry,
            learn_routes,
            status: status_tx,
            session: session_tx,
            auth: auth_tx,
            signals: signal_rx,
            desired,
            network: NetworkStatus::Unknown,
            cooldowns: HashMap::new(),
            silent_routes: HashMap::new(),
        };
        let task = tokio::spawn(state.run());
        Self {
            handle: SupervisorHandle { status, session, http_authorization, signals },
            task,
            session: session_owner,
        }
    }

    pub fn handle(&self) -> &SupervisorHandle {
        &self.handle
    }
}

/// How a session or attempt ended.
enum Ended {
    /// The user or platform asked for something else; no failure.
    Interrupted { reset_retry: bool },
    Failed { error: ConnectionError, established: bool, stable: bool },
}

struct Supervisor {
    context: Arc<ConnectionContext>,
    entry: watch::Receiver<CatalogEntry>,
    learn_routes: Option<LearnRoutes>,
    status: watch::Sender<ConnectionStatus>,
    session: watch::Sender<Option<Arc<RpcSession>>>,
    auth: watch::Sender<Option<HttpAuthorization>>,
    signals: mpsc::UnboundedReceiver<SupervisorSignal>,
    desired: bool,
    network: NetworkStatus,
    cooldowns: HashMap<String, Instant>,
    /// Earlier routes that were silent: (failures, next check).
    silent_routes: HashMap<String, (u32, Instant)>,
}

impl Supervisor {
    /// Applies a signal. Returns whether it resets the retry ladder.
    fn apply(&mut self, signal: SupervisorSignal) -> bool {
        match signal {
            SupervisorSignal::Connect => self.desired = true,
            SupervisorSignal::Disconnect => self.desired = false,
            SupervisorSignal::NetworkChanged(network) => self.network = network,
            SupervisorSignal::Retry | SupervisorSignal::Wakeup => return true,
        }
        false
    }

    fn publish(&self, update: impl FnOnce(&mut ConnectionStatus)) {
        self.status.send_modify(|status| {
            update(status);
            status.desired = self.desired;
            status.network = self.network;
        });
    }

    async fn next_signal(&mut self) -> Option<SupervisorSignal> {
        self.signals.recv().await
    }

    async fn run(mut self) {
        let mut failure_count: u32 = 0;
        let mut backoff_failures: u32 = 0;
        let mut approval_waits: u32 = 0;
        let mut latest_failure: Option<ConnectionError> = None;
        let mut force = false;
        let mut generation: u64 = 0;
        loop {
            if !self.desired {
                (failure_count, backoff_failures, approval_waits, latest_failure) = (0, 0, 0, None);
                self.publish(|s| *s = ConnectionStatus { generation, ..ConnectionStatus::available() });
                let Some(signal) = self.next_signal().await else { return };
                self.apply(signal);
                continue;
            }
            if self.network == NetworkStatus::Offline && !force {
                self.publish(|s| {
                    s.phase = ConnectionPhase::Offline;
                    s.stage = None;
                    s.attempt = failure_count + 1;
                    s.last_failure = latest_failure.clone();
                    s.retry_at = None;
                });
                let Some(signal) = self.next_signal().await else { return };
                if self.apply(signal) {
                    (failure_count, backoff_failures, approval_waits) = (0, 0, 0);
                }
                continue;
            }
            force = false;
            let attempt = failure_count + 1;
            self.publish(|s| {
                s.phase = ConnectionPhase::Connecting;
                s.stage = Some(AttemptStage::Preparing);
                s.attempt = attempt;
                s.last_failure = latest_failure.clone();
                s.retry_at = None;
                s.active_route_id = None;
            });
            let ended = self.attempt(&mut generation).await;
            let ended = match ended {
                Some(ended) => ended,
                None => return,
            };
            let (error, established, stable) = match ended {
                Ended::Interrupted { reset_retry } => {
                    if reset_retry {
                        (failure_count, backoff_failures, approval_waits) = (0, 0, 0);
                        force = true;
                    }
                    continue;
                }
                Ended::Failed { error, established, stable } => (error, established, stable),
            };
            if established {
                // A session came up, so no earlier failure describes the connection.
                latest_failure = None;
                if stable {
                    (failure_count, backoff_failures, approval_waits) = (0, 0, 0);
                }
            } else {
                latest_failure = Some(error.clone());
            }
            if error.is_blocked() {
                self.publish(|s| {
                    s.phase = ConnectionPhase::Blocked;
                    s.stage = None;
                    s.attempt = attempt;
                    s.last_failure = Some(error.clone());
                    s.retry_at = None;
                    s.active_route_id = None;
                });
                let Some(signal) = self.next_signal().await else { return };
                if self.apply(signal) {
                    (failure_count, backoff_failures, approval_waits) = (0, 0, 0);
                    force = true;
                }
                continue;
            }
            let first_loss = established && failure_count == 0;
            failure_count += 1;
            let random: f64 = rand::random();
            if error.is_approval_pending() {
                approval_waits += 1;
            }
            let awaiting_approval = error.is_approval_pending() && approval_waits <= APPROVAL_PENDING_FAST_RETRIES;
            if !awaiting_approval {
                backoff_failures += 1;
            }
            let delay = if awaiting_approval {
                APPROVAL_PENDING_RETRY_DELAY
            } else if first_loss {
                Duration::from_millis((SESSION_LOSS_RETRY_DELAY_MS * (0.5 + random / 2.0)).round() as u64)
            } else if error.is_approval_pending() {
                retry_delay(backoff_failures - 1, random).max(APPROVAL_PENDING_RETRY_DELAY)
            } else {
                retry_delay(backoff_failures - 1, random)
            };
            self.publish(|s| {
                s.phase = ConnectionPhase::Backoff;
                s.stage = None;
                s.attempt = attempt;
                s.last_failure = latest_failure.clone();
                s.retry_at = Some(SystemTime::now() + delay);
                s.active_route_id = None;
            });
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                signal = self.signals.recv() => {
                    let Some(signal) = signal else { return };
                    if self.apply(signal) {
                        (failure_count, backoff_failures, approval_waits) = (0, 0, 0);
                        force = true;
                    }
                }
            }
        }
    }

    /// One attempt and, if it connects, the life of its session. `None`
    /// when the supervisor is shutting down.
    async fn attempt(&mut self, generation: &mut u64) -> Option<Ended> {
        let entry = self.entry.borrow().clone();
        let status = self.status.clone();
        let progress = move |stage: AttemptStage| {
            status.send_modify(|s| s.stage = Some(stage));
        };
        let connecting = driver::connect(&self.context, &entry, &progress);
        let lease = tokio::select! {
            lease = connecting => lease,
            signal = self.signals.recv() => {
                let signal = signal?;
                let reset_retry = self.apply(signal);
                return Some(Ended::Interrupted { reset_retry });
            }
        };
        let lease = match lease {
            Ok(lease) => lease,
            Err(error) => return Some(Ended::Failed { error, established: false, stable: false }),
        };
        *generation += 1;
        let session = lease.session;
        let active_route_id = session.prepared.route_id.clone();
        let connected_at = Instant::now();
        // Observers that see `Connected` must already see its session.
        self.auth.send_replace(Some(session.prepared.http_authorization.clone()));
        self.session.send_replace(Some(session.clone()));
        self.publish(|s| {
            s.phase = ConnectionPhase::Connected;
            s.stage = None;
            s.generation = *generation;
            s.last_failure = None;
            s.retry_at = None;
            s.active_route_id = Some(active_route_id.clone());
            s.refused_route_ids = lease.refused_route_ids.clone().into();
        });
        if let (Some(learn), Some(active)) = (
            &self.learn_routes,
            entry.routes.iter().find(|route| route_id(&route.target) == active_route_id),
        ) {
            learn(&entry, active, &session.direct_endpoints);
        }
        let renewal = tokio::spawn(renew_before_expiry(
            self.context.clone(),
            session.prepared.http_authorization.clone(),
            self.auth.clone(),
        ));
        let mut better_route_check = tokio::time::interval_at(
            Instant::now() + BETTER_ROUTE_CHECK_INTERVAL,
            BETTER_ROUTE_CHECK_INTERVAL,
        );
        let mut switching_to: Option<String> = None;
        let ended = loop {
            tokio::select! {
                error = session.ended() => {
                    break Ended::Failed { error, established: true, stable: connected_at.elapsed() >= BACKOFF_RESET_AFTER };
                }
                signal = self.signals.recv() => {
                    let Some(signal) = signal else { renewal.abort(); return None };
                    let reset_retry = self.apply(signal);
                    // A wakeup, retry or offline report keeps a working session (offline
                    // reports are often wrong, e.g. for a loopback host); only intent ends it.
                    if !self.desired {
                        break Ended::Interrupted { reset_retry: false };
                    }
                    if reset_retry {
                        self.silent_routes.clear();
                    }
                }
                _ = better_route_check.tick() => {
                    if let Some(route) = self.check_better_routes(&active_route_id).await {
                        switching_to = Some(route);
                        break Ended::Interrupted { reset_retry: connected_at.elapsed() >= BACKOFF_RESET_AFTER };
                    }
                }
            }
        };
        renewal.abort();
        self.session.send_replace(None);
        self.auth.send_replace(None);
        drop(session);
        if let Some(route) = switching_to {
            // If the next connect lands elsewhere, that route rests a while.
            self.cooldowns.insert(route, Instant::now() + BETTER_ROUTE_COOLDOWN);
        }
        Some(ended)
    }

    /// Preflights routes ahead of the active one. Returns the first ready one.
    async fn check_better_routes(&mut self, active_route_id: &str) -> Option<String> {
        let entry = self.entry.borrow().clone();
        let current = entry.routes.iter().position(|route| route_id(&route.target) == active_route_id)?;
        let now = Instant::now();
        let candidates: Vec<&ConnectionRoute> = entry.routes[..current]
            .iter()
            .filter(|route| {
                let id = route_id(&route.target);
                self.cooldowns.get(id).is_none_or(|until| *until <= now)
                    && self.silent_routes.get(id).is_none_or(|(_, until)| *until <= now)
            })
            .collect();
        let results = futures::future::join_all(
            candidates.iter().map(|route| driver::preflight(&self.context, &entry, route)),
        )
        .await;
        let mut ready = None;
        for (route, result) in candidates.iter().zip(results) {
            let id = route_id(&route.target).to_owned();
            if result == RoutePreflight::Silent {
                let failures = self.silent_routes.get(&id).map_or(0, |(failures, _)| *failures) + 1;
                let delay = (BETTER_ROUTE_CHECK_INTERVAL * 2u32.pow(failures.min(10))).min(SILENT_ROUTE_BACKOFF_MAX);
                self.silent_routes.insert(id, (failures, now + delay));
                continue;
            }
            self.silent_routes.remove(&id);
            if result == RoutePreflight::Refused {
                self.publish(|s| {
                    if !s.refused_route_ids.contains(&id) {
                        s.refused_route_ids = s.refused_route_ids.iter().cloned().chain([id.clone()]).collect();
                    }
                });
            }
            if result == RoutePreflight::Ready && ready.is_none() {
                ready = Some(id);
            }
        }
        ready
    }
}

/// Keeps a DPoP session's token fresh, renewing a minute before expiry.
/// A relay-minted session stops at expiry (the server closes it); a direct
/// one keeps trying through its renewal window until a blocked failure.
async fn renew_before_expiry(
    context: Arc<ConnectionContext>,
    mut current: HttpAuthorization,
    publish: watch::Sender<Option<HttpAuthorization>>,
) {
    let direct = matches!(
        &current,
        HttpAuthorization::Dpop { renewal: crate::authorization::DpopRenewal::Direct { .. }, .. }
    );
    let Some(mut expires_at) = current.expires_at_epoch_ms() else { return };
    loop {
        let now = now_epoch_ms();
        if !direct && now >= expires_at {
            return;
        }
        // The floor spaces out retries and keeps a clock jump from spinning.
        let floor = if now < expires_at { 5_000 } else { DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS };
        let wait = (expires_at - DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS - now).max(floor);
        tokio::time::sleep(Duration::from_millis(wait as u64)).await;
        match context.auth.renew(&current).await {
            Ok(renewed) => {
                expires_at = expires_at.max(renewed.expires_at_epoch_ms().unwrap_or(expires_at));
                current = renewed;
                publish.send_replace(Some(current.clone()));
            }
            Err(error) if error.is_blocked() => {
                tracing::warn!(%error, "could not renew the session's credential");
                return;
            }
            Err(error) => tracing::debug!(%error, "credential renewal failed, retrying"),
        }
    }
}
