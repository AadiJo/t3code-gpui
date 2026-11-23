//! The decisions behind the git actions row. Ports `GitActionsControl.logic.ts`
//! (and `requiresDefaultBranchConfirmation` from `client-runtime/state/gitActions.ts`):
//! which action is primary, what the menu holds and why an item is disabled,
//! progress and result copy, and how a thread's branch follows git.

use gpui_kit::SharedString;
use t3code_contracts::{
    GitRunStackedActionResult, GitRunStackedActionResultBranchStatus, GitStackedAction, PullRequestState,
    VcsStatusResult,
};

use crate::source_control_presentation::{ChangeRequestTerminology, source_control_definition};
use crate::i18n::t;

/// `GitActionIconName`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitActionIcon {
    Commit,
    Push,
    Pr,
}

/// `GitDialogAction`: what choosing a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitDialogAction {
    Commit,
    Push,
    CreatePr,
}

/// `GitActionMenuItem["id"]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitMenuItemId {
    Commit,
    Push,
    Pr,
}

impl GitMenuItemId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Push => "push",
            Self::Pr => "pr",
        }
    }
}

/// One entry of the git actions menu.
#[derive(Clone, Debug, PartialEq)]
pub struct GitActionMenuItem {
    pub id: GitMenuItemId,
    pub label: SharedString,
    pub disabled: bool,
    pub icon: GitActionIcon,
    pub dialog_action: GitDialogAction,
}

/// What the primary button does.
#[derive(Clone, Debug, PartialEq)]
pub enum GitQuickActionKind {
    RunAction(GitStackedAction),
    RunPull,
    OpenPublish,
    ShowHint(SharedString),
}

/// `GitQuickAction`: the primary button's label and behaviour.
#[derive(Clone, Debug, PartialEq)]
pub struct GitQuickAction {
    pub label: SharedString,
    pub disabled: bool,
    pub kind: GitQuickActionKind,
}

impl GitQuickAction {
    /// The TypeScript `kind` string, for comparisons with the original.
    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            GitQuickActionKind::RunAction(_) => "run_action",
            GitQuickActionKind::RunPull => "run_pull",
            GitQuickActionKind::OpenPublish => "open_publish",
            GitQuickActionKind::ShowHint(_) => "show_hint",
        }
    }

    pub fn action(&self) -> Option<GitStackedAction> {
        match self.kind {
            GitQuickActionKind::RunAction(action) => Some(action),
            _ => None,
        }
    }

    pub fn hint(&self) -> Option<&SharedString> {
        match &self.kind {
            GitQuickActionKind::ShowHint(hint) => Some(hint),
            _ => None,
        }
    }
}

/// The operation a running source control action belongs to (`VcsActionOperation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VcsActionOperation {
    RefreshStatus,
    RunChangeRequest,
    Pull,
    SwitchRef,
    CreateRef,
    CreateWorktree,
    Init,
    PublishRepository,
    PreparePullRequestThread,
}

/// The progress of the running action (`VcsActionState`, the fields the row reads).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VcsActionState {
    pub is_running: bool,
    pub operation: Option<VcsActionOperation>,
    pub current_label: Option<SharedString>,
    pub hook_name: Option<SharedString>,
    pub last_output_line: Option<SharedString>,
    pub phase_started_at_ms: Option<i64>,
    pub hook_started_at_ms: Option<i64>,
}

/// `GitActionProgressPresentation`: the row while an action runs.
#[derive(Clone, Debug, PartialEq)]
pub struct GitActionProgressPresentation {
    pub status: SharedString,
    pub output: Option<SharedString>,
    pub started_at_ms: Option<i64>,
}

/// How long a success stays inline.
pub const GIT_ACTION_SUCCESS_VISIBLE_MS: i64 = 10_000;

/// `GitActionResultToastTiming`: errors stay until dismissed, successes leave after a while.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitActionResultToastTiming {
    pub dismiss_after_visible_ms: Option<i64>,
}

/// Whether a toast reports success or failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastOutcome {
    Error,
    Success,
}

/// `resolveGitActionResultToastTiming`.
pub fn resolve_git_action_result_toast_timing(outcome: ToastOutcome) -> GitActionResultToastTiming {
    GitActionResultToastTiming {
        dismiss_after_visible_ms: (outcome == ToastOutcome::Success).then_some(GIT_ACTION_SUCCESS_VISIBLE_MS),
    }
}

/// The label the client writes while a stacked action has no phase yet.
pub const RUN_ACTION_LABEL: &str = "Running source control action";

