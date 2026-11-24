//! The bridge between the tokio network layer and GPUI. Ports the parts of
//! `apps/web/src/connection` and `environments/` that own the runtime.
//!
//! [`ConnectionRuntime`] is a GPUI global. It owns the tokio runtime, the
//! [`ConnectionRegistry`] opened on the app's data directory, the T3 Connect
//! sign-in and relay client. Views observe [`ConnectionsState`], an entity
//! the bridge keeps in step with each supervisor's `watch` status. Commands
//! run on tokio and hand their result back as a GPUI [`Task`].

pub mod presentation;
pub mod shell;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui_kit::{App, AppContext as _, Entity, Global, SharedString, Task};
use t3code_client_runtime::authorization::{RelayAccess, RemoteAuthorization};
use t3code_client_runtime::connection::catalog::{ConnectionRoute, ConnectionTarget, TargetRoute};
use t3code_client_runtime::connection::credential_store::CredentialStore;
use t3code_client_runtime::connection::onboarding::{PairingRequest, pair};
use t3code_client_runtime::connection::registry::RegistryError;
use t3code_client_runtime::connection::{
    CatalogEntry, ConnectionContext, ConnectionError, ConnectionPhase, ConnectionRegistry, ConnectionStatus,
    SupervisorHandle, SupervisorSignal,
};
use t3code_client_runtime::connection::model::NetworkStatus;
use t3code_client_runtime::environment::EnvironmentId;
use t3code_client_runtime::platform::electron_import::{
    ElectronImport, ElectronPaths, ImportError, SafeStorageKey, load_platform_key, read_electron_catalog,
    read_electron_clerk_client_jwt,
};
use t3code_client_runtime::platform::{ClientPresentation, SecretStore, open_secret_store};
use t3code_client_runtime::relay::{
    ClerkError, ClerkSession, CloudAccountState, CloudConfig, DpopKey, EmailCodeSignIn, RelayClient,
    RelayEnvironmentRecord,
};
use t3code_client_runtime::rpc::RpcSessionOptions;
use t3code_contracts::rpc::methods::ServerProbe;

/// While connected, the round trip is measured this often.
const LATENCY_PROBE_INTERVAL: Duration = Duration::from_secs(30);
/// The sleep detector's tick. A tick that lands much later on the wall
/// clock means the machine slept.
const WAKE_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const WAKE_GAP: Duration = Duration::from_secs(15);

/// Where the Electron import reads, and with which key.
#[derive(Clone)]
pub struct ElectronImportSource {
    pub paths: ElectronPaths,
    /// `None` reads the key from the OS (DPAPI or Keychain). Fixtures pass one.
    pub key: Option<SafeStorageKey>,
}

/// How the runtime starts. [`RuntimeConfig::for_app`] is the real app.
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    /// `None` opens the OS keystore, falling back to a 0600 file in `data_dir`.
    pub secrets: Option<Arc<dyn SecretStore>>,
    /// `None` hides T3 Connect.
    pub cloud: Option<CloudConfig>,
    pub presentation: ClientPresentation,
    pub electron: Option<ElectronImportSource>,
}

impl RuntimeConfig {
    /// The installed app: its own data dir, the OS keystore, production T3 Connect
    /// unless the environment names another (see [`CloudConfig::from_env`]).
    pub fn for_app() -> Self {
        Self {
            data_dir: t3code_client_runtime::platform::default_data_dir(),
            secrets: None,
            cloud: Some(CloudConfig::from_env()),
            presentation: ClientPresentation::default(),
            electron: ElectronPaths::detect().map(|paths| ElectronImportSource { paths, key: None }),
        }
    }
}

/// One saved environment as the UI shows it.
#[derive(Clone, Debug)]
pub struct EnvironmentState {
    pub entry: CatalogEntry,
    /// `None` while switched off: no supervisor runs.
    pub status: Option<ConnectionStatus>,
    /// The last `server.probe` round trip on the live session.
    pub latency: Option<Duration>,
    pub server_version: Option<SharedString>,
}

impl EnvironmentState {
    pub fn id(&self) -> &EnvironmentId {
        &self.entry.environment_id
    }

