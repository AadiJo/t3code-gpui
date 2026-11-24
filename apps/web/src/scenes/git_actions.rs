//! The source control controls on fixture data.
//!
//! - `git-actions`: the branch, pull request and git rows of the thread
//!   details card in each main repository state, the composer's context strip
//!   in three states, and a full card.
//! - `git-actions-panel`: one thread in a chat column, with the full card open
//!   as a popover and the composer strip below. `examples/git_actions_check.rs`
//!   clicks through its menus, picker and dialogs.
//!
//! `-light` variants render in the light appearance.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, prelude::*, px,
};
use t3code_contracts::{
    EditorId, EnvironmentId, IssueTrackerKind, ProjectId, ProjectScript, ProjectScriptIcon, PullRequestDetail,
    PullRequestState, ThreadIssueLink, ThreadIssueLinkSource, VcsStatusResult,
};

use crate::components::branch_toolbar::{BranchToolbar, BranchToolbarEvent, BranchToolbarLayout, BranchToolbarProps};
use crate::components::branch_toolbar_logic::{EnvMode, EnvironmentOption, MachineKind, PreviousWorktreeSeed};
use crate::components::chat::open_in_picker::OpenInPicker;
use crate::components::chat::thread_details_items::ThreadDetailsItemSettings;
use crate::components::chat::thread_details_panel::{ThreadDetailsInput, ThreadDetailsPanel};
use crate::components::chat::thread_details_pr_row::{PullRequestRowData, PullRequestRowHandlers, pull_request_row};
use crate::components::git_actions_control::{GitActionsControl, GitActionsProps};
use crate::components::git_actions_control_fixture::{FixtureGitSource, fixture_open_pr, fixture_status};
use crate::components::git_actions_control_logic::{VcsActionOperation, VcsActionState};
use crate::components::git_actions_control_source::{GitActionsSource, InlineSuccess, SourceControlModel, now_ms};
use crate::components::project_scripts_control::ProjectScriptsControl;
use crate::components::pull_request_thread_dialog::{PullRequestThreadDialog, PullRequestThreadEvent};
use crate::components::ui::popover::glass_surface;
use crate::components::ui::toast::Toaster;
use crate::remote_open::{RemoteOpenHost, RemoteOpenState};
use crate::source_control_presentation::source_control_presentation;
use crate::theme_palette::Appearance;
use crate::{ActiveAppTheme as _, AppTheme};

const PROJECT_CWD: &str = "/home/dev/t3code";

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

/// Toolbar props for a started thread on `branch` in the project checkout.
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
        project_cwd: PROJECT_CWD.into(),
        is_server_thread: false,
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
        pull_request_badge: None,
    }
}

/// Git row props for a thread with full access.
pub fn fixture_git_props() -> GitActionsProps {
    GitActionsProps {
        can_write_source_control: true,
        can_change_thread_branch: true,
        quiet_when_idle: false,
        compact: false,
        show_actions: true,
        show_changes: true,
        thread_id: None,
        project_id: Some(ProjectId::new("project-t3code")),
        offered_source_control_providers: None,
    }
}

/// The project's scripts: a setup script, a dev server and an on-settle lint.
pub fn fixture_scripts() -> Vec<ProjectScript> {
    let script = |id: &str, name: &str, command: &str, icon, setup: bool, settle: bool| ProjectScript {
        id: id.into(),
        name: name.into(),
        command: command.into(),
        icon,
        run_on_worktree_create: setup,
        run_on_settle: settle.then_some(true),
        stop_on_settle: None,
        r#async: None,
        preview_url: None,
        auto_open_preview: None,
    };
    vec![
        script("setup", "Install", "pnpm install", ProjectScriptIcon::Configure, true, false),
        script("dev", "Dev server", "pnpm dev", ProjectScriptIcon::Play, false, false),
        script("lint", "Lint", "pnpm lint", ProjectScriptIcon::Lint, false, true),
        script("test", "Test", "pnpm test", ProjectScriptIcon::Test, false, false),
    ]
}

