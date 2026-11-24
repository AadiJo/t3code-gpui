//! The issue panel's label and assignee pickers: a ghost icon button opening
//! a searchable list of the tracker's candidates. Picks toggle a draft;
//! closing the list writes the whole set once. Ports `IssueLabelPicker` and
//! `IssueAssigneePicker` in `IssueDetailPanel.tsx`.

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};
use gpui_kit::TestSupportExt as _;
use t3code_client_runtime::issue_selection::{
    issue_assignee_held, issue_assignees_after_picks, issue_labels_after_picks, issue_write_after_picks,
};
use t3code_client_runtime::pull_request_selection::SelectionDraft;
use t3code_contracts::{IssueAssigneeCandidate, IssueCandidates, IssueDetail, IssueLabelCandidate, IssueRef, IssueScopeRef, IssueUpdateInput};

use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{IconName, IconSize};
use crate::components::pull_request::pull_request_candidate_picker::{people_ghost, picker_row, picker_surface};
use crate::components::pull_request::pull_request_list_logic::pull_request_label_color;
use crate::components::pull_request::pull_request_presentation::avatar;
use crate::components::pull_request::pull_request_source::IssueSource;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::popover::{PopupOpen, below, press_position};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;

/// Which set the picker writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssuePickerKind {
    Labels,
    Assignees,
}

/// Sent after a write reached the tracker, so the panel and lists read again.
#[derive(Clone, Debug)]
pub struct IssueCandidatesChanged;

