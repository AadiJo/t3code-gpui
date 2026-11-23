//! The data and commands the git, branch, scripts and editor controls need,
//! as one trait whose every method is one RPC from `03-rpc-catalog.md`, plus
//! [`SourceControlModel`], the per-checkout state those controls share (the
//! `vcsEnvironment.status` subscription and `vcsActionManager` state of
//! `client-runtime/state/vcs*.ts`).

use std::sync::Arc;

use futures::{StreamExt as _, future::BoxFuture, stream::BoxStream};
use gpui_kit::{Context, SharedString, Task};
use t3code_contracts::{
    GitActionProgressEvent, GitPreparePullRequestThreadInput, GitPreparePullRequestThreadResult,
    GitPullRequestRefInput, GitResolvePullRequestResult, GitRunStackedActionInput, GitRunStackedActionResult,
    LaunchEditorInput, ProjectMutation, PullRequestDetail, PullRequestRef, VcsCreateRefInput, VcsCreateRefResult,
    VcsInitInput, VcsListRefsInput, VcsListRefsResult, VcsPullInput, VcsPullResult, VcsRefreshStatusPayload,
    VcsRemoveWorktreeInput, VcsStatusLocalResult, VcsStatusLocalResultWorkingTree, VcsStatusRemoteResult,
    VcsStatusResult, VcsStatusStreamEvent, VcsStatusSubscriptionInput, VcsSwitchRefInput, VcsSwitchRefResult,
};

use super::git_actions_control_logic::{VcsActionOperation, VcsActionState};

/// A failed call, with the message the server or transport gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceControlError(pub SharedString);

/// One unary RPC.
pub type RpcFuture<T> = BoxFuture<'static, Result<T, SourceControlError>>;
/// One streaming RPC: each item is a chunk; the stream ends with the exit.
pub type RpcStream<T> = BoxStream<'static, Result<T, SourceControlError>>;

/// Everything the source control controls read and run. The live
/// implementation sends each method as the RPC named on it to the thread's
/// environment; the method's payload and success types are that RPC's.
pub trait GitActionsSource: 'static {
    /// `subscribeVcsStatus` (stream).
    fn subscribe_vcs_status(&self, input: VcsStatusSubscriptionInput) -> RpcStream<VcsStatusStreamEvent>;
    /// `vcs.refreshStatus`.
    fn refresh_status(&self, input: VcsRefreshStatusPayload) -> RpcFuture<VcsStatusResult>;
    /// `vcs.listRefs`.
    fn list_refs(&self, input: VcsListRefsInput) -> RpcFuture<VcsListRefsResult>;
    /// `vcs.switchRef`.
    fn switch_ref(&self, input: VcsSwitchRefInput) -> RpcFuture<VcsSwitchRefResult>;
    /// `vcs.createRef`.
    fn create_ref(&self, input: VcsCreateRefInput) -> RpcFuture<VcsCreateRefResult>;
    /// `vcs.pull`.
    fn pull(&self, input: VcsPullInput) -> RpcFuture<VcsPullResult>;
    /// `vcs.init`.
    fn init(&self, input: VcsInitInput) -> RpcFuture<()>;
    /// `vcs.removeWorktree`.
    fn remove_worktree(&self, input: VcsRemoveWorktreeInput) -> RpcFuture<()>;
    /// `git.runStackedAction` (stream of progress events ending in
    /// `action_finished` or `action_failed`).
    fn run_stacked_action(&self, input: GitRunStackedActionInput) -> RpcStream<GitActionProgressEvent>;
    /// `git.resolvePullRequest`.
    fn resolve_pull_request(&self, input: GitPullRequestRefInput) -> RpcFuture<GitResolvePullRequestResult>;
    /// `git.preparePullRequestThread`.
    fn prepare_pull_request_thread(
        &self,
        input: GitPreparePullRequestThreadInput,
    ) -> RpcFuture<GitPreparePullRequestThreadResult>;
    /// `pullRequests.detail`.
    fn pull_request_detail(&self, input: PullRequestRef) -> RpcFuture<PullRequestDetail>;
    /// `shell.openInEditor`.
    fn open_in_editor(&self, input: LaunchEditorInput) -> RpcFuture<()>;
    /// `projects.mutate` (scripts are saved as a `project.update`).
    fn mutate_project(&self, input: ProjectMutation) -> RpcFuture<()>;
}

