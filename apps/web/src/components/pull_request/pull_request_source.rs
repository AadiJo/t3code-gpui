//! What the pull request and issue surfaces read and run, as one trait per
//! surface. Every method is one RPC from `03-rpc-catalog.md` (named on it),
//! with that RPC's payload and success types; streams in the catalog are
//! streams here. The live implementation sends each to the environment that
//! owns the surface; [`super::pull_request_fixture`] answers from wire-shaped
//! fixtures.

use futures::{future::BoxFuture, stream::BoxStream};
use gpui_kit::SharedString;
use t3code_contracts::*;

/// A failed call, with the message the server or transport gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceError(pub SharedString);

/// One unary RPC.
pub type RpcFuture<T> = BoxFuture<'static, Result<T, SourceError>>;
/// One streaming RPC.
pub type RpcStream<T> = BoxStream<'static, Result<T, SourceError>>;

/// The pull requests page: listing, its filter pickers, and row quick actions. One instance
/// per environment; the page fans out over every environment that answers.
pub trait PullRequestListSource: 'static {
    /// `pullRequests.list`.
    fn list(&self, input: PullRequestListInput) -> RpcFuture<PullRequestListResult>;
    /// `pullRequests.listStats`: diff sizes for rows the list read without them.
    fn list_stats(&self, input: PullRequestListStatsInput) -> RpcFuture<PullRequestListStatsResult>;
    /// `pullRequests.filterOptions`: labels, authors and references the host knows.
    fn filter_options(&self, input: PullRequestFilterOptionsInput) -> RpcFuture<PullRequestFilterOptions>;
    /// `pullRequests.runAction`: a row's close, merge, ready or reopen.
    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>>;
    /// `pullRequests.subscribeRefreshes` (stream): a counter that ticks when the host changed.
    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt>;
}

/// One pull request's detail, on the page or in a right-panel tab.
pub trait PullRequestDetailSource: 'static {
    /// `pullRequests.detail`.
    fn detail(&self, input: PullRequestRef) -> RpcFuture<PullRequestDetail>;
    /// `pullRequests.checks`: a fresher checks read laid over the detail.
    fn checks(&self, input: PullRequestRef) -> RpcFuture<Option<PullRequestChecks>>;
    /// `pullRequests.activity`: comments, review threads and commits.
    fn activity(&self, input: PullRequestRef) -> RpcFuture<PullRequestActivity>;
    /// `pullRequests.threadComments`: the rest of a long review thread.
    fn thread_comments(&self, input: PullRequestThreadCommentsInput) -> RpcFuture<PullRequestThreadCommentsResult>;
    /// `pullRequests.stack`.
    fn stack(&self, input: PullRequestRef) -> RpcFuture<Option<PullRequestStack>>;
    /// `pullRequests.linkedThreads`.
    fn linked_threads(&self, input: PullRequestRef) -> RpcFuture<PullRequestLinkedThreadsResult>;
    /// `pullRequests.changeBreakdown`.
    fn change_breakdown(&self, input: PullRequestRef) -> RpcFuture<PullRequestChangeBreakdown>;
    /// `POST /api/pull-requests/diff` (HTTP, `EnvironmentHttpApi pullRequests.diff`): one page of
    /// the unified patch.
    fn diff(&self, input: PullRequestDiffInput) -> RpcFuture<PullRequestDiffResult>;
    /// `pullRequests.diffFileContents`: both sides of one file, for expanding context.
    fn diff_file_contents(&self, input: PullRequestDiffFileContentsInput) -> RpcFuture<PullRequestDiffFileContentsResult>;
    /// `pullRequests.filesViewed`.
    fn files_viewed(&self, input: PullRequestRef) -> RpcFuture<PullRequestFilesViewedResult>;
    /// `pullRequests.setFilesViewed`.
    fn set_files_viewed(&self, input: PullRequestSetFilesViewedInput) -> RpcFuture<()>;
    /// `pullRequests.reviewGuide`.
    fn review_guide(&self, input: PullRequestRef) -> RpcFuture<PullRequestReviewGuideResult>;
    /// `pullRequests.generateReviewGuide`.
    fn generate_review_guide(&self, input: PullRequestRef) -> RpcFuture<PullRequestReviewGuideResult>;
    /// `pullRequests.runAction`: merge, close, reopen, ready, draft, update branch, auto-merge.
    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>>;
    /// `pullRequests.update`: title, description, maintainer edits.
    fn update(&self, input: PullRequestUpdateInput) -> RpcFuture<()>;
    /// `pullRequests.comment`.
    fn comment(&self, input: PullRequestCommentInput) -> RpcFuture<()>;
    /// `pullRequests.updateComment`.
    fn update_comment(&self, input: PullRequestCommentUpdateInput) -> RpcFuture<()>;
    /// `pullRequests.submitReview`: a verdict with its inline comments.
    fn submit_review(&self, input: PullRequestSubmitReviewInput) -> RpcFuture<()>;
    /// `pullRequests.replyToThread`.
    fn reply_to_thread(&self, input: PullRequestThreadReplyInput) -> RpcFuture<()>;
    /// `pullRequests.setThreadResolution`.
    fn set_thread_resolution(&self, input: PullRequestThreadResolutionInput) -> RpcFuture<()>;
    /// `pullRequests.setReaction`.
    fn set_reaction(&self, input: PullRequestReactionInput) -> RpcFuture<()>;
    /// `pullRequests.reviewerCandidates`.
    fn reviewer_candidates(&self, input: PullRequestReviewerCandidatesInput) -> RpcFuture<PullRequestReviewerCandidateList>;
    /// `pullRequests.requestReviewers`.
    fn request_reviewers(&self, input: PullRequestReviewerRequestInput) -> RpcFuture<()>;
    /// `pullRequests.labelCandidates`.
    fn label_candidates(&self, input: PullRequestRef) -> RpcFuture<PullRequestLabelCandidateList>;
    /// `pullRequests.setLabels`.
    fn set_labels(&self, input: PullRequestLabelChangeInput) -> RpcFuture<()>;
    /// `git.preparePullRequestThread`: checks the branch out into a new thread.
    fn prepare_thread(&self, input: GitPreparePullRequestThreadInput) -> RpcFuture<GitPreparePullRequestThreadResult>;
    /// `pullRequests.subscribeRefreshes` (stream).
    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt>;
}

