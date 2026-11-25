//! The server settings pages, the pull request and issue pages and their right-panel tabs on
//! a live environment: [`ServerSettingsSource`], [`PullRequestListSource`],
//! [`PullRequestDetailSource`] and [`IssueSource`], each method the RPC its trait doc names
//! (the pull request patch is the one HTTP route, `POST /api/pull-requests/diff`).

use std::sync::{Arc, Mutex};

use futures::{FutureExt as _, StreamExt as _};
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use gpui_kit::SharedString;
use t3code_contracts::rpc::methods::*;
use t3code_contracts::*;

use super::rpc::RpcLink;
use crate::components::pull_request::pull_request_source::{
    IssueSource, PullRequestDetailSource, PullRequestListSource, RpcFuture, RpcStream, SourceError,
};
use crate::provider_instances::ServerProvider;
use crate::components::settings::server_settings_data::{Reply, RpcFailure, ServerPanelsSnapshot, ServerSettingsSource};

fn reply<T: Send + 'static>(future: BoxFuture<'static, Result<T, SharedString>>) -> Reply<T> {
    future.map(|result| result.map_err(RpcFailure::new)).boxed()
}

fn unary<T: Send + 'static>(future: BoxFuture<'static, Result<T, SharedString>>) -> RpcFuture<T> {
    future.map(|result| result.map_err(SourceError)).boxed()
}

fn stream_of<T: Send + 'static>(stream: BoxStream<'static, Result<T, SharedString>>) -> RpcStream<T> {
    stream.map(|item| item.map_err(SourceError)).boxed()
}

/// One environment's server settings: the latest snapshot the window keeps current, and the
/// RPCs the panels send.
pub struct LiveServerSettings {
    pub link: RpcLink,
    pub snapshot: Mutex<ServerPanelsSnapshot>,
}

impl ServerSettingsSource for LiveServerSettings {
    fn snapshot(&self) -> ServerPanelsSnapshot {
        self.snapshot.lock().expect("snapshot").clone()
    }
    fn git_source(&self) -> Arc<dyn crate::components::git_actions_control_source::GitActionsSource> {
        Arc::new(super::git::LiveGitSource { link: self.link.clone() })
    }
    fn uninstall_acp_registry_managed_binary(
        &self,
        input: AcpRegistryManagedBinaryUninstallInput,
    ) -> Reply<AcpRegistryManagedBinaryUninstallResult> {
        reply(self.link.call::<ServerUninstallAcpRegistryManagedBinary>(input))
    }
    fn list_webhook_deliveries(&self, input: ScheduledTaskListWebhookDeliveriesInput) -> Reply<ScheduledTaskListWebhookDeliveriesResult> {
        reply(self.link.call::<ScheduledTasksListWebhookDeliveries>(input))
    }

