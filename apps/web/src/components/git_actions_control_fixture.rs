//! A [`GitActionsSource`] that answers from memory, for scenes and scripted
//! checks. It records every call by RPC tag, so a check can show which RPC a
//! click sent.

use std::sync::{Arc, Mutex};

use futures::{FutureExt as _, StreamExt as _, stream};
use t3code_contracts::{
    GitActionProgressEvent, GitActionProgressEventActionFinished, GitActionProgressEventActionStarted,
    GitActionProgressEventPhaseStarted, GitActionProgressPhase, GitPreparePullRequestThreadInput,
    GitPreparePullRequestThreadResult, GitPreparePullRequestThreadResultPullRequest, GitPullRequestRefInput,
    GitResolvePullRequestResult, GitRunStackedActionInput, GitRunStackedActionResult, GitRunStackedActionResultBranch,
    GitRunStackedActionResultBranchStatus, GitRunStackedActionResultCommit, GitRunStackedActionResultCommitStatus,
    GitRunStackedActionResultPr, GitRunStackedActionResultPrStatus, GitRunStackedActionResultPush,
    GitRunStackedActionResultPushStatus, GitRunStackedActionResultToast, GitRunStackedActionResultToastCta,
    GitRunStackedActionResultToastCtaNoneType, LaunchEditorInput, ProjectMutation, PullRequestDetail, PullRequestRef,
    PullRequestState, SourceControlProviderInfo, SourceControlProviderKind, VcsCreateRefInput, VcsCreateRefResult,
    VcsInitInput, VcsListRefsInput, VcsListRefsResult, VcsPullInput, VcsPullResult, VcsPullResultStatus, VcsRef,
    VcsRefreshStatusPayload, VcsRemoveWorktreeInput, VcsStatusLocalResultWorkingTree,
    VcsStatusLocalResultWorkingTreeFilesItem, VcsStatusRemoteResultPr, VcsStatusResult,
    VcsStatusResultBranchChanges, VcsStatusStreamEvent, VcsStatusStreamEventSnapshot, VcsStatusSubscriptionInput,
    VcsSwitchRefInput, VcsSwitchRefResult,
};

use super::git_actions_control_source::{GitActionsSource, RpcFuture, RpcStream, SourceControlError};

/// Answers every call from fields set up front.
#[derive(Clone)]
pub struct FixtureGitSource {
    pub status: VcsStatusResult,
    pub refs: Vec<VcsRef>,
    pub pull_request: Option<PullRequestDetail>,
    /// Makes every write fail with this message.
    pub fail_with: Option<String>,
    /// The status stream reports this error after its snapshot.
    pub status_error: Option<String>,
    pub calls: Arc<Mutex<Vec<String>>>,
}

fn ready<T: Send + 'static>(value: Result<T, SourceControlError>) -> RpcFuture<T> {
    futures::future::ready(value).boxed()
}

impl FixtureGitSource {
    pub fn new(status: VcsStatusResult) -> Self {
        Self { status, refs: fixture_refs(), pull_request: None, fail_with: None, status_error: None, calls: Arc::default() }
    }

    fn record(&self, tag: &str) {
        self.calls.lock().unwrap().push(tag.to_owned());
    }

    fn write<T: Send + 'static>(&self, tag: &str, value: T) -> RpcFuture<T> {
        self.record(tag);
        ready(match &self.fail_with {
            Some(message) => Err(SourceControlError(message.clone().into())),
            None => Ok(value),
        })
    }
}

impl GitActionsSource for FixtureGitSource {
    fn subscribe_vcs_status(&self, _: VcsStatusSubscriptionInput) -> RpcStream<VcsStatusStreamEvent> {
        self.record("subscribeVcsStatus");
        let s = self.status.clone();
        let event = VcsStatusStreamEvent::Snapshot(VcsStatusStreamEventSnapshot {
            local: t3code_contracts::VcsStatusLocalResult {
                is_repo: s.is_repo,
                source_control_provider: s.source_control_provider.clone(),
                has_primary_remote: s.has_primary_remote,
                is_default_ref: s.is_default_ref,
                ref_name: s.ref_name.clone(),
                has_working_tree_changes: s.has_working_tree_changes,
                working_tree: s.working_tree.clone(),
                branch_changes: s.branch_changes.clone().map(|b| t3code_contracts::VcsStatusLocalResultBranchChanges {
                    base_ref: b.base_ref,
                    insertions: b.insertions,
                    deletions: b.deletions,
                }),
            },
            remote: Some(t3code_contracts::VcsStatusRemoteResult {
                has_upstream: s.has_upstream,
                ahead_count: s.ahead_count,
                behind_count: s.behind_count,
                ahead_of_default_count: s.ahead_of_default_count,
                pr: s.pr.clone(),
            }),
        });
        let error = self.status_error.clone().map(|e| Err(SourceControlError(e.into())));
        stream::iter(std::iter::once(Ok(event)).chain(error)).boxed()
    }