/// A pull request detail: open, mergeable, `checks` as given.
pub fn fixture_pull_request_detail(failing: bool, pending: bool) -> PullRequestDetail {
    let check = |name: &str, status: &str| serde_json::json!({ "name": name, "status": status, "description": null, "url": null });
    let checks = vec![
        check("build", "success"),
        check("lint", if failing { "failure" } else { "success" }),
        check("test", if pending { "pending" } else { "success" }),
        check("typecheck", "success"),
    ];
    serde_json::from_value(serde_json::json!({
        "provider": "github",
        "capabilities": {
            "diff": true, "comment": true, "actions": ["merge", "ready", "draft", "close"],
            "mergeMethods": ["squash", "merge"], "search": true,
            "review": { "inlineComment": true, "reply": true, "resolve": true, "verdicts": [] },
            "reviewers": { "request": true, "listCandidates": true }
        },
        "viewerPermissions": { "actions": ["merge", "ready", "close"], "comment": true, "resolve": true, "verdicts": [], "requestReviewers": true },
        "projectId": "project-t3code", "projectTitle": "t3code", "workspaceRoot": PROJECT_CWD,
        "repository": "example/t3code", "number": 418, "title": "feat(web): thread details panel with git actions and the branch picker",
        "body": "", "url": "https://github.com/example/t3code/pull/418", "author": null, "state": "open", "isDraft": false,
        "mergeability": "mergeable", "additions": 1840, "deletions": 120, "changedFiles": 22,
        "headBranch": "feature/thread-details", "baseBranch": "main",
        "createdAt": "2026-11-20T10:00:00Z", "updatedAt": "2026-11-23T09:00:00Z", "mergedAt": null, "closedAt": null,
        "reviewers": [], "labels": [], "checks": checks,
        "mergeCapabilities": { "merge": true, "squash": true, "rebase": false }
    }))
    .expect("fixture pull request detail")
}

/// The pull request row for a fixture detail.
pub fn fixture_pull_request_row(detail: Option<PullRequestDetail>) -> PullRequestRowData {
    PullRequestRowData {
        number: 418,
        title: "feat(web): thread details panel with git actions and the branch picker".into(),
        url: "https://github.com/example/t3code/pull/418".into(),
        state: PullRequestState::Open,
        is_draft: false,
        detail,
        watching: false,
        project_merge_method: None,
        last_merge_method: t3code_contracts::PullRequestMergeMethod::Squash,
    }
}

fn fixture_issue() -> ThreadIssueLink {
    ThreadIssueLink {
        tracker: IssueTrackerKind::new("github"),
        host: "github.com".into(),
        scope: "example/t3code".into(),
        key: "402".into(),
        project_id: ProjectId::new("project-t3code"),
        url: "https://github.com/example/t3code/issues/402".into(),
        source: ThreadIssueLinkSource::Manual,
        linked_at: "2026-11-21T08:00:00Z".into(),
        display_key: "#402".into(),
        title: "Port the thread details panel".into(),
    }
}

/// The repository states the grid shows, in order.
pub fn fixture_states() -> Vec<(&'static str, VcsStatusResult)> {
    let branch = Some("feature/thread-details");
    let mut pr_open = fixture_status(branch, false, true, 0, 0);
    pr_open.pr = Some(fixture_open_pr());
    vec![
        ("clean", fixture_status(branch, false, true, 0, 0)),
        ("dirty", fixture_status(branch, true, true, 0, 0)),
        ("ahead", fixture_status(branch, false, true, 2, 0)),
        ("behind", fixture_status(branch, false, true, 0, 3)),
        ("diverged", fixture_status(branch, false, true, 2, 3)),
        ("no upstream", fixture_status(branch, false, false, 2, 0)),
        ("detached", fixture_status(None, false, false, 0, 0)),
        ("pull request open", pr_open),
        ("action running", fixture_status(branch, true, true, 0, 0)),
        ("error", fixture_status(Some("main"), true, true, 1, 0)),
    ]
}

fn model_for(source: FixtureGitSource, cx: &mut App) -> Entity<SourceControlModel> {
    cx.new(|cx| SourceControlModel::new(Arc::new(source), PROJECT_CWD, cx))
}

/// Puts a model into the running state: a pre-commit hook printing output.
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

/// One grid cell: a label over a card of rows.
fn cell(theme: &AppTheme, label: impl Into<SharedString>, width: gpui_kit::Pixels, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(div().text_size(px(11.)).text_color(theme.colors.muted_foreground).child(label.into()))
        .child(glass_surface(theme).rounded(theme.radii.xxl).w(width).px(px(8.)).pt(px(8.)).pb(px(10.)).children(rows))
        .into_any_element()
}

