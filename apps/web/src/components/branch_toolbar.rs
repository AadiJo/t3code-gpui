//! Where a thread runs and on which ref. Ports `BranchToolbar.tsx`,
//! `BranchToolbarEnvironmentSelector.tsx`, `BranchToolbarEnvModeSelector.tsx`
//! and the trigger of `BranchToolbarBranchSelector.tsx`.
//!
//! Two layouts: the composer's context strip (machine, workspace, then the
//! branch at the right edge, 24px `xs` controls) and the thread details card
//! (one run-context select row and one branch select row). A running turn
//! locks the worktree; a started thread locks the machine.

use std::sync::Arc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Anchor, Entity, EventEmitter, Focusable as _, Hsla, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, Styled as _, Subscription, Window, anchored, deferred, div,
    canvas, point, prelude::*, px, relative, TestSupportExt as _,
};
use t3code_contracts::{EnvironmentId, VcsCreateRefInput, VcsListRefsResult, VcsRef, VcsSwitchRefInput};

use super::branch_picker::{BranchPicker, BranchPickerEvent, BranchPickerItem, BranchPickerOptions};
use super::branch_toolbar_logic::{
    EnvMode, EnvironmentOption, MachineKind, PreviousWorktreeSeed, derive_local_branch_name_from_remote_ref,
    is_worktree_change_blocked, resolve_branch_selection_target, resolve_branch_toolbar_value,
    resolve_branch_trigger_label, resolve_current_workspace_label, resolve_env_mode_label,
    resolve_context_strip_labels_compact, resolve_locked_workspace_label, sanitize_new_ref_name,
    should_show_environment_indicator,
};
use super::chat::thread_details_panel_styles::{DismissPress, below_row, locked_row, row_width, row_icon, select_row};
use super::git_actions_control_source::SourceControlModel;
use super::icons::{Icon, IconName, IconSize};
use super::previous_worktree_item_content::previous_worktree_item_content;
use super::ui::popover::glass_surface;
use super::ui::toast::{ToastData, ToastKind, Toaster};
use super::ui::tooltip::tooltip;
use super::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::ActiveAppTheme as _;
use crate::AppTheme;
use crate::appearance_fonts::{TextSize, mono_font, weight};
use crate::i18n::t;
use crate::source_control_presentation::source_control_presentation;
use crate::theme_color::with_alpha;

/// Which part of the toolbar to draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BranchToolbarLayout {
    /// The composer's context strip.
    #[default]
    Composer,
    /// The thread details card's run-context row.
    PanelWorkspace,
    /// The thread details card's branch row.
    PanelBranch,
}

/// What the toolbar shows and allows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchToolbarProps {
    pub environments: Vec<EnvironmentOption>,
    pub environment_id: Option<EnvironmentId>,
    /// "Auto balance" picked a machine; this names it.
    pub auto_environment_label: Option<SharedString>,
    /// The owner handles machine changes (a draft); a started thread only shows its machine.
    pub can_change_environment: bool,
    pub can_auto_balance: bool,
    pub env_mode: EnvMode,
    /// Each model of a multi-model send starts in its own worktree.
    pub force_new_worktree: bool,
    pub worktree_path: Option<String>,
    pub project_cwd: String,
    pub is_server_thread: bool,
    /// A started thread or a running send: the machine is fixed.
    pub env_locked: bool,
    pub thread_branch: Option<String>,
    pub start_from_origin: bool,
    pub previous_worktree: Option<PreviousWorktreeSeed>,
    /// A turn is running and can be interrupted, which pins its worktree.
    pub turn_running: bool,
    pub can_write_source_control: bool,
    pub can_change_thread_branch: bool,
    pub can_checkout_pull_request: bool,
    pub show_git_controls: bool,
    /// The branch's (or thread's) pull request, shown before the branch in the composer strip.
    pub pull_request_badge: Option<StripPullRequestBadge>,
}

/// The badge's glass tooltip: one mini-list row (`ThreadPullRequestMiniListItem`).
#[derive(Clone)]
struct BadgeTooltip {
    icon: IconName,
    tone: Hsla,
    number: u64,
    title: SharedString,
}

impl Render for BadgeTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme();
        let d = &theme.derived;
        div().pb(px(4.)).child(
            div()
                .w(theme.rem(20.))
                .px(theme.sp(1.))
                .py(theme.sp(1.))
                .rounded(theme.radii.md)
                .border_1()
                .bg(d.dropdown_glass_opaque)
                .border_color(d.dropdown_glass_border)
                .shadow(theme.shadows.xl25.clone())
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_center()
                        .gap(theme.rem(0.5))
                        .px(theme.rem(0.25))
                        .py(theme.rem(0.25))
                        .rounded(theme.radii.sm)
                        .ui_text(theme, TextSize::Xs, weight::NORMAL)
                        .text_color(theme.colors.foreground())
                        .child(Icon::new(self.icon).size(IconSize::Custom(theme.rem(0.75))).color(self.tone))
                        .child(div().flex_none().font(mono_font(weight::NORMAL)).child(format!("#{}", self.number)))
                        .child(div().min_w_0().truncate().text_color(with_alpha(theme.colors.foreground(), 0.75)).child(self.title.clone())),
                ),
        )
    }
}