/// `vcsActionLabelText`: labels the client wrote translate; server phase labels stay.
fn vcs_action_label_text(current_label: &str, hook_name: Option<&str>) -> SharedString {
    if current_label == RUN_ACTION_LABEL {
        return t!("Running source control action");
    }
    if let Some(hook) = hook_name
        && current_label == format!("Running {hook}...")
    {
        return t!("Running {hook}...", hook = hook);
    }
    SharedString::from(current_label.to_owned())
}

/// `resolveGitActionProgressPresentation`.
pub fn resolve_git_action_progress_presentation(state: &VcsActionState) -> Option<GitActionProgressPresentation> {
    let operation = state.operation?;
    if !state.is_running || !matches!(operation, VcsActionOperation::RunChangeRequest | VcsActionOperation::Pull) {
        return None;
    }
    let current_label = state.current_label.as_deref().map(str::trim).filter(|l| !l.is_empty());
    let output = state.last_output_line.as_deref().map(str::trim).filter(|l| !l.is_empty());
    let is_pull = operation == VcsActionOperation::Pull;
    let status = match current_label {
        Some(label) if label != RUN_ACTION_LABEL => vcs_action_label_text(label, state.hook_name.as_deref()),
        _ if is_pull => t!("Pulling latest changes..."),
        _ => t!("Starting source control action..."),
    };
    Some(GitActionProgressPresentation {
        status,
        output: (!is_pull).then(|| output.map(|o| SharedString::from(o.to_owned()))).flatten(),
        started_at_ms: if is_pull { state.phase_started_at_ms } else { state.hook_started_at_ms.or(state.phase_started_at_ms) },
    })
}

/// `formatGitActionElapsed`: "12s", then "1m 5s".
pub fn format_git_action_elapsed(started_at_ms: Option<i64>, now_ms: i64) -> Option<SharedString> {
    let started = started_at_ms?;
    let elapsed = ((now_ms - started) / 1_000).max(0);
    if elapsed < 60 {
        return Some(t!("{seconds}s", seconds = elapsed));
    }
    Some(t!("{minutes}m {seconds}s", minutes = elapsed / 60, seconds = elapsed % 60))
}

/// `changeRequestTerminologyFor`.
pub fn change_request_terminology_for(status: Option<&VcsStatusResult>) -> ChangeRequestTerminology {
    source_control_definition(
        status.and_then(|s| s.source_control_provider.as_ref()).map(|p| p.kind.as_str()),
    )
    .terminology
}

fn has_open_pr(status: &VcsStatusResult) -> bool {
    status.pr.as_ref().is_some_and(|pr| pr.state == PullRequestState::Open)
}

fn ahead_of_default(status: &VcsStatusResult) -> u64 {
    status.ahead_of_default_count.unwrap_or(status.ahead_count)
}

/// `buildMenuItems`.
pub fn build_menu_items(
    status: Option<&VcsStatusResult>,
    is_busy: bool,
    has_primary_remote: bool,
) -> Vec<GitActionMenuItem> {
    let Some(status) = status else { return Vec::new() };
    let term = change_request_terminology_for(Some(status)).term_kind().as_str();
    let has_branch = status.ref_name.is_some();
    let has_open_pr = has_open_pr(status);
    let is_behind = status.behind_count > 0;
    let can_push_without_upstream = has_primary_remote && !status.has_upstream;
    let can_commit = !is_busy && status.has_working_tree_changes;
    let can_push = !is_busy
        && has_branch
        && !is_behind
        && status.ahead_count > 0
        && (status.has_upstream || can_push_without_upstream);
    let can_create_pr = !is_busy
        && has_branch
        && !has_open_pr
        && ahead_of_default(status) > 0
        && !is_behind
        && (status.has_upstream || can_push_without_upstream);

    let commit = GitActionMenuItem {
        id: GitMenuItemId::Commit,
        label: t!("Commit", context = "verb, git action"),
        disabled: !can_commit,
        icon: GitActionIcon::Commit,
        dialog_action: GitDialogAction::Commit,
    };
    if !has_primary_remote {
        return vec![commit];
    }
    let push = GitActionMenuItem {
        id: GitMenuItemId::Push,
        label: t!("Push", context = "verb, git action"),
        disabled: !can_push,
        icon: GitActionIcon::Push,
        dialog_action: GitDialogAction::Push,
    };
    if has_open_pr {
        return vec![commit, push];
    }
    let pr = GitActionMenuItem {
        id: GitMenuItemId::Pr,
        label: t!("Create {term, select, mr {MR} cr {change request} other {PR}}", term = term),
        disabled: !can_create_pr,
        icon: GitActionIcon::Pr,
        dialog_action: GitDialogAction::CreatePr,
    };
    vec![commit, push, pr]
}

