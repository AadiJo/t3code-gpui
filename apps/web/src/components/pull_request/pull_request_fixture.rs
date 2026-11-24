//! Fixture data for the pull request and issue surfaces: wire-shaped JSON
//! decoded through the contract types, served by [`FixtureSource`], which
//! implements every source trait, applies writes to its state so a scripted
//! run sees their effect, and records each RPC it was asked for.

use std::sync::{Arc, Mutex};

use futures::{FutureExt as _, StreamExt as _, stream};
use serde::Deserialize;
use t3code_contracts::*;

use super::pull_request_source::{IssueSource, PullRequestDetailSource, PullRequestListSource, RpcFuture, RpcStream, SourceError};

/// Every fixture value, as the wire carries it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fixtures {
    pub list_studio: PullRequestListResult,
    pub list_devbox: PullRequestListResult,
    pub detail: PullRequestDetail,
    pub activity: PullRequestActivity,
    pub guide: PullRequestReviewGuideResult,
    pub files_viewed: PullRequestFilesViewedResult,
    pub stack: PullRequestStack,
    pub reviewer_candidates: PullRequestReviewerCandidateList,
    pub label_candidates: PullRequestLabelCandidateList,
    pub issue_list: IssueListResult,
    pub issue_detail: IssueDetail,
    pub templates: IssueTemplateList,
    pub candidates: IssueCandidates,
    pub trackers: IssueTrackerConnectionList,
}

/// The fixture file decoded through the contract types.
pub fn fixtures() -> Fixtures {
    serde_json::from_str(include_str!("fixtures.json")).expect("fixtures decode as wire values")
}

/// One RPC the fixture answered, for transcripts.
#[derive(Clone, Debug)]
pub struct RecordedCall {
    pub method: &'static str,
    pub input: String,
}

/// State the fixture serves and mutates.
pub struct FixtureState {
    pub data: Fixtures,
    pub patch: String,
    /// Answer `pullRequests.list` with an error, to show a failing environment.
    pub fail_list: bool,
    pub calls: Vec<RecordedCall>,
}

/// Serves fixtures for every surface. Clone it to share one state between sources.
#[derive(Clone)]
pub struct FixtureSource {
    pub state: Arc<Mutex<FixtureState>>,
}

impl FixtureSource {
    /// A source over `data` and a patch for the Code tab.
    pub fn new(data: Fixtures, patch: String) -> Self {
        Self { state: Arc::new(Mutex::new(FixtureState { data, patch, fail_list: false, calls: Vec::new() })) }
    }

    /// The default fixtures with the default patch.
    pub fn standard() -> Self {
        Self::new(fixtures(), fixture_patch())
    }

    /// A source whose list answer is `list`, failing first when `fail` is set.
    pub fn with_list(&self, list: PullRequestListResult, fail: bool) -> Self {
        let data = { self.state.lock().unwrap().data.clone() };
        let patch = { self.state.lock().unwrap().patch.clone() };
        let source = Self::new(Fixtures { list_studio: list, ..data }, patch);
        source.state.lock().unwrap().fail_list = fail;
        source
    }

    /// Every call so far.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.state.lock().unwrap().calls.clone()
    }

    fn record(&self, method: &'static str, input: &impl serde::Serialize) {
        let input = serde_json::to_string(input).unwrap_or_default();
        self.state.lock().unwrap().calls.push(RecordedCall { method, input });
    }

    fn answer<T: Send + 'static>(&self, method: &'static str, input: &impl serde::Serialize, read: impl FnOnce(&mut FixtureState) -> T) -> RpcFuture<T> {
        self.record(method, input);
        let value = read(&mut self.state.lock().unwrap());
        async move { Ok(value) }.boxed()
    }
}

impl PullRequestListSource for FixtureSource {
    fn list(&self, input: PullRequestListInput) -> RpcFuture<PullRequestListResult> {
        self.record("pullRequests.list", &input);
        let mut state = self.state.lock().unwrap();
        if state.fail_list {
            state.fail_list = false;
            return async { Err(SourceError("Connection to devbox timed out.".into())) }.boxed();
        }
        let value = state.data.list_studio.clone();
        async move { Ok(value) }.boxed()
    }

