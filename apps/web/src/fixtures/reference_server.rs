//! The reference server's settings as the owner's Mac captures saw them: its config (providers,
//! server settings, keybindings, editors), source control and issue tracker discovery, scheduled
//! tasks, storage and diagnostics. `reference_server.json` is a scrubbed dump of what the settings
//! pages read from that server over `/ws` (`scripts/parity/mac_world.py server`).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity};
use serde::Deserialize;
use futures::FutureExt as _;
use t3code_client_runtime::rpc::client::RpcError;
use t3code_contracts::{
    DatabaseStorageStatus, HostResourcesSnapshot, IssueListInput, IssueListResult, IssueTrackerConnectionList, IssuesListError,
    NonNegativeInt, PullRequestActionInput, PullRequestFilterOptions, PullRequestFilterOptionsInput, PullRequestListInput,
    PullRequestListResult, PullRequestListStatsInput, PullRequestListStatsResult, PullRequestSummary, PullRequestsListError,
    ScheduledTask, ServerConfig,
    ServerConfigStreamSnapshotEvent, ServerProcessDiagnosticsResult, ServerTraceDiagnosticsResult, SourceControlDiscoveryResult,
    StorageCleanupReport,
};

use crate::components::issue::issues_page::EnvironmentIssueSource;
use crate::components::pull_request::pull_request_fixture::FixtureSource;
use crate::components::pull_request::pull_request_source::{IssueSource, PullRequestListSource, RpcFuture, RpcStream, SourceError};
use crate::components::pull_request::pull_requests_page::EnvironmentListSource;
use crate::components::settings::server_settings_data::{FixtureServerSettings, ServerPanelsModel, ServerPanelsSnapshot};

const SERVER: &str = include_str!("reference_server.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Dump {
    config: ServerConfigStreamSnapshotEvent,
    source_control: SourceControlDiscoveryResult,
    issue_trackers: IssueTrackerConnectionList,
    scheduled_tasks: Tasks,
    storage_report: Option<StorageCleanupReport>,
    database_storage: Option<DatabaseStorageStatus>,
    trace_diagnostics: ServerTraceDiagnosticsResult,
    process_diagnostics: ServerProcessDiagnosticsResult,
    host_resources: HostResourcesSnapshot,
    pull_requests: Reply<PullRequestListResult, PullRequestsListError>,
    issues: Reply<IssueListResult, IssuesListError>,
}

/// How a listing answered.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Reply<T, E> {
    Ok(T),
    Err(E),
}

impl<T: Clone, E: std::fmt::Debug + Clone> Reply<T, E> {
    /// The value, or the message the live link shows for the failure (`live::rpc::error_message`).
    fn result(&self) -> Result<T, SourceError> {
        match self {
            Reply::Ok(value) => Ok(value.clone()),
            Reply::Err(error) => Err(SourceError(crate::live::rpc::error_message(&RpcError::Failure(error.clone())))),
        }
    }
}

/// `scheduledTasks.subscribe`'s first value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tasks {
    tasks: Vec<ScheduledTask>,
    time_zone: Option<String>,
}

fn dump() -> Dump {
    serde_json::from_str(SERVER).expect("reference_server.json decodes")
}

/// The reference server's config, as environment `env` of the reference world.
pub fn config(env: &crate::components::sidebar::model::EnvironmentId) -> ServerConfig {
    let mut config = dump().config.config;
    config.environment.environment_id = t3code_contracts::EnvironmentId::new(env.0.as_ref());
    config
}

