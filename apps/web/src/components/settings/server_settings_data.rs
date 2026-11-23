//! The data the server-side settings panels read and the actions they run,
//! for one environment. [`ServerSettingsSource`] is the seam: the live
//! implementation (from the settings store and sync layers) reads the
//! environment's shell and config subscriptions and sends each method as the
//! RPC named on it; [`FixtureServerSettings`] answers from memory for scenes
//! and scripted checks.
//!
//! [`ServerPanelsModel`] is the GPUI entity the panels hold. It caches the
//! source's snapshot, runs an action, then re-reads the snapshot.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use futures::FutureExt as _;
use futures::future::BoxFuture;
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString};
use t3code_contracts::{
    AcpRegistryAcceptUrlAuthInput, AcpRegistryAcceptUrlAuthResult, AcpRegistryManagedBinaryUninstallInput,
    AcpRegistryManagedBinaryUninstallResult, AcpRegistryPrepareInput, AcpRegistryPrepareResult, AcpRegistrySearchInput,
    AcpRegistrySearchResult, DatabaseStorageStatus, DeviceConfigureInput, DeviceHostSummary, FilesystemBrowseInput,
    FilesystemBrowseResult, HostResourcesSnapshot, IssueTrackerBindProjectInput, IssueTrackerConnectInput,
    IssueTrackerConnection, IssueTrackerConnectionList, ListTextGeneratorModelsInput, ListTextGeneratorModelsResult,
    ManagedPolicySummary, OtlpEndpointCheckInput, OtlpEndpointCheckResult, Project, ProjectCloneStartInput,
    ProjectCloneStartResult, ProjectMutation, ProviderAuthCancelInput, ProviderAuthCompleteInput,
    ProviderAuthRespondInput, ProviderAuthStartInput, ProviderAuthState, ProviderAuthStatePhase, ProviderInstallState,
    ProviderInstanceMutation, ProviderSetupInput, ScheduledTask, ScheduledTaskDeleteInput,
    ScheduledTaskListWebhookDeliveriesInput, ScheduledTaskListWebhookDeliveriesResult, ScheduledTaskMutationResult,
    ScheduledTaskRotateWebhookTokenInput, ScheduledTaskRunNowInput, ScheduledTaskSetEnabledInput,
    ScheduledTaskUpsertInput, SecretSourceCheckResult, ServerCheckSecretSourcePayload, ServerProcessDiagnosticsResult,
    ServerProviderUpdateInput, ServerRefreshProvidersPayload, ServerSettings, ServerTraceDiagnosticsResult,
    ServerUpdateSettingsPayload, SourceControlDiscoveryResult, SourceControlSignInInput, SourceControlSignInPrompt,
    SourceControlSignInResult, SshDeviceHostConfig, StorageCleanupReport,
};

use crate::provider_instances::ServerProvider;

/// Why an action failed, as the panel shows it (the RPC's typed error, rendered).
#[derive(Clone, Debug, PartialEq)]
pub struct RpcFailure {
    pub message: SharedString,
}

impl RpcFailure {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self { message: message.into() }
    }
}

/// The reply to one RPC.
pub type Reply<T> = BoxFuture<'static, Result<T, RpcFailure>>;