    pub fn is_connected(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.phase == ConnectionPhase::Connected)
    }
}

/// How sure the app is that a relay environment answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayAvailability {
    Checking,
    Online,
    Offline,
    Error,
}

/// One environment on the T3 Connect account.
#[derive(Clone, Debug)]
pub struct RelayEnvironment {
    pub record: RelayEnvironmentRecord,
    pub availability: RelayAvailability,
}

/// The account's environments, as last listed from the relay.
#[derive(Clone, Debug, Default)]
pub struct RelayDiscovery {
    pub refreshing: bool,
    pub error: Option<SharedString>,
    pub environments: Vec<RelayEnvironment>,
}

/// T3 Connect as the UI sees it. Absent when the app has no cloud config.
#[derive(Clone, Debug)]
pub struct CloudState {
    pub account: CloudAccountState,
    pub discovery: RelayDiscovery,
}

/// Everything the environment screens observe. Updated only by the bridge.
pub struct ConnectionsState {
    pub environments: Vec<EnvironmentState>,
    pub cloud: Option<CloudState>,
    /// This install's DPoP verification code, shown while a host approves it.
    pub verification_code: SharedString,
    /// Whether the Electron import has somewhere to read from.
    pub electron_import_available: bool,
}

impl ConnectionsState {
    pub fn environment(&self, id: &EnvironmentId) -> Option<&EnvironmentState> {
        self.environments.iter().find(|env| env.id() == id)
    }
}

/// Updates the bridge hands from tokio to GPUI.
enum BridgeEvent {
    Entries(Arc<[CatalogEntry]>),
    Status { id: EnvironmentId, status: ConnectionStatus, server_version: Option<SharedString> },
    Latency { id: EnvironmentId, latency: Option<Duration> },
    Account(CloudAccountState),
    Discovery(RelayDiscovery),
}

struct Cloud {
    clerk: Arc<ClerkSession>,
    relay: Arc<RelayClient>,
}

/// The GPUI global. See the module docs.
pub struct ConnectionRuntime {
    runtime: Option<tokio::runtime::Runtime>,
    handle: tokio::runtime::Handle,
    registry: ConnectionRegistry,
    auth: Arc<RemoteAuthorization>,
    cloud: Option<Cloud>,
    electron: Option<ElectronImportSource>,
    state: Entity<ConnectionsState>,
    events: mpsc::UnboundedSender<BridgeEvent>,
    data_dir: PathBuf,
}

impl Global for ConnectionRuntime {}

/// Why a command failed, in English for the UI.
#[derive(Clone, Debug)]
pub struct CommandError(pub SharedString);

impl<E: std::fmt::Display> From<E> for CommandError {
    fn from(error: E) -> Self {
        Self(error.to_string().into())
    }
}

/// What the Electron import found, before anything is saved. Holds the
/// decrypted T3 Connect client JWT so saving needs no second Keychain read
/// (each read can prompt on macOS).
#[derive(Clone)]
pub struct ImportPreview {
    pub import: ElectronImport,
    pub has_clerk_session: bool,
    clerk_client_jwt: Option<String>,
}

impl std::fmt::Debug for ImportPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportPreview").field("import", &self.import).field("has_clerk_session", &self.has_clerk_session).finish()
    }
}