/// The issues page and the issue panel.
pub trait IssueSource: 'static {
    /// `issueTrackers.list`: which trackers are connected.
    fn trackers(&self) -> RpcFuture<IssueTrackerConnectionList>;
    /// `issues.list`.
    fn list(&self, input: IssueListInput) -> RpcFuture<IssueListResult>;
    /// `issues.detail`.
    fn detail(&self, input: IssueRef) -> RpcFuture<IssueDetail>;
    /// `issues.comments`: an older page of comments.
    fn comments(&self, input: IssueCommentsPageInput) -> RpcFuture<IssueCommentsPage>;
    /// `issues.templates`.
    fn templates(&self, input: IssueScopeRef) -> RpcFuture<IssueTemplateList>;
    /// `issues.candidates`: labels and people the picker offers.
    fn candidates(&self, input: IssueScopeRef) -> RpcFuture<IssueCandidates>;
    /// `issues.create`.
    fn create(&self, input: IssueCreateInput) -> RpcFuture<IssueCreateResult>;
    /// `issues.comment`.
    fn comment(&self, input: IssueCommentInput) -> RpcFuture<()>;
    /// `issues.update`: title, body, labels, assignees.
    fn update(&self, input: IssueUpdateInput) -> RpcFuture<()>;
    /// `issues.setState`.
    fn set_state(&self, input: IssueSetStateInput) -> RpcFuture<()>;
    /// `issues.react`.
    fn react(&self, input: IssueReactionInput) -> RpcFuture<()>;
    /// `issues.linkedThreads`.
    fn linked_threads(&self, input: ThreadIssueKey) -> RpcFuture<IssueLinkedThreadsResult>;
    /// `pullRequests.subscribeLinkedThreadRefreshes` (stream): linked threads changed.
    fn subscribe_linked_thread_refreshes(&self) -> RpcStream<NonNegativeInt>;
}

/// A pull request reference from a list row.
pub fn pull_request_ref(entry: &PullRequestListEntry) -> PullRequestRef {
    PullRequestRef {
        project_id: entry.project_id.clone(),
        host: Some(entry.host.to_lowercase()),
        expected_account_id: None,
        allow_stale: None,
        repository: entry.repository.clone(),
        number: entry.number,
    }
}