/// A label or assignee picker on one issue.
pub struct IssueCandidatePicker {
    kind: IssuePickerKind,
    source: Rc<dyn IssueSource>,
    reference: IssueRef,
    /// The issue as the panel last read it.
    detail: Option<IssueDetail>,
    allowed: bool,
    disabled_reason: SharedString,
    open: Entity<PopupOpen>,
    was_open: bool,
    query: Entity<InputState>,
    candidates: Option<Result<IssueCandidates, SharedString>>,
    labels: SelectionDraft<IssueLabelCandidate>,
    assignees: SelectionDraft<IssueAssigneeCandidate>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<IssueCandidatesChanged> for IssueCandidatePicker {}

impl IssueCandidatePicker {
    pub fn new(kind: IssuePickerKind, source: Rc<dyn IssueSource>, reference: IssueRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = match kind {
            IssuePickerKind::Labels => t!("Search labels"),
            IssuePickerKind::Assignees => t!("Search people"),
        };
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let open = cx.new(|_| PopupOpen::default());
        let typed = cx.subscribe(&query, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let opened = cx.observe(&open, |this, open, cx| {
            let now = open.read(cx).open;
            if now && !this.was_open {
                this.load(cx);
            } else if !now && this.was_open {
                this.send(cx);
            }
            this.was_open = now;
            cx.notify();
        });
        Self {
            kind,
            source,
            reference,
            detail: None,
            allowed: false,
            disabled_reason: SharedString::default(),
            open,
            was_open: false,
            query,
            candidates: None,
            labels: SelectionDraft::new(),
            assignees: SelectionDraft::new(),
            _subscriptions: vec![typed, opened],
        }
    }

    /// Follows the panel's read of the issue and whether the viewer may change this set.
    pub fn set_issue(&mut self, detail: IssueDetail, allowed: bool, disabled_reason: SharedString, cx: &mut Context<Self>) {
        self.detail = Some(detail);
        self.allowed = allowed;
        self.disabled_reason = disabled_reason;
        cx.notify();
    }

    fn scope(&self) -> IssueScopeRef {
        let r = &self.reference;
        IssueScopeRef { project_id: r.project_id.clone(), tracker: r.tracker.clone(), host: r.host.clone(), scope: r.scope.clone() }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let request = self.source.candidates(self.scope());
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.candidates = Some(result.map_err(|e| e.0));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Writes the picked set as one update when the list closes.
    fn send(&mut self, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let r = self.reference.clone();
        let mut input = IssueUpdateInput {
            project_id: r.project_id,
            tracker: r.tracker,
            host: r.host,
            scope: r.scope,
            key: r.key,
            title: None,
            body: None,
            labels: None,
            assignees: None,
        };
        let title = match self.kind {
            IssuePickerKind::Labels => {
                let held: Vec<String> = detail.labels.iter().map(|l| l.name.clone()).collect();
                let after = issue_labels_after_picks(detail, &self.labels);
                self.labels = SelectionDraft::new();
                let Some(labels) = issue_write_after_picks(after.as_deref(), &held, None) else { return };
                input.labels = Some(labels);
                t!("Could not change labels")
            }
            IssuePickerKind::Assignees => {
                let held: Vec<String> = detail.assignees.iter().map(|a| a.login.clone()).collect();
                let after = issue_assignees_after_picks(detail, &self.assignees);
                self.assignees = SelectionDraft::new();
                let Some(assignees) = issue_write_after_picks(after.as_deref(), &held, None) else { return };
                input.assignees = Some(assignees);
                t!("Could not change assignees")
            }
        };
        let request = self.source.update(input);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |_, cx| match result {
                Ok(()) => cx.emit(IssueCandidatesChanged),
                Err(error) => {
                    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.0), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let needle = self.query.read(cx).value().trim().to_lowercase();
        let note = |text: SharedString| div().p(theme.sp(2.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(text);
        let this = cx.entity();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut truncated = None;
        match (&self.candidates, &self.detail) {
            (None, _) | (_, None) => rows.push(people_ghost(&theme, 4)),
            (Some(Err(e)), _) => {
                let text = match self.kind {
                    IssuePickerKind::Labels => t!("The labels could not be read."),
                    IssuePickerKind::Assignees => t!("The people could not be read."),
                };
                rows.push(note(SharedString::from(format!("{text} {e}"))).into_any_element());
            }
            (Some(Ok(candidates)), Some(detail)) => {
                match self.kind {
                    IssuePickerKind::Labels => {
                        let shown: Vec<&IssueLabelCandidate> = candidates
                            .labels
                            .iter()
                            .filter(|l| needle.is_empty() || l.name.to_lowercase().contains(&needle) || l.description.as_deref().unwrap_or("").to_lowercase().contains(&needle))
                            .collect();
                        if shown.is_empty() {
                            rows.push(note(if needle.is_empty() { t!("This tracker has no labels.") } else { t!("No label matches that.") }).into_any_element());
                        }
                        for label in shown {
                            let held = detail.labels.iter().any(|l| l.name == label.name);
                            let selected = self.labels.is_selected(&label.name, held);
                            let dot = pull_request_label_color(label.color.as_deref())
                                .and_then(|hex| u32::from_str_radix(&hex[1..], 16).ok())
                                .map(|v| gpui_kit::Hsla::from(gpui_kit::rgb(v)))
                                .unwrap_or(c.muted_foreground);
                            let content = div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap(theme.sp(2.))
                                .child(div().size(theme.sp(2.)).flex_none().rounded_full().bg(dot))
                                .child(div().flex_1().min_w_0().truncate().child(SharedString::from(label.name.clone())).children(label.description.clone().map(|d| div().text_color(c.muted_foreground).child(format!(" · {d}")))))
                                .into_any_element();
                            let this = this.clone();
                            let label = label.clone();
                            rows.push(picker_row(&theme, format!("issue-label-{}", label.name), selected, content, move |cx| {
                                this.update(cx, |p, cx| {
                                    p.labels = p.labels.toggle(&label.name, label.clone(), held);
                                    cx.notify();
                                })
                            }));
                        }
                        if candidates.truncated {
                            truncated = Some(t!("More labels exist than are listed here."));
                        }
                    }
                    IssuePickerKind::Assignees => {
                        let shown: Vec<&IssueAssigneeCandidate> = candidates
                            .assignees
                            .iter()
                            .filter(|a| needle.is_empty() || a.login.to_lowercase().contains(&needle) || a.name.as_deref().unwrap_or("").to_lowercase().contains(&needle))
                            .collect();
                        if shown.is_empty() {
                            rows.push(note(if needle.is_empty() { t!("Nobody can be assigned here.") } else { t!("Nobody matches that.") }).into_any_element());
                        }
                        for person in shown {
                            let held = issue_assignee_held(detail, person);
                            let selected = self.assignees.is_selected(&person.id, held);
                            let content = div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .gap(theme.sp(1.5))
                                .child(avatar(&theme, Some(&person.login), 16.))
                                .child(div().flex_1().min_w_0().truncate().child(SharedString::from(person.login.clone())))
                                .into_any_element();
                            let this = this.clone();
                            let person = person.clone();
                            rows.push(picker_row(&theme, format!("issue-assignee-{}", person.login), selected, content, move |cx| {
                                this.update(cx, |p, cx| {
                                    p.assignees = p.assignees.toggle(&person.id, person.clone(), held);
                                    cx.notify();
                                })
                            }));
                        }
                        if candidates.truncated {
                            truncated = Some(t!("More people exist than are listed here."));
                        }
                    }
                }
            }
        }
        div()
            .id("issue-candidate-list")
            .flex()
            .flex_col()
            .max_h(theme.sp(72.))
            .overflow_y_scroll()
            .p(theme.sp(1.))
            .children(rows)
            .children(truncated.map(|t| div().px(theme.sp(2.)).py(theme.sp(1.5)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(t)))
            .into_any_element()
    }
}

impl Render for IssueCandidatePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let (id, icon) = match self.kind {
            IssuePickerKind::Labels => ("issue-label-picker", IconName::Tag),
            IssuePickerKind::Assignees => ("issue-assignee-picker", IconName::UserPlus),
        };
        let trigger = Button::new(id).variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(icon).icon_size(IconSize::Sm);
        if !self.allowed {
            return div()
                .id(SharedString::from(format!("{id}-disabled")))
                .tooltip(crate::components::ui::tooltip::tooltip(crate::components::ui::tooltip::TooltipSurface::new(self.disabled_reason.clone())))
                .child(trigger)
                .into_any_element();
        }
        let open = self.open.read(cx).open;
        if open && !self.was_open {
            self.query.update(cx, |q, cx| q.focus(window, cx));
        }
        let state = self.open.clone();
        let toggle = state.clone();
        div()
            .id(SharedString::from(format!("{id}-root")))
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("{id}-trigger")))
                    .test_support()
                    .child(trigger)
                    .on_click(move |event, _, cx| PopupOpen::toggle_from(&toggle, press_position(event), cx)),
            )
            .when(open, |el| {
                let surface = picker_surface(&theme, &self.query, self.render_list(cx));
                el.child(below(PopupOpen::dismissable(&state, surface)))
            })
            .into_any_element()
    }
}