/// `getMenuActionDisabledReason`: why a disabled item is disabled.
pub fn get_menu_action_disabled_reason(
    item: &GitActionMenuItem,
    status: Option<&VcsStatusResult>,
    is_busy: bool,
    has_primary_remote: bool,
) -> Option<SharedString> {
    if !item.disabled {
        return None;
    }
    if is_busy {
        return Some(t!("Git action in progress."));
    }
    let Some(status) = status else { return Some(t!("Git status is unavailable.")) };
    let has_branch = status.ref_name.is_some();
    let is_ahead = status.ahead_count > 0;
    let is_behind = status.behind_count > 0;
    let term = change_request_terminology_for(Some(status)).term_kind().as_str();

    Some(match item.id {
        GitMenuItemId::Commit if !status.has_working_tree_changes => {
            t!("Worktree is clean. Make changes before committing.")
        }
        GitMenuItemId::Commit => t!("Commit is currently unavailable."),
        GitMenuItemId::Push if !has_branch => t!("Detached HEAD: check out a branch before pushing."),
        GitMenuItemId::Push if is_behind => t!("Branch is behind upstream. Pull/rebase before pushing."),
        GitMenuItemId::Push if !status.has_upstream && !has_primary_remote => {
            t!("Add an \"origin\" remote before pushing.")
        }
        GitMenuItemId::Push if !is_ahead => t!("No local commits to push."),
        GitMenuItemId::Push => t!("Push is currently unavailable."),
        GitMenuItemId::Pr if !has_branch => t!(
            "Detached HEAD: check out a branch before creating a {term, select, mr {merge request} cr {change request} other {pull request}}.",
            term = term
        ),
        GitMenuItemId::Pr if !status.has_upstream && !has_primary_remote => t!(
            "Add an \"origin\" remote before creating a {term, select, mr {merge request} cr {change request} other {pull request}}.",
            term = term
        ),
        GitMenuItemId::Pr if ahead_of_default(status) == 0 => t!(
            "No local commits to include in a {term, select, mr {merge request} cr {change request} other {pull request}}.",
            term = term
        ),
        GitMenuItemId::Pr if is_behind => t!(
            "Branch is behind upstream. Pull/rebase before creating a {term, select, mr {merge request} cr {change request} other {pull request}}.",
            term = term
        ),
        GitMenuItemId::Pr => t!(
            "Create {term, select, mr {merge request} cr {change request} other {pull request}} is currently unavailable.",
            term = term
        ),
    })
}

/// `hasUsableGitMenuContent`: whether the chevron opens anything.
pub fn has_usable_git_menu_content(
    menu_items: &[GitActionMenuItem],
    can_write_source_control: bool,
    can_publish_repository: bool,
    has_notice: bool,
) -> bool {
    can_publish_repository || has_notice || (can_write_source_control && menu_items.iter().any(|item| !item.disabled))
}

fn hint(label: SharedString, hint: SharedString) -> GitQuickAction {
    GitQuickAction { label, disabled: true, kind: GitQuickActionKind::ShowHint(hint) }
}

fn run(label: SharedString, action: GitStackedAction) -> GitQuickAction {
    GitQuickAction { label, disabled: false, kind: GitQuickActionKind::RunAction(action) }
}

