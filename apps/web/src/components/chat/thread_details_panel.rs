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
use std::rc::Rc;

use t3code_contracts::{PullRequestMergeMethod, ThreadIssueLink};

use super::open_in_picker::OpenInPicker;
use super::thread_details_items::{ThreadDetailsItem, ThreadDetailsItemSettings};
use super::thread_details_panel_styles::{row, row_icon, section};
use super::thread_details_pr_row::{
    PullRequestRowAction, PullRequestRowData, PullRequestRowHandlers, checks_popover, issue_row, merge_method_key,
    pull_request_row, resolve_row_action,
};
use crate::components::branch_toolbar::BranchToolbar;
use crate::components::git_actions_control::GitActionsControl;
use crate::components::icons::IconName;
use crate::components::project_scripts_control::ProjectScriptsControl;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
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
    /// Runs a row action. Merge arrives as [`Self::Merge`], after its confirmation.
    PullRequestAction(PullRequestRowAction),
    Merge(PullRequestMergeMethod),
    /// Stop watching the pull request.
    StopWatching,
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
    /// The checks popover is open; `true` in it shows every completed check.
    checks: Option<bool>,
    /// "Merge pull request?" is showing.
    confirming_merge: bool,
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
        Self { input, workspace, branch, git, scripts, open_in, items_menu: None, checks: None, confirming_merge: false }
    }

    pub fn set_input(&mut self, input: ThreadDetailsInput, cx: &mut Context<Self>) {
        self.input = input;
        // The confirmation closes once Merge is no longer the row's action.
        let action = self.input.pull_request.as_ref().and_then(|pr| resolve_row_action(pr.detail.as_ref()));
        if action != Some(PullRequestRowAction::Merge) {
            self.confirming_merge = false;
        }
        cx.notify();
    }

    /// Opens or closes the pull request's checks popover.
    pub fn toggle_checks(&mut self, cx: &mut Context<Self>) {
        self.checks = match self.checks {
            Some(_) => None,
            None => Some(false),
        };
        cx.notify();
    }

    /// Shows the merge confirmation, as the Merge pill does.
    pub fn confirm_merge(&mut self, cx: &mut Context<Self>) {
        self.confirming_merge = true;
        cx.notify();
    }

    fn resolve_merge(&mut self, merge: bool, cx: &mut Context<Self>) {
        self.confirming_merge = false;
        if let Some(pr) = self.input.pull_request.as_ref().filter(|_| merge) {
            cx.emit(ThreadDetailsEvent::Merge(pr.merge_method()));
        }
        cx.notify();
    }

    /// `Merge pull request?` with Cancel and Merge.
    fn render_merge_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pr = self.input.pull_request.as_ref().filter(|_| self.confirming_merge)?;
        let term = pr.term().as_str();
        let method = merge_method_key(pr.merge_method());
        let this = cx.weak_entity();
        let dialog = DialogSurface::new(t!(
            "Merge {term, select, mr {merge request} cr {change request} other {pull request}}?",
            term = term
        ))
        .description(t!(
            "This merges #{number} using {method, select, merge {merge} squash {squash} rebase {rebase} fastForward {fast-forward} other {merge}}.",
            number = pr.number,
            method = method
        ))
        .footer(
            Button::new("merge-cancel")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .label(t!("Cancel"))
                .on_click(cx.listener(|this, _, _, cx| this.resolve_merge(false, cx))),
        )
        .footer(
            Button::new("merge-confirm")
                .size(ButtonSize::Sm)
                .label(t!("Merge", context = "verb, merges a pull request"))
                .on_click(cx.listener(|this, _, _, cx| this.resolve_merge(true, cx))),
        );
        Some(
            deferred(Modal::dialog("merge-confirm-dialog", dialog, move |_, cx| {
                this.update(cx, |this, cx| this.resolve_merge(false, cx)).ok();
            }))
            .with_priority(3)
            .into_any_element(),
        )
    }

    /// The pull request row with its checks popover while open.
    fn pull_request_row(&self, pr: &PullRequestRowData, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.weak_entity();
        let url = pr.url.to_string();
        let open = {
            let this = this.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                this.update(cx, |_, cx| cx.emit(ThreadDetailsEvent::OpenUrl(url.clone()))).ok();
            })
        };
        let action = {
            let this = this.clone();
            Rc::new(move |action, _: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| match action {
                    PullRequestRowAction::Merge => this.confirm_merge(cx),
                    action => cx.emit(ThreadDetailsEvent::PullRequestAction(action)),
                })
                .ok();
            })
        };
        let toggle_checks = {
            let this = this.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| this.toggle_checks(cx)).ok();
            })
        };
        let stop_watching = pr.watching.then(|| {
            let this = this.clone();
            Rc::new(move |_: &mut Window, cx: &mut App| {
                this.update(cx, |_, cx| cx.emit(ThreadDetailsEvent::StopWatching)).ok();
            }) as Rc<dyn Fn(&mut Window, &mut App)>
        });
        let popup = self.checks.zip(pr.detail.as_ref()).map(|(show_all, detail)| {
            let toggle = this.clone();
            let open_url = this.clone();
            let popup = checks_popover(
                theme,
                &detail.checks,
                show_all,
                move |_, cx| {
                    toggle
                        .update(cx, |this, cx| {
                            this.checks = Some(!show_all);
                            cx.notify();
                        })
                        .ok();
                },
                move |url, _, cx| {
                    open_url.update(cx, |_, cx| cx.emit(ThreadDetailsEvent::OpenUrl(url.to_string()))).ok();
                },
            );
            div()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.checks = None;
                    cx.notify();
                }))
                .child(popup)
                .into_any_element()
        });
        pull_request_row(0, theme, pr, PullRequestRowHandlers { open, action, toggle_checks, stop_watching }, popup)
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
                version_control.push(self.pull_request_row(pr, theme, cx));
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
        let merge = self.render_merge_confirm(cx);
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
            .children(merge)
            .test_support()
    }
}

/// The card's popover position: under the toggle, right-aligned (`align="end"`).
pub fn popover_origin(toggle_bottom_right: Point<Pixels>, cx: &App) -> Point<Pixels> {
    let width = cx.app_theme().spacing.thread_details_panel_width;
    gpui_kit::point(toggle_bottom_right.x - width, toggle_bottom_right.y)
}