    fn refresh_status(&self, _: VcsRefreshStatusPayload) -> RpcFuture<VcsStatusResult> {
        self.record("vcs.refreshStatus");
        ready(Ok(self.status.clone()))
    }

    fn list_refs(&self, input: VcsListRefsInput) -> RpcFuture<VcsListRefsResult> {
        self.record("vcs.listRefs");
        let refs: Vec<VcsRef> = match input.query.as_deref() {
            Some(q) => self.refs.iter().filter(|r| r.name.contains(q)).cloned().collect(),
            None => self.refs.clone(),
        };
        let total = refs.len() as u64;
        ready(Ok(VcsListRefsResult {
            is_at_repository_root: Some(true),
            refs,
            is_repo: true,
            has_primary_remote: true,
            next_cursor: None,
            total_count: total,
        }))
    }

    fn switch_ref(&self, input: VcsSwitchRefInput) -> RpcFuture<VcsSwitchRefResult> {
        self.write("vcs.switchRef", VcsSwitchRefResult { ref_name: Some(input.ref_name) })
    }

    fn create_ref(&self, input: VcsCreateRefInput) -> RpcFuture<VcsCreateRefResult> {
        self.write("vcs.createRef", VcsCreateRefResult { ref_name: input.ref_name })
    }

    fn pull(&self, _: VcsPullInput) -> RpcFuture<VcsPullResult> {
        let branch = self.status.ref_name.clone().unwrap_or_default();
        self.write(
            "vcs.pull",
            VcsPullResult { status: VcsPullResultStatus::Pulled, upstream_ref: Some(format!("origin/{branch}")), ref_name: branch },
        )
    }

    fn init(&self, _: VcsInitInput) -> RpcFuture<()> {
        self.write("vcs.init", ())
    }

    fn remove_worktree(&self, _: VcsRemoveWorktreeInput) -> RpcFuture<()> {
        self.write("vcs.removeWorktree", ())
    }

    fn run_stacked_action(&self, input: GitRunStackedActionInput) -> RpcStream<GitActionProgressEvent> {
        self.record("git.runStackedAction");
        if let Some(message) = &self.fail_with {
            return stream::iter([Err(SourceControlError(message.clone().into()))]).boxed();
        }
        let (id, cwd, action) = (input.action_id.clone(), input.cwd.clone(), input.action);
        let result = GitRunStackedActionResult {
            action,
            branch: GitRunStackedActionResultBranch { status: GitRunStackedActionResultBranchStatus::SkippedNotRequested, name: None },
            commit: GitRunStackedActionResultCommit {
                status: GitRunStackedActionResultCommitStatus::Created,
                commit_sha: Some("4f2a9c1".into()),
                subject: input.commit_message.clone().or(Some("Add the thread details panel".into())),
            },
            push: GitRunStackedActionResultPush {
                status: GitRunStackedActionResultPushStatus::SkippedNotRequested,
                branch: None,
                upstream_branch: None,
                set_upstream: None,
            },
            pr: GitRunStackedActionResultPr {
                status: GitRunStackedActionResultPrStatus::SkippedNotRequested,
                url: None,
                number: None,
                base_branch: None,
                head_branch: None,
                title: None,
            },
            toast: GitRunStackedActionResultToast {
                title: "Committed 4f2a9c1".into(),
                description: Some("Add the thread details panel".into()),
                cta: GitRunStackedActionResultToastCta::NoneType(GitRunStackedActionResultToastCtaNoneType {}),
            },
        };
        stream::iter([
            Ok(GitActionProgressEvent::ActionStarted(GitActionProgressEventActionStarted {
                action_id: id.clone(),
                cwd: cwd.clone(),
                action,
                phases: vec![GitActionProgressPhase::Commit],
            })),
            Ok(GitActionProgressEvent::PhaseStarted(GitActionProgressEventPhaseStarted {
                action_id: id.clone(),
                cwd: cwd.clone(),
                action,
                phase: GitActionProgressPhase::Commit,
                label: "Committing...".into(),
            })),
            Ok(GitActionProgressEvent::ActionFinished(GitActionProgressEventActionFinished {
                action_id: id,
                cwd,
                action,
                result,
            })),
        ])
        .boxed()
    }

    fn resolve_pull_request(&self, input: GitPullRequestRefInput) -> RpcFuture<GitResolvePullRequestResult> {
        self.record("git.resolvePullRequest");
        ready(Ok(GitResolvePullRequestResult { pull_request: fixture_resolved_pull_request(&input.reference) }))
    }