impl ConnectionRuntime {
    /// Starts the runtime: opens the catalog and starts a supervisor per
    /// enabled environment, restores the T3 Connect sign-in, and wires
    /// sleep/wake. Call once, before any view reads [`Self::state`].
    pub fn init(config: RuntimeConfig, cx: &mut App) -> anyhow::Result<()> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("t3-connection")
            .enable_all()
            .build()?;
        let handle = runtime.handle().clone();
        let RuntimeConfig { data_dir, secrets, cloud, presentation, electron } = config;
        let stored_data_dir = data_dir.clone();
        let (registry, auth, cloud) = runtime.block_on(async {
            std::fs::create_dir_all(&data_dir)?;
            let secrets = secrets.unwrap_or_else(|| open_secret_store(&data_dir));
            let key = DpopKey::load_or_create(secrets.as_ref())?;
            let cloud = match cloud {
                Some(config) => Some(Cloud {
                    clerk: Arc::new(ClerkSession::new(&config, secrets.clone())?),
                    relay: Arc::new(RelayClient::new(&config.relay_url, key.clone())?),
                }),
                None => None,
            };
            let relay = cloud
                .as_ref()
                .map(|cloud| RelayAccess { client: cloud.relay.clone(), session: cloud.clerk.clone() });
            let auth = Arc::new(RemoteAuthorization::new(presentation, key, CredentialStore::new(secrets), relay));
            let context = Arc::new(ConnectionContext {
                auth: auth.clone(),
                session_options: RpcSessionOptions::default(),
                frame_tap: None,
            });
            let registry = ConnectionRegistry::open(context, &data_dir).await?;
            anyhow::Ok((registry, auth, cloud))
        })?;
        let initial = registry.entries().borrow().clone();
        let state = cx.new(|_| ConnectionsState {
            environments: initial
                .iter()
                .map(|entry| EnvironmentState { entry: entry.clone(), status: None, latency: None, server_version: None })
                .collect(),
            cloud: cloud.as_ref().map(|cloud| CloudState {
                account: cloud.clerk.account().borrow().clone(),
                discovery: RelayDiscovery::default(),
            }),
            verification_code: auth.key().verification_code().into(),
            electron_import_available: electron.is_some(),
        });
        let (events, receiver) = mpsc::unbounded();
        handle.spawn(watch_registry(registry.clone(), events.clone()));
        handle.spawn(detect_wake(registry.clone()));
        if runtime_metrics_enabled() {
            handle.spawn(log_runtime_metrics(handle.clone()));
        }
        if let Some(cloud) = &cloud {
            handle.spawn(watch_account(cloud.clerk.clone(), cloud.relay.clone(), registry.clone(), auth.clone(), events.clone()));
            handle.spawn(restore_account(cloud.clerk.clone()));
        }
        apply_events(state.clone(), receiver, cx);
        cx.set_global(Self {
            runtime: Some(runtime),
            handle,
            registry,
            auth,
            cloud,
            electron,
            state,
            events,
            data_dir: stored_data_dir,
        });
        cx.on_app_quit(|cx| {
            ConnectionRuntime::shutdown(cx);
            async {}
        })
        .detach();
        Ok(())
    }

    /// Whether [`Self::init`] ran. Scenes render without a runtime.
    pub fn is_running(cx: &App) -> bool {
        cx.has_global::<Self>()
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The observable state. Read it in render; observe it to re-render.
    pub fn state(cx: &App) -> Entity<ConnectionsState> {
        cx.global::<Self>().state.clone()
    }

    /// Stops every supervisor and the tokio runtime. Sockets close as their
    /// tasks drop. Safe to call twice.
    pub fn shutdown(cx: &mut App) {
        if !cx.has_global::<Self>() {
            return;
        }
        if let Some(runtime) = cx.global_mut::<Self>().runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }

    /// Runs `future` on tokio and resolves on GPUI with its output.
    pub fn spawn<T: Send + 'static>(
        &self,
        future: impl Future<Output = Result<T, CommandError>> + Send + 'static,
        cx: &App,
    ) -> Task<Result<T, CommandError>> {
        let (sender, receiver) = oneshot::channel();
        self.handle.spawn(async move {
            _ = sender.send(future.await);
        });
        cx.background_executor().spawn(async move {
            receiver.await.unwrap_or_else(|_| Err(CommandError("The app is shutting down.".into())))
        })
    }

    /// The tokio runtime connection work runs on.
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    /// One environment's live session and signing key, for requests outside
    /// the RPC socket (preview stream, tunnel, usage). `None` while switched off.
    pub fn environment_link(&self, id: &EnvironmentId) -> Option<t3code_client_runtime::preview::environment_link::EnvironmentLink> {
        Some(t3code_client_runtime::preview::environment_link::EnvironmentLink {
            environment_id: id.clone(),
            supervisor: self.registry.supervisor(id)?,
            auth: self.auth.clone(),
        })
    }

    /// The tokio runtime network and sync tasks run on.
    pub fn tokio(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    /// The registry of saved environments and their supervisors.
    pub fn registry(&self) -> &ConnectionRegistry {
        &self.registry
    }

    /// Holds the DPoP key and credentials every environment request uses.
    pub fn auth(&self) -> &Arc<RemoteAuthorization> {
        &self.auth
    }

    /// The app's data directory (catalog, secrets, caches).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// Wakes every supervisor: retry now, check better routes.
    pub fn wake(cx: &App) {
        if let Some(this) = cx.try_global::<Self>() {
            this.registry.broadcast(SupervisorSignal::Wakeup);
        }
    }

    /// The supervisor of a running environment: its status and live session.
    pub fn supervisor(&self, id: &EnvironmentId) -> Option<SupervisorHandle> {
        self.registry.supervisor(id)
    }

    /// A user's "Retry now" on one environment.
    pub fn retry(&self, id: &EnvironmentId) {
        if let Some(handle) = self.registry.supervisor(id) {
            handle.signal(SupervisorSignal::Retry);
        }
    }

    /// Pairs and saves. `expected` adds a route to that environment instead.
    pub fn pair(&self, request: PairingRequest, expected: Option<EnvironmentId>, cx: &App) -> Task<Result<SharedString, CommandError>> {
        let (auth, registry) = (self.auth.clone(), self.registry.clone());
        self.spawn(
            async move {
                let paired = pair(&auth, &request, expected.as_ref()).await.map_err(connection_error)?;
                registry.save_paired(&paired).await.map_err(registry_error)?;
                Ok(SharedString::from(paired.label))
            },
            cx,
        )
    }

    pub fn set_enabled(&self, id: EnvironmentId, enabled: bool, cx: &App) -> Task<Result<(), CommandError>> {
        let registry = self.registry.clone();
        self.spawn(async move { registry.set_enabled(&id, enabled).await.map_err(registry_error) }, cx)
    }

    pub fn remove_environment(&self, id: EnvironmentId, cx: &App) -> Task<Result<(), CommandError>> {
        let registry = self.registry.clone();
        self.spawn(async move { registry.remove_environment(&id).await.map_err(registry_error) }, cx)
    }

    pub fn remove_route(&self, id: EnvironmentId, route_id: String, cx: &App) -> Task<Result<(), CommandError>> {
        let registry = self.registry.clone();
        self.spawn(async move { registry.remove_route(&id, &route_id).await.map_err(registry_error) }, cx)
    }

    pub fn reorder_routes(&self, id: EnvironmentId, order: Vec<String>, cx: &App) -> Task<Result<(), CommandError>> {
        let registry = self.registry.clone();
        self.spawn(async move { registry.reorder_routes(&id, &order).await.map_err(registry_error) }, cx)
    }

    pub fn rename(&self, id: EnvironmentId, label: String, cx: &App) -> Task<Result<(), CommandError>> {
        let registry = self.registry.clone();
        self.spawn(async move { registry.rename(&id, &label).await.map_err(registry_error) }, cx)
    }

    /// Whether T3 Connect is configured in this app.
    pub fn has_cloud(&self) -> bool {
        self.cloud.is_some()
    }

    pub fn start_email_code(&self, email: String, cx: &App) -> Task<Result<EmailCodeSignIn, CommandError>> {
        let Some(clerk) = self.clerk() else { return Task::ready(Err(no_cloud())) };
        self.spawn(async move { clerk.start_email_code(&email).await.map_err(clerk_error) }, cx)
    }

    pub fn verify_email_code(&self, sign_in: EmailCodeSignIn, code: String, cx: &App) -> Task<Result<(), CommandError>> {
        let Some(clerk) = self.clerk() else { return Task::ready(Err(no_cloud())) };
        self.spawn(async move { clerk.verify_email_code(&sign_in, &code).await.map(|_| ()).map_err(clerk_error) }, cx)
    }

    /// Signs out and drops T3 Connect routes' cached tokens with the account.
    pub fn sign_out(&self, cx: &App) -> Task<Result<(), CommandError>> {
        let Some(clerk) = self.clerk() else { return Task::ready(Err(no_cloud())) };
        let events = self.events.clone();
        self.spawn(
            async move {
                clerk.sign_out().await.map_err(clerk_error)?;
                _ = events.unbounded_send(BridgeEvent::Discovery(RelayDiscovery::default()));
                Ok(())
            },
            cx,
        )
    }

    /// Lists the account's environments and checks each one's tunnel.
    pub fn refresh_relay(&self, cx: &App) {
        let Some(cloud) = &self.cloud else { return };
        let previous = self.state.read(cx).cloud.as_ref().map(|c| c.discovery.environments.clone()).unwrap_or_default();
        self.handle.spawn(discover_relay(cloud.clerk.clone(), cloud.relay.clone(), self.events.clone(), previous));
    }

    /// Saves a T3 Connect route for an environment found on the account.
    pub fn add_relay_environment(&self, record: RelayEnvironmentRecord, cx: &App) -> Task<Result<(), CommandError>> {
        let registry = self.registry.clone();
        let route = ConnectionRoute {
            target: ConnectionTarget { environment_id: record.environment_id, label: record.label, route: TargetRoute::Relay },
            profile: None,
        };
        self.spawn(async move { registry.save_relay(route).await.map_err(registry_error) }, cx)
    }

    /// Reads and decrypts the Electron catalog. Saves nothing.
    pub fn preview_electron_import(&self, cx: &App) -> Task<Result<ImportPreview, CommandError>> {
        let Some(source) = self.electron.clone() else {
            return Task::ready(Err(CommandError("No desktop app data was found on this computer.".into())));
        };
        self.spawn(
            async move {
                tokio::task::spawn_blocking(move || {
                    let key = electron_key(&source)?;
                    let import = read_electron_catalog(&source.paths, &key)?;
                    let clerk_client_jwt = read_electron_clerk_client_jwt(&source.paths, &key)?;
                    Ok::<_, ImportError>(ImportPreview { import, has_clerk_session: clerk_client_jwt.is_some(), clerk_client_jwt })
                })
                .await
                .map_err(CommandError::from)?
                .map_err(CommandError::from)
            },
            cx,
        )
    }

    /// Saves a previewed import. With `with_clerk_session`, also adopts the
    /// desktop app's T3 Connect sign-in, which both apps then share.
    pub fn apply_electron_import(&self, preview: ImportPreview, with_clerk_session: bool, cx: &App) -> Task<Result<usize, CommandError>> {
        let registry = self.registry.clone();
        let clerk = self.clerk();
        self.spawn(
            async move {
                let added = registry.import(&preview.import).await.map_err(registry_error)?;
                if with_clerk_session
                    && let (Some(clerk), Some(jwt)) = (clerk, preview.clerk_client_jwt)
                {
                    clerk.adopt_client_jwt(jwt).await.map_err(clerk_error)?;
                }
                Ok(added)
            },
            cx,
        )
    }

    fn clerk(&self) -> Option<Arc<ClerkSession>> {
        self.cloud.as_ref().map(|cloud| cloud.clerk.clone())
    }
}

