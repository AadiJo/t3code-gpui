//! Pull request detail logic: the header's primary control and merge method,
//! checks summaries, reviewer verdicts, and the Timeline's conversation.
//! Ports the parts of `pullRequestDetail.logic.ts` and
//! `pullRequestPresentation.tsx` that decide what the detail shows.

use std::sync::{Arc, LazyLock};

use gpui_kit::SharedString;
use regex::Regex;
use t3code_client_runtime::js_sort::{js_sort_by, js_sub_order};
use t3code_client_runtime::pull_request_checks::count_pull_request_checks;
use t3code_client_runtime::pull_request_editing::parse_instant_ms;
use t3code_client_runtime::pull_request_query::js_trim;
use t3code_contracts::{
    IssueCommentReactionsItem, PullRequestAction, PullRequestBaseComparison, PullRequestCheck, PullRequestChecksState,
    PullRequestComment, PullRequestCommentAuthor, PullRequestCommentKind, PullRequestCommit, PullRequestDetail,
    PullRequestDiffSide, PullRequestMergeMethod, PullRequestMergeability, PullRequestReviewThread, PullRequestState,
    PullRequestThreadComment, PullRequestUpdateMethod,
};

use crate::i18n::t;

/// A merge method's menu label.
pub fn merge_method_label(method: PullRequestMergeMethod) -> SharedString {
    match method {
        PullRequestMergeMethod::Merge => t!("Merge"),
        PullRequestMergeMethod::Squash => t!("Squash and merge"),
        PullRequestMergeMethod::Rebase => t!("Rebase and merge"),
        PullRequestMergeMethod::FastForward => t!("Fast-forward merge"),
    }
}

/// Whether the header may merge this one pull request rather than its stack.
pub fn allows_single_pull_request_merge(supports_stack_actions: bool, has_stack: bool, stack_pending: bool, stack_error: bool) -> bool {
    !supports_stack_actions || (!has_stack && !stack_pending && !stack_error)
}

/// The method the merge button uses: the current pick, the project default, then the last used.
pub fn resolve_pull_request_merge_method(
    allowed: &[PullRequestMergeMethod],
    current: Option<PullRequestMergeMethod>,
    project_default: Option<PullRequestMergeMethod>,
    last_selected: PullRequestMergeMethod,
) -> PullRequestMergeMethod {
    [current, project_default, Some(last_selected)]
        .into_iter()
        .flatten()
        .find(|m| allowed.contains(m))
        .or_else(|| allowed.first().copied())
        .unwrap_or(PullRequestMergeMethod::Merge)
}

/// The header's primary button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestPrimaryControl {
    Resolve,
    Ready,
    Merge,
    EnableAutoMerge,
    AutoMergeArmed,
    Merged,
    Closed,
}

