//! One issue: state with the moves the viewer may make, description, labels
//! and assignees with their pickers, linked pull requests, and the activity
//! with reactions and a comment box. Ports `IssueDetailPanel.tsx` and
//! `IssueCommentComposer.tsx`.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::{rems, 
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use t3code_client_runtime::issue_presentation::{IssueWriteAccess, issue_write_access};
use t3code_client_runtime::pull_request_reactions::{PULL_REQUEST_REACTION_ORDER, pull_request_reaction_emoji};
use t3code_contracts::*;

use super::issue_candidate_picker::{IssueCandidatePicker, IssueCandidatesChanged, IssuePickerKind};
use super::issue_presentation::{issue_event_sentence, issue_state_presentation};
use crate::appearance_fonts::{TextSize, weight};
use crate::components::chat::chat_style::ChatColors;
use crate::components::chat::markdown_stream_blocks::parse_message_blocks;
use crate::components::chat_markdown::{MarkdownUiState, MarkdownView, render_markdown};
use crate::components::icons::{Icon, IconName, IconSize};
use crate::components::pull_request::pull_request_presentation::{avatar, label_chip, pull_request_state_presentation, relative_label};
use crate::components::pull_request::pull_request_source::IssueSource;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::TextInput;
use crate::components::ui::menu::{MenuItem, MenuPopup, dropdown_menu};
use crate::i18n::t;
use crate::markdown_pipeline::{MarkdownFlavor, ParsedBlocks};
use crate::{ActiveAppTheme as _, AppTheme};

/// What the panel asks its owner to do.
#[derive(Clone, Debug)]
pub enum IssueDetailEvent {
    /// The issue changed; lists should refresh.
    Changed,
    /// Link the issue to the current thread.
    LinkToThread(IssueRef),
}

/// One issue's panel.
pub struct IssueDetailPanel {
    source: Rc<dyn IssueSource>,
    reference: IssueRef,
    detail: Option<IssueDetail>,
    error: Option<SharedString>,
    status: Option<SharedString>,
    label_picker: Option<Entity<IssueCandidatePicker>>,
    assignee_picker: Option<Entity<IssueCandidatePicker>>,
    _picker_events: Vec<gpui_kit::Subscription>,
    composer: Entity<InputState>,
    markdown: HashMap<String, Vec<ParsedBlocks>>,
    markdown_state: MarkdownUiState,
    /// Whether the client holds the source-control write scope.
    pub can_write: bool,
    pub now_ms: Option<i64>,
}

impl EventEmitter<IssueDetailEvent> for IssueDetailPanel {}

impl IssueDetailPanel {
    /// What right clicks in its Markdown offer on the host (`live::host_menus::right_panel_menus`).
    pub fn with_markdown_menus(mut self, menus: crate::components::chat_markdown::MarkdownMenus) -> Self {
        self.markdown_state.menus = menus;
        self
    }

    /// The panel for `reference`; starts loading at once.
    pub fn new(source: Rc<dyn IssueSource>, reference: IssueRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Leave a comment")));
        let mut panel = Self {
            source,
            reference,
            detail: None,
            error: None,
            status: None,
            label_picker: None,
            assignee_picker: None,
            _picker_events: Vec::new(),
            composer,
            markdown: HashMap::new(),
            markdown_state: MarkdownUiState::default(),
            can_write: true,
            now_ms: None,
        };
        panel.refresh(cx);
        panel
    }

    /// The issue as last read.
    pub fn detail(&self) -> Option<&IssueDetail> {
        self.detail.as_ref()
    }

    /// Reads the issue again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let request = self.source.detail(self.reference.clone());
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(d) => {
                        this.detail = Some(d);
                        this.error = None;
                        this.sync_pickers(cx);
                    }
                    Err(e) => this.error = Some(e.0),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn access(&self) -> Option<IssueWriteAccess> {
        self.detail.as_ref().map(|d| issue_write_access(d, self.can_write))
    }