fn electron_key(source: &ElectronImportSource) -> Result<SafeStorageKey, ImportError> {
    match &source.key {
        Some(key) => Ok(key.clone()),
        None => load_platform_key(&source.paths),
    }
}

fn no_cloud() -> CommandError {
    CommandError("T3 Connect is not configured in this app.".into())
}

fn connection_error(error: ConnectionError) -> CommandError {
    CommandError(error.detail.into())
}

fn registry_error(error: RegistryError) -> CommandError {
    CommandError(error.to_string().into())
}

fn clerk_error(error: ClerkError) -> CommandError {
    CommandError(error.to_string().into())
}

/// Applies bridge events to the state entity on the GPUI thread.
fn apply_events(state: Entity<ConnectionsState>, mut receiver: mpsc::UnboundedReceiver<BridgeEvent>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(event) = receiver.next().await {
            state.update(cx, |state, cx| {
                apply(state, event);
                // Drain what else arrived so a burst costs one render.
                while let Ok(event) = receiver.try_recv() {
                    apply(state, event);
                }
                cx.notify();
            });
        }
    })
    .detach();
}

fn apply(state: &mut ConnectionsState, event: BridgeEvent) {
    match event {
        BridgeEvent::Entries(entries) => {
            let mut previous = std::mem::take(&mut state.environments);
            state.environments = entries
                .iter()
                .map(|entry| {
                    let old = previous
                        .iter()
                        .position(|env| env.entry.environment_id == entry.environment_id)
                        .map(|ix| previous.swap_remove(ix));
                    match old {
                        Some(old) if entry.enabled => EnvironmentState { entry: entry.clone(), ..old },
                        _ => EnvironmentState { entry: entry.clone(), status: None, latency: None, server_version: None },
                    }
                })
                .collect();
        }
        BridgeEvent::Status { id, status, server_version } => {
            if let Some(env) = state.environments.iter_mut().find(|env| *env.id() == id && env.entry.enabled) {
                if status.phase != ConnectionPhase::Connected {
                    env.latency = None;
                }
                if server_version.is_some() {
                    env.server_version = server_version;
                }
                env.status = Some(status);
            }
        }
        BridgeEvent::Latency { id, latency } => {
            if let Some(env) = state.environments.iter_mut().find(|env| *env.id() == id) {
                env.latency = latency;
            }
        }
        BridgeEvent::Account(account) => {
            if let Some(cloud) = &mut state.cloud {
                if !matches!(account, CloudAccountState::SignedIn(_)) {
                    cloud.discovery = RelayDiscovery::default();
                }
                cloud.account = account;
            }
        }
        BridgeEvent::Discovery(discovery) => {
            if let Some(cloud) = &mut state.cloud {
                cloud.discovery = discovery;
            }
        }
    }
}