/// `applyGitStatusStreamEvent`: folds a status stream event into the snapshot.
pub fn apply_git_status_stream_event(current: Option<&VcsStatusResult>, event: VcsStatusStreamEvent) -> Option<VcsStatusResult> {
    Some(match event {
        VcsStatusStreamEvent::Snapshot(e) => merge_git_status_parts(e.local, e.remote),
        VcsStatusStreamEvent::LocalUpdated(e) => merge_git_status_parts(e.local, current.map(remote_part)),
        VcsStatusStreamEvent::RemoteUpdated(e) => {
            let local = current.map(local_part).unwrap_or_else(|| VcsStatusLocalResult {
                is_repo: true,
                source_control_provider: None,
                has_primary_remote: false,
                is_default_ref: false,
                ref_name: None,
                has_working_tree_changes: false,
                working_tree: VcsStatusLocalResultWorkingTree { root_path: None, files: Vec::new(), insertions: 0, deletions: 0 },
                branch_changes: None,
            });
            merge_git_status_parts(local, e.remote)
        }
        VcsStatusStreamEvent::Unknown(_) => return current.cloned(),
    })
}

/// `mergeGitStatusParts`: a missing remote half reads as no upstream.
pub fn merge_git_status_parts(local: VcsStatusLocalResult, remote: Option<VcsStatusRemoteResult>) -> VcsStatusResult {
    let remote = remote.unwrap_or(VcsStatusRemoteResult {
        has_upstream: false,
        ahead_count: 0,
        behind_count: 0,
        ahead_of_default_count: Some(0),
        pr: None,
    });
    VcsStatusResult {
        is_repo: local.is_repo,
        source_control_provider: local.source_control_provider,
        has_primary_remote: local.has_primary_remote,
        is_default_ref: local.is_default_ref,
        ref_name: local.ref_name,
        has_working_tree_changes: local.has_working_tree_changes,
        working_tree: local.working_tree,
        branch_changes: local.branch_changes.map(|b| t3code_contracts::VcsStatusResultBranchChanges {
            base_ref: b.base_ref,
            insertions: b.insertions,
            deletions: b.deletions,
        }),
        has_upstream: remote.has_upstream,
        ahead_count: remote.ahead_count,
        behind_count: remote.behind_count,
        ahead_of_default_count: remote.ahead_of_default_count,
        pr: remote.pr,
    }
}

fn remote_part(status: &VcsStatusResult) -> VcsStatusRemoteResult {
    VcsStatusRemoteResult {
        has_upstream: status.has_upstream,
        ahead_count: status.ahead_count,
        behind_count: status.behind_count,
        ahead_of_default_count: status.ahead_of_default_count,
        pr: status.pr.clone(),
    }
}

fn local_part(status: &VcsStatusResult) -> VcsStatusLocalResult {
    VcsStatusLocalResult {
        is_repo: status.is_repo,
        source_control_provider: status.source_control_provider.clone(),
        has_primary_remote: status.has_primary_remote,
        is_default_ref: status.is_default_ref,
        ref_name: status.ref_name.clone(),
        has_working_tree_changes: status.has_working_tree_changes,
        working_tree: status.working_tree.clone(),
        branch_changes: status.branch_changes.clone().map(|b| t3code_contracts::VcsStatusLocalResultBranchChanges {
            base_ref: b.base_ref,
            insertions: b.insertions,
            deletions: b.deletions,
        }),
    }
}

/// A finished action, shown in place of the row for a while (`InlineGitActionSuccess`).
#[derive(Clone, Debug, PartialEq)]
pub struct InlineSuccess {
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub expires_at_ms: i64,
}

/// The live state of one checkout: its git status, the running action, and
/// the last inline success. The git actions row and the branch toolbar both
/// read it, so a commit's progress and a branch switch stay in step.
pub struct SourceControlModel {
    pub source: Arc<dyn GitActionsSource>,
    pub cwd: SharedString,
    pub status: Option<VcsStatusResult>,
    pub status_error: Option<SharedString>,
    pub action: VcsActionState,
    pub inline_success: Option<InlineSuccess>,
    /// The next action id the client hands the server.
    next_action_id: u64,
    _subscription: Option<Task<()>>,
}

