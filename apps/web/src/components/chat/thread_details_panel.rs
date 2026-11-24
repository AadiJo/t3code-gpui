//! The thread details card (`threadPanel.toggle`, mod+shift+b). Ports
//! `ThreadDetailsPanel.tsx`, `ThreadDetailsCard.tsx`, `ThreadDetailsSection.tsx`
//! and `ThreadDetailsItemsMenu.tsx`: a 280px glass card of two sections,
//! Workspace (machine and worktree, open in editor, project scripts) and
//! Version control (branch, pull requests, issues, git actions, changes), that
//! the right-click menu or "Choose what to show" trims.
//!
//! It opens as a popover under the titlebar toggle or docks inline when the
//! canvas has room. The fork keeps the toggle gray while it opens a popover;
//! [`thread_details_toggle_pressed`] says when it lights up.

use gpui_kit::{
    AnyElement, App, Context, Entity, EventEmitter, IntoElement, MouseButton, ParentElement as _, Pixels, Point,
    Render, Styled as _, TestSupportExt as _, Window, anchored, deferred, div, prelude::*, px,
};
use t3code_contracts::ThreadIssueLink;

use super::open_in_picker::OpenInPicker;
use super::thread_details_items::{ThreadDetailsItem, ThreadDetailsItemSettings};
use super::thread_details_panel_styles::{row, row_icon, section};
use super::thread_details_pr_row::{PullRequestRowAction, PullRequestRowData, issue_row, pull_request_row};
use crate::components::branch_toolbar::BranchToolbar;
use crate::components::git_actions_control::GitActionsControl;
use crate::components::icons::IconName;
use crate::components::project_scripts_control::ProjectScriptsControl;
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup};
use crate::components::ui::popover::glass_surface;
use crate::i18n::t;
use crate::{ActiveAppTheme as _, AppTheme};

/// Where the card sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadDetailsMode {
    /// Under the titlebar toggle, over the chat.
    #[default]
    Popover,
    /// Docked at the canvas's right edge.
    Inline,
}

/// How much of the card fits (`resolveThreadDetailsCardDensity`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadDetailsDensity {
    #[default]
    Full,
    Compact,
    Essential,
}

/// Whether the titlebar toggle draws pressed: only while the card is docked
/// inline. Opening it as a popover leaves the toggle gray (fork `12be669d98`).
pub fn thread_details_toggle_pressed(mode: ThreadDetailsMode, open: bool) -> bool {
    open && mode == ThreadDetailsMode::Inline
}

/// The thread facts the card needs besides its child controls.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThreadDetailsInput {
    pub mode: ThreadDetailsMode,
    pub density: ThreadDetailsDensity,
    /// The thread's folder may get git (not a "No project" scratch thread).
    pub offers_git: bool,
    pub is_git_repo: bool,
    pub show_open_in: bool,
    /// The branch's pull request, or the thread's linked one.
    pub pull_request: Option<PullRequestRowData>,
    pub issues: Vec<ThreadIssueLink>,
    /// The card's height limit (`max-h-[min(100dvh-6.5rem, …)]`).
    pub max_height: Option<Pixels>,
}

/// What the card asks its owner to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadDetailsEvent {
    OpenUrl(String),
    PullRequestAction(PullRequestRowAction),
}

/// The card and the controls it hosts.
pub struct ThreadDetailsPanel {
    input: ThreadDetailsInput,
    workspace: Entity<BranchToolbar>,
    branch: Entity<BranchToolbar>,
    git: Entity<GitActionsControl>,
    scripts: Option<Entity<ProjectScriptsControl>>,
    open_in: Option<Entity<OpenInPicker>>,
    /// The items menu, at a window position.
    items_menu: Option<Point<Pixels>>,
}

impl EventEmitter<ThreadDetailsEvent> for ThreadDetailsPanel {}

impl ThreadDetailsPanel {
    pub fn new(
        input: ThreadDetailsInput,
        workspace: Entity<BranchToolbar>,
        branch: Entity<BranchToolbar>,
        git: Entity<GitActionsControl>,
        scripts: Option<Entity<ProjectScriptsControl>>,
        open_in: Option<Entity<OpenInPicker>>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_global::<ThreadDetailsItemSettings>(|_, cx| cx.notify()).detach();
        Self { input, workspace, branch, git, scripts, open_in, items_menu: None }
    }

    pub fn set_input(&mut self, input: ThreadDetailsInput, cx: &mut Context<Self>) {
        self.input = input;
        cx.notify();
    }

    pub fn git(&self) -> &Entity<GitActionsControl> {
        &self.git
    }

    pub fn branch(&self) -> &Entity<BranchToolbar> {
        &self.branch
    }

    pub fn workspace(&self) -> &Entity<BranchToolbar> {
        &self.workspace
    }

    pub fn scripts(&self) -> Option<&Entity<ProjectScriptsControl>> {
        self.scripts.as_ref()
    }

    pub fn open_in(&self) -> Option<&Entity<OpenInPicker>> {
        self.open_in.as_ref()
    }