/// Follows the catalog and starts a status watcher for each new supervisor.
async fn watch_registry(registry: ConnectionRegistry, events: mpsc::UnboundedSender<BridgeEvent>) {
    let mut entries = registry.entries();
    let mut watched: Vec<(EnvironmentId, SupervisorHandle)> = Vec::new();
    loop {
        let current = entries.borrow_and_update().clone();
        if events.unbounded_send(BridgeEvent::Entries(current.clone())).is_err() {
            return;
        }
        watched.retain(|(id, handle)| {
            registry.supervisor(id).is_some_and(|live| live.status.same_channel(&handle.status))
        });
        for entry in current.iter().filter(|entry| entry.enabled) {
            if watched.iter().any(|(id, _)| *id == entry.environment_id) {
                continue;
            }
            let Some(handle) = registry.supervisor(&entry.environment_id) else { continue };
            tokio::spawn(watch_supervisor(entry.environment_id.clone(), handle.clone(), events.clone()));
            watched.push((entry.environment_id.clone(), handle));
        }
        if entries.changed().await.is_err() {
            return;
        }
    }
}

/// Forwards one supervisor's status and measures latency while connected.
/// Ends when the supervisor is dropped (switched off or removed).
async fn watch_supervisor(id: EnvironmentId, handle: SupervisorHandle, events: mpsc::UnboundedSender<BridgeEvent>) {
    let mut status = handle.status.clone();
    let mut probe = tokio::time::interval(LATENCY_PROBE_INTERVAL);
    probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut probed_generation = 0;
    let mut logged_phase = None;
    loop {
        let current = status.borrow_and_update().clone();
        let connected = current.phase == ConnectionPhase::Connected;
        if runtime_metrics_enabled() && logged_phase != Some(current.phase) {
            logged_phase = Some(current.phase);
            let detail = current.last_failure.as_ref().map(|f| f.detail.as_str()).unwrap_or("");
            eprintln!("runtime-phase t={} env={id} phase={:?} generation={} {detail}", epoch_ms(), current.phase, current.generation);
        }
        let session = connected.then(|| handle.session.borrow().clone()).flatten();
        let server_version = session
            .as_ref()
            .map(|session| SharedString::from(session.prepared.descriptor.server_version.clone()))
            .filter(|version| !version.is_empty());
        let generation = current.generation;
        if events.unbounded_send(BridgeEvent::Status { id: id.clone(), status: current, server_version }).is_err() {
            return;
        }
        if connected && generation != probed_generation {
            probed_generation = generation;
            probe.reset_immediately();
        }
        tokio::select! {
            changed = status.changed() => if changed.is_err() { return },
            _ = probe.tick(), if connected => {
                let session = handle.session.borrow().clone();
                if let Some(session) = session {
                    let (events, id) = (events.clone(), id.clone());
                    tokio::spawn(async move {
                        let started = Instant::now();
                        let call = session.client.call::<ServerProbe>(t3code_contracts::wire::Empty);
                        let answered = matches!(tokio::time::timeout(Duration::from_secs(10), call).await, Ok(Ok(_)));
                        let latency = answered.then(|| started.elapsed());
                        _ = events.unbounded_send(BridgeEvent::Latency { id, latency });
                    });
                }
            }
        }
    }
}