    fn list_stats(&self, input: PullRequestListStatsInput) -> RpcFuture<PullRequestListStatsResult> {
        self.answer("pullRequests.listStats", &input, |_| PullRequestListStatsResult {
            stats: input
                .refs
                .iter()
                .map(|r| PullRequestDiffStat { project_id: r.project_id.clone(), repository: r.repository.clone(), number: r.number, additions: 12, deletions: 3 })
                .collect(),
        })
    }

    fn filter_options(&self, input: PullRequestFilterOptionsInput) -> RpcFuture<PullRequestFilterOptions> {
        self.answer("pullRequests.filterOptions", &input, |_| PullRequestFilterOptions { labels: vec![], authors: vec![], references: None, truncated: false })
    }

    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>> {
        self.answer("pullRequests.runAction", &input, |state| {
            let entry = state.data.list_studio.entries.iter_mut().find(|e| e.number == input.number)?;
            apply_action_to_entry(entry, input.action);
            None
        })
    }

    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt> {
        self.record("pullRequests.subscribeRefreshes", &());
        stream::iter([Ok(0)]).boxed()
    }
}

fn apply_action_to_entry(entry: &mut PullRequestListEntry, action: PullRequestAction) {
    match action {
        PullRequestAction::Merge => entry.state = PullRequestState::Merged,
        PullRequestAction::Close => entry.state = PullRequestState::Closed,
        PullRequestAction::Reopen => entry.state = PullRequestState::Open,
        PullRequestAction::Ready => entry.is_draft = false,
        PullRequestAction::Draft => entry.is_draft = true,
        _ => {}
    }
}

impl PullRequestDetailSource for FixtureSource {
    fn detail(&self, input: PullRequestRef) -> RpcFuture<PullRequestDetail> {
        self.answer("pullRequests.detail", &input, |s| s.data.detail.clone())
    }

    fn checks(&self, input: PullRequestRef) -> RpcFuture<Option<PullRequestChecks>> {
        self.answer("pullRequests.checks", &input, |s| {
            Some(PullRequestChecks { observed_at: s.data.detail.observed_at, state: s.data.detail.state, checks: s.data.detail.checks.clone() })
        })
    }

    fn activity(&self, input: PullRequestRef) -> RpcFuture<PullRequestActivity> {
        self.answer("pullRequests.activity", &input, |s| s.data.activity.clone())
    }

    fn thread_comments(&self, input: PullRequestThreadCommentsInput) -> RpcFuture<PullRequestThreadCommentsResult> {
        self.answer("pullRequests.threadComments", &input, |_| PullRequestThreadCommentsResult { comments: vec![], next_cursor: None, hidden_count: None })
    }

    fn stack(&self, input: PullRequestRef) -> RpcFuture<Option<PullRequestStack>> {
        self.answer("pullRequests.stack", &input, |s| Some(s.data.stack.clone()))
    }

    fn linked_threads(&self, input: PullRequestRef) -> RpcFuture<PullRequestLinkedThreadsResult> {
        self.answer("pullRequests.linkedThreads", &input, |_| PullRequestLinkedThreadsResult { threads: vec![] })
    }

    fn change_breakdown(&self, input: PullRequestRef) -> RpcFuture<PullRequestChangeBreakdown> {
        self.answer("pullRequests.changeBreakdown", &input, |_| None)
    }

    fn diff(&self, input: PullRequestDiffInput) -> RpcFuture<PullRequestDiffResult> {
        self.answer("POST /api/pull-requests/diff", &input, |s| PullRequestDiffResult {
            patch: s.patch.clone(),
            truncated: false,
            next_cursor: None,
            omitted_file_stats: None,
            withheld_files_readable: None,
        })
    }

    fn diff_file_contents(&self, input: PullRequestDiffFileContentsInput) -> RpcFuture<PullRequestDiffFileContentsResult> {
        self.answer("pullRequests.diffFileContents", &input, |_| PullRequestDiffFileContentsResult { old_contents: String::new(), new_contents: String::new() })
    }

    fn files_viewed(&self, input: PullRequestRef) -> RpcFuture<PullRequestFilesViewedResult> {
        self.answer("pullRequests.filesViewed", &input, |s| s.data.files_viewed.clone())
    }

