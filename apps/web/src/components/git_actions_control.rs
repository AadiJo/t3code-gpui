//! The git rows of the thread details card. Ports `GitActionsControl.tsx`:
//! a split row whose primary half runs the action [`resolve_quick_action`]
//! picks and whose chevron opens commit, push and create-PR; progress and an
//! inline success replace the row while an action runs and after it lands;
//! the Changes row; the commit dialog; and the default-branch confirmation.

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, TestSupportExt as _, Window, component::input::TextareaState, div, prelude::*, px,
};
use t3code_contracts::{
    GitRunStackedActionInput, GitRunStackedActionResultPrStatus, GitStackedAction, ProjectId, ThreadId,
    VcsInitInput, VcsPullInput, VcsPullResultStatus, VcsStatusResult,
};

use super::chat::thread_details_panel_styles::{
    DismissPress, row_height, row_icon_size, row_width, below_row, observed, row, row_base, row_busy_fill, row_icon, split_row, split_secondary,
};
use super::git_actions_control_logic::{
    GitActionIcon, GitActionMenuItem, GitDialogAction, GitQuickAction, GitQuickActionKind, ToastOutcome,
    VcsActionOperation, build_menu_items, change_request_terminology_for, format_git_action_elapsed,
    get_menu_action_disabled_reason, has_usable_git_menu_content, requires_default_branch_confirmation,
    resolve_changes_totals, resolve_default_branch_action_dialog_copy, resolve_feature_branch_unavailable_reason,
    resolve_git_action_progress_presentation, resolve_git_action_result_toast_timing, resolve_quick_action,
    resolve_thread_branch_update, GIT_ACTION_SUCCESS_VISIBLE_MS,
};
use super::git_actions_control_publish::{PublishRepositoryDialog, PublishRepositoryEvent};
use super::git_actions_control_source::{InlineSuccess, SourceControlError, SourceControlModel, now_ms};
use super::icons::{Icon, IconName, IconSize, IconSource};
use super::ui::button::{Button, ButtonSize, ButtonVariant};
use super::ui::checkbox::{CheckState, Checkbox};
use super::ui::dialog::{DialogSurface, Modal};
use super::ui::input::TextArea;
use super::ui::popover::glass_surface;
use super::ui::spinner::{Spinner, SpinnerSize};
use super::ui::toast::{ToastData, ToastKind, Toaster};
use super::ui::tooltip::tooltip;
use super::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::appearance_fonts::{TextSize, mono_font, weight};
use crate::i18n::t;
use crate::source_control_presentation::source_control_presentation;
use crate::theme_color::with_alpha;
use crate::{ActiveAppTheme as _, AppTheme};

/// Who is asking and what the thread allows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GitActionsProps {
    /// The session holds `source-control:write`.
    pub can_write_source_control: bool,
    /// The session may change the thread's branch (`canChangeThreadBranch`).
    pub can_change_thread_branch: bool,
    /// A fresh draft or a thread with no turns: the rows stay hidden until there is work.
    pub quiet_when_idle: bool,
    /// The card is short: only progress and successes show.
    pub compact: bool,
    pub show_actions: bool,
    pub show_changes: bool,
    pub thread_id: Option<ThreadId>,
    pub project_id: Option<ProjectId>,
    /// The server's `offeredSourceControlProviders`; unset offers GitHub only.
    pub offered_source_control_providers: Option<std::collections::BTreeMap<String, bool>>,
}

/// What the rows ask their owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum GitActionsEvent {
    /// Open the diff panel (the Changes row).
    OpenChanges,
    /// An action created a branch; the thread adopts it.
    ThreadBranchChanged(String),
    /// Open a link (the published repository's "Open on" button).
    OpenUrl(String),
    /// Open Settings > Source control & issues ("Setup Required").
    OpenSourceControlSettings,
}

/// One action waiting on the default-branch confirmation (`PendingDefaultBranchAction`).
#[derive(Clone, Debug, PartialEq)]
struct PendingDefaultBranchAction {
    action: GitStackedAction,
    branch: String,
    includes_commit: bool,
    commit_message: Option<String>,
    file_paths: Option<Vec<String>>,
}

/// The commit dialog while it is open.
struct CommitDialog {
    message: Entity<TextareaState>,
    excluded: HashSet<String>,
    editing_files: bool,
}

/// What [`GitActionsControl::run_action`] runs.
#[derive(Clone, Debug, Default)]
struct RunInput {
    commit_message: Option<String>,
    skip_default_branch_prompt: bool,
    feature_branch: bool,
    file_paths: Option<Vec<String>>,
}

/// The git action rows for one checkout.
pub struct GitActionsControl {
    model: Entity<SourceControlModel>,
    props: GitActionsProps,
    menu_open: bool,
    commit_dialog: Option<CommitDialog>,
    pending_default: Option<PendingDefaultBranchAction>,
    dismissed: DismissPress,
    /// A message from a commit that failed, offered again when the dialog reopens.
    restore_message: Option<String>,
    /// Re-renders once a second while an action runs, for the elapsed time.
    ticker: Option<Task<()>>,
    /// The Publish repository dialog while it is open.
    publish: Option<(Entity<PublishRepositoryDialog>, Subscription)>,
}