/// The states grid.
pub struct GitActionsScene {
    states: Vec<(&'static str, Entity<BranchToolbar>, Entity<GitActionsControl>, Option<PullRequestRowData>)>,
    strips: Vec<(&'static str, Entity<BranchToolbar>)>,
}

impl GitActionsScene {
    pub fn new(cx: &mut App) -> Self {
        let mut states = Vec::new();
        for (label, status) in fixture_states() {
            let branch_name = status.ref_name.clone();
            let mut source = FixtureGitSource::new(status);
            if label == "error" {
                source.fail_with = Some("pre-commit hook failed".into());
                source.status_error = Some("fatal: unable to access remote: could not resolve host".into());
            }
            let model = model_for(source, cx);
            let props = fixture_toolbar_props(branch_name.as_deref());
            let branch = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelBranch, model.clone(), props, cx));
            let git = cx.new(|cx| GitActionsControl::new(model.clone(), fixture_git_props(), cx));
            match label {
                "action running" => set_running(&model, cx),
                _ => {}
            }
            let pr = (label == "pull request open").then(|| fixture_pull_request_row(Some(fixture_pull_request_detail(true, false))));
            states.push((label, branch, git, pr));
            if label == "error" {
                // The error cell's menu is open so its status error shows.
                let git = &states.last().unwrap().2;
                git.update(cx, |git, cx| git.set_menu_open(true, cx));
            }
        }
        let strip = |label, props: BranchToolbarProps, cx: &mut App| {
            let model = model_for(FixtureGitSource::new(fixture_status(Some("feature/thread-details"), true, true, 0, 0)), cx);
            (label, cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::Composer, model, props, cx)))
        };
        let draft = fixture_toolbar_props(Some("feature/thread-details"));
        let worktree = BranchToolbarProps { env_mode: EnvMode::Worktree, thread_branch: Some("main".into()), start_from_origin: true, ..draft.clone() };
        let running = BranchToolbarProps {
            env_locked: true,
            is_server_thread: true,
            worktree_path: Some("/home/dev/t3code/.t3/worktrees/thread-details".into()),
            env_mode: EnvMode::Worktree,
            turn_running: true,
            ..draft.clone()
        };
        let strips = vec![
            strip("composer strip: draft, current checkout", draft, cx),
            strip("composer strip: draft, new worktree from main", worktree, cx),
            strip("composer strip: started thread, turn running (locked)", running, cx),
        ];
        Self { states, strips }
    }
}

impl Render for GitActionsScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let width = theme.spacing.thread_details_panel_width;
        let states = self.states.iter().map(|(label, branch, git, pr)| {
            let mut rows = vec![branch.clone().into_any_element()];
            if let Some(pr) = pr {
                let handlers = PullRequestRowHandlers {
                    open: std::rc::Rc::new(|_, _| {}),
                    action: std::rc::Rc::new(|_, _, _| {}),
                    toggle_checks: std::rc::Rc::new(|_, _| {}),
                    stop_watching: None,
                };
                rows.push(pull_request_row(0, &theme, pr, handlers, None));
            }
            rows.push(git.clone().into_any_element());
            cell(&theme, *label, width, rows)
        });
        let strips = self.strips.iter().map(|(label, strip)| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_size(px(11.)).text_color(theme.colors.muted_foreground).child(*label))
                .child(
                    div()
                        .w(px(560.))
                        .rounded(theme.radii.xxl)
                        .bg(theme.derived.surface_glass_opaque)
                        .pb(px(8.))
                        .child(strip.clone()),
                )
                .into_any_element()
        });
        div()
            .size_full()
            .bg(theme.colors.background())
            .p(px(20.))
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(div().flex().flex_wrap().gap(px(20.)).children(states))
            .child(div().flex().flex_wrap().gap(px(20.)).children(strips))
    }
}