    fn set_files_viewed(&self, input: PullRequestSetFilesViewedInput) -> RpcFuture<()> {
        self.answer("pullRequests.setFilesViewed", &input, |s| {
            for file in &input.files {
                let state = if file.viewed { PullRequestFileViewedState::Viewed } else { PullRequestFileViewedState::Unviewed };
                match s.data.files_viewed.files.iter_mut().find(|f| f.path == file.path) {
                    Some(held) => held.state = state,
                    None => s.data.files_viewed.files.push(PullRequestFileViewed { path: file.path.clone(), state }),
                }
            }
        })
    }

    fn review_guide(&self, input: PullRequestRef) -> RpcFuture<PullRequestReviewGuideResult> {
        self.answer("pullRequests.reviewGuide", &input, |s| s.data.guide.clone())
    }

    fn generate_review_guide(&self, input: PullRequestRef) -> RpcFuture<PullRequestReviewGuideResult> {
        self.answer("pullRequests.generateReviewGuide", &input, |s| s.data.guide.clone())
    }

    fn run_action(&self, input: PullRequestActionInput) -> RpcFuture<Option<PullRequestSummary>> {
        self.answer("pullRequests.runAction", &input, |s| {
            let detail = &mut s.data.detail;
            match input.action {
                PullRequestAction::Merge => {
                    detail.state = PullRequestState::Merged;
                    detail.merged_at = Some("2026-11-23T10:00:00Z".into());
                    detail.merged_by = Some(PullRequestDetailMergedBy { is_bot: None, login: "sam".into(), name: None, avatar_url: None });
                }
                PullRequestAction::Close => {
                    detail.state = PullRequestState::Closed;
                    detail.closed_at = Some("2026-11-23T10:00:00Z".into());
                }
                PullRequestAction::Reopen => {
                    detail.state = PullRequestState::Open;
                    detail.closed_at = None;
                }
                PullRequestAction::Ready => detail.is_draft = false,
                PullRequestAction::Draft => detail.is_draft = true,
                PullRequestAction::UpdateBranch => {
                    detail.base_comparison = Some(PullRequestBaseComparison::UpToDate);
                    detail.behind_by = Some(0);
                }
                _ => {}
            }
            None
        })
    }

    fn update(&self, input: PullRequestUpdateInput) -> RpcFuture<()> {
        self.answer("pullRequests.update", &input, |s| {
            if let Some(title) = &input.title {
                s.data.detail.title = title.clone();
            }
            if let Some(body) = &input.body {
                s.data.detail.body = body.clone();
            }
        })
    }

    fn comment(&self, input: PullRequestCommentInput) -> RpcFuture<()> {
        self.answer("pullRequests.comment", &input, |s| {
            let id = format!("new{}", s.data.activity.comments.len());
            s.data.activity.comments.push(PullRequestComment {
                id,
                kind: PullRequestCommentKind::IssueComment,
                author: Some(PullRequestCommentAuthor { is_bot: None, login: "sam".into(), name: None, avatar_url: None }),
                body: input.body.clone(),
                created_at: "2026-11-23T10:00:00Z".into(),
                edited_at: None,
                url: None,
                path: None,
                review_state: None,
                reactions: None,
                can_edit: Some(true),
            });
        })
    }

    fn update_comment(&self, input: PullRequestCommentUpdateInput) -> RpcFuture<()> {
        self.answer("pullRequests.updateComment", &input, |s| {
            if let Some(c) = s.data.activity.comments.iter_mut().find(|c| c.id == input.comment_id) {
                c.body = input.body.clone();
                c.edited_at = Some("2026-11-23T10:00:00Z".into());
            }
        })
    }

    fn submit_review(&self, input: PullRequestSubmitReviewInput) -> RpcFuture<()> {
        self.answer("pullRequests.submitReview", &input, |_| ())
    }

    fn reply_to_thread(&self, input: PullRequestThreadReplyInput) -> RpcFuture<()> {
        self.answer("pullRequests.replyToThread", &input, |s| {
            let Some(thread) = s.data.activity.review_threads.iter_mut().find(|t| t.id == input.thread_id) else { return };
            let id = format!("reply{}", thread.comments.len());
            thread.comments.push(PullRequestThreadComment {
                id: id.clone(),
                author: Some(PullRequestThreadCommentAuthor { is_bot: None, login: "sam".into(), name: None, avatar_url: None }),
                body: input.body.clone(),
                created_at: "2026-11-23T10:00:00Z".into(),
                edited_at: None,
                url: None,
                review_url: None,
                reactions: None,
                can_edit: Some(true),
            });
            s.data.activity.comments.push(PullRequestComment {
                id,
                kind: PullRequestCommentKind::ReviewComment,
                author: Some(PullRequestCommentAuthor { is_bot: None, login: "sam".into(), name: None, avatar_url: None }),
                body: input.body.clone(),
                created_at: "2026-11-23T10:00:00Z".into(),
                edited_at: None,
                url: None,
                path: Some(thread.path.clone()),
                review_state: None,
                reactions: None,
                can_edit: Some(true),
            });
        })
    }