/// Everything the server panels read from one environment. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct ServerPanelsSnapshot {
    pub environment_id: SharedString,
    pub environment_label: SharedString,
    /// The session may read but not change settings (`settings:write` missing).
    pub read_only: bool,
    /// `ServerConfig.settings`, kept current by `subscribeServerConfig`.
    pub settings: Arc<ServerSettings>,
    /// `ServerConfig.providers`, kept current by `subscribeServerConfig`.
    pub providers: Arc<[Arc<ServerProvider>]>,
    /// `ServerConfig.managedPolicy`.
    pub managed_policy: Option<Arc<ManagedPolicySummary>>,
    /// `ServerConfig.environment.capabilities.projectProviderInstanceOverrides`.
    pub supports_project_provider_overrides: bool,
    /// Projects from the shell snapshot (`orchestration.subscribeShell`).
    pub projects: Arc<[Project]>,
    /// `scheduledTasks.subscribe`.
    pub scheduled_tasks: Arc<[ScheduledTask]>,
    /// The server's zone for fixed-time schedules (`ScheduledTaskListResult.timeZone`).
    pub scheduled_task_time_zone: Option<SharedString>,
    /// The latest `provider.auth.subscribe` value per instance.
    pub provider_auth: Arc<HashMap<String, ProviderAuthState>>,
    /// The latest `provider.install.subscribe` value per instance.
    pub provider_install: Arc<HashMap<String, ProviderInstallState>>,
    /// `server.discoverSourceControl`, once loaded.
    pub source_control: Option<Arc<SourceControlDiscoveryResult>>,
    /// `issueTrackers.list`, once loaded.
    pub issue_trackers: Option<Arc<IssueTrackerConnectionList>>,
    /// The latest `server.getStorageCleanupReport` value.
    pub storage_report: Option<Arc<StorageCleanupReport>>,
    /// The latest `server.getDatabaseStorage` value.
    pub database_storage: Option<Arc<DatabaseStorageStatus>>,
    /// `server.getTraceDiagnostics`, once loaded.
    pub trace_diagnostics: Option<Arc<ServerTraceDiagnosticsResult>>,
    /// `server.getProcessDiagnostics`, once loaded.
    pub process_diagnostics: Option<Arc<ServerProcessDiagnosticsResult>>,
    /// `server.getHostResources`, once loaded.
    pub host_resources: Option<Arc<HostResourcesSnapshot>>,
}