    fn after_write(&mut self, request: crate::components::pull_request::pull_request_source::RpcFuture<()>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.status = result.err().map(|e| e.0);
                cx.emit(IssueDetailEvent::Changed);
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Moves the issue to `state`.
    pub fn set_state(&mut self, state: &str, cx: &mut Context<Self>) {
        let r = self.reference.clone();
        let request = self.source.set_state(IssueSetStateInput { project_id: r.project_id, tracker: r.tracker, host: r.host, scope: r.scope, key: r.key, state: state.to_owned() });
        self.after_write(request, cx);
    }

    /// Posts the composer's comment.
    pub fn send_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.composer.read(cx).value().to_string();
        if body.trim().is_empty() {
            return;
        }
        self.composer.update(cx, |c, cx| c.set_value(String::new(), window, cx));
        let r = self.reference.clone();
        let request = self.source.comment(IssueCommentInput { project_id: r.project_id, tracker: r.tracker, host: r.host, scope: r.scope, key: r.key, body });
        self.after_write(request, cx);
    }

    /// Toggles the viewer's reaction on the issue.
    pub fn react(&mut self, content: IssueCommentReactionsItemContent, cx: &mut Context<Self>) {
        let reacted = !self.detail.as_ref().and_then(|d| d.reactions.as_ref()).is_some_and(|rs| rs.iter().any(|r| r.content == content && r.viewer_has_reacted));
        if let Some(d) = self.detail.as_mut() {
            let reactions = d.reactions.get_or_insert_with(Vec::new);
            match reactions.iter_mut().find(|r| r.content == content) {
                Some(r) => {
                    r.viewer_has_reacted = reacted;
                    if reacted { r.count += 1 } else { r.count = r.count.saturating_sub(1) }
                }
                None => reactions.push(IssueCommentReactionsItem { content, count: 1, actors: vec![], viewer_has_reacted: true }),
            }
            reactions.retain(|r| r.count > 0);
        }
        let r = self.reference.clone();
        let request = self.source.react(IssueReactionInput {
            project_id: r.project_id,
            tracker: r.tracker,
            host: r.host,
            scope: r.scope,
            key: r.key,
            subject_id: None,
            content: IssueReactionInputContent::from_wire(content.as_str()).expect("same reaction set"),
            reacted,
        });
        self.after_write(request, cx);
        cx.notify();
    }