    fn set_thread_resolution(&self, input: PullRequestThreadResolutionInput) -> RpcFuture<()> {
        self.answer("pullRequests.setThreadResolution", &input, |s| {
            if let Some(t) = s.data.activity.review_threads.iter_mut().find(|t| t.id == input.thread_id) {
                t.is_resolved = input.resolved;
            }
        })
    }

    fn set_reaction(&self, input: PullRequestReactionInput) -> RpcFuture<()> {
        self.answer("pullRequests.setReaction", &input, |s| {
            let content = IssueCommentReactionsItemContent::from_wire(input.content.as_str()).expect("same reaction set");
            let reactions = match &input.subject_id {
                Some(id) => match s.data.activity.comments.iter_mut().find(|c| &c.id == id) {
                    Some(c) => c.reactions.get_or_insert_with(Vec::new),
                    None => return,
                },
                None => s.data.activity.reactions.get_or_insert_with(Vec::new),
            };
            match reactions.iter_mut().position(|r| r.content == content) {
                Some(i) => {
                    let r = &mut reactions[i];
                    if r.viewer_has_reacted != input.reacted {
                        r.viewer_has_reacted = input.reacted;
                        if input.reacted { r.count += 1 } else { r.count -= 1 }
                    }
                    if r.count == 0 {
                        reactions.remove(i);
                    }
                }
                None if input.reacted => reactions.push(IssueCommentReactionsItem { content, count: 1, actors: vec![], viewer_has_reacted: true }),
                None => {}
            }
        })
    }

    fn reviewer_candidates(&self, input: PullRequestReviewerCandidatesInput) -> RpcFuture<PullRequestReviewerCandidateList> {
        self.answer("pullRequests.reviewerCandidates", &input, |s| s.data.reviewer_candidates.clone())
    }

    fn request_reviewers(&self, input: PullRequestReviewerRequestInput) -> RpcFuture<()> {
        self.answer("pullRequests.requestReviewers", &input, |_| ())
    }

    fn label_candidates(&self, input: PullRequestRef) -> RpcFuture<PullRequestLabelCandidateList> {
        self.answer("pullRequests.labelCandidates", &input, |s| s.data.label_candidates.clone())
    }

    fn set_labels(&self, input: PullRequestLabelChangeInput) -> RpcFuture<()> {
        self.answer("pullRequests.setLabels", &input, |s| {
            for name in &input.labels {
                let labels = &mut s.data.detail.labels;
                if input.applied {
                    if !labels.iter().any(|l| &l.name == name) {
                        labels.push(PullRequestDetailLabelsItem { name: name.clone(), color: None, description: None });
                    }
                } else {
                    labels.retain(|l| &l.name != name);
                }
            }
        })
    }

    fn prepare_thread(&self, input: GitPreparePullRequestThreadInput) -> RpcFuture<GitPreparePullRequestThreadResult> {
        self.record("git.preparePullRequestThread", &input);
        async { Err(SourceError("Checkout is not available in fixtures.".into())) }.boxed()
    }

    fn subscribe_refreshes(&self) -> RpcStream<NonNegativeInt> {
        self.record("pullRequests.subscribeRefreshes", &());
        stream::iter([Ok(0)]).boxed()
    }
}

impl IssueSource for FixtureSource {
    fn trackers(&self) -> RpcFuture<IssueTrackerConnectionList> {
        self.answer("issueTrackers.list", &(), |s| s.data.trackers.clone())
    }

    fn list(&self, input: IssueListInput) -> RpcFuture<IssueListResult> {
        self.answer("issues.list", &input, |s| s.data.issue_list.clone())
    }

    fn detail(&self, input: IssueRef) -> RpcFuture<IssueDetail> {
        self.answer("issues.detail", &input, |s| s.data.issue_detail.clone())
    }