impl PullRequestPrimaryControl {
    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Ready => "ready",
            Self::Merge => "merge",
            Self::EnableAutoMerge => "enable-auto-merge",
            Self::AutoMergeArmed => "auto-merge-armed",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// What decides the header's primary control.
#[derive(Debug, Clone, Copy)]
pub struct PrimaryControlInput {
    pub state: PullRequestState,
    pub is_draft: bool,
    pub mergeability: PullRequestMergeability,
    pub checks_state: Option<PullRequestChecksState>,
    pub auto_merge_enabled: Option<bool>,
    pub has_merge_method: bool,
    pub can_merge: bool,
    pub can_mark_ready: bool,
    pub can_enable_auto_merge: bool,
}

/// Picks the header's primary control.
pub fn resolve_pull_request_primary_control(input: PrimaryControlInput) -> Option<PullRequestPrimaryControl> {
    use PullRequestPrimaryControl as C;
    if input.state == PullRequestState::Merged {
        return Some(C::Merged);
    }
    if input.state == PullRequestState::Closed {
        return Some(C::Closed);
    }
    if input.mergeability == PullRequestMergeability::Conflicting {
        return Some(C::Resolve);
    }
    if input.is_draft {
        return input.can_mark_ready.then_some(C::Ready);
    }
    if input.auto_merge_enabled == Some(true) {
        return Some(C::AutoMergeArmed);
    }
    if !input.has_merge_method {
        return None;
    }
    if input.auto_merge_enabled == Some(false)
        && input.checks_state.is_some_and(|s| s != PullRequestChecksState::Passing)
        && input.can_enable_auto_merge
    {
        return Some(C::EnableAutoMerge);
    }
    input.can_merge.then_some(C::Merge)
}

/// Merge methods both the host and this repository allow.
pub fn allowed_pull_request_merge_methods(detail: &PullRequestDetail) -> Vec<PullRequestMergeMethod> {
    let caps = &detail.merge_capabilities;
    detail
        .capabilities
        .merge_methods
        .iter()
        .copied()
        .filter(|m| match m {
            PullRequestMergeMethod::Merge => caps.merge,
            PullRequestMergeMethod::Squash => caps.squash,
            PullRequestMergeMethod::Rebase => caps.rebase,
            PullRequestMergeMethod::FastForward => caps.fast_forward == Some(true),
        })
        .collect()
}

/// Whether the host supports `action` and grants it to the viewer.
pub fn can_perform_pull_request_action(detail: &PullRequestDetail, action: PullRequestAction) -> bool {
    detail.capabilities.actions.contains(&action) && detail.viewer_permissions.actions.contains(&action)
}

/// An open pull request whose branch conflicts with its base.
pub fn is_pull_request_conflicting(detail: &PullRequestDetail) -> bool {
    detail.state == PullRequestState::Open && detail.mergeability == PullRequestMergeability::Conflicting
}

/// Whether the branch can be deleted from the header.
pub fn can_delete_pull_request_branch(detail: &PullRequestDetail) -> bool {
    detail.state != PullRequestState::Open
        && detail.capabilities.delete_branch == Some(true)
        && can_perform_pull_request_action(detail, PullRequestAction::DeleteBranch)
}

/// How far behind its base an open, mergeable branch is, and the update methods offered.
pub fn resolve_base_freshness(detail: &PullRequestDetail) -> Option<(Option<u64>, Vec<PullRequestUpdateMethod>)> {
    if detail.state != PullRequestState::Open || detail.base_comparison != Some(PullRequestBaseComparison::Behind) {
        return None;
    }
    if detail.mergeability != PullRequestMergeability::Mergeable {
        return None;
    }
    let offered = detail.capabilities.update_methods.clone().unwrap_or_default();
    let allowed = detail.viewer_permissions.update_methods.clone().unwrap_or_default();
    Some((detail.behind_by, offered.into_iter().filter(|m| allowed.contains(m)).collect()))
}

/// Whether the detail must be re-read from the host after `action`.
pub fn pull_request_action_needs_host_refresh(action: PullRequestAction) -> bool {
    matches!(action, PullRequestAction::UpdateBranch | PullRequestAction::ApproveWorkflows)
}

/// The checks row's sentence. Ports `describePullRequestChecks`.
pub fn describe_pull_request_checks(checks: &[PullRequestCheck]) -> SharedString {
    let c = count_pull_request_checks(checks);
    if c.total == 0 {
        return t!("No checks reported");
    }
    let (total, pending, failed, passed) = (c.total as f64, c.pending as f64, c.failed as f64, c.passed as f64);
    let action_required = c.action_required as f64;
    let mut parts: Vec<SharedString> = Vec::new();
    if c.pending > 0 {
        parts.push(t!("{pending} of {total} running", pending = pending, total = total));
    }
    if c.action_required > 0 {
        parts.push(t!("{actionRequired} of {total} awaiting action", actionRequired = action_required, total = total));
    }
    if c.failed > 0 {
        parts.push(if parts.is_empty() {
            t!("{failed} of {total} failing", failed = failed, total = total)
        } else {
            t!("{failed} failed", failed = failed)
        });
    }
    if parts.is_empty() {
        return if c.passed == c.total { t!("All checks passed") } else { t!("{passed} of {total} passing", passed = passed, total = total) };
    }
    parts.iter().map(|p| p.as_ref()).collect::<Vec<_>>().join(" · ").into()
}

/// The checks badge's sentence. Ports `summarizePullRequestChecks`.
pub fn summarize_pull_request_checks(checks: &[PullRequestCheck]) -> SharedString {
    let c = count_pull_request_checks(checks);
    let total = c.total as f64;
    if c.total == 0 {
        return t!("No checks reported");
    }
    let workflows = c.workflow_approvals;
    let others = c.action_required - workflows;
    if c.failed > 0 {
        return t!("{failed} of {total} failing", failed = c.failed as f64, total = total);
    }
    if workflows > 0 && others > 0 {
        return t!(
            "{workflows, plural, one {{workflows} workflow} other {{workflows} workflows}} and {checks, plural, one {{checks} check} other {{checks} checks}} awaiting action",
            workflows = workflows as f64,
            checks = others as f64
        );
    }
    if workflows > 0 {
        return t!(
            "{count, plural, one {{count} workflow awaiting approval} other {{count} workflows awaiting approval}}",
            count = workflows as f64
        );
    }
    if others > 0 {
        return t!(
            "{count, plural, one {{count} check awaiting action} other {{count} checks awaiting action}}",
            count = others as f64
        );
    }
    if c.pending > 0 {
        return t!("{pending} of {total} running", pending = c.pending as f64, total = total);
    }
    if c.passed == c.total { t!("All checks passed") } else { t!("{passed} of {total} passing", passed = c.passed as f64, total = total) }
}

/// A reviewer's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestReviewOutcome {
    Approved,
    ChangesRequested,
    Dismissed,
}