/// One environment's server-side settings. Each method is one RPC.
pub trait ServerSettingsSource: Send + Sync + 'static {
    /// The current values (`server.getConfig` + `subscribeServerConfig`, the shell
    /// subscription, `scheduledTasks.subscribe`, `provider.auth.subscribe`,
    /// `provider.install.subscribe`, and the latest replies of the loaders below).
    fn snapshot(&self) -> ServerPanelsSnapshot;

    /// `server.updateSettings`: a settings patch, optionally with one provider instance mutation.
    fn update_settings(&self, input: ServerUpdateSettingsPayload) -> Reply<ServerSettings>;
    /// `server.refreshProviders`.
    fn refresh_providers(&self, input: ServerRefreshProvidersPayload) -> Reply<Vec<Arc<ServerProvider>>>;
    /// `server.updateProvider`: run the provider's update command.
    fn update_provider(&self, input: ServerProviderUpdateInput) -> Reply<Vec<Arc<ServerProvider>>>;
    /// `server.listTextGeneratorModels`.
    fn list_text_generator_models(&self, input: ListTextGeneratorModelsInput) -> Reply<ListTextGeneratorModelsResult>;

    /// `provider.auth.start`.
    fn start_provider_auth(&self, input: ProviderAuthStartInput) -> Reply<ProviderAuthState>;
    /// `provider.auth.respond`: browser consent, terminal input or credentials.
    fn respond_provider_auth(&self, input: ProviderAuthRespondInput) -> Reply<ProviderAuthState>;
    /// `provider.auth.complete`: a pasted or delivered callback URL.
    fn complete_provider_auth(&self, input: ProviderAuthCompleteInput) -> Reply<ProviderAuthState>;
    /// `provider.auth.cancel`.
    fn cancel_provider_auth(&self, input: ProviderAuthCancelInput) -> Reply<ProviderAuthState>;
    /// `provider.auth.logout`.
    fn logout_provider_auth(&self, input: ProviderSetupInput) -> Reply<ProviderAuthState>;
    /// `provider.install.start`.
    fn start_provider_install(&self, input: ProviderSetupInput) -> Reply<ProviderInstallState>;
    /// `server.acceptAcpRegistryUrlAuth`.
    fn accept_acp_registry_url_auth(&self, input: AcpRegistryAcceptUrlAuthInput) -> Reply<AcpRegistryAcceptUrlAuthResult>;
    /// `server.searchAcpRegistry`.
    fn search_acp_registry(&self, input: AcpRegistrySearchInput) -> Reply<AcpRegistrySearchResult>;
    /// `server.prepareAcpRegistryAgent`.
    fn prepare_acp_registry_agent(&self, input: AcpRegistryPrepareInput) -> Reply<AcpRegistryPrepareResult>;
    /// `server.uninstallAcpRegistryManagedBinary`.
    fn uninstall_acp_registry_managed_binary(
        &self,
        input: AcpRegistryManagedBinaryUninstallInput,
    ) -> Reply<AcpRegistryManagedBinaryUninstallResult>;

    /// `server.discoverSourceControl`.
    fn discover_source_control(&self) -> Reply<SourceControlDiscoveryResult>;
    /// `sourceControl.signInStart`: a device code to show and a page to open.
    fn source_control_sign_in_start(&self, input: SourceControlSignInInput) -> Reply<SourceControlSignInPrompt>;
    /// `sourceControl.signInWait`: resolves when the browser step finishes.
    fn source_control_sign_in_wait(&self, input: SourceControlSignInInput) -> Reply<SourceControlSignInResult>;
    /// `sourceControl.signInCancel`.
    fn source_control_sign_in_cancel(&self, input: SourceControlSignInInput) -> Reply<()>;
    /// `issueTrackers.list`.
    fn list_issue_trackers(&self) -> Reply<IssueTrackerConnectionList>;
    /// `issueTrackers.connect`.
    fn connect_issue_tracker(&self, input: IssueTrackerConnectInput) -> Reply<IssueTrackerConnection>;
    /// `issueTrackers.bindProject`.
    fn bind_issue_tracker_project(&self, input: IssueTrackerBindProjectInput) -> Reply<()>;

    /// `scheduledTasks.upsert`.
    fn upsert_scheduled_task(&self, input: ScheduledTaskUpsertInput) -> Reply<ScheduledTaskMutationResult>;
    /// `scheduledTasks.setEnabled`.
    fn set_scheduled_task_enabled(&self, input: ScheduledTaskSetEnabledInput) -> Reply<ScheduledTaskMutationResult>;
    /// `scheduledTasks.delete`.
    fn delete_scheduled_task(&self, input: ScheduledTaskDeleteInput) -> Reply<()>;
    /// `scheduledTasks.runNow`.
    fn run_scheduled_task_now(&self, input: ScheduledTaskRunNowInput) -> Reply<ScheduledTask>;
    /// `scheduledTasks.rotateWebhookToken`.
    fn rotate_webhook_token(&self, input: ScheduledTaskRotateWebhookTokenInput) -> Reply<ScheduledTaskMutationResult>;
    /// `scheduledTasks.listWebhookDeliveries`.
    fn list_webhook_deliveries(
        &self,
        input: ScheduledTaskListWebhookDeliveriesInput,
    ) -> Reply<ScheduledTaskListWebhookDeliveriesResult>;

    /// `projects.mutate`: rename, icon, scripts, pin or archive.
    fn mutate_project(&self, input: ProjectMutation) -> Reply<Project>;
    /// `filesystem.browse`: folders on the host, for adding a project.
    fn browse_filesystem(&self, input: FilesystemBrowseInput) -> Reply<FilesystemBrowseResult>;
    /// `projectClone.start`.
    fn start_project_clone(&self, input: ProjectCloneStartInput) -> Reply<ProjectCloneStartResult>;

    /// `device.configure`.
    fn configure_devices(&self, input: DeviceConfigureInput) -> Reply<()>;
    /// `device.testHost`.
    fn test_device_host(&self, input: SshDeviceHostConfig) -> Reply<DeviceHostSummary>;
    /// `server.checkSecretSource`.
    fn check_secret_source(&self, input: ServerCheckSecretSourcePayload) -> Reply<SecretSourceCheckResult>;

    /// `server.runStorageCleanup`.
    fn run_storage_cleanup(&self) -> Reply<StorageCleanupReport>;
    /// `server.reclaimDatabaseSpace`.
    fn reclaim_database_space(&self) -> Reply<()>;
    /// `server.getTraceDiagnostics`.
    fn trace_diagnostics(&self) -> Reply<ServerTraceDiagnosticsResult>;
    /// `server.getProcessDiagnostics`.
    fn process_diagnostics(&self) -> Reply<ServerProcessDiagnosticsResult>;
    /// `server.getHostResources`.
    fn host_resources(&self) -> Reply<HostResourcesSnapshot>;
    /// `server.checkOtlpEndpoint`.
    fn check_otlp_endpoint(&self, input: OtlpEndpointCheckInput) -> Reply<OtlpEndpointCheckResult>;
}