    fn update_settings(&self, input: ServerUpdateSettingsPayload) -> Reply<ServerSettings> {
        reply(self.link.call::<ServerUpdateSettings>(input))
    }
    fn refresh_providers(&self, input: ServerRefreshProvidersPayload) -> Reply<Vec<Arc<ServerProvider>>> {
        reply(self.link.call::<ServerRefreshProviders>(input).map(|r| r.map(|p| p.providers.into_iter().map(Arc::new).collect())).boxed())
    }
    fn update_provider(&self, input: ServerProviderUpdateInput) -> Reply<Vec<Arc<ServerProvider>>> {
        reply(self.link.call::<ServerUpdateProvider>(input).map(|r| r.map(|p| p.providers.into_iter().map(Arc::new).collect())).boxed())
    }
    fn list_text_generator_models(&self, input: ListTextGeneratorModelsInput) -> Reply<ListTextGeneratorModelsResult> {
        reply(self.link.call::<ServerListTextGeneratorModels>(input))
    }
    fn start_provider_auth(&self, input: ProviderAuthStartInput) -> Reply<ProviderAuthState> {
        reply(self.link.call::<ProviderAuthStart>(input))
    }
    fn respond_provider_auth(&self, input: ProviderAuthRespondInput) -> Reply<ProviderAuthState> {
        reply(self.link.call::<ProviderAuthRespond>(input))
    }
    fn complete_provider_auth(&self, input: ProviderAuthCompleteInput) -> Reply<ProviderAuthState> {
        reply(self.link.call::<ProviderAuthComplete>(input))
    }
    fn cancel_provider_auth(&self, input: ProviderAuthCancelInput) -> Reply<ProviderAuthState> {
        reply(self.link.call::<ProviderAuthCancel>(input))
    }
    fn logout_provider_auth(&self, input: ProviderSetupInput) -> Reply<ProviderAuthState> {
        reply(self.link.call::<ProviderAuthLogout>(input))
    }
    fn start_provider_install(&self, input: ProviderSetupInput) -> Reply<ProviderInstallState> {
        reply(self.link.call::<ProviderInstallStart>(input))
    }
    fn accept_acp_registry_url_auth(&self, input: AcpRegistryAcceptUrlAuthInput) -> Reply<AcpRegistryAcceptUrlAuthResult> {
        reply(self.link.call::<ServerAcceptAcpRegistryUrlAuth>(input))
    }
    fn search_acp_registry(&self, input: AcpRegistrySearchInput) -> Reply<AcpRegistrySearchResult> {
        reply(self.link.call::<ServerSearchAcpRegistry>(input))
    }
    fn prepare_acp_registry_agent(&self, input: AcpRegistryPrepareInput) -> Reply<AcpRegistryPrepareResult> {
        reply(self.link.call::<ServerPrepareAcpRegistryAgent>(input))
    }
    fn discover_source_control(&self) -> Reply<SourceControlDiscoveryResult> {
        reply(self.link.call::<ServerDiscoverSourceControl>(Default::default()))
    }
    fn source_control_sign_in_start(&self, input: SourceControlSignInInput) -> Reply<SourceControlSignInPrompt> {
        reply(self.link.call::<SourceControlSignInStart>(input))
    }
    fn source_control_sign_in_wait(&self, input: SourceControlSignInInput) -> Reply<SourceControlSignInResult> {
        reply(self.link.call::<SourceControlSignInWait>(input))
    }
    fn source_control_sign_in_cancel(&self, input: SourceControlSignInInput) -> Reply<()> {
        reply(self.link.call::<SourceControlSignInCancel>(input))
    }
    fn list_issue_trackers(&self) -> Reply<IssueTrackerConnectionList> {
        reply(self.link.call::<IssueTrackersList>(Default::default()))
    }
    fn connect_issue_tracker(&self, input: IssueTrackerConnectInput) -> Reply<IssueTrackerConnection> {
        reply(self.link.call::<IssueTrackersConnect>(input))
    }
    fn bind_issue_tracker_project(&self, input: IssueTrackerBindProjectInput) -> Reply<()> {
        reply(self.link.call::<IssueTrackersBindProject>(input))
    }
    fn upsert_scheduled_task(&self, input: ScheduledTaskUpsertInput) -> Reply<ScheduledTaskMutationResult> {
        reply(self.link.call::<ScheduledTasksUpsert>(input))
    }
    fn set_scheduled_task_enabled(&self, input: ScheduledTaskSetEnabledInput) -> Reply<ScheduledTaskMutationResult> {
        reply(self.link.call::<ScheduledTasksSetEnabled>(input))
    }
    fn delete_scheduled_task(&self, input: ScheduledTaskDeleteInput) -> Reply<()> {
        reply(self.link.call::<ScheduledTasksDelete>(input).map(|r| r.map(|_| ())).boxed())
    }
    fn run_scheduled_task_now(&self, input: ScheduledTaskRunNowInput) -> Reply<ScheduledTask> {
        reply(self.link.call::<ScheduledTasksRunNow>(input).map(|r| r.map(|result| result.task)).boxed())
    }
    fn rotate_webhook_token(&self, input: ScheduledTaskRotateWebhookTokenInput) -> Reply<ScheduledTaskMutationResult> {
        reply(self.link.call::<ScheduledTasksRotateWebhookToken>(input))
    }
    fn mutate_project(&self, input: ProjectMutation) -> Reply<Project> {
        reply(self.link.call::<ProjectsMutate>(input))
    }
    fn browse_filesystem(&self, input: FilesystemBrowseInput) -> Reply<FilesystemBrowseResult> {
        reply(self.link.call::<FilesystemBrowse>(input))
    }
    fn start_project_clone(&self, input: ProjectCloneStartInput) -> Reply<ProjectCloneStartResult> {
        reply(self.link.call::<ProjectCloneStart>(input))
    }
    fn configure_devices(&self, input: DeviceConfigureInput) -> Reply<()> {
        reply(self.link.call::<DeviceConfigure>(input).map(|r| r.map(|_| ())).boxed())
    }
    fn test_device_host(&self, input: SshDeviceHostConfig) -> Reply<DeviceHostSummary> {
        reply(self.link.call::<DeviceTestHost>(input))
    }
    fn check_secret_source(&self, input: ServerCheckSecretSourcePayload) -> Reply<SecretSourceCheckResult> {
        reply(self.link.call::<ServerCheckSecretSource>(input))
    }
    fn run_storage_cleanup(&self) -> Reply<StorageCleanupReport> {
        reply(self.link.call::<ServerRunStorageCleanup>(Default::default()))
    }
    fn reclaim_database_space(&self) -> Reply<()> {
        reply(self.link.call::<ServerReclaimDatabaseSpace>(Default::default()))
    }
    fn trace_diagnostics(&self) -> Reply<ServerTraceDiagnosticsResult> {
        reply(self.link.call::<ServerGetTraceDiagnostics>(Default::default()))
    }
    fn process_diagnostics(&self) -> Reply<ServerProcessDiagnosticsResult> {
        reply(self.link.call::<ServerGetProcessDiagnostics>(Default::default()))
    }
    fn host_resources(&self) -> Reply<HostResourcesSnapshot> {
        reply(self.link.call::<ServerGetHostResources>(Default::default()))
    }
    fn check_otlp_endpoint(&self, input: OtlpEndpointCheckInput) -> Reply<OtlpEndpointCheckResult> {
        reply(self.link.call::<ServerCheckOtlpEndpoint>(input))
    }

}