impl PullRequestReviewOutcome {
    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes-requested",
            Self::Dismissed => "dismissed",
        }
    }
}

/// Normalizes host verdict spellings; other review states are not verdicts.
pub fn pull_request_review_outcome(review_state: Option<&str>) -> Option<PullRequestReviewOutcome> {
    match js_trim(review_state?).to_lowercase().replace('_', "-").as_str() {
        "approved" => Some(PullRequestReviewOutcome::Approved),
        "changes-requested" => Some(PullRequestReviewOutcome::ChangesRequested),
        "dismissed" => Some(PullRequestReviewOutcome::Dismissed),
        _ => None,
    }
}

/// The newest commit's date as the host wrote it.
pub fn newest_pull_request_commit_at(commits: &[PullRequestCommit]) -> Option<&str> {
    let mut newest = None;
    let mut newest_at = f64::NEG_INFINITY;
    for commit in commits {
        let Some(at) = parse_instant_ms(&commit.committed_date) else { continue };
        if at <= newest_at {
            continue;
        }
        newest = Some(commit.committed_date.as_str());
        newest_at = at;
    }
    newest
}

/// A verdict given before the newest commit.
pub fn is_pull_request_verdict_stale(at: &str, newest_commit_at: Option<&str>) -> bool {
    match (parse_instant_ms(at), newest_commit_at.and_then(parse_instant_ms)) {
        (Some(v), Some(c)) => v < c,
        _ => false,
    }
}

/// One reviewer's latest verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewOutcomeEntry {
    pub key: String,
    pub actor: Option<PullRequestCommentAuthor>,
    pub outcome: PullRequestReviewOutcome,
    pub at: String,
    pub stale: bool,
}

/// Each reviewer's latest verdict, dismissed ones left out.
pub fn latest_pull_request_review_outcomes(comments: &[PullRequestComment], commits: &[PullRequestCommit]) -> Vec<ReviewOutcomeEntry> {
    let newest = newest_pull_request_commit_at(commits);
    let mut latest: Vec<ReviewOutcomeEntry> = Vec::new();
    for comment in comments {
        let Some(outcome) = pull_request_review_outcome(comment.review_state.as_deref()) else { continue };
        let login = comment.author.as_ref().map_or_else(|| format!("ghost:{}", comment.id), |a| a.login.clone());
        let entry = ReviewOutcomeEntry {
            key: login.clone(),
            actor: comment.author.clone(),
            outcome,
            at: comment.created_at.clone(),
            stale: is_pull_request_verdict_stale(&comment.created_at, newest),
        };
        match latest.iter_mut().find(|e| e.key == login) {
            // Hosts can return reviews out of order.
            Some(current) => {
                let newer = match (parse_instant_ms(&current.at), parse_instant_ms(&comment.created_at)) {
                    (Some(a), Some(b)) => a <= b,
                    _ => true,
                };
                if newer {
                    *current = entry;
                }
            }
            None => latest.push(entry),
        }
    }
    latest.into_iter().filter(|e| e.outcome != PullRequestReviewOutcome::Dismissed).collect()
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());

/// Marker-only bodies are empty; otherwise the trimmed text, including fenced HTML comments.
pub fn visible_body(body: &str) -> Option<String> {
    let stripped = HTML_COMMENT.replace_all(body, "");
    (!js_trim(&stripped).is_empty()).then(|| js_trim(body).to_owned())
}