/// `resolveQuickAction`: the primary button for a repository state.
pub fn resolve_quick_action(
    status: Option<&VcsStatusResult>,
    is_busy: bool,
    is_default_ref: bool,
    has_primary_remote: bool,
) -> GitQuickAction {
    let commit_label = || t!("Commit", context = "verb, git action");
    let push_label = || t!("Push", context = "verb, git action");
    if is_busy {
        return hint(commit_label(), t!("Git action in progress."));
    }
    let Some(status) = status else { return hint(commit_label(), t!("Git status is unavailable.")) };

    let has_open_pr = has_open_pr(status);
    let is_ahead = status.ahead_count > 0;
    let has_default_branch_delta = ahead_of_default(status) > 0;
    let is_behind = status.behind_count > 0;
    let term = change_request_terminology_for(Some(status)).term_kind().as_str();
    let up_to_date = || t!("Branch is up to date. No action needed.");
    let push_or_create = || {
        if has_open_pr || is_default_ref {
            run(push_label(), if is_default_ref { GitStackedAction::CommitPush } else { GitStackedAction::Push })
        } else {
            run(
                t!("Push & create {term, select, mr {MR} cr {change request} other {PR}}", term = term),
                GitStackedAction::CreatePr,
            )
        }
    };

    if status.ref_name.is_none() {
        return hint(
            commit_label(),
            t!(
                "Create and checkout a ref before pushing or opening a {term, select, mr {merge request} cr {change request} other {pull request}}.",
                term = term
            ),
        );
    }

    if status.has_working_tree_changes {
        if is_behind || (!status.has_upstream && !has_primary_remote) {
            return run(commit_label(), GitStackedAction::Commit);
        }
        if has_open_pr || is_default_ref {
            return run(t!("Commit & push"), GitStackedAction::CommitPush);
        }
        return run(
            t!("Commit, push & {term, select, mr {MR} cr {change request} other {PR}}", term = term),
            GitStackedAction::CommitPushPr,
        );
    }

    if !status.has_upstream {
        if !has_primary_remote {
            return GitQuickAction { label: t!("Publish repository"), disabled: false, kind: GitQuickActionKind::OpenPublish };
        }
        if !is_ahead {
            return if has_open_pr {
                hint(commit_label(), up_to_date())
            } else {
                hint(push_label(), t!("No local commits to push."))
            };
        }
        return push_or_create();
    }

    if is_ahead && is_behind {
        return hint(t!("Sync ref"), t!("Branch has diverged from upstream. Rebase/merge first."));
    }
    if is_behind {
        return GitQuickAction { label: t!("Pull"), disabled: false, kind: GitQuickActionKind::RunPull };
    }
    if is_ahead {
        return push_or_create();
    }
    if has_open_pr && status.has_upstream {
        return hint(commit_label(), up_to_date());
    }
    if has_default_branch_delta && !is_default_ref {
        return run(
            t!("Create {term, select, mr {MR} cr {change request} other {PR}}", term = term),
            GitStackedAction::CreatePr,
        );
    }
    hint(commit_label(), up_to_date())
}

/// `requiresDefaultBranchConfirmation`: actions that push to the default ref ask first.
pub fn requires_default_branch_confirmation(action: GitStackedAction, is_default_branch: bool) -> bool {
    is_default_branch
        && matches!(
            action,
            GitStackedAction::Push
                | GitStackedAction::CreatePr
                | GitStackedAction::CommitPush
                | GitStackedAction::CommitPushPr
        )
}

/// `DefaultBranchActionDialogCopy`.
#[derive(Clone, Debug, PartialEq)]
pub struct DefaultBranchActionDialogCopy {
    pub title: SharedString,
    pub description: SharedString,
    pub continue_label: SharedString,
}

/// `resolveDefaultBranchActionDialogCopy`. `action` is one of the confirmable actions.
pub fn resolve_default_branch_action_dialog_copy(
    action: GitStackedAction,
    branch: &str,
    includes_commit: bool,
    terminology: ChangeRequestTerminology,
) -> DefaultBranchActionDialogCopy {
    let term = terminology.term_kind().as_str();
    let pushes_only = matches!(action, GitStackedAction::Push | GitStackedAction::CommitPush);
    match (pushes_only, includes_commit) {
        (true, true) => DefaultBranchActionDialogCopy {
            title: t!("Commit & push to default ref?"),
            description: t!(
                "This action will commit and push changes on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
                branch = branch
            ),
            continue_label: t!("Commit & push to {branch}", branch = branch),
        },
        (true, false) => DefaultBranchActionDialogCopy {
            title: t!("Push to default ref?"),
            description: t!(
                "This action will push local commits on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
                branch = branch
            ),
            continue_label: t!("Push to {branch}", branch = branch),
        },
        (false, true) => DefaultBranchActionDialogCopy {
            title: t!(
                "Commit, push & create {term, select, mr {MR} cr {change request} other {PR}} from default ref?",
                term = term
            ),
            description: t!(
                "This action will commit, push, and create a {term, select, mr {merge request} cr {change request} other {pull request}} on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
                term = term,
                branch = branch
            ),
            continue_label: t!("Commit, push & create {term, select, mr {MR} cr {change request} other {PR}}", term = term),
        },
        (false, false) => DefaultBranchActionDialogCopy {
            title: t!("Push & create {term, select, mr {MR} cr {change request} other {PR}} from default ref?", term = term),
            description: t!(
                "This action will push local commits and create a {term, select, mr {merge request} cr {change request} other {pull request}} on \"{branch}\". You can continue on this ref or create a feature ref and run the same action there.",
                term = term,
                branch = branch
            ),
            continue_label: t!("Push & create {term, select, mr {MR} cr {change request} other {PR}}", term = term),
        },
    }
}