/// The entity the panels hold: the source and its latest snapshot.
pub struct ServerPanelsModel {
    source: Arc<dyn ServerSettingsSource>,
    snapshot: ServerPanelsSnapshot,
    /// Actions in flight, by key, so a control can show it is busy and not double-send.
    pending: HashMap<SharedString, ()>,
    /// What the last failed action said, by key.
    errors: HashMap<SharedString, SharedString>,
}

impl ServerPanelsModel {
    pub fn new(source: Arc<dyn ServerSettingsSource>) -> Self {
        let snapshot = source.snapshot();
        Self { source, snapshot, pending: HashMap::new(), errors: HashMap::new() }
    }

    pub fn snapshot(&self) -> &ServerPanelsSnapshot {
        &self.snapshot
    }

    pub fn source(&self) -> &Arc<dyn ServerSettingsSource> {
        &self.source
    }

    pub fn is_pending(&self, key: &str) -> bool {
        self.pending.contains_key(key)
    }

    pub fn error(&self, key: &str) -> Option<&SharedString> {
        self.errors.get(key)
    }

    /// Re-reads the source; the live store calls this when a subscription pushes.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.snapshot = self.source.snapshot();
        cx.notify();
    }

    /// Runs one action under `key`, then refreshes and hands the result to `done`.
    /// A second call with the same key while one is in flight is dropped.
    pub fn run<T: Send + 'static>(
        &mut self,
        key: impl Into<SharedString>,
        call: impl FnOnce(&dyn ServerSettingsSource) -> Reply<T>,
        done: impl FnOnce(Result<T, RpcFailure>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        if self.pending.contains_key(&key) {
            return;
        }
        let reply = call(self.source.as_ref());
        self.pending.insert(key.clone(), ());
        self.errors.remove(&key);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = reply.await;
            let failure = result.as_ref().err().map(|e| e.message.clone());
            this.update(cx, |this, cx| {
                this.pending.remove(&key);
                if let Some(message) = failure {
                    this.errors.insert(key, message);
                }
                this.refresh(cx);
            })
            .ok();
            cx.update(|cx| done(result, cx));
        })
        .detach();
    }
}

/// Wraps a source in a model entity.
pub fn panels_model(source: Arc<dyn ServerSettingsSource>, cx: &mut App) -> Entity<ServerPanelsModel> {
    cx.new(|_| ServerPanelsModel::new(source))
}

fn ready<T: Send + 'static>(value: Result<T, RpcFailure>) -> Reply<T> {
    futures::future::ready(value).boxed()
}