/// The pull request page and tabs on one environment.
pub struct LivePullRequests {
    pub link: RpcLink,
}

impl PullRequestListSource for LivePullRequests {
    fn list(&self, input: PullRequestListInput) -> RpcFuture<PullRequestListResult> {
        unary(self.link.call::<PullRequestsList>(input))
    }
    fn list_stats(&self, input: PullRequestListStatsInput) -> RpcFuture<PullRequestListStatsResult> {
        unary(self.link.call::<PullRequestsListStats>(input))
    }
    fn filter_options(&self, input: PullRequestFilterOptionsInput) -> RpcFuture<PullRequestFilterOptions> {
        unary(self.link.call::<PullRequestsFilterOptions>(input))
    }
    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>> {
        unary(self.link.call_routed::<PullRequestsRunAction>(input))
    }
    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt> {
        stream_of(self.link.stream::<PullRequestsSubscribeRefreshes>(Default::default()))
    }

}

impl PullRequestDetailSource for LivePullRequests {
    fn detail(&self, input: PullRequestRef) -> RpcFuture<PullRequestDetail> {
        unary(self.link.call_routed::<PullRequestsDetail>(input))
    }
    fn checks(&self, input: PullRequestRef) -> RpcFuture<Option<PullRequestChecks>> {
        unary(self.link.call_routed::<PullRequestsChecks>(input))
    }
    fn activity(&self, input: PullRequestRef) -> RpcFuture<PullRequestActivity> {
        unary(self.link.call_routed::<PullRequestsActivity>(input))
    }
    fn thread_comments(&self, input: PullRequestThreadCommentsInput) -> RpcFuture<PullRequestThreadCommentsResult> {
        unary(self.link.call_routed::<PullRequestsThreadComments>(input))
    }
    fn stack(&self, input: PullRequestRef) -> RpcFuture<Option<PullRequestStack>> {
        unary(self.link.call_routed::<PullRequestsStack>(input))
    }
    fn linked_threads(&self, input: PullRequestRef) -> RpcFuture<PullRequestLinkedThreadsResult> {
        unary(self.link.call::<PullRequestsLinkedThreads>(input))
    }
    fn change_breakdown(&self, input: PullRequestRef) -> RpcFuture<PullRequestChangeBreakdown> {
        unary(self.link.call_routed::<PullRequestsChangeBreakdown>(input))
    }
    fn diff(&self, input: PullRequestDiffInput) -> RpcFuture<PullRequestDiffResult> {
        let link = self.link.clone();
        unary(self.link.run(async move {
            let body = serde_json::to_vec(&input).map_err(|e| SharedString::from(e.to_string()))?;
            let bytes = link
                .link
                .fetch_relative(reqwest::Method::POST, "/api/pull-requests/diff", Some((body, "application/json".into())))
                .await
                .map_err(|e| SharedString::from(e.to_string()))?;
            serde_json::from_slice(&bytes).map_err(|e| SharedString::from(e.to_string()))
        }))
    }
    fn diff_file_contents(&self, input: PullRequestDiffFileContentsInput) -> RpcFuture<PullRequestDiffFileContentsResult> {
        unary(self.link.call_routed::<PullRequestsDiffFileContents>(input))
    }
    fn files_viewed(&self, input: PullRequestRef) -> RpcFuture<PullRequestFilesViewedResult> {
        unary(self.link.call_routed::<PullRequestsFilesViewed>(input))
    }
    fn set_files_viewed(&self, input: PullRequestSetFilesViewedInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsSetFilesViewed>(input))
    }
    fn review_guide(&self, input: PullRequestRef) -> RpcFuture<PullRequestReviewGuideResult> {
        unary(self.link.call::<PullRequestsReviewGuide>(input))
    }
    fn generate_review_guide(&self, input: PullRequestRef) -> RpcFuture<PullRequestReviewGuideResult> {
        unary(self.link.call::<PullRequestsGenerateReviewGuide>(input))
    }
    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>> {
        unary(self.link.call_routed::<PullRequestsRunAction>(input))
    }
    fn update(&self, input: PullRequestUpdateInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsUpdate>(input))
    }
    fn comment(&self, input: PullRequestCommentInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsComment>(input))
    }
    fn update_comment(&self, input: PullRequestCommentUpdateInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsUpdateComment>(input))
    }
    fn submit_review(&self, input: PullRequestSubmitReviewInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsSubmitReview>(input))
    }
    fn reply_to_thread(&self, input: PullRequestThreadReplyInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsReplyToThread>(input))
    }
    fn set_thread_resolution(&self, input: PullRequestThreadResolutionInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsSetThreadResolution>(input))
    }
    fn set_reaction(&self, input: PullRequestReactionInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsSetReaction>(input))
    }
    fn reviewer_candidates(&self, input: PullRequestReviewerCandidatesInput) -> RpcFuture<PullRequestReviewerCandidateList> {
        unary(self.link.call_routed::<PullRequestsReviewerCandidates>(input))
    }
    fn request_reviewers(&self, input: PullRequestReviewerRequestInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsRequestReviewers>(input))
    }
    fn label_candidates(&self, input: PullRequestRef) -> RpcFuture<PullRequestLabelCandidateList> {
        unary(self.link.call_routed::<PullRequestsLabelCandidates>(input))
    }
    fn set_labels(&self, input: PullRequestLabelChangeInput) -> RpcFuture<()> {
        unary(self.link.call_routed::<PullRequestsSetLabels>(input))
    }
    fn prepare_thread(&self, input: GitPreparePullRequestThreadInput) -> RpcFuture<GitPreparePullRequestThreadResult> {
        unary(self.link.call::<GitPreparePullRequestThread>(input))
    }
    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt> {
        stream_of(self.link.stream::<PullRequestsSubscribeRefreshes>(Default::default()))
    }

}