/// A review thread from the conversation: an inline one on a line, or an overview one.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationThread {
    pub id: String,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub side: PullRequestDiffSide,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub comments: Vec<PullRequestThreadComment>,
    pub comment_count: Option<u64>,
    pub next_comments_cursor: Option<String>,
    pub can_resolve: Option<bool>,
}

impl From<&PullRequestReviewThread> for ConversationThread {
    fn from(t: &PullRequestReviewThread) -> Self {
        Self {
            id: t.id.clone(),
            path: Some(t.path.clone()),
            line: t.line,
            side: t.side,
            is_resolved: t.is_resolved,
            is_outdated: t.is_outdated,
            comments: t.comments.clone(),
            comment_count: t.comment_count,
            next_comments_cursor: t.next_comments_cursor.clone(),
            can_resolve: t.can_resolve,
        }
    }
}

/// A loose comment, or a review thread with its root and replies.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversationItem {
    Comment(PullRequestComment),
    Thread { root: PullRequestComment, replies: Vec<PullRequestComment>, thread: Arc<ConversationThread>, more_replies: u64, has_more_replies: bool, url: Option<String> },
}

impl ConversationItem {
    /// When the item started.
    pub fn created_at(&self) -> &str {
        match self {
            Self::Comment(c) => &c.created_at,
            Self::Thread { root, .. } => &root.created_at,
        }
    }

    /// Every comment the item holds.
    pub fn comments(&self) -> Vec<&PullRequestComment> {
        match self {
            Self::Comment(c) => vec![c],
            Self::Thread { root, replies, .. } => std::iter::once(root).chain(replies).collect(),
        }
    }
}

fn instant(iso: &str) -> f64 {
    parse_instant_ms(iso).unwrap_or(f64::NAN)
}


/// Threads woven into the loose comments at their roots' times. Ports `pullRequestConversationItems`.
pub fn pull_request_conversation_items(comments: &[PullRequestComment], threads: &[ConversationThread]) -> Vec<ConversationItem> {
    let mut claimed = std::collections::HashSet::new();
    let mut items_threads: Vec<ConversationItem> = Vec::new();
    for thread in threads {
        // Keep the conversation's objects for editing and reactions; some hosts only include a
        // comment in the thread, so normalize that copy when the flat conversation lacks it.
        let mut ordered: Vec<PullRequestComment> = thread
            .comments
            .iter()
            .map(|tc| {
                comments.iter().find(|c| c.id == tc.id).cloned().unwrap_or_else(|| PullRequestComment {
                    id: tc.id.clone(),
                    kind: PullRequestCommentKind::ReviewComment,
                    author: tc.author.as_ref().map(|a| PullRequestCommentAuthor {
                        is_bot: a.is_bot,
                        login: a.login.clone(),
                        name: a.name.clone(),
                        avatar_url: a.avatar_url.clone(),
                    }),
                    body: tc.body.clone(),
                    created_at: tc.created_at.clone(),
                    edited_at: tc.edited_at.clone(),
                    url: tc.url.clone(),
                    path: thread.path.clone(),
                    review_state: None,
                    reactions: tc.reactions.clone(),
                    can_edit: tc.can_edit,
                })
            })
            .collect();
        js_sort_by(&mut ordered, |l, r| js_sub_order(instant(&l.created_at), instant(&r.created_at)));
        if ordered.is_empty() {
            continue;
        }
        for c in &ordered {
            claimed.insert(c.id.clone());
        }
        let more_replies = thread.comment_count.unwrap_or(ordered.len() as u64).saturating_sub(ordered.len() as u64);
        let url = ordered.iter().find_map(|c| c.url.clone());
        let root = ordered.remove(0);
        items_threads.push(ConversationItem::Thread {
            root,
            replies: ordered,
            thread: Arc::new(thread.clone()),
            more_replies,
            has_more_replies: more_replies > 0 || thread.next_comments_cursor.is_some(),
            url,
        });
    }
    js_sort_by(&mut items_threads, |l, r| js_sub_order(instant(l.created_at()), instant(r.created_at())));
    let mut items = Vec::new();
    let mut threads_iter = items_threads.into_iter().peekable();
    // Preserve loose comments' existing order, inserting each thread at its finding's time.
    for comment in comments {
        if claimed.contains(&comment.id) {
            continue;
        }
        while let Some(next) = threads_iter.peek() {
            if instant(next.created_at()) < instant(&comment.created_at) {
                items.push(threads_iter.next().unwrap());
            } else {
                break;
            }
        }
        items.push(ConversationItem::Comment(comment.clone()));
    }
    items.extend(threads_iter);
    items
}

