//! The source control controls on fixture data: the thread details card's
//! branch and git rows in each main repository state, the composer's context
//! strip, and the menus, picker and dialogs open. `git-actions-light` renders
//! the same in the light appearance; `git-actions-popups` opens the popups.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, px,
};
use t3code_contracts::{EnvironmentId, PullRequestState, VcsStatusResult};

use crate::components::branch_toolbar::{BranchToolbar, BranchToolbarLayout, BranchToolbarProps};
use crate::components::branch_toolbar_logic::{EnvMode, EnvironmentOption, MachineKind, PreviousWorktreeSeed};
use crate::components::git_actions_control::{GitActionsControl, GitActionsProps};
use crate::components::git_actions_control_fixture::{FixtureGitSource, fixture_open_pr, fixture_status};
use crate::components::git_actions_control_logic::{VcsActionOperation, VcsActionState};
use crate::components::git_actions_control_source::{InlineSuccess, SourceControlModel, now_ms};
use crate::components::ui::popover::glass_surface;
use crate::theme_palette::{Appearance, ThemeDocument, set_app_theme};
use crate::{ActiveAppTheme as _, AppTheme};

/// One repository state to draw.
pub struct StateCell {
    pub label: &'static str,
    pub model: Entity<SourceControlModel>,
    pub branch: Entity<BranchToolbar>,
    pub git: Entity<GitActionsControl>,
}

/// The fixture machines.
pub fn fixture_environments() -> Vec<EnvironmentOption> {
    vec![
        EnvironmentOption {
            environment_id: EnvironmentId::new("env-studio"),
            label: "Studio".into(),
            is_primary: false,
            machine: MachineKind::Desktop,
        },
        EnvironmentOption {
            environment_id: EnvironmentId::new("env-devbox"),
            label: "devbox".into(),
            is_primary: false,
            machine: MachineKind::Server,
        },
    ]
}

/// Toolbar props for a draft on `branch` in the project checkout.
pub fn fixture_toolbar_props(branch: Option<&str>) -> BranchToolbarProps {
    BranchToolbarProps {
        environments: fixture_environments(),
        environment_id: Some(EnvironmentId::new("env-studio")),
        auto_environment_label: None,
        can_change_environment: true,
        can_auto_balance: false,
        env_mode: EnvMode::Local,
        force_new_worktree: false,
        worktree_path: None,
        project_cwd: "/home/dev/t3code".into(),
        is_server_thread: true,
        env_locked: false,
        thread_branch: branch.map(Into::into),
        start_from_origin: false,
        previous_worktree: Some(PreviousWorktreeSeed {
            branch: Some("fix/branch-picker-keys".into()),
            worktree_path: "/home/dev/t3code/.t3/worktrees/picker".into(),
        }),
        turn_running: false,
        can_write_source_control: true,
        can_change_thread_branch: true,
        can_checkout_pull_request: true,
        show_git_controls: true,
    }
}

/// Git row props for a started thread with full access.
pub fn fixture_git_props() -> GitActionsProps {
    GitActionsProps {
        can_write_source_control: true,
        can_change_thread_branch: true,
        quiet_when_idle: false,
        compact: false,
        show_actions: true,
        show_changes: true,
        thread_id: None,
        project_id: None,
    }
}

/// The repository states the scene shows, in order.
pub fn fixture_states() -> Vec<(&'static str, VcsStatusResult)> {
    let mut pr_open = fixture_status(Some("feature/thread-details"), false, true, 0, 0);
    pr_open.pr = Some(fixture_open_pr());
    let mut merged = fixture_status(Some("feature/thread-details"), false, true, 0, 0);
    merged.pr = Some(t3code_contracts::VcsStatusRemoteResultPr { state: PullRequestState::Merged, ..fixture_open_pr() });
    let _ = merged;
    let mut no_remote = fixture_status(Some("feature/thread-details"), false, false, 2, 0);
    no_remote.has_primary_remote = false;
    let _ = no_remote;
    vec![
        ("clean", fixture_status(Some("feature/thread-details"), false, true, 0, 0)),
        ("dirty", fixture_status(Some("feature/thread-details"), true, true, 0, 0)),
        ("ahead", fixture_status(Some("feature/thread-details"), false, true, 2, 0)),
        ("behind", fixture_status(Some("feature/thread-details"), false, true, 0, 3)),
        ("diverged", fixture_status(Some("feature/thread-details"), false, true, 2, 3)),
        ("no upstream", fixture_status(Some("feature/thread-details"), false, false, 2, 0)),
        ("detached", fixture_status(None, false, false, 0, 0)),
        ("pull request open", pr_open),
        ("action running", fixture_status(Some("feature/thread-details"), true, true, 0, 0)),
        ("error", fixture_status(Some("main"), true, true, 1, 0)),
    ]
}