    /// Creates the label and assignee pickers once the issue says the tracker has them.
    fn ensure_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = &self.detail else { return };
        let (labels, assignees) = (detail.capabilities.labels, detail.capabilities.assignees);
        let mut created = false;
        for (kind, wanted) in [(IssuePickerKind::Labels, labels), (IssuePickerKind::Assignees, assignees)] {
            let slot = match kind {
                IssuePickerKind::Labels => &self.label_picker,
                IssuePickerKind::Assignees => &self.assignee_picker,
            };
            if !wanted || slot.is_some() {
                continue;
            }
            let (source, reference) = (self.source.clone(), self.reference.clone());
            let picker = cx.new(|cx| IssueCandidatePicker::new(kind, source, reference, window, cx));
            self._picker_events.push(cx.subscribe(&picker, |this, _, _: &IssueCandidatesChanged, cx| {
                cx.emit(IssueDetailEvent::Changed);
                this.refresh(cx);
            }));
            match kind {
                IssuePickerKind::Labels => self.label_picker = Some(picker),
                IssuePickerKind::Assignees => self.assignee_picker = Some(picker),
            }
            created = true;
        }
        if created {
            self.sync_pickers(cx);
        }
    }

    /// Hands the pickers the latest read and what the viewer may change.
    fn sync_pickers(&mut self, cx: &mut Context<Self>) {
        let (Some(detail), Some(access)) = (self.detail.clone(), self.access()) else { return };
        let blocked = access.blocked.map(|b| SharedString::new_static(b.message()));
        if let Some(p) = &self.label_picker {
            let reason = blocked.clone().unwrap_or_else(|| t!("Changing labels needs triage access on this tracker."));
            let detail = detail.clone();
            p.update(cx, |p, cx| p.set_issue(detail, access.labels, reason, cx));
        }
        if let Some(p) = &self.assignee_picker {
            let reason = blocked.unwrap_or_else(|| t!("Changing assignees needs triage access on this tracker."));
            p.update(cx, |p, cx| p.set_issue(detail, access.assignees, reason, cx));
        }
    }

    fn markdown(&mut self, key: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let cache_key = format!("{key}\0{}\0{text}", crate::markdown_pipeline::math_enabled());
        let pieces = self.markdown.entry(cache_key.clone()).or_insert_with(|| parse_message_blocks(text, MarkdownFlavor::ASSISTANT, false)).clone();
        let colors = ChatColors::get(cx);
        let k = cache_key.bytes().fold(1469598103934665603u64, |h, b| (h ^ u64::from(b)).wrapping_mul(1099511628211));
        let view = MarkdownView { key: k, pieces: &pieces, colors: &colors, user: false, streaming: false, piece_opacity: None, find: None, state: &self.markdown_state, actions: Rc::new(|_, _, _| {}) };
        render_markdown(&view, window, cx).into_any_element()
    }

    /// `PullRequestMetaRow`s for Labels and Assignees, each with its picker.
    fn render_meta_rows(&self, detail: &IssueDetail, theme: &AppTheme) -> AnyElement {
        let c = &theme.colors;
        let row = |icon: IconName, label: SharedString, value: AnyElement| {
            div()
                .flex()
                .items_center()
                .min_h(theme.sp(6.))
                .gap(theme.sp(2.))
                .ui_text(theme, TextSize::Xs, weight::NORMAL)
                .child(div().w(theme.rem(6.)).flex_none().flex().items_center().gap(theme.sp(1.5)).text_color(c.muted_foreground).child(Icon::new(icon).size(IconSize::Sm).color(c.muted_foreground)).child(label))
                .child(div().flex_1().min_w_0().text_color(c.foreground()).child(value))
        };
        let none = |text: SharedString| div().text_color(c.muted_foreground).child(text).into_any_element();
        let labels = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(1.))
            .when(detail.labels.is_empty(), |el| el.child(none(t!("None", context = "no labels"))))
            .children(detail.labels.iter().map(|l| label_chip(theme, &l.name, l.color.as_deref())))
            .children(self.label_picker.clone())
            .into_any_element();
        let assignees = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.sp(1.5))
            .when(detail.assignees.is_empty(), |el| el.child(none(t!("None", context = "no assignees"))))
            .children(detail.assignees.iter().map(|a| div().flex().items_center().gap(theme.sp(1.)).child(avatar(theme, Some(&a.login), 16.)).child(SharedString::from(a.login.clone()))))
            .children(self.assignee_picker.clone())
            .into_any_element();
        div()
            .flex()
            .flex_col()
            .gap(theme.sp(2.))
            .child(row(IconName::Tag, t!("Labels"), labels))
            .child(row(IconName::Users, t!("Assignees"), assignees))
            .into_any_element()
    }

    fn section(theme: &AppTheme, title: SharedString) -> gpui_kit::Div {
        div().flex().flex_col().gap(rems(0.375)).child(div().ui_text(theme, TextSize::Xs, weight::SEMIBOLD).text_color(theme.colors.muted_foreground).child(title))
    }
}