/// A resolved thread's comment, or a dismissed review.
pub fn is_pull_request_comment_finished(comment: &PullRequestComment, thread: Option<&ConversationThread>) -> bool {
    thread.is_some_and(|t| t.is_resolved)
        || pull_request_review_outcome(comment.review_state.as_deref()) == Some(PullRequestReviewOutcome::Dismissed)
}

/// Items with any member passing the bot and resolved filters.
pub fn filter_pull_request_conversation_items(items: &[ConversationItem], bots: bool, resolved: bool) -> Vec<ConversationItem> {
    items
        .iter()
        .filter(|item| {
            let thread = match item {
                ConversationItem::Thread { thread, .. } => Some(thread.as_ref()),
                ConversationItem::Comment(_) => None,
            };
            // Matching any member keeps the finding beside a human reply to a bot.
            item.comments().into_iter().any(|c| {
                let verdict = matches!(
                    pull_request_review_outcome(c.review_state.as_deref()),
                    Some(PullRequestReviewOutcome::Approved | PullRequestReviewOutcome::ChangesRequested)
                );
                // A bot's approval or change request is a verdict, not chatter.
                (bots || c.author.as_ref().and_then(|a| a.is_bot) != Some(true) || verdict)
                    && (resolved || !is_pull_request_comment_finished(c, thread))
            })
        })
        .cloned()
        .collect()
}

/// What a timeline event is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineEventKind {
    Opened,
    Commit,
    Comment,
    Review,
    Merged,
    Closed,
}

/// A review thread hanging off a timeline event.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineThread {
    pub thread: Arc<ConversationThread>,
    pub replies: Vec<TimelineEvent>,
    pub more_replies: u64,
    pub has_more_replies: bool,
    pub url: Option<String>,
}

/// One row of the Timeline tab.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineEvent {
    pub id: String,
    pub at: String,
    pub kind: TimelineEventKind,
    pub body: Option<String>,
    pub url: Option<String>,
    pub actor: Option<PullRequestCommentAuthor>,
    pub commit: Option<PullRequestCommit>,
    pub path: Option<String>,
    pub review_state: Option<String>,
    pub reactions: Vec<IssueCommentReactionsItem>,
    pub comment: Option<PullRequestComment>,
    pub thread: Option<TimelineThread>,
}

fn comment_event(comment: &PullRequestComment) -> TimelineEvent {
    TimelineEvent {
        id: comment.id.clone(),
        at: comment.created_at.clone(),
        kind: if comment.kind == PullRequestCommentKind::Review { TimelineEventKind::Review } else { TimelineEventKind::Comment },
        body: visible_body(&comment.body),
        url: comment.url.clone(),
        actor: comment.author.clone(),
        commit: None,
        path: comment.path.clone(),
        review_state: comment.review_state.clone(),
        reactions: comment.reactions.clone().unwrap_or_default(),
        comment: Some(comment.clone()),
        thread: None,
    }
}

fn bare_event(id: &str, at: &str, kind: TimelineEventKind, actor: Option<PullRequestCommentAuthor>) -> TimelineEvent {
    TimelineEvent {
        id: id.into(),
        at: at.into(),
        kind,
        body: None,
        url: None,
        actor,
        commit: None,
        path: None,
        review_state: None,
        reactions: vec![],
        comment: None,
        thread: None,
    }
}

/// What the Timeline builds from: the detail, and its activity read.
#[derive(Debug, Clone)]
pub struct TimelineSource<'a> {
    pub created_at: &'a str,
    pub author: Option<PullRequestCommentAuthor>,
    pub commits: &'a [PullRequestCommit],
    pub comments: &'a [PullRequestComment],
    pub threads: &'a [ConversationThread],
    pub merged_at: Option<&'a str>,
    pub closed_at: Option<&'a str>,
}