/// `resolveFeatureBranchUnavailableReason`: the server names a feature branch
/// from the commit it makes, so the action must commit changes.
pub fn resolve_feature_branch_unavailable_reason(
    action: GitStackedAction,
    branch: &str,
    has_working_tree_changes: bool,
) -> Option<SharedString> {
    if !has_working_tree_changes {
        return Some(t!(
            "There are no changes to commit, so a feature branch can't be named. Create a branch yourself or continue on {branch}.",
            branch = branch
        ));
    }
    if matches!(action, GitStackedAction::Push | GitStackedAction::CreatePr) {
        return Some(t!(
            "This action doesn't commit your changes, so a feature branch can't be named. Create a branch yourself or continue on {branch}.",
            branch = branch
        ));
    }
    None
}

/// `resolveThreadBranchUpdate`: the branch an action created, which the thread adopts.
pub fn resolve_thread_branch_update(result: &GitRunStackedActionResult) -> Option<String> {
    if result.branch.status != GitRunStackedActionResultBranchStatus::Created {
        return None;
    }
    result.branch.name.clone().filter(|name| !name.is_empty())
}

/// The temporary `t3/<hex>` branch a new worktree starts on (`isTemporaryWorktreeBranch`).
pub fn is_temporary_worktree_branch(ref_name: &str) -> bool {
    fn hex(s: &str, len: usize) -> bool {
        s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    fn uuid_v4(s: &str) -> bool {
        let parts: Vec<&str> = s.split('-').collect();
        matches!(parts.as_slice(), [a, b, c, d, e]
            if hex(a, 8) && hex(b, 4) && hex(c, 4) && c.starts_with('4')
                && hex(d, 4) && matches!(d.as_bytes()[0], b'8' | b'9' | b'a' | b'b') && hex(e, 12))
    }
    let name = ref_name.trim().to_lowercase();
    let after = |prefix: &str| name.strip_prefix(prefix);
    after("t3-").or_else(|| after("t3/")).is_some_and(|rest| hex(rest, 8))
        || after("t3code-").or_else(|| after("t3code/")).is_some_and(|rest| hex(rest, 8))
        || after("t3code/").is_some_and(uuid_v4)
}

/// `resolveLiveThreadBranchUpdate`: the branch a draft adopts when git reports
/// another one. `Some(None)` means detached; `None` means no change.
pub fn resolve_live_thread_branch_update(
    thread_branch: Option<&str>,
    status: Option<&VcsStatusResult>,
) -> Option<Option<String>> {
    let status = status?;
    let git_branch = status.ref_name.as_deref();
    if git_branch.is_none() && thread_branch.is_some() {
        return None;
    }
    if thread_branch == git_branch {
        return None;
    }
    if let (Some(thread), Some(git)) = (thread_branch, git_branch)
        && !is_temporary_worktree_branch(thread)
        && is_temporary_worktree_branch(git)
    {
        return None;
    }
    Some(git_branch.map(str::to_owned))
}

/// `ChangesLineTotals` without the per-file breakdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangesLineTotals {
    pub insertions: u64,
    pub deletions: u64,
    /// The ref the counts compare against; `None` means uncommitted changes against HEAD.
    pub base_ref: Option<String>,
}

/// `resolveChangesTotals` from status alone: a zero branch total must not hide
/// a dirty working tree.
pub fn resolve_changes_totals(status: Option<&VcsStatusResult>) -> Option<ChangesLineTotals> {
    let status = status?;
    let tree = ChangesLineTotals {
        insertions: status.working_tree.insertions,
        deletions: status.working_tree.deletions,
        base_ref: None,
    };
    let branch = status.branch_changes.as_ref().map(|b| ChangesLineTotals {
        insertions: b.insertions,
        deletions: b.deletions,
        base_ref: b.base_ref.clone(),
    });
    let nonzero = |t: &ChangesLineTotals| t.insertions > 0 || t.deletions > 0;
    Some(match branch {
        Some(branch) if nonzero(&branch) => branch,
        _ if nonzero(&tree) => tree,
        Some(branch) => branch,
        None => tree,
    })
}