    fn comments(&self, input: IssueCommentsPageInput) -> RpcFuture<IssueCommentsPage> {
        self.answer("issues.comments", &input, |_| IssueCommentsPage { comments: vec![], next_cursor: None })
    }

    fn templates(&self, input: IssueScopeRef) -> RpcFuture<IssueTemplateList> {
        self.answer("issues.templates", &input, |s| s.data.templates.clone())
    }

    fn candidates(&self, input: IssueScopeRef) -> RpcFuture<IssueCandidates> {
        self.answer("issues.candidates", &input, |s| s.data.candidates.clone())
    }

    fn create(&self, input: IssueCreateInput) -> RpcFuture<IssueCreateResult> {
        self.answer("issues.create", &input, |_| IssueCreateResult {
            key: "460".into(),
            display_key: "#460".into(),
            url: "https://github.com/t3tools/t3code/issues/460".into(),
        })
    }

    fn comment(&self, input: IssueCommentInput) -> RpcFuture<()> {
        self.answer("issues.comment", &input, |s| {
            s.data.issue_detail.comments.push(IssueComment {
                id: format!("new{}", s.data.issue_detail.comments.len()),
                author: Some(IssueCommentAuthor { is_bot: None, login: "sam".into(), name: None, avatar_url: None }),
                body: input.body.clone(),
                created_at: "2026-11-23T10:00:00Z".into(),
                url: None,
                reactions: None,
            });
        })
    }

    fn update(&self, input: IssueUpdateInput) -> RpcFuture<()> {
        self.answer("issues.update", &input, |s| {
            if let Some(labels) = &input.labels {
                s.data.issue_detail.labels = labels.iter().map(|n| IssueDetailLabelsItem { name: n.clone(), color: None, description: None }).collect();
            }
        })
    }

    fn set_state(&self, input: IssueSetStateInput) -> RpcFuture<()> {
        self.answer("issues.setState", &input, |s| {
            if let Some(state) = s.data.issue_detail.states.iter().find(|st| st.name == input.state).cloned() {
                s.data.issue_detail.state = state;
            }
        })
    }

    fn react(&self, input: IssueReactionInput) -> RpcFuture<()> {
        self.answer("issues.react", &input, |_| ())
    }

    fn linked_threads(&self, input: ThreadIssueKey) -> RpcFuture<IssueLinkedThreadsResult> {
        self.answer("issues.linkedThreads", &input, |_| IssueLinkedThreadsResult { threads: vec![] })
    }

    fn subscribe_linked_thread_refreshes(&self) -> RpcStream<NonNegativeInt> {
        self.record("pullRequests.subscribeLinkedThreadRefreshes", &());
        stream::iter([Ok(0)]).boxed()
    }
}

/// A list answer with `count` generated rows, for the performance check.
pub fn large_list(base: &PullRequestListResult, count: usize) -> PullRequestListResult {
    const WORDS: [&str; 16] =
        ["sidebar", "composer", "terminal", "diff", "settings", "perf", "crash", "scroll", "theme", "mobile", "server", "review", "guide", "stack", "issue", "relay"];
    let template = base.entries[0].clone();
    let entries = (0..count)
        .map(|i| {
            let mut e = template.clone();
            e.number = 10_000 + i as u64;
            e.title = format!("{} {} {} #{i}", WORDS[i % 16], WORDS[(i / 16) % 16], WORDS[(i * 7) % 16]);
            e.url = format!("https://github.com/t3tools/t3code/pull/{}", e.number);
            e.head_branch = format!("dev/{}-{i}", WORDS[(i * 3) % 16]);
            e.updated_at = format!("2026-{:02}-{:02}T{:02}:00:00Z", 1 + i % 11, 1 + i % 28, i % 24);
            e.state = [PullRequestState::Open, PullRequestState::Open, PullRequestState::Merged, PullRequestState::Closed][i % 4];
            e.is_draft = i % 9 == 0;
            e.stack = None;
            e
        })
        .collect();
    PullRequestListResult { entries, ..base.clone() }
}

