//! The label and reviewer pickers of a pull request's summary: a ghost icon
//! button opening a searchable list. Picks toggle a draft; closing the list
//! sends the changes. Ports `PullRequestCandidatePicker.tsx`,
//! `PullRequestLabelPicker.tsx` and `PullRequestReviewerPicker.tsx`.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _,
};
use t3code_client_runtime::pull_request_reviewers::{
    ReviewerSearchInput, ReviewerSearchState, ReviewerTruncation, host_reviewer_query, reviewer_candidate_view,
};
use t3code_client_runtime::pull_request_selection::{SelectionDraft, change_batches};
use t3code_contracts::{
    PullRequestLabelCandidate, PullRequestLabelCandidateList, PullRequestLabelChangeInput, PullRequestRef,
    PullRequestReviewerCandidate, PullRequestReviewerCandidateList, PullRequestReviewerCandidatesInput, PullRequestReviewerKind,
    PullRequestReviewerRequestInput, PullRequestReviewerRequestInputReviewersItem,
};

use super::pull_request_list_logic::pull_request_label_color;
use super::pull_request_presentation::avatar;
use super::pull_request_source::PullRequestDetailSource;
use crate::ActiveAppTheme as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::popover::{PopupOpen, below, glass_surface};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::i18n::t;

/// Which list a picker edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Labels,
    Reviewers,
}

/// Sent after the picker's changes reached the host, so the detail reads again.
#[derive(Clone, Debug)]
pub struct CandidatesChanged;

enum Loaded<T> {
    Pending,
    Ready(T),
    Failed(SharedString),
}