/// Merges `patch` into `target` the way the server's deep patch does for
/// plain objects: objects merge key by key, anything else replaces, `null` removes.
pub fn merge_json(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(target), serde_json::Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    target.remove(key);
                } else if let Some(existing) = target.get_mut(key) {
                    merge_json(existing, value);
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

/// A source answered from memory. Writes change the stored snapshot so a
/// scripted check sees the panel react; nothing leaves the process.
pub struct FixtureServerSettings {
    state: Mutex<ServerPanelsSnapshot>,
    /// What each auth start, respond or complete returns next, by instance.
    pub auth_script: Mutex<HashMap<String, Vec<ProviderAuthState>>>,
    /// Every call, in order, as `method` strings, for scripted checks.
    pub calls: Mutex<Vec<String>>,
}

impl FixtureServerSettings {
    pub fn new(snapshot: ServerPanelsSnapshot) -> Self {
        Self { state: Mutex::new(snapshot), auth_script: Mutex::new(HashMap::new()), calls: Mutex::new(Vec::new()) }
    }

    fn log(&self, call: impl Into<String>) {
        self.calls.lock().expect("calls").push(call.into());
    }

    fn with<T>(&self, f: impl FnOnce(&mut ServerPanelsSnapshot) -> T) -> T {
        f(&mut self.state.lock().expect("fixture state"))
    }

    fn next_auth(&self, instance: &str, fallback: ProviderAuthState) -> ProviderAuthState {
        let next = self.auth_script.lock().expect("auth script").get_mut(instance).and_then(|queue| {
            (!queue.is_empty()).then(|| queue.remove(0))
        });
        let state = next.unwrap_or(fallback);
        self.with(|s| {
            let mut auth = (*s.provider_auth).clone();
            auth.insert(instance.to_owned(), state.clone());
            s.provider_auth = Arc::new(auth);
        });
        state
    }

    fn idle_auth(instance: &str, phase: ProviderAuthStatePhase) -> ProviderAuthState {
        ProviderAuthState {
            instance_id: t3code_contracts::ProviderInstanceId::new(instance),
            phase,
            flow_id: None,
            authorization_url: None,
            expires_at: None,
            message: None,
            methods: None,
            interaction: None,
            credential_owner: None,
        }
    }
}

fn not_in_fixture<T: Send + 'static>(what: &str) -> Reply<T> {
    ready(Err(RpcFailure::new(format!("{what} is not available in this fixture."))))
}

impl ServerSettingsSource for FixtureServerSettings {
    fn snapshot(&self) -> ServerPanelsSnapshot {
        self.with(|s| s.clone())
    }

    fn update_settings(&self, input: ServerUpdateSettingsPayload) -> Reply<ServerSettings> {
        self.log("server.updateSettings");
        let result = self.with(|s| {
            let mut settings = serde_json::to_value(&*s.settings).expect("settings encode");
            let patch = serde_json::to_value(&input.patch).expect("patch encode");
            merge_json(&mut settings, &patch);
            if let Some(mutation) = &input.provider_instance_mutation {
                let instances = settings
                    .as_object_mut()
                    .expect("settings object")
                    .entry("providerInstances")
                    .or_insert_with(|| serde_json::json!({}));
                match mutation {
                    ProviderInstanceMutation::Create(m) => {
                        instances[m.instance_id.as_str()] = serde_json::to_value(&m.instance).expect("instance");
                    }
                    ProviderInstanceMutation::Upsert(m) => {
                        instances[m.instance_id.as_str()] = serde_json::to_value(&m.instance).expect("instance");
                    }
                    ProviderInstanceMutation::Remove(m) => {
                        if let Some(map) = instances.as_object_mut() {
                            map.remove(m.instance_id.as_str());
                        }
                    }
                    ProviderInstanceMutation::Unknown(_) => {}
                }
            }
            let next: ServerSettings = serde_json::from_value(settings).map_err(|e| RpcFailure::new(e.to_string()))?;
            // Mirror the instance's enabled flag onto its snapshot, as the server's reconcile does.
            let providers: Vec<Arc<ServerProvider>> = s
                .providers
                .iter()
                .map(|p| {
                    let instance = next.provider_instances.as_ref().and_then(|i| i.get(&p.instance_id));
                    let enabled = match instance {
                        Some(instance) => crate::provider_instances::instance_enabled(instance),
                        None => p.enabled,
                    };
                    let mut p = (**p).clone();
                    p.enabled = enabled;
                    if let Some(name) = instance.and_then(|i| i.display_name.clone()) {
                        p.display_name = Some(name);
                    }
                    Arc::new(p)
                })
                .collect();
            s.providers = providers.into();
            s.settings = Arc::new(next.clone());
            Ok(next)
        });
        ready(result)
    }