/// Builds the model and both views for one state.
pub fn state_cell(label: &'static str, status: VcsStatusResult, window: &mut Window, cx: &mut App) -> StateCell {
    let branch_name = status.ref_name.clone();
    let mut source = FixtureGitSource::new(status);
    if label == "error" {
        source.fail_with = Some("pre-commit hook failed: lint found 3 problems".into());
    }
    let model = cx.new(|cx| SourceControlModel::new(Arc::new(source), "/home/dev/t3code", cx));
    let branch = cx.new(|cx| {
        BranchToolbar::new(BranchToolbarLayout::PanelBranch, model.clone(), fixture_toolbar_props(branch_name.as_deref()), cx)
    });
    let git = cx.new(|cx| GitActionsControl::new(model.clone(), fixture_git_props(), cx));
    let _ = window;
    StateCell { label, model, branch, git }
}

/// Puts a model into the running state the "action running" cell shows.
pub fn set_running(model: &Entity<SourceControlModel>, cx: &mut App) {
    model.update(cx, |m, cx| {
        let now = now_ms();
        m.action = VcsActionState {
            is_running: true,
            operation: Some(VcsActionOperation::RunChangeRequest),
            current_label: Some("Running pre-commit...".into()),
            hook_name: Some("pre-commit".into()),
            last_output_line: Some("eslint: checking 214 files".into()),
            phase_started_at_ms: Some(now - 14_000),
            hook_started_at_ms: Some(now - 8_000),
        };
        cx.notify();
    });
}

/// Puts a model into the "just finished" state.
pub fn set_success(model: &Entity<SourceControlModel>, cx: &mut App) {
    model.update(cx, |m, cx| {
        m.inline_success = Some(InlineSuccess {
            title: "Pushed feature/thread-details".into(),
            description: Some("2 commits to origin/feature/thread-details".into()),
            expires_at_ms: now_ms() + 600_000,
        });
        cx.notify();
    });
}

/// The grid of state cells.
pub struct GitActionsScene {
    cells: Vec<StateCell>,
}

impl GitActionsScene {
    pub fn new(window: &mut Window, cx: &mut App) -> Self {
        let cells: Vec<StateCell> =
            fixture_states().into_iter().map(|(label, status)| state_cell(label, status, window, cx)).collect();
        for cell in &cells {
            if cell.label == "action running" {
                set_running(&cell.model, cx);
            }
            if cell.label == "error" {
                cell.model.update(cx, |m, cx| {
                    m.status_error = Some("fatal: unable to access remote: Could not resolve host".into());
                    cx.notify();
                });
            }
        }
        Self { cells }
    }
}

fn cell_card(theme: &AppTheme, label: SharedString, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(11.)).text_color(theme.colors.muted_foreground).child(label))
        .child(
            glass_surface(theme)
                .rounded(theme.radii.xxl)
                .w(theme.spacing.thread_details_panel_width)
                .px(px(8.))
                .pt(px(8.))
                .pb(px(10.))
                .children(rows),
        )
        .into_any_element()
}

impl Render for GitActionsScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        div()
            .size_full()
            .bg(theme.colors.background())
            .p(px(20.))
            .flex()
            .flex_wrap()
            .gap(px(20.))
            .children(self.cells.iter().map(|cell| {
                cell_card(
                    &theme,
                    cell.label.into(),
                    vec![cell.branch.clone().into_any_element(), cell.git.clone().into_any_element()],
                )
            }))
    }
}

pub fn build(window: &mut Window, cx: &mut App) -> AnyView {
    let scene = GitActionsScene::new(window, cx);
    cx.new(|_| scene).into()
}

pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    set_app_theme(AppTheme::new(&ThemeDocument::graphite(), Appearance::Light), cx);
    build(window, cx)
}
