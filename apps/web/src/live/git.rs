//! The branch toolbar, git rows, project scripts and open-in picker on a live thread:
//! [`GitActionsSource`] over the environment's session, and the props the shell row, the
//! project and the server config give them.

use std::sync::Arc;

use futures::{FutureExt as _, StreamExt as _};
use gpui_kit::{App, SharedString};
use t3code_contracts::rpc::methods::{
    GitPreparePullRequestThread, GitResolvePullRequest, GitRunStackedAction, ProjectsMutate, PullRequestsDetail, ServerDiscoverSourceControl,
    ShellOpenInEditor, SourceControlPublishRepository, SubscribeVcsStatus, VcsCreateRef, VcsInit, VcsListRefs, VcsPull, VcsRefreshStatus, VcsRemoveWorktree, VcsSwitchRef,
};
use t3code_contracts::*;

use super::LiveData;
use super::rpc::RpcLink;
use crate::components::branch_toolbar::BranchToolbarProps;
use crate::components::branch_toolbar_logic::{EnvMode, EnvironmentOption, MachineKind};
use crate::components::git_actions_control::GitActionsProps;
use crate::components::git_actions_control_source::{GitActionsSource, RpcFuture, RpcStream, SourceControlError};
use crate::remote_open::{RemoteOpenHost, RemoteOpenState, RemoteOpenTarget, remote_capable_editors, resolve_remote_open_state};
use crate::state::StateBridge;
use crate::state::app_data::{GitControlsSetup, ThreadView};

/// [`GitActionsSource`] on one environment. Each method is the RPC its trait doc names.
pub struct LiveGitSource {
    link: RpcLink,
}

fn unary<T: Send + 'static>(future: futures::future::BoxFuture<'static, Result<T, SharedString>>) -> RpcFuture<T> {
    future.map(|result| result.map_err(SourceControlError)).boxed()
}

fn stream<T: Send + 'static>(stream: futures::stream::BoxStream<'static, Result<T, SharedString>>) -> RpcStream<T> {
    stream.map(|item| item.map_err(SourceControlError)).boxed()
}

impl GitActionsSource for LiveGitSource {
    fn subscribe_vcs_status(&self, input: VcsStatusSubscriptionInput) -> RpcStream<VcsStatusStreamEvent> {
        stream(self.link.stream::<SubscribeVcsStatus>(input))
    }
    fn refresh_status(&self, input: VcsRefreshStatusPayload) -> RpcFuture<VcsStatusResult> {
        unary(self.link.call::<VcsRefreshStatus>(input))
    }
    fn list_refs(&self, input: VcsListRefsInput) -> RpcFuture<VcsListRefsResult> {
        unary(self.link.call::<VcsListRefs>(input))
    }
    fn switch_ref(&self, input: VcsSwitchRefInput) -> RpcFuture<VcsSwitchRefResult> {
        unary(self.link.call::<VcsSwitchRef>(input))
    }
    fn create_ref(&self, input: VcsCreateRefInput) -> RpcFuture<VcsCreateRefResult> {
        unary(self.link.call::<VcsCreateRef>(input))
    }
    fn pull(&self, input: VcsPullInput) -> RpcFuture<VcsPullResult> {
        unary(self.link.call::<VcsPull>(input))
    }
    fn init(&self, input: VcsInitInput) -> RpcFuture<()> {
        unary(self.link.call::<VcsInit>(input).map(|r| r.map(|_| ())).boxed())
    }
    fn remove_worktree(&self, input: VcsRemoveWorktreeInput) -> RpcFuture<()> {
        unary(self.link.call::<VcsRemoveWorktree>(input).map(|r| r.map(|_| ())).boxed())
    }
    fn run_stacked_action(&self, input: GitRunStackedActionInput) -> RpcStream<GitActionProgressEvent> {
        stream(self.link.stream::<GitRunStackedAction>(input))
    }
    fn resolve_pull_request(&self, input: GitPullRequestRefInput) -> RpcFuture<GitResolvePullRequestResult> {
        unary(self.link.call::<GitResolvePullRequest>(input))
    }
    fn prepare_pull_request_thread(&self, input: GitPreparePullRequestThreadInput) -> RpcFuture<GitPreparePullRequestThreadResult> {
        unary(self.link.call::<GitPreparePullRequestThread>(input))
    }
    fn pull_request_detail(&self, input: PullRequestRef) -> RpcFuture<PullRequestDetail> {
        unary(self.link.call::<PullRequestsDetail>(input))
    }
    fn open_in_editor(&self, input: LaunchEditorInput) -> RpcFuture<()> {
        unary(self.link.call::<ShellOpenInEditor>(input).map(|r| r.map(|_| ())).boxed())
    }
    fn mutate_project(&self, input: ProjectMutation) -> RpcFuture<()> {
        unary(self.link.call::<ProjectsMutate>(input).map(|r| r.map(|_| ())).boxed())
    }
    fn source_control_discovery(&self) -> RpcFuture<SourceControlDiscoveryResult> {
        unary(self.link.call::<ServerDiscoverSourceControl>(Default::default()))
    }
    fn publish_repository(&self, input: SourceControlPublishRepositoryInput) -> RpcFuture<SourceControlPublishRepositoryResult> {
        unary(self.link.call::<SourceControlPublishRepository>(input))
    }
}