/// Lists the account's environments and checks each one's tunnel. A
/// failure keeps `previous` on screen with the error.
async fn discover_relay(
    clerk: Arc<ClerkSession>,
    relay: Arc<RelayClient>,
    events: mpsc::UnboundedSender<BridgeEvent>,
    previous: Vec<RelayEnvironment>,
) {
    let send = |discovery| _ = events.unbounded_send(BridgeEvent::Discovery(discovery));
    send(RelayDiscovery { refreshing: true, error: None, environments: previous.clone() });
    let jwt = match clerk.template_token().await {
        Ok(Some(jwt)) => jwt,
        Ok(None) => return send(RelayDiscovery::default()),
        Err(error) => {
            return send(RelayDiscovery { refreshing: false, error: Some(error.to_string().into()), environments: previous });
        }
    };
    let records = match relay.list_environments(&jwt).await {
        Ok(records) => records,
        Err(error) => {
            return send(RelayDiscovery { refreshing: false, error: Some(error.to_string().into()), environments: previous });
        }
    };
    let checking = records
        .iter()
        .map(|record| RelayEnvironment { record: record.clone(), availability: RelayAvailability::Checking })
        .collect();
    send(RelayDiscovery { refreshing: false, error: None, environments: checking });
    let statuses =
        futures::future::join_all(records.iter().map(|record| relay.environment_status(&jwt, &record.environment_id))).await;
    let environments = records
        .into_iter()
        .zip(statuses)
        .map(|(record, status)| RelayEnvironment {
            record,
            availability: match status {
                Ok(status) if status.status == "online" => RelayAvailability::Online,
                Ok(_) => RelayAvailability::Offline,
                Err(_) => RelayAvailability::Error,
            },
        })
        .collect();
    send(RelayDiscovery { refreshing: false, error: None, environments });
}