/// One thread with the full card and the composer strip, for scripted checks.
pub struct GitPanelScene {
    pub source: FixtureGitSource,
    pub model: Entity<SourceControlModel>,
    pub panel: Entity<ThreadDetailsPanel>,
    pub strip: Entity<BranchToolbar>,
    pub dialog: Option<Entity<PullRequestThreadDialog>>,
    /// Every event the controls raised, for the check's transcript.
    pub events: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl GitPanelScene {
    /// `source` answers every call; give it a `fail_with` to see failures.
    pub fn new(source: FixtureGitSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_items(source, false, window, cx)
    }

    /// `all_items` turns every details item on and fills Servers, Automations
    /// and Context window, and the pull request is watched.
    pub fn with_items(mut source: FixtureGitSource, all_items: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The owner's setting: Usage hidden (the default) and Project scripts hidden.
        let mut settings = ThreadDetailsItemSettings::default();
        if all_items {
            settings.usage_enabled = true;
        } else {
            settings.set(crate::components::chat::thread_details_items::ThreadDetailsItem::Scripts, false);
        }
        cx.set_global(settings);

        let branch_name = source.status.ref_name.clone();
        let has_pr = source.status.pr.is_some();
        source.pull_request = Some(fixture_pull_request_detail(false, true));
        let model = cx.new(|cx| SourceControlModel::new(Arc::new(source.clone()), PROJECT_CWD, cx));
        let arc: Arc<dyn GitActionsSource> = Arc::new(source.clone());
        let mut props = fixture_toolbar_props(branch_name.as_deref());
        if all_items {
            props.pull_request_badge = Some(crate::components::branch_toolbar::StripPullRequestBadge {
                number: 418,
                url: "https://github.com/example/t3code/pull/418".into(),
                title: "feat(web): thread details panel with git actions and the branch picker".into(),
                state: t3code_contracts::PullRequestState::Open,
                is_draft: false,
                others: 0,
                label: "PR #418 open".into(),
            });
        }
        let workspace = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelWorkspace, model.clone(), props.clone(), cx));
        let branch = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::PanelBranch, model.clone(), props.clone(), cx));
        let strip = cx.new(|cx| BranchToolbar::new(BranchToolbarLayout::Composer, model.clone(), props, cx));
        let git = cx.new(|cx| GitActionsControl::new(model.clone(), fixture_git_props(), cx));
        let scripts = cx.new(|_| ProjectScriptsControl::new(arc.clone(), ProjectId::new("project-t3code"), fixture_scripts(), true));
        let remote = RemoteOpenState::RemoteLinks(RemoteOpenHost { kind: None, host: "studio".into(), username: Some("dev".into()) });
        let open_in = cx.new(|_| {
            OpenInPicker::new(arc.clone(), vec![EditorId::Vscode, EditorId::Cursor, EditorId::Zed], remote, Some(PROJECT_CWD.into()))
                .with_environment_label("Studio")
        });
        open_in.update(cx, |o, cx| o.set_remote_capable_editors(vec![EditorId::Vscode, EditorId::Cursor, EditorId::Zed], cx));
        let input = ThreadDetailsInput {
            offers_git: true,
            is_git_repo: true,
            show_open_in: true,
            pull_request: has_pr.then(|| {
                let mut row = fixture_pull_request_row(source.pull_request.clone());
                row.watching = all_items;
                row
            }),
            issues: vec![fixture_issue()],
            ..Default::default()
        };
        let input = if all_items { with_all_sections(input) } else { input };
        let panel = cx.new(|cx| ThreadDetailsPanel::new(input, workspace.clone(), branch.clone(), git.clone(), Some(scripts.clone()), Some(open_in), cx));
        let mut subscriptions = Vec::new();
        for toolbar in [&workspace, &branch, &strip] {
            subscriptions.push(cx.subscribe_in(toolbar, window, |this, _, event: &BranchToolbarEvent, window, cx| {
                this.events.push(format!("{event:?}"));
                if let BranchToolbarEvent::CheckoutPullRequest(reference) = event {
                    this.open_pull_request_dialog(reference, window, cx);
                }
                cx.notify();
            }));
        }
        subscriptions.push(cx.subscribe(&git, |this, _, event, cx| {
            this.events.push(format!("{event:?}"));
            cx.notify();
        }));
        subscriptions.push(cx.subscribe(&scripts, |this, _, event, cx| {
            this.events.push(format!("{event:?}"));
            cx.notify();
        }));
        Self { source, model, panel, strip, dialog: None, events: Vec::new(), _subscriptions: subscriptions }
    }

    /// Opens "Checkout pull request" for a reference, as the picker's item does.
    pub fn open_pull_request_dialog(&mut self, reference: &str, window: &mut Window, cx: &mut Context<Self>) {
        let presentation = source_control_presentation(self.model.read(cx).status.as_ref().and_then(|s| s.source_control_provider.as_ref()));
        let source: Arc<dyn GitActionsSource> = Arc::new(self.source.clone());
        let dialog = cx.new(|cx| PullRequestThreadDialog::new(source, PROJECT_CWD.into(), None, presentation, reference, window, cx));
        self._subscriptions.push(cx.subscribe(&dialog, |this, _, event: &PullRequestThreadEvent, cx| {
            this.events.push(format!("{event:?}"));
            this.dialog = None;
            cx.notify();
        }));
        self.dialog = Some(dialog);
        cx.notify();
    }

    /// The RPC tags the fixture received, in order.
    pub fn calls(&self) -> Vec<String> {
        self.source.calls.lock().unwrap().clone()
    }
}