/// The composer strip's pull request badge (`ThreadPullRequestBadgeControl`):
/// the state icon and `#N` (or `+N` when more are linked), with a tooltip that
/// lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct StripPullRequestBadge {
    pub number: u64,
    pub url: SharedString,
    pub title: SharedString,
    pub state: t3code_contracts::PullRequestState,
    pub is_draft: bool,
    /// Other linked pull requests: the badge reads `+N` and opens the list.
    pub others: u64,
    /// The status tooltip (`PrStatusIndicator.tooltip`).
    pub label: SharedString,
}
#[derive(Clone, Debug, PartialEq)]
pub enum BranchToolbarEvent {
    EnvironmentChanged(EnvironmentId),
    AutoEnvironment,
    EnvModeChanged(EnvMode),
    UsePreviousWorktree,
    StartFromOriginChanged(bool),
    /// The thread moved to this ref (and worktree).
    ThreadBranchChanged { branch: String, worktree_path: Option<String> },
    /// Check this pull request out into a thread.
    CheckoutPullRequest(String),
    /// The badge of a single pull request: open it.
    OpenPullRequest(SharedString),
    /// The badge of several: open the Pull requests tab.
    OpenPullRequestList,
}

/// The open popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Popup {
    RunContext,
    Environment,
    Workspace,
    Branch,
}

/// The branch and worktree controls of one thread.
pub struct BranchToolbar {
    layout: BranchToolbarLayout,
    props: BranchToolbarProps,
    model: Entity<SourceControlModel>,
    /// The model's shared ref list, mirrored so render can borrow it.
    refs: Option<Arc<VcsListRefsResult>>,
    /// The composer strip's measured width.
    strip_width: Option<Pixels>,
    /// The strip's labels collapsed to icons (`data-compact`).
    compact: bool,
    open: Option<Popup>,
    picker: Option<(Entity<BranchPicker>, Subscription)>,
    /// A checkout or create is in flight.
    pending: bool,
    dismissed: DismissPress,
}

impl EventEmitter<BranchToolbarEvent> for BranchToolbar {}

/// Composer control `xs`: 24px tall, 7px padding, 4px gap, 12px text.

/// The first page of refs the picker loads (`OFFLINE_BRANCH_LIST_LIMIT`).
const REFS_PAGE: u64 = 100;
/// "Stop the current turn to switch worktrees."
fn worktree_change_blocked() -> SharedString {
    t!("Stop the current turn to switch worktrees.")
}

fn machine_icon(kind: MachineKind) -> IconName {
    match kind {
        MachineKind::Laptop => IconName::Laptop,
        MachineKind::Desktop => IconName::Monitor,
        MachineKind::Server => IconName::Server,
    }
}

/// Floats `content` above where this element is laid out, its `corner` on the
/// anchor (the composer strip opens popups upward).
fn above(corner: Anchor, content: impl IntoElement) -> impl IntoElement {
    deferred(anchored().anchor(corner).offset(point(px(0.), px(-4.))).snap_to_window_with_margin(px(8.)).child(content))
        .with_priority(1)
}