    fn refresh_providers(&self, _input: ServerRefreshProvidersPayload) -> Reply<Vec<Arc<ServerProvider>>> {
        self.log("server.refreshProviders");
        ready(Ok(self.with(|s| s.providers.to_vec())))
    }

    fn update_provider(&self, input: ServerProviderUpdateInput) -> Reply<Vec<Arc<ServerProvider>>> {
        self.log(format!("server.updateProvider {}", input.provider));
        ready(Ok(self.with(|s| s.providers.to_vec())))
    }

    fn list_text_generator_models(&self, _input: ListTextGeneratorModelsInput) -> Reply<ListTextGeneratorModelsResult> {
        self.log("server.listTextGeneratorModels");
        ready(Ok(ListTextGeneratorModelsResult { models: Vec::new(), tested_model: None }))
    }

    fn start_provider_auth(&self, input: ProviderAuthStartInput) -> Reply<ProviderAuthState> {
        self.log(format!("provider.auth.start {}", input.instance_id));
        let id = input.instance_id.to_string();
        ready(Ok(self.next_auth(&id, Self::idle_auth(&id, ProviderAuthStatePhase::Starting))))
    }

    fn respond_provider_auth(&self, input: ProviderAuthRespondInput) -> Reply<ProviderAuthState> {
        self.log(format!("provider.auth.respond {}", input.instance_id));
        let id = input.instance_id.to_string();
        ready(Ok(self.next_auth(&id, Self::idle_auth(&id, ProviderAuthStatePhase::Verifying))))
    }

    fn complete_provider_auth(&self, input: ProviderAuthCompleteInput) -> Reply<ProviderAuthState> {
        self.log(format!("provider.auth.complete {}", input.instance_id));
        let id = input.instance_id.to_string();
        ready(Ok(self.next_auth(&id, Self::idle_auth(&id, ProviderAuthStatePhase::Succeeded))))
    }

    fn cancel_provider_auth(&self, input: ProviderAuthCancelInput) -> Reply<ProviderAuthState> {
        self.log(format!("provider.auth.cancel {}", input.instance_id));
        let id = input.instance_id.to_string();
        ready(Ok(self.next_auth(&id, Self::idle_auth(&id, ProviderAuthStatePhase::Cancelled))))
    }

    fn logout_provider_auth(&self, input: ProviderSetupInput) -> Reply<ProviderAuthState> {
        self.log(format!("provider.auth.logout {}", input.instance_id));
        let id = input.instance_id.to_string();
        ready(Ok(self.next_auth(&id, Self::idle_auth(&id, ProviderAuthStatePhase::Idle))))
    }

    fn start_provider_install(&self, input: ProviderSetupInput) -> Reply<ProviderInstallState> {
        self.log(format!("provider.install.start {}", input.instance_id));
        not_in_fixture("Installing providers")
    }

    fn accept_acp_registry_url_auth(&self, _input: AcpRegistryAcceptUrlAuthInput) -> Reply<AcpRegistryAcceptUrlAuthResult> {
        self.log("server.acceptAcpRegistryUrlAuth");
        ready(Ok(AcpRegistryAcceptUrlAuthResult { accepted: true }))
    }

    fn search_acp_registry(&self, _input: AcpRegistrySearchInput) -> Reply<AcpRegistrySearchResult> {
        self.log("server.searchAcpRegistry");
        not_in_fixture("The ACP Registry")
    }

    fn prepare_acp_registry_agent(&self, _input: AcpRegistryPrepareInput) -> Reply<AcpRegistryPrepareResult> {
        self.log("server.prepareAcpRegistryAgent");
        not_in_fixture("The ACP Registry")
    }

    fn uninstall_acp_registry_managed_binary(
        &self,
        _input: AcpRegistryManagedBinaryUninstallInput,
    ) -> Reply<AcpRegistryManagedBinaryUninstallResult> {
        self.log("server.uninstallAcpRegistryManagedBinary");
        not_in_fixture("The ACP Registry")
    }