/// What the server settings pages show for the reference server, filled the way
/// `live::sources::apply_config` and the panels' own loads fill it on a live connection.
pub fn snapshot(env: &crate::components::sidebar::model::EnvironmentId) -> ServerPanelsSnapshot {
    let dump = dump();
    let config = dump.config.config;
    let supports_overrides =
        serde_json::to_value(&config.environment.capabilities).ok().and_then(|c| c.get("projectProviderInstanceOverrides").and_then(|v| v.as_bool()));
    ServerPanelsSnapshot {
        environment_id: env.0.to_string().into(),
        environment_label: config.environment.label.clone().into(),
        read_only: false,
        settings: Arc::new(config.settings.clone()),
        providers: config.providers.iter().cloned().map(Arc::new).collect(),
        managed_policy: config.managed_policy.clone().map(Arc::new),
        supports_project_provider_overrides: supports_overrides.unwrap_or(false),
        projects: super::reference_world::projects().into(),
        scheduled_tasks: dump.scheduled_tasks.tasks.into(),
        scheduled_task_time_zone: dump.scheduled_tasks.time_zone.map(Into::into),
        provider_auth: Arc::new(HashMap::new()),
        provider_install: Arc::new(HashMap::new()),
        source_control: Some(Arc::new(dump.source_control)),
        issue_trackers: Some(Arc::new(dump.issue_trackers)),
        storage_report: dump.storage_report.map(Arc::new),
        database_storage: dump.database_storage.map(Arc::new),
        trace_diagnostics: Some(Arc::new(dump.trace_diagnostics)),
        process_diagnostics: Some(Arc::new(dump.process_diagnostics)),
        host_resources: Some(Arc::new(dump.host_resources)),
    }
}

/// A panels model over [`snapshot`], for the settings modal's server pages.
pub fn model(env: &crate::components::sidebar::model::EnvironmentId, cx: &mut App) -> Entity<ServerPanelsModel> {
    let source = Arc::new(FixtureServerSettings::new(snapshot(env)));
    cx.new(|_| ServerPanelsModel::new(source))
}

/// The reference server's pull request listing: what `pullRequests.list` answered (the server
/// has no GitHub credential); the rest of the page's calls answer from the standard fixture.
struct ReferencePullRequests {
    list: Result<PullRequestListResult, SourceError>,
    rest: FixtureSource,
}

impl PullRequestListSource for ReferencePullRequests {
    fn list(&self, _: PullRequestListInput) -> RpcFuture<PullRequestListResult> {
        let list = self.list.clone();
        async move { list }.boxed()
    }
    fn list_stats(&self, input: PullRequestListStatsInput) -> RpcFuture<PullRequestListStatsResult> {
        self.rest.list_stats(input)
    }
    fn filter_options(&self, input: PullRequestFilterOptionsInput) -> RpcFuture<PullRequestFilterOptions> {
        self.rest.filter_options(input)
    }
    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>> {
        PullRequestListSource::run_action(&self.rest, input)
    }
    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt> {
        futures::StreamExt::boxed(futures::stream::pending())
    }
}

/// The pull requests page's one environment: the reference server, under its label.
pub fn pull_request_sources(env: &crate::components::sidebar::model::EnvironmentId) -> Vec<EnvironmentListSource> {
    let dump = dump();
    let label = dump.config.config.environment.label.clone();
    let source = ReferencePullRequests { list: dump.pull_requests.result(), rest: FixtureSource::standard() };
    vec![EnvironmentListSource { environment_id: t3code_contracts::EnvironmentId::new(env.0.as_ref()), label: label.into(), source: Rc::new(source) }]
}

/// The issues page's one environment: the reference server's trackers and `issues.list` answer.
pub fn issue_sources(env: &crate::components::sidebar::model::EnvironmentId) -> Vec<EnvironmentIssueSource> {
    let dump = dump();
    let label = dump.config.config.environment.label.clone();
    let fixture = FixtureSource::standard();
    {
        let mut state = fixture.state.lock().expect("fixture state");
        state.data.trackers = dump.issue_trackers;
        if let Ok(list) = dump.issues.result() {
            state.data.issue_list = list;
        }
    }
    let source: Rc<dyn IssueSource> = Rc::new(fixture);
    vec![EnvironmentIssueSource { environment_id: t3code_contracts::EnvironmentId::new(env.0.as_ref()), label: label.into(), source }]
}