impl EventEmitter<GitActionsEvent> for GitActionsControl {}

/// `formatDiffCount`: grouped thousands.
pub fn format_diff_count(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `GitActionItemIcon`: the pull request item draws the host's mark.
fn action_icon(icon: GitActionIcon, host: IconSource) -> IconSource {
    match icon {
        GitActionIcon::Commit => IconName::GitCommitHorizontal.into(),
        GitActionIcon::Push => IconName::CloudUpload.into(),
        GitActionIcon::Pr => host,
    }
}

/// `GitQuickActionIcon`: a hint has no action, so it matches the label; a pull
/// request action draws the host's mark.
fn quick_action_icon(quick: &GitQuickAction, host: IconSource) -> IconSource {
    if let GitQuickActionKind::RunAction(action) = &quick.kind
        && !matches!(action, GitStackedAction::Commit | GitStackedAction::Push | GitStackedAction::CommitPush)
    {
        return host;
    }
    let name = match &quick.kind {
        GitQuickActionKind::OpenPublish => IconName::CloudUpload,
        GitQuickActionKind::RunPull => IconName::CloudDownload,
        GitQuickActionKind::RunAction(GitStackedAction::Commit) => IconName::GitCommitHorizontal,
        GitQuickActionKind::RunAction(GitStackedAction::Push | GitStackedAction::CommitPush) => IconName::CloudUpload,
        GitQuickActionKind::RunAction(_) => IconName::GitPullRequestArrow,
        GitQuickActionKind::ShowHint(_) if quick.label == t!("Commit", context = "verb, git action") => {
            IconName::GitCommitHorizontal
        }
        GitQuickActionKind::ShowHint(_) if quick.label == t!("Push", context = "verb, git action") => IconName::CloudUpload,
        GitQuickActionKind::ShowHint(_) => IconName::Info,
    };
    name.into()
}

/// A failed action's toast; it stays until dismissed (`timeout: 0`).
fn error_toast(title: SharedString, error: &SourceControlError, cx: &mut App) {
    let timing = resolve_git_action_result_toast_timing(ToastOutcome::Error);
    let description = if error.0.is_empty() { t!("An error occurred.") } else { error.0.clone() };
    let toast = ToastData::new(ToastKind::Error, title).description(description);
    let toast = match timing.dismiss_after_visible_ms {
        None => toast.persistent(),
        Some(ms) => toast.timeout(Duration::from_millis(ms as u64)),
    };
    Toaster::push(toast, cx);
}

impl GitActionsControl {
    pub fn new(model: Entity<SourceControlModel>, props: GitActionsProps, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model, props, menu_open: false, commit_dialog: None, pending_default: None, dismissed: DismissPress::default(), restore_message: None, ticker: None, publish: None }
    }

    pub fn model(&self) -> &Entity<SourceControlModel> {
        &self.model
    }

    pub fn props(&self) -> &GitActionsProps {
        &self.props
    }

    pub fn set_props(&mut self, props: GitActionsProps, cx: &mut Context<Self>) {
        if self.props != props {
            self.props = props;
            cx.notify();
        }
    }

    /// Opens the actions menu (also what the chevron does).
    pub fn set_menu_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.menu_open = open;
        if open {
            self.refresh_status(cx);
        }
        cx.notify();
    }

    /// Opens the commit dialog, as the menu's Commit item does.
    pub fn open_commit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore = self.restore_message.take().unwrap_or_default();
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .placeholder(t!("Leave empty to auto-generate"))
                .default_value(restore)
        });
        self.commit_dialog = Some(CommitDialog { message, excluded: HashSet::new(), editing_files: false });
        self.menu_open = false;
        cx.notify();
    }

    fn close_commit_dialog(&mut self, cx: &mut Context<Self>) {
        self.commit_dialog = None;
        cx.notify();
    }

    fn refresh_status(&self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let request = model.source.refresh_status(t3code_contracts::VcsRefreshStatusPayload {
            cwd: model.cwd.to_string(),
            local_only: None,
        });
        cx.background_spawn(async move {
            let _ = request.await;
        })
        .detach();
    }

    fn status(&self, cx: &App) -> Option<VcsStatusResult> {
        self.model.read(cx).status.clone()
    }

    /// `runGitActionWithToast`.
    fn run_action(&mut self, action: GitStackedAction, input: RunInput, cx: &mut Context<Self>) {
        if !self.props.can_write_source_control || (input.feature_branch && !self.props.can_change_thread_branch) {
            return;
        }
        let status = self.status(cx);
        let branch = status.as_ref().and_then(|s| s.ref_name.clone());
        let is_default = !input.feature_branch && status.as_ref().is_some_and(|s| s.is_default_ref);
        let commits = matches!(action, GitStackedAction::Commit | GitStackedAction::CommitPush | GitStackedAction::CommitPushPr);
        let includes_commit = commits
            && (action == GitStackedAction::Commit
                || status.as_ref().is_some_and(|s| s.has_working_tree_changes)
                || input.feature_branch);
        if !input.skip_default_branch_prompt && requires_default_branch_confirmation(action, is_default) {
            if let Some(branch) = branch {
                self.pending_default = Some(PendingDefaultBranchAction {
                    action,
                    branch,
                    includes_commit,
                    commit_message: input.commit_message,
                    file_paths: input.file_paths,
                });
                cx.notify();
                return;
            }
        }
        let terminology = change_request_terminology_for(status.as_ref());
        let request = GitRunStackedActionInput {
            action_id: String::new(),
            cwd: String::new(),
            action,
            commit_message: input.commit_message.clone(),
            feature_branch: input.feature_branch.then_some(true),
            file_paths: input.file_paths,
            thread_id: self.props.thread_id.clone(),
            project_id: self.props.project_id.clone(),
        };
        let task = self.model.update(cx, |model, cx| model.run_stacked_action(request, cx));
        let model = self.model.clone();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            cx.update(|cx| {
                let committed = match result {
                    Err(error) => {
                        model.update(cx, |m, cx| m.finish(None, cx));
                        error_toast(t!("Action failed"), &error, cx);
                        false
                    }
                    Ok(result) => {
                        if let Some(branch) = resolve_thread_branch_update(&result) {
                            this.update(cx, |_, cx| cx.emit(GitActionsEvent::ThreadBranchChanged(branch))).ok();
                        }
                        let created = result.pr.status == GitRunStackedActionResultPrStatus::Created;
                        let success = InlineSuccess {
                            title: if created {
                                t!("Created {term, select, mr {MR} cr {change request} other {PR}}", term = terminology.term_kind().as_str())
                            } else {
                                result.toast.title.clone().into()
                            },
                            description: if created { None } else { result.toast.description.clone().map(Into::into) },
                            expires_at_ms: now_ms() + GIT_ACTION_SUCCESS_VISIBLE_MS,
                        };
                        model.update(cx, |m, cx| m.finish(Some(success), cx));
                        true
                    }
                };
                // Keep a typed message for the retry unless the user already started a new one.
                if !committed && let Some(message) = input.commit_message {
                    this.update(cx, |this, _| {
                        this.restore_message.get_or_insert(message);
                    })
                    .ok();
                }
            })
        })
        .detach();
        self.ensure_ticker(cx);
    }

    /// The Pull quick action (`vcs.pull`).
    fn run_pull(&mut self, cx: &mut Context<Self>) {
        let (source, cwd) = {
            let model = self.model.read(cx);
            (model.source.clone(), model.cwd.to_string())
        };
        self.model.update(cx, |m, cx| m.begin(VcsActionOperation::Pull, cx));
        let request = source.pull(VcsPullInput { cwd });
        let model = self.model.clone();
        cx.spawn(async move |_, cx| {
            let result = request.await;
            cx.update(|cx| match result {
                Err(error) => {
                    model.update(cx, |m, cx| m.finish(None, cx));
                    error_toast(t!("Pull failed"), &error, cx);
                }
                Ok(pull) => {
                    let pulled = pull.status == VcsPullResultStatus::Pulled;
                    let description = match (pulled, &pull.upstream_ref) {
                        (true, Some(upstream)) => {
                            t!("Updated {ref} from {upstream}", r#ref = pull.ref_name.as_str(), upstream = upstream.as_str())
                        }
                        (true, None) => t!("Updated {ref} from upstream", r#ref = pull.ref_name.as_str()),
                        (false, _) => t!("{ref} is already synchronized.", r#ref = pull.ref_name.as_str()),
                    };
                    let success = InlineSuccess {
                        title: if pulled { t!("Pulled") } else { t!("Already up to date") },
                        description: Some(description),
                        expires_at_ms: now_ms() + GIT_ACTION_SUCCESS_VISIBLE_MS,
                    };
                    model.update(cx, |m, cx| m.finish(Some(success), cx));
                }
            })
        })
        .detach();
        self.ensure_ticker(cx);
    }

    fn init_git(&mut self, cx: &mut Context<Self>) {
        let (source, cwd) = {
            let model = self.model.read(cx);
            (model.source.clone(), model.cwd.to_string())
        };
        self.model.update(cx, |m, cx| m.begin(VcsActionOperation::Init, cx));
        let request = source.init(VcsInitInput { cwd, kind: None });
        let model = self.model.clone();
        cx.spawn(async move |_, cx| {
            let result = request.await;
            cx.update(|cx| {
                model.update(cx, |m, cx| m.finish(None, cx));
                if let Err(error) = result {
                    let description = if error.0.is_empty() { t!("An error occurred.") } else { error.0.clone() };
                    Toaster::push(ToastData::new(ToastKind::Error, t!("Git initialization failed")).description(description), cx);
                }
            })
        })
        .detach();
    }

    /// Opens the Publish repository dialog (the quick action and the menu item).
    pub fn open_publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_open = false;
        let (source, cwd) = {
            let model = self.model.read(cx);
            (model.source.clone(), model.cwd.clone())
        };
        let offered = self.props.offered_source_control_providers.clone();
        let dialog = cx.new(|cx| PublishRepositoryDialog::new(source, cwd, offered, window, cx));
        let subscription = cx.subscribe(&dialog, |this, _, event: &PublishRepositoryEvent, cx| {
            match event {
                PublishRepositoryEvent::Close => this.publish = None,
                PublishRepositoryEvent::OpenSourceControlSettings => {
                    this.publish = None;
                    cx.emit(GitActionsEvent::OpenSourceControlSettings);
                }
                PublishRepositoryEvent::OpenUrl(url) => cx.emit(GitActionsEvent::OpenUrl(url.clone())),
                PublishRepositoryEvent::Published => this.refresh_status(cx),
            }
            cx.notify();
        });
        self.publish = Some((dialog, subscription));
        cx.notify();
    }

    fn run_quick_action(&mut self, quick: GitQuickAction, window: &mut Window, cx: &mut Context<Self>) {
        match quick.kind {
            GitQuickActionKind::OpenPublish => self.open_publish(window, cx),
            GitQuickActionKind::RunPull => self.run_pull(cx),
            GitQuickActionKind::ShowHint(hint) => {
                Toaster::push(ToastData::new(ToastKind::Info, quick.label).description(hint), cx);
            }
            GitQuickActionKind::RunAction(action) => self.run_action(action, RunInput::default(), cx),
        }
    }

    fn choose_menu_item(&mut self, item: &GitActionMenuItem, window: &mut Window, cx: &mut Context<Self>) {
        if item.disabled {
            return;
        }
        self.menu_open = false;
        match item.dialog_action {
            GitDialogAction::Push => self.run_action(GitStackedAction::Push, RunInput::default(), cx),
            GitDialogAction::CreatePr => self.run_action(GitStackedAction::CreatePr, RunInput::default(), cx),
            GitDialogAction::Commit => self.open_commit_dialog(window, cx),
        }
        cx.notify();
    }

    /// The dialog's Commit or "Commit on new branch".
    fn commit_from_dialog(&mut self, feature_branch: bool, cx: &mut Context<Self>) {
        let Some(dialog) = self.commit_dialog.take() else { return };
        let message = dialog.message.read(cx).value().trim().to_owned();
        let files: Vec<String> =
            self.status(cx).map(|s| s.working_tree.files.into_iter().map(|f| f.path).collect()).unwrap_or_default();
        let selected: Vec<String> = files.into_iter().filter(|p| !dialog.excluded.contains(p)).collect();
        let input = RunInput {
            commit_message: (!message.is_empty()).then_some(message),
            skip_default_branch_prompt: feature_branch,
            feature_branch,
            file_paths: (!dialog.excluded.is_empty()).then_some(selected),
        };
        self.run_action(GitStackedAction::Commit, input, cx);
        cx.notify();
    }

    fn continue_pending(&mut self, feature_branch: bool, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_default.take() else { return };
        self.run_action(
            pending.action,
            RunInput {
                commit_message: pending.commit_message,
                skip_default_branch_prompt: true,
                feature_branch,
                file_paths: pending.file_paths,
            },
            cx,
        );
        cx.notify();
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |this, cx| {
                        let now = now_ms();
                        let model = this.model.read(cx);
                        let live = model.action.is_running || model.visible_success(now).is_some();
                        cx.notify();
                        live
                    })
                    .unwrap_or(false);
                if !keep {
                    this.update(cx, |this, _| this.ticker = None).ok();
                    break;
                }
            }
        }));
    }

    fn render_menu(
        &self,
        items: &[GitActionMenuItem],
        status: Option<&VcsStatusResult>,
        is_busy: bool,
        can_publish: bool,
        width: gpui_kit::Pixels,
        theme: &AppTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let has_primary_remote = status.is_some_and(|s| s.has_primary_remote);
        let term = change_request_terminology_for(status).term_kind().as_str();
        let host_icon = source_control_presentation(status.and_then(|s| s.source_control_provider.as_ref())).icon;
        let mut rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let reason = get_menu_action_disabled_reason(item, status, is_busy, has_primary_remote);
                let disabled = item.disabled || !self.props.can_write_source_control;
                let item_for_click = item.clone();
                let highlight = c.highlight();
                div()
                    .id(("git-menu-item", index))
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .min_h(theme.sp(7.))
                    .px(theme.sp(2.))
                    .py(theme.sp(1.))
                    .rounded(theme.radii.sm)
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(div().mx(px(-2.)).child(Icon::new(action_icon(item.icon, host_icon)).size(IconSize::Custom(theme.rem(1.))).color(c.muted_foreground).opacity(0.8)))
                    .child(div().flex_1().min_w_0().truncate().child(item.label.clone()))
                    .when(disabled, |r| r.opacity(DISABLED_OPACITY))
                    .when_some(reason.filter(|_| item.disabled), |r, reason| r.tooltip(tooltip(reason)))
                    .when(!disabled, |r| {
                        r.cursor_pointer().hover(move |s| s.bg(highlight)).on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_menu_item(&item_for_click, window, cx)
                        }))
                    })
                    .test_support()
                    .into_any_element()
            })
            .collect();
        if can_publish {
            let highlight = c.highlight();
            rows.push(
                div()
                    .id("git-menu-publish")
                    .flex()
                    .items_center()
                    .gap(theme.sp(2.))
                    .min_h(theme.sp(7.))
                    .px(theme.sp(2.))
                    .rounded(theme.radii.sm)
                    .ui_text(theme, TextSize::Sm, weight::NORMAL)
                    .child(Icon::new(IconName::CloudUpload).size(IconSize::Custom(theme.rem(1.))).color(c.muted_foreground).opacity(0.8))
                    .child(t!("Publish repository..."))
                    .when(is_busy, |r| r.opacity(DISABLED_OPACITY))
                    .when(!is_busy, |r| {
                        r.cursor_pointer().hover(move |s| s.bg(highlight)).on_click(cx.listener(|this, _, window, cx| {
                            this.open_publish(window, cx);
                        }))
                    })
                    .into_any_element(),
            );
        }
        let notice = |text: SharedString, color| {
            div().px(theme.sp(2.)).py(theme.sp(1.5)).ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(color).child(text).into_any_element()
        };
        if let Some(s) = status {
            if s.ref_name.is_none() {
                rows.push(notice(
                    t!(
                        "Detached HEAD: create and check out a branch to enable push and {term, select, mr {merge request} cr {change request} other {pull request}} actions.",
                        term = term
                    ),
                    c.warning,
                ));
            } else if !s.has_working_tree_changes && s.behind_count > 0 && s.ahead_count == 0 {
                rows.push(notice(t!("Behind upstream. Pull/rebase first."), c.warning));
            }
        }
        if let Some(error) = self.model.read(cx).status_error.clone() {
            rows.push(notice(error, c.destructive()));
        }
        let close = cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
            this.menu_open = false;
            this.dismissed.record(event.position);
            cx.notify();
        });
        below_row(
            glass_surface(theme)
                .id("git-actions-menu")
                .occlude()
                .w(width)
                .p(theme.sp(1.))
                .on_mouse_down_out(close)
                .children(rows),
        )
    }

    fn render_changes_row(&self, status: Option<&VcsStatusResult>, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let totals = resolve_changes_totals(status);
        let (insertions, deletions) = totals.as_ref().map_or((0, 0), |t| (t.insertions, t.deletions));
        let base_label = totals.as_ref().map(|t| match &t.base_ref {
            None => t!("Uncommitted"),
            Some(base) => t!("vs {ref}", r#ref = base.as_str()),
        });
        let f = &theme.fixed;
        let muted = theme.colors.muted_foreground;
        row_base("git-changes-row", theme, false, false)
            .w_full()
            .child(row_icon(theme, IconName::FileDiff))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_baseline()
                    .gap(theme.rem(0.375))
                    .child(div().flex_none().child(t!("Changes")))
                    .children(base_label.map(|label| {
                        div().min_w_0().truncate().font_weight(weight::NORMAL).text_color(muted).child(label)
                    }))
                    .child(
                        div()
                            .ml_auto()
                            .flex()
                            .flex_none()
                            .items_baseline()
                            .gap(theme.rem(0.5))
                            .pl(theme.rem(0.5))
                            .font(mono_font(weight::MEDIUM))
                            .child(div().text_color(if insertions > 0 { f.diff_addition_foreground } else { muted }).child(format!("+{}", format_diff_count(insertions))))
                            .child(div().text_color(if deletions > 0 { f.diff_deletion_foreground } else { muted }).child(format!("-{}", format_diff_count(deletions)))),
                    ),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(GitActionsEvent::OpenChanges)))
            .test_support()
            .into_any_element()
    }

    fn render_commit_dialog(&self, dialog: &CommitDialog, status: Option<&VcsStatusResult>, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let files = status.map(|s| s.working_tree.files.clone()).unwrap_or_default();
        let selected: Vec<_> = files.iter().filter(|f| !dialog.excluded.contains(&f.path)).collect();
        let all_selected = dialog.excluded.is_empty();
        let none_selected = selected.is_empty();
        let is_default = status.is_some_and(|s| s.is_default_ref);
        let muted = c.muted_foreground;
        let f = &theme.fixed;
        let card_fill = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.035) } else { c.card() };
        let ring = if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.05) } else { gpui_kit::hsla(0., 0., 0., 0.05) };
        let file_rows = files.iter().enumerate().map(|(i, file)| {
            let excluded = dialog.excluded.contains(&file.path);
            let path = file.path.clone();
            div()
                .id(("commit-file", i))
                .flex()
                .w_full()
                .items_center()
                .gap(theme.rem(0.5))
                .rounded(theme.radii.md)
                .px(theme.rem(0.5))
                .py(theme.rem(0.25))
                .font(mono_font(weight::NORMAL))
                .hover(|s| s.bg(with_alpha(c.highlight(), 0.5)))
                .when(dialog.editing_files, |r| {
                    let path = path.clone();
                    r.child(observed(
                        ("include-file", i),
                        Checkbox::new(("include-file-box", i), if excluded { CheckState::Unchecked } else { CheckState::Checked })
                            .on_change({
                                let this = cx.entity();
                                move |_, _, cx| {
                                    this.update(cx, |this, cx| {
                                        if let Some(dialog) = &mut this.commit_dialog
                                            && !dialog.excluded.remove(&path)
                                        {
                                            dialog.excluded.insert(path.clone());
                                        }
                                        cx.notify();
                                    })
                                }
                            }),
                    ))
                })
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .justify_between()
                        .gap(theme.rem(0.75))
                        .child(div().flex_1().min_w_0().truncate().when(excluded, |d| d.text_color(muted)).child(file.path.clone()))
                        .child(if excluded {
                            div().flex_none().text_color(muted).child(t!("Excluded"))
                        } else {
                            div()
                                .flex()
                                .flex_none()
                                .child(div().text_color(f.diff_addition_foreground).child(format!("+{}", format_diff_count(file.insertions))))
                                .child(div().text_color(muted).child(" / "))
                                .child(div().text_color(f.diff_deletion_foreground).child(format!("-{}", format_diff_count(file.deletions))))
                        }),
                )
        });
        let summary = div()
            .flex()
            .flex_col()
            .gap(theme.rem(0.75))
            .rounded(theme.radii.xl)
            .bg(card_fill)
            .border_1()
            .border_color(ring)
            .p(theme.rem(0.75))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.rem(0.5))
                    .child(div().text_color(muted).child(t!("Branch", context = "git branch")))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .justify_between()
                            .child(
                                div().font_weight(weight::MEDIUM).child(
                                    status.and_then(|s| s.ref_name.clone()).map(SharedString::from).unwrap_or_else(|| t!("(detached HEAD)")),
                                ),
                            )
                            .when(is_default, |d| d.child(div().text_color(c.warning).child(t!("Default branch")))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.rem(0.5))
                            .when(dialog.editing_files && !files.is_empty(), |d| {
                                let all_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
                                let state = if all_selected {
                                    CheckState::Checked
                                } else if none_selected {
                                    CheckState::Unchecked
                                } else {
                                    CheckState::Indeterminate
                                };
                                let this = cx.entity();
                                d.child(Checkbox::new("include-all", state).on_change(move |_, _, cx| {
                                    this.update(cx, |this, cx| {
                                        if let Some(dialog) = &mut this.commit_dialog {
                                            dialog.excluded = if dialog.excluded.is_empty() {
                                                all_paths.iter().cloned().collect()
                                            } else {
                                                HashSet::new()
                                            };
                                        }
                                        cx.notify();
                                    })
                                }))
                            })
                            .child(div().text_color(muted).child(t!("Files")))
                            .when(!all_selected && !dialog.editing_files, |d| {
                                d.child(div().text_color(muted).child(t!("({selected} of {total})", selected = selected.len(), total = files.len())))
                            }),
                    )
                    .when(!files.is_empty(), |d| {
                        d.child(observed(
                            "commit-edit-files",
                            Button::new("commit-edit-files-button")
                                .variant(ButtonVariant::Ghost)
                                .size(ButtonSize::Xs)
                                .label(if dialog.editing_files {
                                    t!("Done", context = "button, finish editing the file list")
                                } else {
                                    t!("Edit", context = "verb")
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(dialog) = &mut this.commit_dialog {
                                        dialog.editing_files = !dialog.editing_files;
                                    }
                                    cx.notify();
                                })),
                        ))
                    }),
            )
            .child(if files.is_empty() {
                div().font_weight(weight::MEDIUM).child(t!("none", context = "no files")).into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.5))
                    .child(
                        div()
                            .id("commit-files")
                            .h(theme.rem(11.))
                            .overflow_y_scroll()
                            .rounded(theme.radii.lg)
                            .bg(if theme.is_dark() { gpui_kit::hsla(0., 0., 1., 0.025) } else { c.card() })
                            .border_1()
                            .border_color(ring)
                            .p(theme.rem(0.25))
                            .flex()
                            .flex_col()
                            .gap(theme.rem(0.25))
                            .children(file_rows),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .font(mono_font(weight::NORMAL))
                            .child(div().text_color(f.diff_addition_foreground).child(format!("+{}", format_diff_count(selected.iter().map(|f| f.insertions).sum()))))
                            .child(div().text_color(muted).child(" / "))
                            .child(div().text_color(f.diff_deletion_foreground).child(format!("-{}", format_diff_count(selected.iter().map(|f| f.deletions).sum())))),
                    )
                    .into_any_element()
            });
        let surface = DialogSurface::new(t!("Commit changes"))
            .description(t!("Review and confirm your commit. Leave the message blank to auto-generate one."))
            .child(summary)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.rem(0.25))
                    .child(div().font_weight(weight::MEDIUM).child(t!("Commit message (optional)")))
                    .child(TextArea::new(&dialog.message)),
            )
            .footer(
                Button::new("commit-cancel")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t!("Cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_commit_dialog(cx))),
            )
            .footer(
                Button::new("commit-new-branch")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t!("Commit on new branch"))
                    .disabled(!self.props.can_change_thread_branch || none_selected)
                    .on_click(cx.listener(|this, _, _, cx| this.commit_from_dialog(true, cx))),
            )
            .footer(observed(
                "commit-confirm",
                Button::new("commit-confirm-button")
                    .size(ButtonSize::Sm)
                    .label(t!("Commit", context = "verb"))
                    .disabled(!self.props.can_write_source_control || none_selected)
                    .on_click(cx.listener(|this, _, _, cx| this.commit_from_dialog(false, cx))),
            ));
        let close = cx.listener(|this, _: &(), _, cx| this.close_commit_dialog(cx));
        let close = std::rc::Rc::new(close);
        Modal::dialog("commit-dialog", div().id("dialog-content").occlude().w_full().max_w(cx.app_theme().rem(32.)).child(surface), move |window, cx| close(&(), window, cx)).into_any_element()
    }

    fn render_default_branch_dialog(
        &self,
        pending: &PendingDefaultBranchAction,
        status: Option<&VcsStatusResult>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copy = resolve_default_branch_action_dialog_copy(
            pending.action,
            &pending.branch,
            pending.includes_commit,
            change_request_terminology_for(status),
        );
        // Kept enabled until live status loads; the server rejects stale requests.
        let unavailable = resolve_feature_branch_unavailable_reason(
            pending.action,
            &pending.branch,
            status.is_none_or(|s| s.has_working_tree_changes),
        );
        let theme = cx.app_theme();
        let mut surface = DialogSurface::new(copy.title).description(copy.description);
        if let Some(reason) = &unavailable {
            surface = surface.child(
                div().ui_text(theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(reason.clone()),
            );
        }
        let surface = surface
            .footer(
                div().mr_auto().child(
                    Button::new("default-abort")
                        .variant(ButtonVariant::Outline)
                        .size(ButtonSize::Sm)
                        .label(t!("Abort"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.pending_default = None;
                            cx.notify();
                        })),
                ),
            )
            .footer(observed(
                "default-continue",
                Button::new("default-continue-button")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(copy.continue_label)
                    .disabled(!self.props.can_write_source_control)
                    .on_click(cx.listener(|this, _, _, cx| this.continue_pending(false, cx))),
            ))
            .footer(
                Button::new("default-feature")
                    .size(ButtonSize::Sm)
                    .label(t!("Check out feature branch & continue"))
                    .disabled(!self.props.can_change_thread_branch || unavailable.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.continue_pending(true, cx))),
            );
        let abort = std::rc::Rc::new(cx.listener(|this, _: &(), _, cx| {
            this.pending_default = None;
            cx.notify();
        }));
        Modal::dialog("default-branch-dialog", div().id("dialog-content").occlude().w_full().max_w(cx.app_theme().rem(32.)).child(surface), move |window, cx| abort(&(), window, cx)).into_any_element()
    }
}