/// A source whose calls all fail: the thread's environment is not saved.
struct Disconnected;

impl Disconnected {
    fn fail<T: Send + 'static>() -> RpcFuture<T> {
        async { Err(SourceControlError("This environment is not connected.".into())) }.boxed()
    }
    fn fail_stream<T: Send + 'static>() -> RpcStream<T> {
        futures::stream::once(async { Err(SourceControlError("This environment is not connected.".into())) }).boxed()
    }
}

impl GitActionsSource for Disconnected {
    fn subscribe_vcs_status(&self, _: VcsStatusSubscriptionInput) -> RpcStream<VcsStatusStreamEvent> {
        Self::fail_stream()
    }
    fn refresh_status(&self, _: VcsRefreshStatusPayload) -> RpcFuture<VcsStatusResult> {
        Self::fail()
    }
    fn list_refs(&self, _: VcsListRefsInput) -> RpcFuture<VcsListRefsResult> {
        Self::fail()
    }
    fn switch_ref(&self, _: VcsSwitchRefInput) -> RpcFuture<VcsSwitchRefResult> {
        Self::fail()
    }
    fn create_ref(&self, _: VcsCreateRefInput) -> RpcFuture<VcsCreateRefResult> {
        Self::fail()
    }
    fn pull(&self, _: VcsPullInput) -> RpcFuture<VcsPullResult> {
        Self::fail()
    }
    fn init(&self, _: VcsInitInput) -> RpcFuture<()> {
        Self::fail()
    }
    fn remove_worktree(&self, _: VcsRemoveWorktreeInput) -> RpcFuture<()> {
        Self::fail()
    }
    fn run_stacked_action(&self, _: GitRunStackedActionInput) -> RpcStream<GitActionProgressEvent> {
        Self::fail_stream()
    }
    fn resolve_pull_request(&self, _: GitPullRequestRefInput) -> RpcFuture<GitResolvePullRequestResult> {
        Self::fail()
    }
    fn prepare_pull_request_thread(&self, _: GitPreparePullRequestThreadInput) -> RpcFuture<GitPreparePullRequestThreadResult> {
        Self::fail()
    }
    fn pull_request_detail(&self, _: PullRequestRef) -> RpcFuture<PullRequestDetail> {
        Self::fail()
    }
    fn open_in_editor(&self, _: LaunchEditorInput) -> RpcFuture<()> {
        Self::fail()
    }
    fn mutate_project(&self, _: ProjectMutation) -> RpcFuture<()> {
        Self::fail()
    }
}

/// The environment a view's thread or draft runs on.
fn view_environment(data: &LiveData, view: &ThreadView, cx: &App) -> Option<EnvironmentId> {
    match super::view_target(view) {
        Some(key) => Some(crate::state::shell::runtime_ids(&key).0),
        None => data.draft_environment(view, cx),
    }
}

/// The environment's server runs on this machine (or no session says otherwise).
pub fn is_loopback(link: Option<&RpcLink>) -> bool {
    let host = link.and_then(|l| l.link.current_session()).and_then(|s| s.prepared.http_base_url.host_str().map(str::to_owned));
    host.as_deref().is_none_or(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]" | "::1"))
}

/// `resolveRemoteOpenTarget`: a loopback server opens editors itself; another machine opens
/// them here through the server's advertised SSH targets.
fn remote_open_state(link: Option<&RpcLink>, config: Option<&ServerConfig>) -> RemoteOpenState {
    if is_loopback(link) {
        return resolve_remote_open_state(Some(&RemoteOpenTarget::Local));
    }
    let advertised = config
        .and_then(|c| c.remote_open_targets.as_ref())
        .into_iter()
        .flatten()
        .map(|target| RemoteOpenHost { kind: Some(target.kind), host: target.host.clone(), username: target.username.clone() })
        .collect();
    resolve_remote_open_state(Some(&RemoteOpenTarget::Remote { ssh_alias: None, advertised }))
}

