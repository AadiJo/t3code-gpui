//! The base branch for a new worktree, as a form field (settings and
//! scheduled tasks use it). Ports `WorktreeBaseBranchPicker.tsx`: an outline
//! trigger naming "From {branch}" and the ref picker with "Start from remote".

use std::sync::Arc;

use gpui_kit::{
    Context, Entity, EventEmitter, Focusable as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::*, px,
};
use t3code_contracts::{VcsListRefsInput, VcsRef};

use super::branch_picker::{BranchPicker, BranchPickerEvent, BranchPickerItem, BranchPickerOptions};
use super::branch_toolbar_logic::{EnvMode, resolve_branch_trigger_label};
use super::git_actions_control_source::GitActionsSource;
use super::icons::{Icon, IconName, IconSize};
use super::chat::thread_details_panel_styles::below_row;
use super::ui::{DISABLED_OPACITY, TextStyleExt as _};
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};

/// What the field reports.
#[derive(Clone, Debug, PartialEq)]
pub enum WorktreeBaseBranchEvent {
    Changed(String),
    StartFromOriginChanged(bool),
}

/// The base-branch field.
pub struct WorktreeBaseBranchPicker {
    source: Arc<dyn GitActionsSource>,
    cwd: Option<String>,
    value: String,
    start_from_origin: bool,
    disabled: bool,
    refs: Vec<VcsRef>,
    picker: Option<(Entity<BranchPicker>, Subscription)>,
}

impl EventEmitter<WorktreeBaseBranchEvent> for WorktreeBaseBranchPicker {}

impl WorktreeBaseBranchPicker {
    pub fn new(source: Arc<dyn GitActionsSource>, cwd: Option<String>, value: String, start_from_origin: bool, cx: &mut Context<Self>) -> Self {
        let mut picker = Self { source, cwd, value, start_from_origin, disabled: false, refs: Vec::new(), picker: None };
        picker.load(cx);
        picker
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.disabled = disabled;
        cx.notify();
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(cwd) = self.cwd.clone() else { return };
        let request = self.source.list_refs(VcsListRefsInput {
            cwd,
            query: None,
            cursor: None,
            include_matching_remote_refs: None,
            ref_kind: None,
            refresh: None,
            limit: Some(100),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(result) = request.await {
                this.update(cx, |this, cx| {
                    this.refs = result.refs;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Opens the picker (the trigger's click).
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.disabled || self.cwd.is_none() {
            return;
        }
        let options = BranchPickerOptions {
            start_from_origin: Some(self.start_from_origin),
            project_cwd: self.cwd.clone(),
            ..Default::default()
        };
        let picker = cx.new(|cx| BranchPicker::new(self.refs.clone(), Some(self.value.clone()), options, cx));
        let subscription = cx.subscribe(&picker, |this, _, event: &BranchPickerEvent, cx| {
            match event {
                BranchPickerEvent::Select(BranchPickerItem::Ref(r)) => {
                    this.value = r.name.clone();
                    cx.emit(WorktreeBaseBranchEvent::Changed(r.name.clone()));
                    this.picker = None;
                }
                BranchPickerEvent::StartFromOriginChanged(on) => {
                    this.start_from_origin = *on;
                    cx.emit(WorktreeBaseBranchEvent::StartFromOriginChanged(*on));
                }
                _ => this.picker = None,
            }
            cx.notify();
        });
        window.focus(&picker.focus_handle(cx), cx);
        self.picker = Some((picker, subscription));
        cx.notify();
    }
}

impl Render for WorktreeBaseBranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let is_remote = self.refs.iter().find(|r| r.name == self.value).map(|r| r.is_remote == Some(true));
        let label: SharedString = resolve_branch_trigger_label(
            None,
            EnvMode::Worktree,
            (!self.value.is_empty()).then_some(self.value.as_str()),
            is_remote,
            self.start_from_origin,
        );
        let disabled = self.disabled || self.cwd.is_none();
        // `Button variant="outline" size="sm"`, full width.
        let trigger = div()
            .id("worktree-base-trigger")
            .flex()
            .w_full()
            .items_center()
            .gap(theme.rem(0.375))
            .h(theme.sp(8.))
            .px(theme.sp(3.))
            .rounded(theme.radii.lg)
            .border_1()
            .border_color(c.input)
            .bg(theme.derived.outline_fill)
            .ui_text(&theme, TextSize::Sm, weight::MEDIUM)
            .child(Icon::new(IconName::GitBranch).size(IconSize::Custom(theme.rem(0.875))).color(c.muted_foreground))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .child(Icon::new(IconName::ChevronDown).size(IconSize::Custom(theme.rem(0.875))).color(c.muted_foreground))
            .when(disabled, |t| t.opacity(DISABLED_OPACITY))
            .when(!disabled, |t| {
                let hover = theme.derived.outline_hover;
                t.cursor_pointer().hover(move |s| s.bg(hover)).on_click(cx.listener(|this, _, window, cx| this.open(window, cx)))
            });
        div().flex().flex_col().w_full().child(trigger).children(self.picker.as_ref().map(|(p, _)| below_row(p.clone())))
    }
}