/// Milliseconds since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl SourceControlModel {
    /// Subscribes to the checkout's status (with the remote half).
    pub fn new(source: Arc<dyn GitActionsSource>, cwd: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let cwd: SharedString = cwd.into();
        let mut stream =
            source.subscribe_vcs_status(VcsStatusSubscriptionInput { cwd: cwd.to_string(), include_remote: Some(true) });
        let subscription = cx.spawn(async move |this, cx| {
            while let Some(event) = stream.next().await {
                let alive = this.update(cx, |model, cx| {
                    match event {
                        Ok(event) => {
                            model.status = apply_git_status_stream_event(model.status.as_ref(), event);
                            model.status_error = None;
                        }
                        Err(error) => model.status_error = Some(error.0),
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        Self {
            source,
            cwd,
            status: None,
            status_error: None,
            action: VcsActionState::default(),
            inline_success: None,
            next_action_id: 0,
            _subscription: Some(subscription),
        }
    }

    /// Whether a commit, push, pull or publish is running (`useSourceControlActionRunning`).
    pub fn is_action_running(&self) -> bool {
        self.action.is_running
            && matches!(
                self.action.operation,
                Some(VcsActionOperation::RunChangeRequest | VcsActionOperation::Pull | VcsActionOperation::PublishRepository)
            )
    }

    /// The inline success while it is still visible.
    pub fn visible_success(&self, now_ms: i64) -> Option<&InlineSuccess> {
        self.inline_success.as_ref().filter(|s| s.expires_at_ms > now_ms)
    }

    /// Starts tracking an operation and returns its action id (`beginVcsActionState`).
    pub fn begin(&mut self, operation: VcsActionOperation, cx: &mut Context<Self>) -> String {
        self.next_action_id += 1;
        let now = now_ms();
        self.action = VcsActionState {
            is_running: true,
            operation: Some(operation),
            current_label: None,
            hook_name: None,
            last_output_line: None,
            phase_started_at_ms: Some(now),
            hook_started_at_ms: None,
        };
        self.inline_success = None;
        cx.notify();
        format!("local-vcs-action:{}", self.next_action_id)
    }

    /// `applyVcsActionProgressEvent` for the fields the row shows.
    pub fn apply_progress(&mut self, event: &GitActionProgressEvent, cx: &mut Context<Self>) {
        let now = now_ms();
        let action = &mut self.action;
        match event {
            GitActionProgressEvent::ActionStarted(_) => {
                action.operation = Some(VcsActionOperation::RunChangeRequest);
                action.phase_started_at_ms = Some(now);
                action.hook_started_at_ms = None;
                action.hook_name = None;
                action.last_output_line = None;
            }
            GitActionProgressEvent::PhaseStarted(e) => {
                action.current_label = Some(e.label.clone().into());
                action.phase_started_at_ms = Some(now);
                action.hook_started_at_ms = None;
                action.hook_name = None;
                action.last_output_line = None;
            }
            GitActionProgressEvent::HookStarted(e) => {
                action.current_label = Some(format!("Running {}...", e.hook_name).into());
                action.hook_name = Some(e.hook_name.clone().into());
                action.hook_started_at_ms = Some(now);
                action.last_output_line = None;
            }
            GitActionProgressEvent::HookOutput(e) => action.last_output_line = Some(e.text.clone().into()),
            GitActionProgressEvent::HookFinished(_) => {
                action.hook_name = None;
                action.hook_started_at_ms = None;
                action.last_output_line = None;
            }
            _ => {}
        }
        cx.notify();
    }

    /// Ends the running operation.
    pub fn finish(&mut self, success: Option<InlineSuccess>, cx: &mut Context<Self>) {
        self.action = VcsActionState::default();
        self.inline_success = success;
        cx.notify();
    }

    /// Runs `git.runStackedAction`, folding progress into [`Self::action`].
    pub fn run_stacked_action(
        &mut self,
        mut input: GitRunStackedActionInput,
        cx: &mut Context<Self>,
    ) -> Task<Result<GitRunStackedActionResult, SourceControlError>> {
        input.action_id = self.begin(VcsActionOperation::RunChangeRequest, cx);
        input.cwd = self.cwd.to_string();
        let mut stream = self.source.run_stacked_action(input);
        cx.spawn(async move |this, cx| {
            let mut outcome = Err(SourceControlError("Source control action failed.".into()));
            while let Some(event) = stream.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        outcome = Err(error);
                        break;
                    }
                };
                match &event {
                    GitActionProgressEvent::ActionFinished(e) => outcome = Ok(e.result.clone()),
                    GitActionProgressEvent::ActionFailed(e) => outcome = Err(SourceControlError(e.message.clone().into())),
                    _ => {}
                }
                this.update(cx, |model, cx| model.apply_progress(&event, cx)).ok();
            }
            outcome
        })
    }
}