impl Render for IssueDetailPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_pickers(window, cx);
        let theme = cx.app_theme().clone();
        let Some(detail) = self.detail.clone() else {
            let text = self.error.clone().unwrap_or_else(|| t!("Loading issue"));
            return div().id("issue-detail").test_support().size_full().p(rems(1.25)).bg(theme.colors.background()).text_color(theme.colors.muted_foreground).child(text);
        };
        let access = self.access().expect("detail loaded");
        let now = self.now_ms.unwrap_or_else(crate::timestamp_format::now_ms);
        let (state_icon, state_tone) = issue_state_presentation(&theme, &detail.state);
        let this = cx.entity();
        let states = access.states.clone();
        let state_menu = (!states.is_empty()).then(|| {
            let this = this.clone();
            dropdown_menu("issue-state", Button::new("issue-state-trigger").size(ButtonSize::Sm).variant(ButtonVariant::Outline).trailing_icon(IconName::ChevronDown).label(SharedString::from(detail.state.name.clone())), move |_, _| {
                let mut menu = MenuPopup::new();
                for state in states.clone() {
                    let this = this.clone();
                    let name = state.name.clone();
                    menu = menu.item(MenuItem::new(SharedString::from(state.name.clone())).on_select(move |_, cx| this.update(cx, |p, cx| p.set_state(&name, cx))));
                }
                menu
            })
        });
        let body = if detail.body.trim().is_empty() { self.markdown("body", &t!("_No description provided._"), window, cx) } else { self.markdown("body", &detail.body.clone(), window, cx) };
        // Comments and events in the order they happened.
        let mut activity: Vec<(String, AnyElement)> = Vec::new();
        for comment in &detail.comments {
            let login = comment.author.as_ref().map_or("ghost".to_owned(), |a| a.login.clone());
            let body = self.markdown(&comment.id, &comment.body, window, cx);
            activity.push((
                comment.created_at.clone(),
                div()
                    .flex()
                    .gap(rems(0.625))
                    .child(avatar(&theme, Some(&login), 24.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(rems(0.25))
                            .child(div().flex().gap(rems(0.375)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(div().text_color(theme.colors.text).font_weight(weight::SEMIBOLD).child(SharedString::from(login))).child(relative_label(&comment.created_at, now)))
                            .child(div().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(body))
                            .children(comment.reactions.as_ref().filter(|r| !r.is_empty()).map(|rs| {
                                div().flex().gap(px(4.)).children(rs.iter().map(|r| div().flex().gap(px(3.)).px(px(6.)).h(px(20.)).items_center().rounded_full().border_1().border_color(theme.colors.border).ui_text(&theme, TextSize::Xs, weight::MEDIUM).text_color(theme.colors.text).child(pull_request_reaction_emoji(r.content)).child(r.count.to_string())))
                            })),
                    )
                    .into_any_element(),
            ));
        }
        for event in &detail.events {
            let actor = event.actor.as_ref().map_or("ghost".to_owned(), |a| a.login.clone());
            activity.push((
                event.created_at.clone(),
                div()
                    .flex()
                    .items_center()
                    .gap(rems(0.5))
                    .pl(rems(0.25))
                    .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                    .text_color(theme.colors.muted_foreground)
                    .child(Icon::new(IconName::CircleDot).size(IconSize::Xs))
                    .child(div().text_color(theme.colors.text).child(SharedString::from(actor)))
                    .child(issue_event_sentence(event))
                    .child(div().ml_auto().child(relative_label(&event.created_at, now)))
                    .into_any_element(),
            ));
        }
        activity.sort_by(|a, b| a.0.cmp(&b.0));
        let reactions = detail.reactions.clone().unwrap_or_default();
        let mut reaction_row = div().flex().flex_wrap().gap(rems(0.25));
        for content in PULL_REQUEST_REACTION_ORDER {
            let held = reactions.iter().find(|r| r.content == content);
            if held.is_none() && !access.react {
                continue;
            }
            if held.is_none() && content != t3code_contracts::IssueCommentReactionsItemContent::ThumbsUp && content != t3code_contracts::IssueCommentReactionsItemContent::Heart {
                continue;
            }
            let reacted = held.is_some_and(|r| r.viewer_has_reacted);
            reaction_row = reaction_row.child(
                div()
                    .id(SharedString::from(format!("issue-react-{}", content.as_str())))
                    .test_support()
                    .flex()
                    .items_center()
                    .gap(rems(0.25))
                    .h(rems(1.375))
                    .px(rems(0.4375))
                    .rounded_full()
                    .border_1()
                    .border_color(if reacted { theme.colors.primary().opacity(0.5) } else { theme.colors.border })
                    .when(reacted, |el| el.bg(theme.colors.primary().opacity(0.12)))
                    .ui_text(&theme, TextSize::Xs, weight::MEDIUM)
                    .text_color(theme.colors.text)
                    .cursor_pointer()
                    .child(pull_request_reaction_emoji(content))
                    .children(held.map(|r| r.count.to_string()))
                    .when(access.react, |el| el.on_click(cx.listener(move |this, _, _, cx| this.react(content, cx)))),
            );
        }
        let reference = self.reference.clone();
        div()
            .id("issue-detail")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(rems(0.625))
                    .px(rems(1.25))
                    .pt(rems(1.))
                    .pb(rems(0.75))
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(rems(0.5))
                            .child(div().pt(rems(0.1875)).child(Icon::new(state_icon).size(IconSize::Lg).color(state_tone)))
                            .child(div().flex_1().ui_text(&theme, TextSize::Lg, weight::SEMIBOLD).text_color(theme.colors.text).child(SharedString::from(detail.title.clone())))
                            .child(div().ui_text(&theme, TextSize::Lg, weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(detail.display_key.clone()))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(rems(0.5))
                            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .children(state_menu)
                            .when(access.states.is_empty(), |el| el.child(div().flex().items_center().gap(rems(0.25)).px(rems(0.5)).h(rems(1.25)).rounded_full().bg(state_tone.opacity(0.14)).text_color(state_tone).child(SharedString::from(detail.state.name.clone()))))
                            .child(avatar(&theme, detail.author.as_ref().map(|a| a.login.as_str()), 16.))
                            .child(SharedString::from(detail.author.as_ref().map_or("ghost".into(), |a| a.login.clone())))
                            .child(relative_label(&detail.updated_at, now))
                            .child(div().flex_1())
                            .child(Button::new("issue-link-thread").size(ButtonSize::Sm).variant(ButtonVariant::Ghost).icon(IconName::MessageSquare).label(t!("Link to thread")).on_click(cx.listener(move |_, _, _, cx| cx.emit(IssueDetailEvent::LinkToThread(reference.clone()))))),
                    )
                    .when_some(access.blocked, |el, b| {
                        el.child(div().flex().items_center().gap(rems(0.375)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(Icon::new(IconName::TriangleAlert).size(IconSize::Xs)).child(t!("Read-only")).child(SharedString::new_static(b.message())))
                    })
                    .when_some(self.status.clone(), |el, s| el.child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.destructive()).child(s))),
            )
            .child(
                div()
                    .id("issue-detail-body")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(rems(1.25))
                            .p(rems(1.25))
                            .child(div().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(body))
                            .child(reaction_row)
                            .child(self.render_meta_rows(&detail, &theme))
                            .when(!detail.linked_pull_requests.is_empty(), |el| {
                                el.child(Self::section(&theme, t!("Linked pull requests")).children(detail.linked_pull_requests.iter().map(|pr| {
                                    let (icon, tone, _) = pull_request_state_presentation(&theme, pr.state, pr.is_draft);
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(rems(0.375))
                                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                                        .text_color(theme.colors.text)
                                        .child(Icon::new(icon).size(IconSize::Sm).color(tone))
                                        .child(div().text_color(theme.colors.muted_foreground).child(format!("#{}", pr.number)))
                                        .child(SharedString::from(pr.title.clone()))
                                        .when(pr.closes_issue, |el| el.child(div().px(rems(0.375)).rounded_full().bg(theme.colors.muted).ui_text(&theme, TextSize::Xs3, weight::MEDIUM).child(t!("Closes this issue"))))
                                })))
                            })
                            .child(Self::section(&theme, t!("Activity")).gap(rems(0.875)).children(activity.into_iter().map(|(_, el)| el)))
                            .child(if access.comment {
                                div()
                                    .flex()
                                    .gap(rems(0.375))
                                    .child(div().id("issue-comment-input").flex_1().child(TextInput::new(&self.composer)))
                                    .child(Button::new("issue-comment-send").size(ButtonSize::Sm).label(t!("Comment", context = "verb, posts the comment")).on_click(cx.listener(|this, _, window, cx| this.send_comment(window, cx))))
                                    .into_any_element()
                            } else {
                                div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(t!("You cannot comment here.")).into_any_element()
                            }),
                    ),
            )
    }
}