/// Every event newest first. Ports `buildPullRequestTimeline`.
pub fn build_pull_request_timeline(source: TimelineSource<'_>) -> Vec<TimelineEvent> {
    let mut events = vec![bare_event("created", source.created_at, TimelineEventKind::Opened, source.author.clone())];
    for commit in source.commits {
        let author = commit.authors.as_ref().and_then(|a| a.first()).map(|a| PullRequestCommentAuthor {
            is_bot: a.is_bot,
            login: a.login.clone(),
            name: a.name.clone(),
            avatar_url: a.avatar_url.clone(),
        });
        let mut event = bare_event(&commit.oid, &commit.committed_date, TimelineEventKind::Commit, author);
        event.body = (!commit.message_headline.is_empty()).then(|| commit.message_headline.clone());
        event.commit = Some(commit.clone());
        events.push(event);
    }
    for item in pull_request_conversation_items(source.comments, source.threads) {
        match item {
            ConversationItem::Comment(c) => events.push(comment_event(&c)),
            ConversationItem::Thread { root, replies, thread, more_replies, has_more_replies, url } => {
                let mut event = comment_event(&root);
                event.thread = Some(TimelineThread { thread, replies: replies.iter().map(comment_event).collect(), more_replies, has_more_replies, url });
                events.push(event);
            }
        }
    }
    if let Some(at) = source.merged_at.filter(|s| !s.is_empty()) {
        events.push(bare_event("merged", at, TimelineEventKind::Merged, None));
    }
    if let Some(at) = source.closed_at.filter(|s| !s.is_empty())
        && source.merged_at.is_none_or(str::is_empty)
    {
        events.push(bare_event("closed", at, TimelineEventKind::Closed, None));
    }
    let key = |e: &TimelineEvent| parse_instant_ms(&e.at).filter(|v| *v != 0.0).unwrap_or(0.0);
    js_sort_by(&mut events, |l, r| js_sub_order(key(r), key(l)));
    events
}

/// A Timeline row: one event, or a run of plain comments shown together.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineRow {
    Event(TimelineEvent),
    Comments(Vec<TimelineEvent>),
}

/// Folds runs of plain comments into one row.
pub fn group_pull_request_timeline_conversations(events: Vec<TimelineEvent>) -> Vec<TimelineRow> {
    let mut rows: Vec<TimelineRow> = Vec::new();
    for event in events {
        let plain = matches!(event.kind, TimelineEventKind::Comment | TimelineEventKind::Review)
            && event.thread.is_none()
            && pull_request_review_outcome(event.review_state.as_deref()).is_none();
        if plain {
            if let Some(TimelineRow::Comments(run)) = rows.last_mut() {
                run.push(event);
                continue;
            }
            rows.push(TimelineRow::Comments(vec![event]));
        } else {
            rows.push(TimelineRow::Event(event));
        }
    }
    rows
}

/// Comments in the order the reader picked.
pub fn order_pull_request_comments<T: Clone>(comments: &[T], newest_first: bool) -> Vec<T> {
    let mut out = comments.to_vec();
    if newest_first {
        out.reverse();
    }
    out
}

/// The thread panel's one suggested next step for its pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPanelPullRequestAction {
    Resolve,
    Ready,
    Fix,
    Merge,
}

impl ThreadPanelPullRequestAction {
    /// The TypeScript name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolve => "resolve",
            Self::Ready => "ready",
            Self::Fix => "fix",
            Self::Merge => "merge",
        }
    }
}

/// Ports `resolveThreadPanelPullRequestAction`.
pub fn resolve_thread_panel_pull_request_action(detail: &PullRequestDetail) -> Option<ThreadPanelPullRequestAction> {
    use t3code_client_runtime::pull_request_checks::{ChecksClass, classify_pull_request_checks};
    if detail.state != PullRequestState::Open {
        return None;
    }
    if is_pull_request_conflicting(detail) {
        return Some(ThreadPanelPullRequestAction::Resolve);
    }
    if detail.is_draft {
        return can_perform_pull_request_action(detail, PullRequestAction::Ready).then_some(ThreadPanelPullRequestAction::Ready);
    }
    match classify_pull_request_checks(&detail.checks) {
        ChecksClass::Failing => Some(ThreadPanelPullRequestAction::Fix),
        ChecksClass::Pending => None,
        _ => (can_perform_pull_request_action(detail, PullRequestAction::Merge) && !allowed_pull_request_merge_methods(detail).is_empty())
            .then_some(ThreadPanelPullRequestAction::Merge),
    }
}

/// A guide stop's subline: its file count, or that it has no code.
pub fn stop_subline(file_count: usize) -> SharedString {
    if file_count == 0 {
        t!("Context, no code")
    } else {
        t!("{count, plural, one {{count} file} other {{count} files}}", count = file_count as f64)
    }
}