/// The issue page and tabs on one environment.
pub struct LiveIssues {
    pub link: RpcLink,
}

impl IssueSource for LiveIssues {
    fn trackers(&self) -> RpcFuture<IssueTrackerConnectionList> {
        unary(self.link.call::<IssueTrackersList>(Default::default()))
    }
    fn list(&self, input: IssueListInput) -> RpcFuture<IssueListResult> {
        unary(self.link.call::<IssuesList>(input))
    }
    fn detail(&self, input: IssueRef) -> RpcFuture<IssueDetail> {
        unary(self.link.call::<IssuesDetail>(input))
    }
    fn comments(&self, input: IssueCommentsPageInput) -> RpcFuture<IssueCommentsPage> {
        unary(self.link.call::<IssuesComments>(input))
    }
    fn templates(&self, input: IssueScopeRef) -> RpcFuture<IssueTemplateList> {
        unary(self.link.call::<IssuesTemplates>(input))
    }
    fn candidates(&self, input: IssueScopeRef) -> RpcFuture<IssueCandidates> {
        unary(self.link.call::<IssuesCandidates>(input))
    }
    fn create(&self, input: IssueCreateInput) -> RpcFuture<IssueCreateResult> {
        unary(self.link.call::<IssuesCreate>(input))
    }
    fn comment(&self, input: IssueCommentInput) -> RpcFuture<()> {
        unary(self.link.call::<IssuesComment>(input))
    }
    fn update(&self, input: IssueUpdateInput) -> RpcFuture<()> {
        unary(self.link.call::<IssuesUpdate>(input))
    }
    fn set_state(&self, input: IssueSetStateInput) -> RpcFuture<()> {
        unary(self.link.call::<IssuesSetState>(input))
    }
    fn react(&self, input: IssueReactionInput) -> RpcFuture<()> {
        unary(self.link.call::<IssuesReact>(input))
    }
    fn linked_threads(&self, input: ThreadIssueKey) -> RpcFuture<IssueLinkedThreadsResult> {
        unary(self.link.call::<IssuesLinkedThreads>(input))
    }
    fn subscribe_linked_thread_refreshes(&self) -> RpcStream<NonNegativeInt> {
        stream_of(self.link.stream::<PullRequestsSubscribeLinkedThreadRefreshes>(Default::default()))
    }

}

/// Keeps a server settings source's snapshot in step with its environment's config.
pub fn apply_config(source: &LiveServerSettings, label: SharedString, config: &ServerConfig) {
    let mut snapshot = source.snapshot.lock().expect("snapshot");
    snapshot.environment_id = SharedString::from(source.link.environment_id().to_string());
    snapshot.environment_label = label;
    snapshot.settings = Arc::new(config.settings.clone());
    snapshot.providers = config.providers.iter().cloned().map(Arc::new).collect();
    snapshot.managed_policy = config.managed_policy.clone().map(Arc::new);
}