/// A label or reviewer picker.
pub struct CandidatePicker {
    kind: PickerKind,
    source: Rc<dyn PullRequestDetailSource>,
    reference: PullRequestRef,
    allowed: bool,
    searchable: bool,
    open: Entity<PopupOpen>,
    was_open: bool,
    query: Entity<InputState>,
    labels: Option<Loaded<PullRequestLabelCandidateList>>,
    reviewers: Option<Loaded<PullRequestReviewerCandidateList>>,
    /// The host's answer for `debounced`, past a truncated first page.
    search: Option<(String, Loaded<PullRequestReviewerCandidateList>)>,
    debounced: String,
    label_draft: SelectionDraft<PullRequestLabelCandidate>,
    reviewer_draft: SelectionDraft<PullRequestReviewerCandidate>,
    _debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CandidatesChanged> for CandidatePicker {}

fn reviewer_key(candidate: &PullRequestReviewerCandidate) -> String {
    format!("{}:{}", candidate.kind.as_str(), candidate.id)
}

impl CandidatePicker {
    /// A picker for `kind`; `allowed` false shows the disabled button with its reason.
    pub fn new(
        kind: PickerKind,
        source: Rc<dyn PullRequestDetailSource>,
        reference: PullRequestRef,
        allowed: bool,
        searchable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match kind {
            PickerKind::Labels => t!("Search labels"),
            PickerKind::Reviewers => t!("Search people with access"),
        };
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let open = cx.new(|_| PopupOpen::default());
        let typed = cx.subscribe(&query, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = input.read(cx).value().trim().to_owned();
                this._debounce = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(250)).await;
                    this.update(cx, |this, cx| {
                        this.debounced = text;
                        this.search_host(cx);
                        cx.notify();
                    })
                    .ok();
                }));
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
            allowed,
            searchable,
            open,
            was_open: false,
            query,
            labels: None,
            reviewers: None,
            search: None,
            debounced: String::new(),
            label_draft: SelectionDraft::new(),
            reviewer_draft: SelectionDraft::new(),
            _debounce: None,
            _subscriptions: vec![typed, opened],
        }
    }

    fn candidates_input(&self, query: Option<String>) -> PullRequestReviewerCandidatesInput {
        let r = &self.reference;
        PullRequestReviewerCandidatesInput {
            project_id: r.project_id.clone(),
            host: r.host.clone(),
            expected_account_id: r.expected_account_id.clone(),
            allow_stale: r.allow_stale,
            repository: r.repository.clone(),
            number: r.number,
            query,
        }
    }

    /// Reads the candidates each time the list opens.
    fn load(&mut self, cx: &mut Context<Self>) {
        match self.kind {
            PickerKind::Labels => {
                self.labels = Some(Loaded::Pending);
                let request = self.source.label_candidates(self.reference.clone());
                cx.spawn(async move |this, cx| {
                    let result = request.await;
                    this.update(cx, |this, cx| {
                        this.labels = Some(match result {
                            Ok(list) => Loaded::Ready(list),
                            Err(e) => Loaded::Failed(e.0),
                        });
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            PickerKind::Reviewers => {
                self.reviewers = Some(Loaded::Pending);
                let request = self.source.reviewer_candidates(self.candidates_input(None));
                cx.spawn(async move |this, cx| {
                    let result = request.await;
                    this.update(cx, |this, cx| {
                        this.reviewers = Some(match result {
                            Ok(list) => Loaded::Ready(list),
                            Err(e) => Loaded::Failed(e.0),
                        });
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn loaded_reviewers(&self) -> Option<&PullRequestReviewerCandidateList> {
        match &self.reviewers {
            Some(Loaded::Ready(list)) => Some(list),
            _ => None,
        }
    }

    fn search_input<'a>(&'a self, query: &'a str) -> ReviewerSearchInput<'a> {
        ReviewerSearchInput { loaded: self.loaded_reviewers(), searchable: self.searchable, query, debounced_query: &self.debounced }
    }

    /// Asks the host for people past a truncated first page (`reviewerSearch`).
    fn search_host(&mut self, cx: &mut Context<Self>) {
        if self.kind != PickerKind::Reviewers {
            return;
        }
        let Some(query) = host_reviewer_query(&self.search_input("")) else { return };
        if self.search.as_ref().is_some_and(|(q, _)| *q == query) {
            return;
        }
        self.search = Some((query.clone(), Loaded::Pending));
        let request = self.source.reviewer_candidates(self.candidates_input(Some(query.clone())));
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if this.search.as_ref().is_some_and(|(q, _)| *q == query) {
                    this.search = Some((query, match result {
                        Ok(list) => Loaded::Ready(list),
                        Err(e) => Loaded::Failed(e.0),
                    }));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Sends the draft's changes when the list closes.
    fn send(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        let r = self.reference.clone();
        match self.kind {
            PickerKind::Labels => {
                let latest: Vec<PullRequestLabelCandidate> = match &self.labels {
                    Some(Loaded::Ready(list)) => list.candidates.clone(),
                    _ => Vec::new(),
                };
                let changes = self.label_draft.changes(|name, label| latest.iter().find(|l| l.name == name).map_or(label.is_applied, |l| l.is_applied));
                self.label_draft = SelectionDraft::new();
                if changes.add.is_empty() && changes.remove.is_empty() {
                    return;
                }
                cx.spawn(async move |this, cx| {
                    // Adding first keeps the Labels row from showing empty between the two writes.
                    for (labels, applied) in [(changes.add, true), (changes.remove, false)] {
                        for batch in change_batches(&labels) {
                            let names: Vec<String> = batch.iter().map(|l| l.name.clone()).collect();
                            let input = PullRequestLabelChangeInput {
                                project_id: r.project_id.clone(),
                                host: r.host.clone(),
                                expected_account_id: r.expected_account_id.clone(),
                                allow_stale: r.allow_stale,
                                repository: r.repository.clone(),
                                number: r.number,
                                labels: names.clone(),
                                applied,
                            };
                            if let Err(error) = source.set_labels(input).await {
                                let names = names.join(", ");
                                cx.update(|cx| {
                                    let title = if applied { t!("Could not put {names} on", names = names.as_str()) } else { t!("Could not take {names} off", names = names.as_str()) };
                                    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.0), cx);
                                });
                            }
                        }
                    }
                    this.update(cx, |_, cx| cx.emit(CandidatesChanged)).ok();
                })
                .detach();
            }
            PickerKind::Reviewers => {
                let mut latest: Vec<PullRequestReviewerCandidate> = self.loaded_reviewers().map(|l| l.candidates.clone()).unwrap_or_default();
                if let Some((_, Loaded::Ready(found))) = &self.search {
                    latest.extend(found.candidates.iter().cloned());
                }
                let changes = self
                    .reviewer_draft
                    .changes(|key, person| latest.iter().find(|c| reviewer_key(c) == key).map_or(person.is_requested, |c| c.is_requested));
                self.reviewer_draft = SelectionDraft::new();
                if changes.add.is_empty() && changes.remove.is_empty() {
                    return;
                }
                cx.spawn(async move |this, cx| {
                    // Removing first lets a host that keeps one reviewer take the new one.
                    for (people, requested) in [(changes.remove, false), (changes.add, true)] {
                        for batch in change_batches(&people) {
                            let input = PullRequestReviewerRequestInput {
                                project_id: r.project_id.clone(),
                                host: r.host.clone(),
                                expected_account_id: r.expected_account_id.clone(),
                                allow_stale: r.allow_stale,
                                repository: r.repository.clone(),
                                number: r.number,
                                reviewers: batch.iter().map(|p| PullRequestReviewerRequestInputReviewersItem { id: p.id.clone(), kind: p.kind }).collect(),
                                requested,
                            };
                            if let Err(error) = source.request_reviewers(input).await {
                                let logins = batch.iter().map(|p| p.login.as_str()).collect::<Vec<_>>().join(", ");
                                cx.update(|cx| {
                                    let title = if requested {
                                        t!("Could not ask {logins} for a review", logins = logins.as_str())
                                    } else {
                                        t!("Could not take back the review request to {logins}", logins = logins.as_str())
                                    };
                                    Toaster::push(ToastData::new(ToastKind::Error, title).description(error.0), cx);
                                });
                            }
                        }
                    }
                    this.update(cx, |this, cx| {
                        this.search = None;
                        cx.emit(CandidatesChanged)
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let query = self.query.read(cx).value().trim().to_owned();
        let note = |text: SharedString, py: f32| div().px(theme.sp(2.)).py(theme.sp(py)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(text);
        let item = |id: String, selected: bool, content: AnyElement, on_click: Box<dyn Fn(&mut App)>| {
            div()
                .id(SharedString::from(id))
                .test_support()
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .min_h(theme.sp(7.))
                .px(theme.sp(2.))
                .py(theme.sp(1.))
                .rounded(theme.radii.sm)
                .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                .text_color(c.foreground())
                .cursor_pointer()
                .hover(|s| s.bg(c.foreground().opacity(0.09)))
                .child(content)
                .when(selected, |el| el.child(Icon::new(IconName::Check).size(IconSize::Sm).color(c.foreground())))
                .on_click(move |_, _, cx| on_click(cx))
                .into_any_element()
        };
        let this = cx.entity();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut truncated: Option<SharedString> = None;
        match self.kind {
            PickerKind::Labels => match &self.labels {
                None | Some(Loaded::Pending) => rows.push(people_ghost(&theme, 4)),
                Some(Loaded::Failed(e)) => rows.push(note(SharedString::from(format!("{} {e}", t!("The labels could not be read."))), 2.).into_any_element()),
                Some(Loaded::Ready(list)) => {
                    let needle = query.to_lowercase();
                    let shown: Vec<&PullRequestLabelCandidate> = list
                        .candidates
                        .iter()
                        .filter(|l| needle.is_empty() || l.name.to_lowercase().contains(&needle) || l.description.as_deref().unwrap_or("").to_lowercase().contains(&needle))
                        .collect();
                    if shown.is_empty() {
                        rows.push(note(if query.is_empty() { t!("This repository has no labels.") } else { t!("No label matches that.") }, 2.).into_any_element());
                    }
                    for label in shown {
                        let selected = self.label_draft.is_selected(&label.name, label.is_applied);
                        let dot = pull_request_label_color(label.color.as_deref())
                            .and_then(|hex| u32::from_str_radix(&hex[1..], 16).ok())
                            .map(|value| gpui_kit::Hsla::from(gpui_kit::rgb(value)))
                            .unwrap_or(c.muted_foreground);
                        let content = div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(theme.sp(2.))
                            .child(div().size(theme.sp(2.)).flex_none().rounded_full().bg(dot))
                            .child(
                                div().flex_1().min_w_0().truncate().child(SharedString::from(label.name.clone())).children(
                                    label.description.clone().map(|d| div().text_color(c.muted_foreground).child(format!(" · {d}"))),
                                ),
                            )
                            .into_any_element();
                        let this = this.clone();
                        let label = label.clone();
                        rows.push(item(
                            format!("label-candidate-{}", label.name),
                            selected,
                            content,
                            Box::new(move |cx| {
                                this.update(cx, |p, cx| {
                                    p.label_draft = p.label_draft.toggle(&label.name, label.clone(), label.is_applied);
                                    cx.notify();
                                })
                            }),
                        ));
                    }
                    if list.truncated {
                        truncated = Some(t!("This repository has more labels than are listed here. Apply the rest on the host."));
                    }
                }
            },
            PickerKind::Reviewers => match &self.reviewers {
                None | Some(Loaded::Pending) => rows.push(people_ghost(&theme, 4)),
                Some(Loaded::Failed(e)) => {
                    rows.push(note(SharedString::from(format!("{} {e}", t!("The people with access could not be read."))), 2.).into_any_element())
                }
                Some(Loaded::Ready(_)) => {
                    let search = match &self.search {
                        Some((_, Loaded::Ready(list))) => ReviewerSearchState { data: Some(list), is_pending: false, error: None },
                        Some((_, Loaded::Pending)) => ReviewerSearchState { data: None, is_pending: true, error: None },
                        Some((_, Loaded::Failed(e))) => ReviewerSearchState { data: None, is_pending: false, error: Some(e.as_ref()) },
                        None => ReviewerSearchState { data: None, is_pending: false, error: None },
                    };
                    let view = reviewer_candidate_view(&self.search_input(&query), &search);
                    if view.search_pending {
                        rows.push(people_ghost(&theme, 4));
                    } else if view.candidates.is_empty() {
                        let text = match &view.search_error {
                            Some(e) => SharedString::from(format!("{} {e}", t!("The people with access could not be read."))),
                            None if query.is_empty() => t!("Nobody else has access to this repository."),
                            None => t!("Nobody with access matches that."),
                        };
                        rows.push(note(text, 2.).into_any_element());
                    }
                    for person in view.candidates {
                        let key = reviewer_key(&person);
                        let selected = self.reviewer_draft.is_selected(&key, person.is_requested);
                        let content = div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap(theme.sp(1.5))
                            .child(avatar(&theme, Some(&person.login), 16.))
                            .child(div().flex_1().min_w_0().truncate().child(SharedString::from(person.login.clone())))
                            .when(person.kind == PullRequestReviewerKind::Team, |el| el.child(div().flex_none().text_color(c.muted_foreground).child("team")))
                            .into_any_element();
                        let this = this.clone();
                        rows.push(item(
                            format!("reviewer-candidate-{key}"),
                            selected,
                            content,
                            Box::new(move |cx| {
                                this.update(cx, |p, cx| {
                                    p.reviewer_draft = p.reviewer_draft.toggle(&key, person.clone(), person.is_requested);
                                    cx.notify();
                                })
                            }),
                        ));
                    }
                    truncated = view.truncation.map(|t| match t {
                        ReviewerTruncation::Unsearchable => {
                            t!("This repository has more people with access than are listed here. Ask for the rest on the host.")
                        }
                        ReviewerTruncation::Searchable => t!("This repository has more people with access than are listed here. Search to find them."),
                        ReviewerTruncation::SearchFailed => t!(
                            "Only the people already listed are matched: the host search failed. {error}",
                            error = view.search_error.as_deref().unwrap_or("")
                        ),
                        ReviewerTruncation::SearchTruncated => t!("More people match than the host returned. Keep typing to narrow it down."),
                    });
                }
            },
        }
        div()
            .id("candidate-list")
            .flex()
            .flex_col()
            .max_h(theme.sp(72.))
            .overflow_y_scroll()
            .p(theme.sp(1.))
            .children(rows)
            .children(truncated.map(|text| note(text, 1.5)))
            .into_any_element()
    }
}

/// `PullRequestPeopleGhost`: still placeholder rows while candidates load.
pub fn people_ghost(theme: &crate::AppTheme, rows: usize) -> AnyElement {
    let bar = theme.colors.muted_foreground.opacity(0.15);
    div()
        .flex()
        .flex_col()
        .children((0..rows).map(|i| {
            div()
                .flex()
                .items_center()
                .gap(theme.sp(2.))
                .min_h(theme.sp(7.))
                .px(theme.sp(2.))
                .child(div().size(theme.sp(4.)).rounded_full().bg(bar))
                .child(div().h(theme.sp(3.)).w(gpui_kit::relative([0.5, 0.4, 0.6, 0.33][i % 4])).rounded(theme.radii.sm).bg(bar))
        }))
        .into_any_element()
}

impl Render for CandidatePicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let (icon, label, reason) = match self.kind {
            PickerKind::Labels => (IconName::Tag, t!("Change labels"), t!("Changing labels needs triage access on this repository")),
            PickerKind::Reviewers => (IconName::UserPlus, t!("Request a review"), t!("Asking someone to review needs write access on this repository")),
        };
        let id = match self.kind {
            PickerKind::Labels => "pr-label-picker",
            PickerKind::Reviewers => "pr-reviewer-picker",
        };
        let _ = label;
        let trigger = Button::new(id).variant(ButtonVariant::Ghost).size(ButtonSize::IconXs).icon(icon).icon_size(IconSize::Sm);
        if !self.allowed {
            return div()
                .id(SharedString::from(format!("{id}-disabled")))
                .tooltip(crate::components::ui::tooltip::tooltip(reason))
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
            .child(div().id(SharedString::from(format!("{id}-trigger"))).test_support().child(trigger).on_click(move |event, _, cx| {
                PopupOpen::toggle_from(&toggle, crate::components::ui::popover::press_position(event), cx)
            }))
            .when(open, |el| {
                let c = &theme.colors;
                let input = Input::new(&self.query)
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .p_0()
                    .h(theme.sp(6.5))
                    .ui_text(&theme, TextSize::Sm, weight::NORMAL)
                    .w_full();
                let surface = glass_surface(&theme)
                    .w(theme.sp(72.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                        div().px(theme.sp(3.)).pt(theme.sp(2.5)).child(
                            div()
                                .relative()
                                .border_b_1()
                                .border_color(c.border.opacity(0.7))
                                .pb(theme.sp(1.5))
                                .child(div().absolute().top(theme.sp(1.5)).left_0().child(Icon::new(IconName::Search).size(IconSize::Md).color(c.muted_foreground.opacity(0.55))))
                                .child(div().pl(theme.sp(5.)).child(input)),
                        ),
                    )
                    .child(self.render_list(cx));
                el.child(below(PopupOpen::dismissable(&state, surface)))
            })
            .into_any_element()
    }
}

/// The picker's popup: the borderless search field over its list (`ComboboxPopup w-72`).
pub fn picker_surface(theme: &crate::AppTheme, query: &Entity<InputState>, list: AnyElement) -> gpui_kit::Div {
    let c = &theme.colors;
    let input = Input::new(query).appearance(false).bordered(false).focus_bordered(false).p_0().h(theme.sp(6.5)).ui_text(theme, TextSize::Sm, weight::NORMAL).w_full();
    glass_surface(theme)
        .w(theme.sp(72.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(
            div().px(theme.sp(3.)).pt(theme.sp(2.5)).child(
                div()
                    .relative()
                    .border_b_1()
                    .border_color(c.border.opacity(0.7))
                    .pb(theme.sp(1.5))
                    .child(div().absolute().top(theme.sp(1.5)).left_0().child(Icon::new(IconName::Search).size(IconSize::Md).color(c.muted_foreground.opacity(0.55))))
                    .child(div().pl(theme.sp(5.)).child(input)),
            ),
        )
        .child(list)
}

/// One candidate row: content, then a check while picked (`ComboboxItem`).
pub fn picker_row(theme: &crate::AppTheme, id: String, selected: bool, content: AnyElement, on_click: impl Fn(&mut App) + 'static) -> AnyElement {
    let c = &theme.colors;
    div()
        .id(SharedString::from(id))
        .test_support()
        .flex()
        .items_center()
        .gap(theme.sp(2.))
        .min_h(theme.sp(7.))
        .px(theme.sp(2.))
        .py(theme.sp(1.))
        .rounded(theme.radii.sm)
        .ui_text(theme, TextSize::Sm, weight::NORMAL)
        .text_color(c.foreground())
        .cursor_pointer()
        .hover(|s| s.bg(c.foreground().opacity(0.09)))
        .child(content)
        .when(selected, |el| el.child(Icon::new(IconName::Check).size(IconSize::Sm).color(c.foreground())))
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}