    fn discover_source_control(&self) -> Reply<SourceControlDiscoveryResult> {
        self.log("server.discoverSourceControl");
        let result = self.with(|s| s.source_control.as_deref().cloned());
        ready(result.ok_or_else(|| RpcFailure::new("No source control in this fixture.")))
    }

    fn source_control_sign_in_start(&self, input: SourceControlSignInInput) -> Reply<SourceControlSignInPrompt> {
        self.log(format!("sourceControl.signInStart {}", input.host));
        ready(Ok(SourceControlSignInPrompt {
            user_code: "WDJB-MJHT".into(),
            verification_uri: format!("https://{}/login/device", input.host),
            expires_at: t3code_contracts::SidebarAutoSettleAfterDays::Number(900_000.),
        }))
    }

    fn source_control_sign_in_wait(&self, input: SourceControlSignInInput) -> Reply<SourceControlSignInResult> {
        self.log(format!("sourceControl.signInWait {}", input.host));
        // The fixture never finishes a device-code sign-in on its own.
        futures::future::pending().boxed()
    }

    fn source_control_sign_in_cancel(&self, input: SourceControlSignInInput) -> Reply<()> {
        self.log(format!("sourceControl.signInCancel {}", input.host));
        ready(Ok(()))
    }

    fn list_issue_trackers(&self) -> Reply<IssueTrackerConnectionList> {
        self.log("issueTrackers.list");
        let result = self.with(|s| s.issue_trackers.as_deref().cloned());
        ready(Ok(result.unwrap_or(IssueTrackerConnectionList { trackers: Vec::new(), bindings: BTreeMap::new() })))
    }

    fn connect_issue_tracker(&self, input: IssueTrackerConnectInput) -> Reply<IssueTrackerConnection> {
        self.log(format!("issueTrackers.connect {}", input.tracker.as_str()));
        not_in_fixture("Connecting issue trackers")
    }

    fn bind_issue_tracker_project(&self, _input: IssueTrackerBindProjectInput) -> Reply<()> {
        self.log("issueTrackers.bindProject");
        ready(Ok(()))
    }

    fn upsert_scheduled_task(&self, input: ScheduledTaskUpsertInput) -> Reply<ScheduledTaskMutationResult> {
        self.log(format!("scheduledTasks.upsert {}", input.title));
        not_in_fixture("Saving scheduled tasks")
    }

    fn set_scheduled_task_enabled(&self, input: ScheduledTaskSetEnabledInput) -> Reply<ScheduledTaskMutationResult> {
        self.log(format!("scheduledTasks.setEnabled {} {}", input.id, input.enabled));
        let result = self.with(|s| {
            let mut tasks = s.scheduled_tasks.to_vec();
            let task = tasks.iter_mut().find(|t| t.id == input.id).ok_or_else(|| RpcFailure::new("No such task."))?;
            task.enabled = input.enabled;
            let task = task.clone();
            s.scheduled_tasks = tasks.into();
            Ok(ScheduledTaskMutationResult { task })
        });
        ready(result)
    }

    fn delete_scheduled_task(&self, input: ScheduledTaskDeleteInput) -> Reply<()> {
        self.log(format!("scheduledTasks.delete {}", input.id));
        self.with(|s| s.scheduled_tasks = s.scheduled_tasks.iter().filter(|t| t.id != input.id).cloned().collect());
        ready(Ok(()))
    }

    fn run_scheduled_task_now(&self, input: ScheduledTaskRunNowInput) -> Reply<ScheduledTask> {
        self.log(format!("scheduledTasks.runNow {}", input.id));
        let task = self.with(|s| s.scheduled_tasks.iter().find(|t| t.id == input.id).cloned());
        ready(task.ok_or_else(|| RpcFailure::new("No such task.")))
    }