/// Everything the thread's controls are built from.
pub fn controls(data: &LiveData, view: &ThreadView, cx: &mut App) -> GitControlsSetup {
    let environment_id = view_environment(data, view, cx);
    let link = environment_id.as_ref().and_then(|id| RpcLink::new(id, cx));
    let environment = environment_id.as_ref().and_then(|id| StateBridge::environment(id, cx));
    let config = environment.as_ref().and_then(|e| e.read(cx).config.clone());
    let scripts = environment
        .as_ref()
        .zip(view.project_id.as_ref())
        .and_then(|(env, project)| {
            let env = env.read(cx);
            let shell = env.shell.snapshot.as_ref()?;
            shell.projects.iter().find(|p| p.id.0.as_ref() == project.0.as_ref()).map(|p| p.scripts.clone())
        })
        .unwrap_or_default();
    let open_state = remote_open_state(link.as_ref(), config.as_deref());
    let editors = match &open_state {
        RemoteOpenState::LocalExec => config.as_ref().map(|c| c.available_editors.clone()).unwrap_or_default(),
        _ => remote_capable_editors(),
    };
    let source: Arc<dyn GitActionsSource> = match link {
        Some(link) => Arc::new(LiveGitSource { link }),
        None => Arc::new(Disconnected),
    };
    let thread_id = super::view_target(view).map(|key| crate::state::shell::runtime_ids(&key).1);
    GitControlsSetup {
        source,
        git: GitActionsProps {
            can_write_source_control: true,
            can_change_thread_branch: true,
            quiet_when_idle: view.timeline.entries.is_empty(),
            compact: false,
            show_actions: true,
            show_changes: true,
            thread_id,
            project_id: view.project_id.as_ref().map(|p| ProjectId::new(p.0.as_ref())),
            offered_source_control_providers: config.as_ref().and_then(|c| c.settings.offered_source_control_providers.clone()),
        },
        scripts,
        editors,
        open_state,
        is_git_repo: true,
    }
}

/// The branch toolbar's props: the saved environments, the thread's checkout and branch.
pub fn toolbar_props(data: &LiveData, view: &ThreadView, cx: &mut App) -> BranchToolbarProps {
    let environment_id = view_environment(data, view, cx);
    let environments = StateBridge::environments(cx)
        .read(cx)
        .list
        .iter()
        .map(|env| {
            let env = env.read(cx);
            EnvironmentOption {
                environment_id: env.environment_id().clone(),
                label: env
                    .config
                    .as_ref()
                    .map(|c| SharedString::from(c.environment.label.clone()))
                    .unwrap_or_else(|| SharedString::from(env.environment_id().to_string())),
                is_primary: false,
                machine: t3code_contracts::server::resolve_environment_machine_kind(
                    env.config.as_ref().map(|c| &c.environment),
                    env.config.as_ref().map(|c| &c.settings),
                ),
            }
        })
        .collect();
    let row = super::view_target(view).and_then(|key| StateBridge::shell(cx).read(cx).row(&key).cloned());
    let draft = data.draft_workspace(view);
    let worktree_path = match &draft {
        Some(draft) => draft.worktree_path.clone(),
        None => row.as_ref().and_then(|row| row.source.worktree_path.clone()),
    };
    let worktree_path_for_seed = worktree_path.clone();
    let project_root = view
        .project_id
        .as_ref()
        .and_then(|project| {
            StateBridge::shell(cx)
                .read(cx)
                .snapshot()
                .projects
                .iter()
                .find(|p| p.project_ref.project_id == *project)
                .map(|p| p.workspace_root.to_string())
        })
        .unwrap_or_else(|| view.cwd.to_string());
    BranchToolbarProps {
        environments,
        environment_id,
        auto_environment_label: None,
        can_change_environment: view.is_draft,
        can_auto_balance: false,
        env_mode: match &draft {
            Some(draft) => draft.env_mode,
            None if worktree_path.is_some() => EnvMode::Worktree,
            None => EnvMode::Local,
        },
        force_new_worktree: false,
        worktree_path,
        project_cwd: project_root,
        is_server_thread: !view.is_draft,
        env_locked: !view.is_draft,
        thread_branch: view.branch.as_ref().map(|b| b.to_string()),
        start_from_origin: draft.as_ref().is_some_and(|d| d.start_from_origin),
        previous_worktree: draft
            .as_ref()
            .and(view.project_id.as_ref())
            .zip(view_environment(data, view, cx))
            .and_then(|(project, environment)| previous_worktree(&environment, project.0.as_ref(), worktree_path_for_seed.as_deref(), cx)),
        turn_running: view.running,
        can_write_source_control: true,
        can_change_thread_branch: true,
        can_checkout_pull_request: true,
        show_git_controls: true,
        pull_request_badge: row.as_ref().and_then(|row| crate::thread_pull_requests::strip_badge(row.source.pull_requests.as_deref()?)),
    }
}

/// The worktree of the project's most recently updated other thread, offered to a draft as
/// "Previous worktree" (`resolvePreviousWorktreeSeed` over the project's thread shells).
pub fn previous_worktree(
    environment_id: &EnvironmentId,
    project: &str,
    current: Option<&str>,
    cx: &App,
) -> Option<crate::components::branch_toolbar_logic::PreviousWorktreeSeed> {
    use crate::components::branch_toolbar_logic::{WorktreeThread, resolve_previous_worktree_seed};
    let environment = StateBridge::environment(environment_id, cx)?;
    let environment = environment.read(cx);
    let threads: Vec<WorktreeThread> = environment
        .shell
        .snapshot
        .as_ref()?
        .threads
        .iter()
        .filter(|t| t.project_id.as_str() == project)
        .map(|t| WorktreeThread {
            branch: t.branch.clone(),
            worktree_path: t.worktree_path.clone(),
            updated_at_ms: t3code_client_runtime::state::models::parse_ms(&t.updated_at),
            archived: t.archived_at.is_some(),
        })
        .collect();
    resolve_previous_worktree_seed(&threads, current)
}