fn file_patch(path: &str, old: &[&str], new: &[&str], at: usize) -> String {
    let mut out = format!("diff --git a/{path} b/{path}\nindex 1111111..2222222 100644\n--- a/{path}\n+++ b/{path}\n");
    out += &format!("@@ -{at},{} +{at},{} @@\n", old.len() + 2, new.len() + 2);
    out += " // context above\n";
    for line in old {
        out += &format!("-{line}\n");
    }
    for line in new {
        out += &format!("+{line}\n");
    }
    out += " // context below\n";
    out
}

/// The Code tab's patch for the fixture pull request.
pub fn fixture_patch() -> String {
    let mut patch = String::new();
    patch += "diff --git a/apps/web/src/components/pull_request/pull_request_list_logic.rs b/apps/web/src/components/pull_request/pull_request_list_logic.rs\nindex 1111111..2222222 100644\n--- a/apps/web/src/components/pull_request/pull_request_list_logic.rs\n+++ b/apps/web/src/components/pull_request/pull_request_list_logic.rs\n@@ -1,6 +1,14 @@\n //! The pull requests page's list logic.\n \n-pub struct PullRequestRow { entry: PullRequestListEntry }\n+/// A list row with what filtering reads, built once.\n+pub struct PullRequestRow {\n+    pub entry: PullRequestListEntry,\n+    /// Lower-cased search text.\n+    pub haystack: String,\n+    pub title_lower: String,\n+    pub updated_ms: Option<f64>,\n+    pub created_ms: Option<f64>,\n+}\n \n impl PullRequestRow {\n     pub fn matches(&self, needle: &str) -> bool {\n-        self.entry.title.to_lowercase().contains(needle)\n+        self.haystack.contains(needle)\n     }\n";
    patch += "diff --git a/apps/web/src/components/pull_request/pull_requests_page.rs b/apps/web/src/components/pull_request/pull_requests_page.rs\nnew file mode 100644\nindex 0000000..3333333\n--- /dev/null\n+++ b/apps/web/src/components/pull_request/pull_requests_page.rs\n@@ -0,0 +1,8 @@\n+//! The pull requests page.\n+\n+pub fn render_rows(rows: &[Row]) -> impl IntoElement {\n+    uniform_list(\"pull-requests\", rows.len(), |range, _, _| {\n+        range.map(render_row).collect()\n+    })\n+    .track_scroll(handle)\n+}\n";
    patch += &file_patch("apps/web/src/components/pull_request/mod.rs", &["pub mod pull_request_list;"], &["pub mod pull_request_list_logic;", "pub mod pull_requests_page;"], 3);
    patch += &file_patch("packages/client-runtime/src/pull_request_query.rs", &["let lower = value.to_lowercase();"], &["let lower = js_trim(value).to_lowercase();"], 40);
    patch += &file_patch("packages/client-runtime/src/lib.rs", &[], &["pub mod pull_request_query;"], 10);
    patch += &file_patch("docs/user/pull-requests.md", &["Search matches titles."], &["Search matches titles, branches, authors and repositories.", "Typed qualifiers such as `label:bug` become filters."], 12);
    patch += &file_patch("apps/web/src/components/pull_request/pull_request_row.rs", &["div().h(px(56.))"], &["div().h(px(ROW_HEIGHT))"], 20);
    patch += &file_patch("apps/web/src/components/pull_request/pull_request_stack.rs", &["// stack column"], &["/// The stack column: position of size.", "pub fn stack_label(m: &Membership) -> String { format!(\"{}/{}\", m.position, m.size) }"], 1);
    patch += &file_patch("apps/web/src/components/pull_request/pull_request_list_logic_tests.rs", &["assert!(true);"], &["assert_eq!(rank(&rows, \"fix\")[0].number, 482);"], 5);
    patch
}

/// A patch of `files` files, each a few hunks, for the Code tab performance check.
pub fn large_patch(files: usize) -> String {
    let mut patch = String::new();
    for i in 0..files {
        let path = format!("src/generated/module_{i:03}.rs");
        let old: Vec<String> = (0..6).map(|l| format!("    let value_{l} = compute({l}, {i});")).collect();
        let new: Vec<String> = (0..8).map(|l| format!("    let value_{l} = compute_cached({l}, {i}).unwrap_or_default();")).collect();
        let old: Vec<&str> = old.iter().map(String::as_str).collect();
        let new: Vec<&str> = new.iter().map(String::as_str).collect();
        patch += &file_patch(&path, &old, &new, 10 + i % 40);
    }
    patch
}