impl BranchToolbar {
    pub fn new(
        layout: BranchToolbarLayout,
        model: Entity<SourceControlModel>,
        props: BranchToolbarProps,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |this: &mut Self, model, cx| {
            let refs = model.read(cx).refs.clone();
            if refs.as_ref().map(Arc::as_ptr) != this.refs.as_ref().map(Arc::as_ptr) {
                if let (Some((picker, _)), Some(refs)) = (&this.picker, &refs) {
                    let listed = refs.refs.clone();
                    picker.update(cx, |p, cx| p.set_refs(listed, cx));
                }
                this.refs = refs;
            }
            cx.notify();
        })
        .detach();
        let refs = model.read(cx).refs.clone();
        let mut toolbar = Self {
            layout,
            props,
            model,
            refs,
            open: None,
            picker: None,
            pending: false,
            dismissed: DismissPress::default(),
            strip_width: None,
            compact: false,
        };
        if toolbar.refs.is_none() {
            toolbar.load_refs(cx);
        }
        toolbar
    }

    pub fn props(&self) -> &BranchToolbarProps {
        &self.props
    }

    pub fn set_props(&mut self, props: BranchToolbarProps, cx: &mut Context<Self>) {
        if self.props != props {
            self.props = props;
            if let Some((picker, _)) = &self.picker {
                let options = self.picker_options(cx);
                picker.update(cx, |p, cx| p.set_options(options, cx));
            }
            cx.notify();
        }
    }

    /// Loads (or reloads) the thread's shared first page of refs.
    fn load_refs(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.load_refs(REFS_PAGE, cx));
    }

    fn refs(&self) -> &[VcsRef] {
        self.refs.as_ref().map_or(&[], |r| r.refs.as_slice())
    }

    fn active_worktree_path(&self) -> Option<&str> {
        if self.props.force_new_worktree { None } else { self.props.worktree_path.as_deref() }
    }

    fn effective_env_mode(&self) -> EnvMode {
        if self.props.force_new_worktree { EnvMode::Worktree } else { self.props.env_mode }
    }

    fn env_mode_locked(&self) -> bool {
        self.props.env_locked || (self.props.is_server_thread && self.active_worktree_path().is_some())
    }

    fn selecting_worktree_base(&self) -> bool {
        self.effective_env_mode() == EnvMode::Worktree && !self.props.env_locked && self.active_worktree_path().is_none()
    }

    fn current_git_branch<'a>(&'a self, cx: &'a App) -> Option<&'a str> {
        self.model
            .read(cx)
            .status
            .as_ref()
            .and_then(|s| s.ref_name.as_deref())
            .or_else(|| self.refs().iter().find(|r| r.current).map(|r| r.name.as_str()))
    }

    /// The ref the trigger names (`resolveBranchToolbarValue`).
    pub fn resolved_branch(&self, cx: &App) -> Option<String> {
        resolve_branch_toolbar_value(
            self.effective_env_mode(),
            self.active_worktree_path(),
            self.props.thread_branch.as_deref(),
            self.current_git_branch(cx),
        )
        .map(str::to_owned)
    }

    fn trigger_label(&self, cx: &App) -> SharedString {
        let branch = self.resolved_branch(cx);
        let is_remote = branch
            .as_deref()
            .and_then(|b| self.refs().iter().find(|r| r.name == b))
            .map(|r| r.is_remote == Some(true));
        resolve_branch_trigger_label(
            self.active_worktree_path(),
            self.effective_env_mode(),
            branch.as_deref(),
            is_remote,
            self.props.start_from_origin,
        )
    }

    fn next_worktree_for(&self, r: &VcsRef) -> Option<String> {
        if self.selecting_worktree_base() {
            return None;
        }
        resolve_branch_selection_target(
            &self.props.project_cwd,
            None,
            self.active_worktree_path(),
            r.is_default,
            r.worktree_path.as_deref(),
        )
        .next_worktree_path
    }

    fn is_blocked(&self, r: &VcsRef) -> bool {
        is_worktree_change_blocked(
            self.props.turn_running,
            self.props.worktree_path.as_deref(),
            self.next_worktree_for(r).as_deref(),
        )
    }

    fn picker_options(&self, cx: &App) -> BranchPickerOptions {
        let blocked: Vec<String> = self.refs().iter().filter(|r| self.is_blocked(r)).map(|r| r.name.clone()).collect();
        let presentation = source_control_presentation(
            self.model.read(cx).status.as_ref().and_then(|s| s.source_control_provider.as_ref()),
        );
        let term = presentation.terminology.term_kind().as_str();
        let status = match &self.refs {
            None => Some(t!("Loading refs...")),
            Some(r) if r.next_cursor.is_some() => Some(t!(
                "{total, plural, one {Showing {shown} of {total} refs} other {Showing {shown} of {total} refs}}",
                shown = r.refs.len(),
                total = r.total_count
            )),
            Some(_) => None,
        };
        let mut disabled_refs = blocked.clone();
        if !self.props.can_change_thread_branch {
            disabled_refs.extend(self.refs().iter().map(|r| r.name.clone()));
        }
        BranchPickerOptions {
            can_create_ref: self.props.can_change_thread_branch && !self.selecting_worktree_base(),
            can_checkout_pull_request: self.props.can_change_thread_branch && self.props.can_checkout_pull_request,
            term,
            host_icon: Some(presentation.icon),
            start_from_origin: self.selecting_worktree_base().then_some(self.props.start_from_origin),
            notice: (!blocked.is_empty()).then(worktree_change_blocked),
            status,
            disabled_refs,
            project_cwd: Some(self.props.project_cwd.clone()),
            width: Some(match self.layout {
                BranchToolbarLayout::Composer => cx.app_theme().rem(20.),
                _ => row_width(cx.app_theme()),
            }),
        }
    }

    /// Opens or closes the branch picker, focusing its search field.
    pub fn set_branch_picker_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !open {
            self.close(cx);
            return;
        }
        if self.pending {
            return;
        }
        let value = self.resolved_branch(cx);
        let options = self.picker_options(cx);
        let refs = self.refs().to_vec();
        let picker = cx.new(|cx| BranchPicker::new(refs, value, options, cx));
        let subscription = cx.subscribe(&picker, |this, _, event: &BranchPickerEvent, cx| match event {
            BranchPickerEvent::Dismiss(at) => {
                if let Some(at) = at {
                    this.dismissed.record(*at);
                }
                this.close(cx)
            }
            BranchPickerEvent::StartFromOriginChanged(on) => cx.emit(BranchToolbarEvent::StartFromOriginChanged(*on)),
            BranchPickerEvent::Select(item) => {
                let item = item.clone();
                this.close(cx);
                this.select(item, cx);
            }
        });
        window.focus(&picker.focus_handle(cx), cx);
        self.picker = Some((picker, subscription));
        self.open = Some(Popup::Branch);
        self.refresh_local_status(cx);
        cx.notify();
    }

    /// The open picker, for scripted typing and keys.
    pub fn branch_picker(&self) -> Option<&Entity<BranchPicker>> {
        self.picker.as_ref().map(|(p, _)| p)
    }

    /// Opens the run-context, machine or workspace menu (for scenes).
    pub fn open_menu(&mut self, menu: &str, cx: &mut Context<Self>) {
        self.open = match menu {
            "run-context" => Some(Popup::RunContext),
            "environment" => Some(Popup::Environment),
            "workspace" => Some(Popup::Workspace),
            _ => None,
        };
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.picker = None;
        cx.notify();
    }

    fn toggle_from(&mut self, popup: Popup, event: &gpui_kit::ClickEvent, cx: &mut Context<Self>) {
        if !self.dismissed.consumes(event) {
            self.toggle(popup, cx);
        }
    }

    fn toggle(&mut self, popup: Popup, cx: &mut Context<Self>) {
        self.open = if self.open == Some(popup) { None } else { Some(popup) };
        self.picker = None;
        cx.notify();
    }

    /// Opening the picker refreshes local status (`refreshLocalStatusForPicker`).
    fn refresh_local_status(&self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let request = model.source.refresh_status(t3code_contracts::VcsRefreshStatusPayload {
            cwd: model.cwd.to_string(),
            local_only: Some(true),
        });
        cx.background_spawn(async move {
            let _ = request.await;
        })
        .detach();
    }

    fn select(&mut self, item: BranchPickerItem, cx: &mut Context<Self>) {
        match item {
            BranchPickerItem::CheckoutPullRequest(reference) => cx.emit(BranchToolbarEvent::CheckoutPullRequest(reference)),
            BranchPickerItem::CreateRef(query) => self.create_ref(query, cx),
            BranchPickerItem::Ref(r) => self.select_ref(r, cx),
        }
    }

    /// `selectBranch`: a new worktree's base is only remembered; otherwise the
    /// checkout switches, reusing the worktree that already has the ref.
    fn select_ref(&mut self, r: VcsRef, cx: &mut Context<Self>) {
        if self.is_blocked(&r) {
            return;
        }
        let local_name = if r.is_remote == Some(true) {
            derive_local_branch_name_from_remote_ref(&r.name).to_owned()
        } else {
            r.name.clone()
        };
        if self.selecting_worktree_base() {
            cx.emit(BranchToolbarEvent::ThreadBranchChanged { branch: r.name.clone(), worktree_path: None });
            return;
        }
        let target = resolve_branch_selection_target(
            &self.props.project_cwd,
            None,
            self.active_worktree_path(),
            r.is_default,
            r.worktree_path.as_deref(),
        );
        if target.reuse_existing_worktree && !target.stays_in_active_checkout {
            cx.emit(BranchToolbarEvent::ThreadBranchChanged { branch: r.name.clone(), worktree_path: target.next_worktree_path });
            return;
        }
        if !self.props.can_write_source_control {
            return;
        }
        let request =
            self.model.read(cx).source.switch_ref(VcsSwitchRefInput { cwd: target.checkout_cwd.clone(), ref_name: r.name.clone() });
        self.pending = true;
        cx.notify();
        let next_worktree = target.next_worktree_path;
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.pending = false;
                match result {
                    Ok(switched) => cx.emit(BranchToolbarEvent::ThreadBranchChanged {
                        branch: switched.ref_name.unwrap_or(local_name),
                        worktree_path: next_worktree,
                    }),
                    Err(error) => {
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Failed to switch ref.")).description(error.0), cx);
                    }
                }
                this.load_refs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `createRef`: creates the ref in the current checkout and switches to it.
    fn create_ref(&mut self, raw: String, cx: &mut Context<Self>) {
        let name = sanitize_new_ref_name(&raw);
        if name.is_empty() || !self.props.can_write_source_control {
            return;
        }
        let cwd = self.active_worktree_path().unwrap_or(&self.props.project_cwd).to_owned();
        let request = self.model.read(cx).source.create_ref(VcsCreateRefInput { cwd, ref_name: name, switch_ref: Some(true) });
        let worktree = self.active_worktree_path().map(str::to_owned);
        self.pending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.pending = false;
                match result {
                    Ok(created) => {
                        cx.emit(BranchToolbarEvent::ThreadBranchChanged { branch: created.ref_name, worktree_path: worktree })
                    }
                    Err(error) => {
                        Toaster::push(
                            ToastData::new(ToastKind::Error, t!("Failed to create and switch ref.")).description(error.0),
                            cx,
                        );
                    }
                }
                this.load_refs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn workspace_icon(&self) -> IconName {
        if self.effective_env_mode() == EnvMode::Worktree {
            IconName::FolderGit2
        } else if self.active_worktree_path().is_some() {
            IconName::FolderGit
        } else {
            IconName::Folder
        }
    }

    fn workspace_label(&self) -> SharedString {
        if self.props.force_new_worktree {
            resolve_env_mode_label(EnvMode::Worktree)
        } else if self.env_mode_locked() {
            resolve_locked_workspace_label(self.active_worktree_path(), self.effective_env_mode())
        } else if self.effective_env_mode() == EnvMode::Worktree {
            resolve_env_mode_label(EnvMode::Worktree)
        } else {
            resolve_current_workspace_label(self.active_worktree_path())
        }
    }

    fn active_environment(&self) -> Option<&EnvironmentOption> {
        let id = self.props.environment_id.as_ref()?;
        self.props.environments.iter().find(|e| &e.environment_id == id)
    }

    fn environment_label(&self) -> SharedString {
        self.props
            .auto_environment_label
            .clone()
            .or_else(|| self.active_environment().map(|e| e.label.clone()))
            .unwrap_or_else(|| t!("Run on"))
    }

    fn can_pick_environment(&self) -> bool {
        self.props.environments.len() > 1
    }

    /// A menu row: 28px, 12px icon, check-free radio selection fill.
    fn menu_row(
        &self,
        id: impl Into<gpui_kit::ElementId>,
        theme: &AppTheme,
        icon: Option<IconName>,
        content: AnyElement,
        selected: bool,
        disabled: bool,
        on_select: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let highlight = theme.colors.highlight();
        div()
            .id(id.into())
            .flex()
            .items_center()
            .gap(theme.rem(0.375))
            .min_h(theme.sp(7.))
            .px(theme.sp(2.))
            .py(theme.sp(1.))
            .rounded(theme.radii.sm)
            .ui_text(theme, TextSize::Sm, weight::NORMAL)
            .when(selected, |r| r.bg(theme.derived.item_selected))
            .when(disabled, |r| r.opacity(DISABLED_OPACITY))
            .when(!disabled, |r| {
                r.cursor_pointer().hover(move |s| s.bg(highlight)).on_click(cx.listener(move |this, _, _, cx| {
                    this.open = None;
                    on_select(this, cx);
                    cx.notify();
                }))
            })
            .children(icon.map(|i| Icon::new(i).size(IconSize::Custom(theme.rem(0.75)))))
            .child(content)
            .into_any_element()
    }

    fn group_label(theme: &AppTheme, label: SharedString) -> AnyElement {
        div()
            .px(theme.sp(2.))
            .py(theme.sp(1.5))
            .ui_text(theme, TextSize::Xs, weight::MEDIUM)
            .text_color(theme.colors.muted_foreground)
            .child(label)
            .into_any_element()
    }

    fn environment_rows(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![Self::group_label(theme, t!("Run on"))];
        let locked = self.props.env_locked;
        if self.props.can_auto_balance {
            let label = self.props.auto_environment_label.clone().unwrap_or_else(|| t!("Auto balance"));
            rows.push(self.menu_row(
                "env-auto",
                theme,
                Some(IconName::Scale),
                div().min_w_0().truncate().child(label).into_any_element(),
                self.props.auto_environment_label.is_some(),
                locked,
                |_, cx| cx.emit(BranchToolbarEvent::AutoEnvironment),
                cx,
            ));
        }
        for (index, env) in self.props.environments.iter().enumerate() {
            let selected = self.props.auto_environment_label.is_none() && Some(&env.environment_id) == self.props.environment_id.as_ref();
            let id = env.environment_id.clone();
            let disabled = locked || (!self.props.can_change_environment && !selected);
            rows.push(self.menu_row(
                ("env-option", index),
                theme,
                Some(machine_icon(env.machine)),
                div().min_w_0().truncate().child(env.label.clone()).into_any_element(),
                selected,
                disabled,
                move |_, cx| cx.emit(BranchToolbarEvent::EnvironmentChanged(id.clone())),
                cx,
            ));
        }
        rows
    }

    fn workspace_rows(&self, theme: &AppTheme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let locked = self.env_mode_locked();
        let mode = self.effective_env_mode();
        let mut rows = vec![Self::group_label(theme, t!("Workspace"))];
        rows.push(self.menu_row(
            "workspace-local",
            theme,
            Some(if self.active_worktree_path().is_some() { IconName::FolderGit } else { IconName::Folder }),
            div().min_w_0().truncate().child(resolve_current_workspace_label(self.active_worktree_path())).into_any_element(),
            mode == EnvMode::Local,
            locked || self.props.force_new_worktree,
            |_, cx| cx.emit(BranchToolbarEvent::EnvModeChanged(EnvMode::Local)),
            cx,
        ));
        rows.push(self.menu_row(
            "workspace-worktree",
            theme,
            Some(IconName::FolderGit2),
            div().min_w_0().truncate().child(resolve_env_mode_label(EnvMode::Worktree)).into_any_element(),
            mode == EnvMode::Worktree,
            locked,
            |_, cx| cx.emit(BranchToolbarEvent::EnvModeChanged(EnvMode::Worktree)),
            cx,
        ));
        let can_use_previous = !self.props.is_server_thread && !locked && !self.props.force_new_worktree;
        if let Some(seed) = self.props.previous_worktree.as_ref().filter(|_| can_use_previous) {
            rows.push(self.menu_row(
                "workspace-previous",
                theme,
                None,
                previous_worktree_item_content(theme, seed.branch.as_deref()).into_any_element(),
                false,
                locked,
                |_, cx| cx.emit(BranchToolbarEvent::UsePreviousWorktree),
                cx,
            ));
        }
        rows
    }

    fn popup_surface(theme: &AppTheme, width: Option<Pixels>, rows: Vec<AnyElement>, cx: &mut Context<Self>) -> impl IntoElement {
        glass_surface(theme)
            .id("branch-toolbar-menu")
            .occlude()
            .min_w(theme.rem(10.))
            .when_some(width, |s, w| s.w(w))
            .p(theme.sp(1.))
            .on_mouse_down_out(cx.listener(|this, event: &gpui_kit::MouseDownEvent, _, cx| {
                if this.open != Some(Popup::Branch) {
                    this.open = None;
                    this.dismissed.record(event.position);
                    cx.notify();
                }
            }))
            .children(rows)
    }

    /// A composer `xs` control: 24px, 12px icon and text.
    fn xs_control(id: &'static str, theme: &AppTheme, text: Hsla, open: bool, disabled: bool) -> gpui_kit::Stateful<gpui_kit::Div> {
        let hover = theme.colors.sidebar_row_hover;
        div()
            .id(id)
            .flex()
            .flex_none()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.25))
            .h(theme.rem(1.5))
            .px(px(7.))
            .rounded(theme.radii.control)
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(text)
            .when(open, |c| c.bg(hover).text_color(theme.colors.foreground()))
            .when(disabled, |c| c.opacity(DISABLED_OPACITY))
            .when(!disabled, |c| c.cursor_pointer().hover(move |s| s.bg(hover)))
    }

    /// A strip label: hidden while the strip is compact (`ComposerContextLabel`).
    fn strip_label(&self, label: SharedString) -> impl IntoElement {
        div().min_w_0().max_w(px(240.)).truncate().when(self.compact, |l| l.max_w(px(0.)).invisible()).child(label)
    }

    /// `run-context` in the strip below the `@xl` composer width: one control
    /// for the machine and workspace.
    fn render_run_context_control(&mut self, theme: &AppTheme, text: Hsla, show_indicator: bool, cx: &mut Context<Self>) -> AnyElement {
        let label = self
            .props
            .auto_environment_label
            .clone()
            .unwrap_or_else(|| if show_indicator { self.environment_label() } else { self.workspace_label() });
        let icon = if self.props.auto_environment_label.is_some() {
            IconName::Scale
        } else if show_indicator {
            machine_icon(self.active_environment().map_or(MachineKind::Server, |e| e.machine))
        } else {
            self.workspace_icon()
        };
        let locked = self.props.env_locked && (self.env_mode_locked() || self.props.force_new_worktree);
        let open = self.open == Some(Popup::RunContext);
        let control = Self::xs_control("strip-run-context", theme, text, open, false)
            .flex_initial()
            .max_w(relative(0.48))
            .child(Icon::new(icon).size(IconSize::Custom(theme.rem(0.75))))
            .child(self.strip_label(label));
        let control = if locked {
            control.cursor_default()
        } else {
            control.on_click(cx.listener(|this, e, _, cx| this.toggle_from(Popup::RunContext, e, cx)))
        };
        let menu = open.then(|| {
            let mut rows = Vec::new();
            if self.can_pick_environment() && !self.props.environments.is_empty() {
                rows.extend(self.environment_rows(theme, cx));
                rows.push(div().mx(theme.sp(2.)).my(theme.sp(1.)).h(px(1.)).bg(theme.colors.border).into_any_element());
            }
            rows.extend(self.workspace_rows(theme, cx));
            div().absolute().top_0().left_0().child(above(Anchor::BottomLeft, Self::popup_surface(theme, None, rows, cx)))
        });
        div().relative().min_w_0().child(control.test_support()).children(menu).into_any_element()
    }

    /// The pull request badge before the branch (`ThreadPullRequestBadgeControl`).
    fn render_pull_request_badge(&self, theme: &AppTheme, text: Hsla, cx: &mut Context<Self>) -> Option<AnyElement> {
        let badge = self.props.pull_request_badge.clone()?;
        let (icon, tone) =
            crate::components::chat::thread_details_pr_row::pull_request_state_tone(theme, badge.state, badge.is_draft);
        let opens_list = badge.others > 0;
        let label: SharedString = if opens_list { format!("+{}", badge.others + 1).into() } else { format!("#{}", badge.number).into() };
        let url = badge.url.clone();
        let tip_title = badge.title.clone();
        let number = badge.number;
        let tip = BadgeTooltip { icon, tone, number, title: tip_title };
        let tip = move |_: &mut Window, cx: &mut App| cx.new(|_| tip.clone()).into();
        let control = Self::xs_control("strip-pull-request", theme, text, false, false)
            .tooltip(tip)
            .child(Icon::new(icon).size(IconSize::Custom(theme.rem(0.75))).color(tone))
            .child(div().text_color(with_alpha(theme.colors.foreground(), 0.6)).child(label))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(if opens_list { BranchToolbarEvent::OpenPullRequestList } else { BranchToolbarEvent::OpenPullRequest(url.clone()) })
            }));
        Some(control.test_support().into_any_element())
    }

    fn render_composer(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        // `text-foreground/70 dark:text-muted-foreground`.
        let text = if theme.is_dark() { theme.colors.muted_foreground } else { with_alpha(theme.colors.foreground(), 0.7) };
        let show_indicator = should_show_environment_indicator(self.active_environment().map(|e| e.is_primary), self.can_pick_environment());
        // `@xl/composer-surface`: below 36rem the machine and workspace share one control.
        let narrow = self.strip_width.is_some_and(|w| w < theme.rem(36.));
        let mut left = div().flex().flex_1().min_w_0().items_center().gap(theme.rem(0.25)).min_h(theme.rem(1.5));
        if narrow && self.props.show_git_controls {
            left = left.child(self.render_run_context_control(theme, text, show_indicator, cx));
        } else if show_indicator {
            let locked = self.props.env_locked;
            let icon = if self.props.auto_environment_label.is_some() {
                IconName::Scale
            } else {
                machine_icon(self.active_environment().map_or(MachineKind::Server, |e| e.machine))
            };
            let control = Self::xs_control("strip-environment", theme, text, self.open == Some(Popup::Environment), false)
                .tooltip(tooltip(self.environment_label()))
                .child(Icon::new(icon).size(IconSize::Custom(theme.rem(0.75))))
                .child(self.strip_label(self.environment_label()));
            let control = if locked || !self.can_pick_environment() {
                control.cursor_default()
            } else {
                control.on_click(cx.listener(|this, e, _, cx| this.toggle_from(Popup::Environment, e, cx)))
            };
            let menu = (self.open == Some(Popup::Environment)).then(|| {
                let rows = self.environment_rows(theme, cx);
                div().absolute().top_0().left_0().child(above(Anchor::BottomLeft, Self::popup_surface(theme, None, rows, cx)))
            });
            left = left.child(div().relative().child(control.test_support()).children(menu));
            if self.props.show_git_controls {
                left = left.child(div().mx(theme.rem(0.125)).w(px(1.)).h(theme.rem(0.875)).bg(theme.colors.border));
            }
        }
        if self.props.show_git_controls && !narrow {
            let locked = self.env_mode_locked() || self.props.force_new_worktree;
            let tip = if self.props.force_new_worktree {
                t!("Each model starts in its own worktree.")
            } else {
                self.active_worktree_path().map(|p| SharedString::from(p.to_owned())).unwrap_or_else(|| self.workspace_label())
            };
            let mode = Self::xs_control("strip-workspace", theme, text, self.open == Some(Popup::Workspace), false)
                .tooltip(tooltip(tip))
                .child(Icon::new(self.workspace_icon()).size(IconSize::Custom(theme.rem(0.75))))
                .child(self.strip_label(self.workspace_label()))
                .when(!locked, |c| c.child(Icon::new(IconName::ChevronsUpDown).size(IconSize::Custom(theme.rem(0.75))).opacity(0.7)));
            let mode = if locked {
                mode.cursor_default().text_color(with_alpha(theme.colors.muted_foreground, 0.7))
            } else {
                mode.on_click(cx.listener(|this, e, _, cx| this.toggle_from(Popup::Workspace, e, cx)))
            };
            let menu = (self.open == Some(Popup::Workspace)).then(|| {
                let rows = self.workspace_rows(theme, cx);
                let width = self.props.previous_worktree.as_ref().map(|_| theme.rem(21.));
                div().absolute().top_0().left_0().child(above(Anchor::BottomLeft, Self::popup_surface(theme, width, rows, cx)))
            });
            left = left.child(div().relative().min_w_0().child(mode.test_support()).children(menu));
        }
        let mut strip = div()
            .flex()
            .w_full()
            .items_center()
            .gap(theme.rem(0.25))
            .px(theme.rem(0.5))
            .pt(theme.rem(0.5))
            .ui_text(theme, TextSize::Xs, weight::NORMAL)
            .text_color(text)
            .child(left);
        if self.props.show_git_controls {
            let disabled = self.refs.is_none() || self.pending;
            let branch = Self::xs_control("strip-branch", theme, text, self.open == Some(Popup::Branch), disabled)
                .max_w(theme.rem(16.25))
                .tooltip(tooltip(t!("Branch: {branch}", branch = self.trigger_label(cx))))
                .child(Icon::new(IconName::GitBranch).size(IconSize::Custom(theme.rem(0.75))))
                .child(self.strip_label(self.trigger_label(cx)))
                .when(!disabled, |c| {
                    c.on_click(cx.listener(|this, e, window, cx| {
                        if this.dismissed.consumes(e) {
                            return;
                        }
                        let open = this.open != Some(Popup::Branch);
                        this.set_branch_picker_open(open, window, cx);
                    }))
                });
            let picker = self.picker.as_ref().filter(|_| self.open == Some(Popup::Branch)).map(|(picker, _)| {
                div().absolute().top_0().right_0().child(above(Anchor::BottomRight, picker.clone()))
            });
            let badge = self.render_pull_request_badge(theme, text, cx);
            strip = strip.child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(theme.rem(0.25))
                    .when(!narrow, |d| d.ml_auto())
                    .children(badge)
                    .child(div().relative().min_w_0().child(branch.test_support()).children(picker)),
            );
        }
        let labels = self.strip_labels(cx);
        let this = cx.entity();
        strip
            .child(
                // Measures the strip after layout: its width picks the narrow layout, and
                // the labels' natural widths decide whether they collapse to icons.
                canvas(
                    move |bounds, window, cx| {
                        let theme = cx.app_theme();
                        let size = px(TextSize::Xs.size(theme.fonts.interface));
                        let font = crate::appearance_fonts::ui_font(weight::NORMAL);
                        let text_width: Pixels = labels
                            .iter()
                            .map(|label| {
                                let run = gpui_kit::TextRun { len: label.len(), font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
                                window.text_system().shape_line(label.clone(), size, &[run], None).width.min(px(240.))
                            })
                            .fold(px(0.), |a, b| a + b);
                        (bounds.size.width, text_width)
                    },
                    move |_, (width, text_width), _, cx| {
                        this.update(cx, |this, cx| this.measure_strip(width, text_width, cx));
                    },
                )
                .absolute()
                .size_full(),
            )
            .relative()
            .into_any_element()
    }

    /// The strip's labels as drawn, for the overflow measure.
    fn strip_labels(&self, cx: &App) -> Vec<SharedString> {
        let mut labels = Vec::new();
        let show_indicator = should_show_environment_indicator(self.active_environment().map(|e| e.is_primary), self.can_pick_environment());
        if show_indicator {
            labels.push(self.environment_label());
        }
        if self.props.show_git_controls {
            labels.push(self.workspace_label());
            labels.push(self.trigger_label(cx));
        }
        labels
    }

    /// `useLabelsOverflow` and the `@xl` container query, from the measured strip.
    /// Each control's chrome (padding, icon, gap) is counted with the labels'
    /// natural widths; once compact, labels return only with 16px to spare.
    fn measure_strip(&mut self, width: Pixels, text_width: Pixels, cx: &mut Context<Self>) {
        let theme = cx.app_theme();
        let controls = self.strip_labels(cx).len() as f32;
        let chrome = (px(14.) + theme.rem(0.75) + theme.rem(0.25)) * controls + theme.rem(0.25) * (controls - 1.).max(0.);
        let available = width - theme.rem(1.);
        let needed = text_width + chrome;
        let compact = resolve_context_strip_labels_compact(self.compact, needed.into(), available.into());
        if self.strip_width != Some(width) || self.compact != compact {
            self.strip_width = Some(width);
            self.compact = compact;
            cx.notify();
        }
    }

    fn render_panel_workspace(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let show_indicator = self.active_environment().is_some();
        let locked = self.props.env_locked || (self.env_mode_locked() && !self.can_pick_environment());
        let label = if show_indicator { self.environment_label() } else { self.workspace_label() };
        let icon = if show_indicator {
            if self.props.auto_environment_label.is_some() {
                IconName::Scale
            } else {
                machine_icon(self.active_environment().map_or(MachineKind::Server, |e| e.machine))
            }
        } else {
            self.workspace_icon()
        };
        let workspace_path = if self.props.force_new_worktree || (self.effective_env_mode() == EnvMode::Worktree && self.active_worktree_path().is_none()) {
            None
        } else {
            Some(self.active_worktree_path().unwrap_or(&self.props.project_cwd).to_owned())
        };
        let workspace_tip = if self.props.force_new_worktree {
            t!("Each model starts in its own worktree.")
        } else {
            workspace_path.clone().map(SharedString::from).unwrap_or_else(|| self.workspace_label())
        };
        let content = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.625))
            .child(row_icon(theme, icon))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(show_indicator, |d| {
                d.child(
                    div()
                        .id("run-context-workspace-icon")
                        .tooltip(tooltip(workspace_tip.clone()))
                        .child(row_icon(theme, self.workspace_icon())),
                )
            });
        if locked {
            return locked_row(theme, content).into_any_element();
        }
        let open = self.open == Some(Popup::RunContext);
        let trigger = select_row("run-context", theme, content, false, open)
            .on_click(cx.listener(|this, e, _, cx| this.toggle_from(Popup::RunContext, e, cx)));
        let menu = open.then(|| {
            let mut rows = Vec::new();
            if self.can_pick_environment() && !self.props.environments.is_empty() {
                rows.extend(self.environment_rows(theme, cx));
                rows.push(div().mx(theme.sp(2.)).my(theme.sp(1.)).h(px(1.)).bg(theme.colors.border).into_any_element());
            }
            rows.extend(self.workspace_rows(theme, cx));
            below_row(Self::popup_surface(theme, Some(row_width(&theme)), rows, cx))
        });
        div().flex().flex_col().w_full().child(trigger.test_support()).children(menu).into_any_element()
    }

    fn render_panel_branch(&mut self, theme: &AppTheme, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.refs.is_none() || self.pending;
        let open = self.open == Some(Popup::Branch);
        let label = self.trigger_label(cx);
        let content = div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(theme.rem(0.625))
            .child(row_icon(theme, IconName::GitBranch))
            .child(div().flex_1().min_w_0().truncate().child(label));
        let trigger = select_row("branch-select", theme, content, disabled, open).when(!disabled, |t| {
            t.on_click(cx.listener(|this, e, window, cx| {
                if this.dismissed.consumes(e) {
                            return;
                        }
                        let open = this.open != Some(Popup::Branch);
                this.set_branch_picker_open(open, window, cx);
            }))
        });
        let picker = self.picker.as_ref().filter(|_| open).map(|(picker, _)| below_row(picker.clone()));
        div().flex().flex_col().w_full().child(trigger.test_support()).children(picker).into_any_element()
    }
}

impl Render for BranchToolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        match self.layout {
            BranchToolbarLayout::Composer => self.render_composer(&theme, cx),
            BranchToolbarLayout::PanelWorkspace => self.render_panel_workspace(&theme, cx),
            BranchToolbarLayout::PanelBranch => self.render_panel_branch(&theme, cx),
        }
    }
}