impl Render for GitActionsControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let (status, status_error, is_busy, progress, success) = {
            let model = self.model.read(cx);
            (
                model.status.clone(),
                model.status_error.clone(),
                model.is_action_running(),
                resolve_git_action_progress_presentation(&model.action),
                model.visible_success(now_ms()).cloned(),
            )
        };
        if progress.is_some() || success.is_some() {
            self.ensure_ticker(cx);
        }
        let status_ref = status.as_ref();
        let is_repo = status_ref.is_none_or(|s| s.is_repo);
        let has_primary_remote = status_ref.is_some_and(|s| s.has_primary_remote);
        let is_default = status_ref.is_some_and(|s| s.is_default_ref);
        let items = build_menu_items(status_ref, is_busy, has_primary_remote);
        let quick = resolve_quick_action(status_ref, is_busy, is_default, has_primary_remote);
        let host_icon = source_control_presentation(status_ref.and_then(|s| s.source_control_provider.as_ref())).icon;
        let disabled_reason = if !self.props.can_write_source_control {
            Some(t!("This connection cannot change source control."))
        } else if quick.disabled {
            Some(quick.hint().cloned().unwrap_or_else(|| t!("This action is currently unavailable.")))
        } else {
            None
        };
        let totals = resolve_changes_totals(status_ref);
        let has_changes = totals.as_ref().is_some_and(|t| t.insertions > 0 || t.deletions > 0);
        let quiet = self.props.quiet_when_idle && progress.is_none() && status_error.is_none() && quick.disabled && !has_changes;
        let can_publish = self.props.can_write_source_control && is_repo && status.is_some() && !has_primary_remote;
        let detached = status_ref.is_some_and(|s| s.ref_name.is_none());
        let clean_behind = status_ref.is_some_and(|s| {
            s.ref_name.is_some() && !s.has_working_tree_changes && s.behind_count > 0 && s.ahead_count == 0
        });
        let has_menu = has_usable_git_menu_content(
            &items,
            self.props.can_write_source_control,
            can_publish,
            detached || clean_behind || status_error.is_some(),
        );
        let row_width = self::row_width(&theme);

        let mut root = div().flex().flex_col().w_full();
        if self.props.show_actions {
            if !is_repo {
                let pending = self.model.read(cx).action.operation == Some(VcsActionOperation::Init);
                root = root.child(
                    row(
                        "git-init",
                        &theme,
                        Some(row_icon(&theme, IconName::GitBranchPlus)),
                        if pending { t!("Initializing...") } else { t!("Initialize Git") },
                        !self.props.can_write_source_control || pending,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.init_git(cx))),
                );
            } else if !(quiet || (self.props.compact && progress.is_none() && success.is_none())) {
                let now = now_ms();
                let primary = if let Some(progress) = &progress {
                    let elapsed = format_git_action_elapsed(progress.started_at_ms, now);
                    row_base("git-primary", &theme, false, false)
                        .h_auto()
                        .min_h(row_height(&theme))
                        .py(theme.rem(0.375))
                        .items_start()
                        .gap(theme.rem(0.75))
                        .child(div().h(theme.rem(1.125)).flex().items_center().child(Spinner::new(SpinnerSize::Sm)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().child(progress.status.clone()))
                                .children(progress.output.clone().map(|output| {
                                    div()
                                        .id("git-progress-output")
                                        .pt(theme.rem(0.125))
                                        .truncate()
                                        .ui_text(&theme, TextSize::Xs2, weight::NORMAL)
                                        .text_color(theme.colors.muted_foreground)
                                        .tooltip(tooltip(output.clone()))
                                        .child(output)
                                })),
                        )
                        .children(elapsed.map(|elapsed| {
                            div()
                                .flex_none()
                                .h(theme.rem(1.125))
                                .flex()
                                .items_center()
                                .ui_text(&theme, TextSize::Xs2, weight::NORMAL)
                                .text_color(theme.colors.muted_foreground)
                                .child(elapsed)
                        }))
                } else if let Some(success) = &success {
                    row_base("git-primary", &theme, false, false)
                        .h_auto()
                        .min_h(row_height(&theme))
                        .py(theme.rem(0.375))
                        .items_start()
                        .gap(theme.rem(0.75))
                        .child(
                            div()
                                .h(theme.rem(1.125))
                                .flex()
                                .items_center()
                                .child(Icon::new(IconName::Check).size(IconSize::Custom(theme.rem(0.875))).color(theme.fixed.success)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(div().truncate().child(success.title.clone()))
                                .children(success.description.clone().map(|d| {
                                    div()
                                        .pt(theme.rem(0.125))
                                        .truncate()
                                        .ui_text(&theme, TextSize::Xs2, weight::NORMAL)
                                        .text_color(theme.colors.muted_foreground)
                                        .child(d)
                                })),
                        )
                } else {
                    let enabled = self.props.can_write_source_control && !is_busy && !quick.disabled;
                    let quick_for_click = quick.clone();
                    let primary = row_base("git-primary", &theme, false, false)
                        .child(Icon::new(quick_action_icon(&quick, host_icon)).size(row_icon_size(&theme)).color(theme.colors.muted_foreground))
                        .child(div().flex_1().min_w_0().truncate().child(quick.label.clone()))
                        .tooltip(tooltip(disabled_reason.clone().unwrap_or_else(|| quick.label.clone())));
                    if enabled {
                        primary.on_click(cx.listener(move |this, _, window, cx| this.run_quick_action(quick_for_click.clone(), window, cx)))
                    } else {
                        // `aria-disabled`: dimmed, and the reason shows on hover.
                        primary.opacity(DISABLED_OPACITY).cursor_not_allowed()
                    }
                };
                let busy = progress.is_some();
                let secondary = split_secondary("git-menu-trigger", &theme, is_busy || !has_menu, self.menu_open)
                    .tooltip(tooltip(t!("Git action options")))
                    .on_click(cx.listener(|this, event, _, cx| {
                        if this.dismissed.consumes(event) {
                            return;
                        }
                        let open = !this.menu_open;
                        this.set_menu_open(open, cx);
                    }));
                let group = if busy {
                    div()
                        .flex()
                        .w_full()
                        .rounded(theme.radii.lg)
                        .bg(row_busy_fill(&theme))
                        .child(primary.flex_1())
                } else {
                    split_row("git-actions-split", &theme, primary, secondary, self.menu_open, false)
                };
                let menu = (self.menu_open && !busy)
                    .then(|| self.render_menu(&items, status_ref, is_busy, can_publish, row_width, &theme, cx));
                root = root.child(div().flex().flex_col().w_full().child(group).children(menu));
            }
        }
        if self.props.show_changes && is_repo && !quiet {
            root = root.child(self.render_changes_row(status_ref, &theme, cx));
        }
        if let Some(dialog) = &self.commit_dialog {
            root = root.child(self.render_commit_dialog(dialog, status_ref, &theme, cx));
        }
        if let Some(pending) = self.pending_default.clone() {
            root = root.child(self.render_default_branch_dialog(&pending, status_ref, cx));
        }
        if let Some((dialog, _)) = &self.publish {
            root = root.child(dialog.clone());
        }
        root
    }
}