/// `T3CODE_RUNTIME_METRICS=1` prints the tokio runtime's task counts every
/// 30 s and each environment's phase changes to stderr, for soak runs.
fn runtime_metrics_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("T3CODE_RUNTIME_METRICS").is_ok_and(|v| v == "1"))
}

fn epoch_ms() -> u128 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

async fn log_runtime_metrics(handle: tokio::runtime::Handle) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let metrics = handle.metrics();
        eprintln!(
            "runtime-metrics t={} alive_tasks={} workers={} global_queue={}",
            epoch_ms(),
            metrics.num_alive_tasks(),
            metrics.num_workers(),
            metrics.global_queue_depth()
        );
    }
}

/// Loads the stored T3 Connect sign-in. While Clerk cannot be reached the
/// account stays `Loading` (relay routes wait, nothing is cleaned up) and the
/// load is retried with backoff, as clerk-js keeps retrying `GET /client`.
async fn restore_account(clerk: Arc<ClerkSession>) {
    let mut delay = Duration::from_secs(1);
    loop {
        match clerk.restore().await {
            Ok(()) => return,
            Err(ClerkError::Store(error)) => {
                tracing::warn!(%error, "could not read the stored T3 Connect sign-in");
                return;
            }
            Err(error) => tracing::warn!(%error, retry_in = ?delay, "could not reach T3 Connect sign-in"),
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(300));
    }
}