    fn rotate_webhook_token(&self, input: ScheduledTaskRotateWebhookTokenInput) -> Reply<ScheduledTaskMutationResult> {
        self.log(format!("scheduledTasks.rotateWebhookToken {}", input.id));
        not_in_fixture("Rotating webhook tokens")
    }

    fn list_webhook_deliveries(
        &self,
        input: ScheduledTaskListWebhookDeliveriesInput,
    ) -> Reply<ScheduledTaskListWebhookDeliveriesResult> {
        self.log(format!("scheduledTasks.listWebhookDeliveries {}", input.id));
        not_in_fixture("Webhook deliveries")
    }

    fn mutate_project(&self, _input: ProjectMutation) -> Reply<Project> {
        self.log("projects.mutate");
        not_in_fixture("Changing projects")
    }

    fn browse_filesystem(&self, input: FilesystemBrowseInput) -> Reply<FilesystemBrowseResult> {
        self.log(format!("filesystem.browse {}", input.partial_path));
        let parent = if input.partial_path.ends_with('/') {
            input.partial_path.clone()
        } else {
            input.partial_path.rsplit_once('/').map(|(p, _)| format!("{p}/")).unwrap_or_else(|| "~/".into())
        };
        let entries = ["Code", "Documents", "t3code", "website"]
            .iter()
            .map(|name| t3code_contracts::FilesystemBrowseEntry { name: (*name).into(), full_path: format!("{parent}{name}") })
            .collect();
        ready(Ok(FilesystemBrowseResult { parent_path: parent, entries }))
    }

    fn start_project_clone(&self, _input: ProjectCloneStartInput) -> Reply<ProjectCloneStartResult> {
        self.log("projectClone.start");
        not_in_fixture("Cloning")
    }

    fn configure_devices(&self, _input: DeviceConfigureInput) -> Reply<()> {
        self.log("device.configure");
        ready(Ok(()))
    }

    fn test_device_host(&self, input: SshDeviceHostConfig) -> Reply<DeviceHostSummary> {
        self.log(format!("device.testHost {}", input.label));
        not_in_fixture("Testing device hosts")
    }

    fn check_secret_source(&self, _input: ServerCheckSecretSourcePayload) -> Reply<SecretSourceCheckResult> {
        self.log("server.checkSecretSource");
        ready(Ok(SecretSourceCheckResult::Resolved(t3code_contracts::SecretSourceCheckResultResolved {})))
    }

    fn run_storage_cleanup(&self) -> Reply<StorageCleanupReport> {
        self.log("server.runStorageCleanup");
        let report = self.with(|s| s.storage_report.as_deref().cloned());
        ready(report.ok_or_else(|| RpcFailure::new("No cleanup report in this fixture.")))
    }

    fn reclaim_database_space(&self) -> Reply<()> {
        self.log("server.reclaimDatabaseSpace");
        ready(Ok(()))
    }

    fn trace_diagnostics(&self) -> Reply<ServerTraceDiagnosticsResult> {
        self.log("server.getTraceDiagnostics");
        let value = self.with(|s| s.trace_diagnostics.as_deref().cloned());
        ready(value.ok_or_else(|| RpcFailure::new("No traces in this fixture.")))
    }

    fn process_diagnostics(&self) -> Reply<ServerProcessDiagnosticsResult> {
        self.log("server.getProcessDiagnostics");
        let value = self.with(|s| s.process_diagnostics.as_deref().cloned());
        ready(value.ok_or_else(|| RpcFailure::new("No processes in this fixture.")))
    }

    fn host_resources(&self) -> Reply<HostResourcesSnapshot> {
        self.log("server.getHostResources");
        let value = self.with(|s| s.host_resources.as_deref().cloned());
        ready(value.ok_or_else(|| RpcFailure::new("No host resources in this fixture.")))
    }

    fn check_otlp_endpoint(&self, _input: OtlpEndpointCheckInput) -> Reply<OtlpEndpointCheckResult> {
        self.log("server.checkOtlpEndpoint");
        not_in_fixture("Checking OTLP endpoints")
    }
}