impl Render for GitPanelScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let composer = div()
            .absolute()
            .bottom(px(20.))
            .left(px(40.))
            .w(px(620.))
            .child(
                div()
                    .rounded(px(20.))
                    .border_1()
                    .border_color(c.border)
                    .bg(theme.derived.surface_glass_opaque)
                    .child(div().h(px(76.)).px(px(16.)).pt(px(14.)).text_size(px(14.)).text_color(c.placeholder).child("Ask anything, @ to mention files"))
                    .child(div().pb(px(8.)).child(self.strip.clone())),
            );
        div()
            .relative()
            .size_full()
            .bg(c.background())
            .child(
                div()
                    .h(px(52.))
                    .px(px(20.))
                    .flex()
                    .items_center()
                    .text_size(px(14.))
                    .text_color(c.muted_foreground)
                    .child("t3code  /  Port the thread details panel"),
            )
            .child(div().absolute().top(px(52.)).right(px(12.)).child(self.panel.clone()))
            .child(composer)
            .children(self.dialog.clone())
            .child(Toaster::view(cx))
    }
}

pub fn build(_: &mut Window, cx: &mut App) -> AnyView {
    let scene = GitActionsScene::new(cx);
    cx.new(|_| scene).into()
}

pub fn build_light(window: &mut Window, cx: &mut App) -> AnyView {
    crate::theme_palette::ThemeSettings::set_appearance(Appearance::Light, cx);
    build(window, cx)
}

/// The panel scene on a dirty branch with an open pull request.
/// Servers, automations and a context window for the every-item scene.
fn with_all_sections(input: ThreadDetailsInput) -> ThreadDetailsInput {
    use crate::components::chat::thread_details_sections::{
        AutomationRunStatus, ContextWindowRow, ThreadAutomationRow, ThreadServerRow,
    };
    ThreadDetailsInput {
        servers: vec![
            ThreadServerRow { port: 5173, process_name: Some("vite".into()), from_terminal: false, forwarded_to: Some(5173), can_stop: true, can_forward: true },
            ThreadServerRow { port: 3000, process_name: Some("node".into()), from_terminal: true, forwarded_to: None, can_stop: true, can_forward: true },
        ],
        automations: vec![ThreadAutomationRow {
            id: "task-nightly".into(),
            title: "Nightly dependency check".into(),
            schedule_line: "Every day at 9:00 AM".into(),
            last_run: AutomationRunStatus::Succeeded,
            enabled: true,
            webhook: false,
        }],
        can_operate_automations: true,
        context_window: Some(ContextWindowRow {
            used_tokens: 84_200,
            max_tokens: Some(200_000),
            used_percentage: Some(42.1),
            auto_compact_threshold: Some(160_000),
        }),
        ..input
    }
}

/// The thread details card with every item turned on and filled.
pub fn build_panel_all(window: &mut Window, cx: &mut App) -> AnyView {
    let mut status = fixture_status(Some("feature/thread-details"), true, true, 0, 0);
    status.pr = Some(fixture_open_pr());
    cx.new(|cx| GitPanelScene::with_items(FixtureGitSource::new(status), true, window, cx)).into()
}

pub fn build_panel(window: &mut Window, cx: &mut App) -> AnyView {
    let mut status = fixture_status(Some("feature/thread-details"), true, true, 0, 0);
    status.pr = Some(fixture_open_pr());
    cx.new(|cx| GitPanelScene::new(FixtureGitSource::new(status), window, cx)).into()
}

pub fn build_panel_light(window: &mut Window, cx: &mut App) -> AnyView {
    crate::theme_palette::ThemeSettings::set_appearance(Appearance::Light, cx);
    build_panel(window, cx)
}