/// Mirrors the account into the UI and, like the web's cloud account
/// service, cleans up when Clerk confirms a sign-out or the account changes:
/// T3 Connect routes are removed and relay tokens forgotten, so nothing
/// keeps connecting as the previous account. Each sign-in lists the
/// account's environments.
async fn watch_account(
    clerk: Arc<ClerkSession>,
    relay: Arc<RelayClient>,
    registry: ConnectionRegistry,
    auth: Arc<RemoteAuthorization>,
    events: mpsc::UnboundedSender<BridgeEvent>,
) {
    let mut account = clerk.account();
    let mut previous: Option<CloudAccountState> = None;
    loop {
        let current = account.borrow_and_update().clone();
        if events.unbounded_send(BridgeEvent::Account(current.clone())).is_err() {
            return;
        }
        let user = |state: &CloudAccountState| match state {
            CloudAccountState::SignedIn(account) => Some(account.user_id.clone()),
            _ => None,
        };
        // Every confirmed signed-out notice cleans up (removal is idempotent): routes imported
        // after an earlier notice must not wait on a sign-in that is not coming.
        let changed_account = match (&previous, &current) {
            (_, CloudAccountState::SignedOut) => true,
            (Some(before @ CloudAccountState::SignedIn(_)), CloudAccountState::SignedIn(_)) => user(before) != user(&current),
            _ => false,
        };
        if changed_account {
            auth.reset_relay_tokens().await;
            if let Err(error) = registry.remove_relay_environments().await {
                tracing::warn!(%error, "could not remove the previous account's T3 Connect routes");
            }
        }
        let signed_in_now = user(&current).is_some() && previous.as_ref().and_then(user) != user(&current);
        if signed_in_now {
            tokio::spawn(discover_relay(clerk.clone(), relay.clone(), events.clone(), Vec::new()));
        }
        if !matches!(current, CloudAccountState::Loading) {
            previous = Some(current);
        }
        if account.changed().await.is_err() {
            return;
        }
    }
}

/// GPUI exposes no sleep/wake or network events, so one tick does both. A
/// tick that lands far later on the wall clock than it should is taken as a
/// wake: supervisors retry at once. Each tick also asks the OS for a route to
/// the internet (a UDP `connect`, which sends nothing): losing it reports
/// offline, like the browser's `offline` event, and getting it back or a new
/// local address reports a network change that wakes pending retries.
async fn detect_wake(registry: ConnectionRegistry) {
    let mut tick = tokio::time::interval(WAKE_CHECK_INTERVAL);
    let mut last = SystemTime::now();
    let mut network: Option<Option<std::net::IpAddr>> = None;
    loop {
        tick.tick().await;
        let now = SystemTime::now();
        if now.duration_since(last).unwrap_or_default() > WAKE_GAP {
            tracing::info!("woke from sleep; waking supervisors");
            t3code_client_runtime::rpc::http::reset_http_pool();
            if runtime_metrics_enabled() {
                eprintln!("runtime-wake t={} gap_ms={}", epoch_ms(), now.duration_since(last).unwrap_or_default().as_millis());
            }
            registry.broadcast(SupervisorSignal::Wakeup);
        }
        last = now;
        let route = tokio::task::spawn_blocking(local_route_address).await.unwrap_or(None);
        if network != Some(route) {
            let first = network.is_none();
            network = Some(route);
            let status = if route.is_some() { NetworkStatus::Online } else { NetworkStatus::Offline };
            if first && route.is_some() {
                continue;
            }
            tracing::info!(?status, "network changed");
            t3code_client_runtime::rpc::http::reset_http_pool();
            registry.broadcast(SupervisorSignal::NetworkChanged(status));
            if route.is_some() {
                registry.broadcast(SupervisorSignal::Wakeup);
            }
        }
    }
}

/// The local address the OS would use to reach the internet, or `None` with
/// no route (Wi-Fi off, cable out).
fn local_route_address() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("1.1.1.1", 53)).ok()?;
    Some(socket.local_addr().ok()?.ip())
}