    fn prepare_pull_request_thread(&self, input: GitPreparePullRequestThreadInput) -> RpcFuture<GitPreparePullRequestThreadResult> {
        let pull_request = fixture_resolved_pull_request(&input.reference);
        self.write(
            "git.preparePullRequestThread",
            GitPreparePullRequestThreadResult {
                branch: pull_request.head_branch.clone(),
                pull_request,
                worktree_path: Some("/home/dev/t3code/.t3/worktrees/pr-418".into()),
                is_on_pull_request_head: Some(true),
            },
        )
    }

    fn pull_request_detail(&self, _: PullRequestRef) -> RpcFuture<PullRequestDetail> {
        self.record("pullRequests.detail");
        ready(self.pull_request.clone().ok_or_else(|| SourceControlError("No pull request".into())))
    }

    fn open_in_editor(&self, _: LaunchEditorInput) -> RpcFuture<()> {
        self.write("shell.openInEditor", ())
    }

    fn mutate_project(&self, _: ProjectMutation) -> RpcFuture<()> {
        self.write("projects.mutate", ())
    }
}

fn fixture_resolved_pull_request(reference: &str) -> GitPreparePullRequestThreadResultPullRequest {
    GitPreparePullRequestThreadResultPullRequest {
        number: reference.trim_start_matches('#').parse().unwrap_or(418),
        title: "feat(web): thread details panel".into(),
        url: "https://github.com/example/t3code/pull/418".into(),
        base_branch: "main".into(),
        head_branch: "feature/thread-details".into(),
        state: PullRequestState::Open,
    }
}

/// A ref list with a current, a default, a remote and a worktree ref.
pub fn fixture_refs() -> Vec<VcsRef> {
    let r = |name: &str, current: bool, default: bool, remote: bool, worktree: Option<&str>| VcsRef {
        name: name.into(),
        is_remote: remote.then_some(true),
        remote_name: remote.then(|| "origin".into()),
        upstream_ref: None,
        current,
        is_default: default,
        worktree_path: worktree.map(Into::into),
    };
    vec![
        r("feature/thread-details", true, false, false, Some("/home/dev/t3code")),
        r("main", false, true, false, None),
        r("fix/branch-picker-keys", false, false, false, Some("/home/dev/t3code/.t3/worktrees/picker")),
        r("feat/commit-dialog", false, false, false, None),
        r("chore/bump-deps", false, false, false, None),
        r("origin/release/0.1.12", false, false, true, None),
        r("origin/feat/remote-open", false, false, true, None),
        r("spike/gpui-port", false, false, false, None),
        r("docs/source-control", false, false, false, None),
    ]
}

/// A status: on `branch`, `dirty` files, `ahead`/`behind` an upstream.
pub fn fixture_status(branch: Option<&str>, dirty: bool, upstream: bool, ahead: u64, behind: u64) -> VcsStatusResult {
    let files = if dirty {
        vec![
            ("apps/web/src/components/git_actions_control.rs", 412, 18),
            ("apps/web/src/components/branch_toolbar.rs", 233, 0),
            ("apps/web/src/scenes/git_actions.rs", 96, 4),
        ]
    } else {
        Vec::new()
    };
    let (insertions, deletions) = files.iter().fold((0, 0), |(i, d), f| (i + f.1, d + f.2));
    VcsStatusResult {
        is_repo: true,
        source_control_provider: Some(SourceControlProviderInfo {
            kind: SourceControlProviderKind::new("github"),
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
        }),
        has_primary_remote: true,
        is_default_ref: branch == Some("main"),
        ref_name: branch.map(Into::into),
        has_working_tree_changes: dirty,
        working_tree: VcsStatusLocalResultWorkingTree {
            root_path: Some("/home/dev/t3code".into()),
            files: files
                .into_iter()
                .map(|(path, insertions, deletions)| VcsStatusLocalResultWorkingTreeFilesItem { path: path.into(), insertions, deletions })
                .collect(),
            insertions,
            deletions,
        },
        branch_changes: Some(VcsStatusResultBranchChanges { base_ref: Some("origin/main".into()), insertions: 0, deletions: 0 }),
        has_upstream: upstream,
        ahead_count: ahead,
        behind_count: behind,
        ahead_of_default_count: Some(ahead),
        pr: None,
    }
}

/// An open pull request on the status's branch.
pub fn fixture_open_pr() -> VcsStatusRemoteResultPr {
    VcsStatusRemoteResultPr {
        number: 418,
        title: "feat(web): thread details panel".into(),
        url: "https://github.com/example/t3code/pull/418".into(),
        base_ref: "main".into(),
        head_ref: "feature/thread-details".into(),
        state: PullRequestState::Open,
        is_draft: Some(false),
        updated_at: None,
    }
}