    /// Opens the "Show in thread details" menu at a window position.
    pub fn open_items_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.items_menu = Some(at);
        cx.notify();
    }

    /// Closes the items menu, as choosing an item does.
    pub fn close_items_menu(&mut self, cx: &mut Context<Self>) {
        self.items_menu = None;
        cx.notify();
    }

    fn render_items_menu(&self, at: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let settings = ThreadDetailsItemSettings::get(cx);
        let mut menu = MenuPopup::new().label(t!("Show in thread details"));
        for item in ThreadDetailsItem::ALL {
            let visible = settings.visible(item);
            menu = menu.item(MenuItem::new(item.label()).kind(MenuItemKind::Checkbox(visible)).on_select(move |_, cx| {
                let mut settings = ThreadDetailsItemSettings::get(cx);
                settings.set(item, !visible);
                cx.set_global(settings);
            }));
        }
        let menu = menu
            .separator()
            .item(MenuItem::new(t!("Reset to default")).disabled(settings.is_default()).on_select(|_, cx| {
                cx.set_global(ThreadDetailsItemSettings::default());
            }))
            .on_dismiss({
                let this = cx.entity();
                move |_, cx| this.update(cx, |this, cx| {
                    this.items_menu = None;
                    cx.notify();
                })
            });
        deferred(
            anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                div()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.items_menu = None;
                        cx.notify();
                    }))
                    .child(menu),
            ),
        )
        .with_priority(2)
        .into_any_element()
    }

    fn sections(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let settings = ThreadDetailsItemSettings::get(cx);
        let visible = |item| settings.visible(item);
        let density = self.input.density;
        let git_repo = self.input.offers_git && self.input.is_git_repo;
        let mut workspace = Vec::new();
        if density == ThreadDetailsDensity::Full && visible(ThreadDetailsItem::Environment) {
            workspace.push(self.workspace.clone().into_any_element());
        }
        if density != ThreadDetailsDensity::Essential && visible(ThreadDetailsItem::OpenIn) && self.input.show_open_in {
            workspace.extend(self.open_in.clone().map(IntoElement::into_any_element));
        }
        if visible(ThreadDetailsItem::Scripts) {
            workspace.extend(self.scripts.clone().map(IntoElement::into_any_element));
        }

        let mut version_control = Vec::new();
        if git_repo && visible(ThreadDetailsItem::Branch) {
            version_control.push(self.branch.clone().into_any_element());
        }
        // Without a branch selector, linked pull requests still need somewhere to show.
        if visible(ThreadDetailsItem::Branch) {
            if let Some(pr) = &self.input.pull_request {
                let this = cx.entity();
                let url = pr.url.to_string();
                let action_target = cx.entity();
                version_control.push(pull_request_row(
                    0,
                    theme,
                    pr,
                    move |_, cx| this.update(cx, |_, cx| cx.emit(ThreadDetailsEvent::OpenUrl(url.clone()))),
                    move |action, _, cx| action_target.update(cx, |_, cx| cx.emit(ThreadDetailsEvent::PullRequestAction(action))),
                ));
            }
            for (index, issue) in self.input.issues.iter().enumerate() {
                let this = cx.entity();
                let url = issue.url.clone();
                version_control.push(issue_row(index, theme, issue, move |_, cx| {
                    this.update(cx, |_, cx| cx.emit(ThreadDetailsEvent::OpenUrl(url.clone())))
                }));
            }
        }
        if self.input.offers_git && (visible(ThreadDetailsItem::GitActions) || (visible(ThreadDetailsItem::Changes) && self.input.is_git_repo)) {
            let props_actions = visible(ThreadDetailsItem::GitActions);
            let props_changes = visible(ThreadDetailsItem::Changes);
            let compact = density != ThreadDetailsDensity::Full;
            self.git.update(cx, |git, cx| {
                let mut props = git.props().clone();
                props.show_actions = props_actions;
                props.show_changes = props_changes;
                props.compact = compact;
                git.set_props(props, cx);
            });
            version_control.push(self.git.clone().into_any_element());
        }

        let mut out = Vec::new();
        let has_workspace = !workspace.is_empty();
        out.extend(section(theme, false, workspace));
        out.extend(section(theme, has_workspace && density == ThreadDetailsDensity::Full, version_control));
        out
    }
}

impl Render for ThreadDetailsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let sections = self.sections(&theme, cx);
        // Shown only when no section has content, so an emptied card can be refilled.
        let empty = sections.is_empty().then(|| {
            div().p(theme.rem(0.5)).child(
                row("choose-items", &theme, Some(row_icon(&theme, IconName::SlidersHorizontal)), t!("Choose what to show"), false)
                    .on_click(cx.listener(|this, event: &gpui_kit::ClickEvent, _, cx| {
                        this.open_items_menu(event.position(), cx)
                    })),
            )
        });
        let menu = self.items_menu.map(|at| self.render_items_menu(at, cx));
        glass_surface(&theme)
            .id("thread-details-card")
            .rounded(theme.radii.xxl)
            .w(theme.spacing.thread_details_panel_width)
            .when_some(self.input.max_height, |c, h| c.max_h(h))
            .overflow_y_scroll()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| this.open_items_menu(event.position, cx)),
            )
            .children(sections)
            .children(empty)
            .children(menu)
            .test_support()
    }
}

/// The card's popover position: under the toggle, right-aligned (`align="end"`).
pub fn popover_origin(toggle_bottom_right: Point<Pixels>, cx: &App) -> Point<Pixels> {
    let width = cx.app_theme().spacing.thread_details_panel_width;
    gpui_kit::point(toggle_bottom_right.x - width, toggle_bottom_right.y)
}
