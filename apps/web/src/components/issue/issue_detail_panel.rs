//! One issue: state with the moves the viewer may make, description, labels
//! and assignees with their pickers, linked pull requests, and the activity
//! with reactions and a comment box. Ports `IssueDetailPanel.tsx` and
//! `IssueCommentComposer.tsx`.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::InputState;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use t3code_client_runtime::issue_presentation::{IssueWriteAccess, issue_write_access};
use t3code_client_runtime::issue_selection::{issue_labels_after_picks, issue_write_after_picks};
use t3code_client_runtime::pull_request_reactions::{PULL_REQUEST_REACTION_ORDER, pull_request_reaction_emoji};
use t3code_client_runtime::pull_request_selection::SelectionDraft;
use t3code_contracts::*;

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
use crate::components::ui::menu::{MenuItem, MenuItemKind, MenuPopup, dropdown_menu};
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
    candidates: Option<IssueCandidates>,
    error: Option<SharedString>,
    status: Option<SharedString>,
    label_draft: SelectionDraft<IssueLabelCandidate>,
    labels_open: bool,
    composer: Entity<InputState>,
    markdown: HashMap<String, Vec<ParsedBlocks>>,
    markdown_state: MarkdownUiState,
    /// Whether the client holds the source-control write scope.
    pub can_write: bool,
    pub now_ms: Option<i64>,
}

impl EventEmitter<IssueDetailEvent> for IssueDetailPanel {}

impl IssueDetailPanel {
    /// The panel for `reference`; starts loading at once.
    pub fn new(source: Rc<dyn IssueSource>, reference: IssueRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Leave a comment")));
        let mut panel = Self {
            source,
            reference,
            detail: None,
            candidates: None,
            error: None,
            status: None,
            label_draft: SelectionDraft::new(),
            labels_open: false,
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
                    }
                    Err(e) => this.error = Some(e.0),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn scope(&self) -> IssueScopeRef {
        let r = &self.reference;
        IssueScopeRef { project_id: r.project_id.clone(), tracker: r.tracker.clone(), host: r.host.clone(), scope: r.scope.clone() }
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

    /// Opens or closes the label picker; closing writes the picks as one update.
    pub fn toggle_label_picker(&mut self, cx: &mut Context<Self>) {
        if !self.labels_open {
            self.labels_open = true;
            if self.candidates.is_none() {
                let request = self.source.candidates(self.scope());
                cx.spawn(async move |this, cx| {
                    let result = request.await;
                    this.update(cx, |this, cx| {
                        this.candidates = result.ok();
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            cx.notify();
            return;
        }
        self.labels_open = false;
        let Some(detail) = &self.detail else { return };
        let held: Vec<String> = detail.labels.iter().map(|l| l.name.clone()).collect();
        let after = issue_labels_after_picks(detail, &self.label_draft);
        self.label_draft = SelectionDraft::new();
        if let Some(labels) = issue_write_after_picks(after.as_deref(), &held, None) {
            let r = self.reference.clone();
            let request = self.source.update(IssueUpdateInput {
                project_id: r.project_id,
                tracker: r.tracker,
                host: r.host,
                scope: r.scope,
                key: r.key,
                title: None,
                body: None,
                labels: Some(labels),
                assignees: None,
            });
            self.after_write(request, cx);
        }
        cx.notify();
    }

    fn toggle_label(&mut self, candidate: IssueLabelCandidate, cx: &mut Context<Self>) {
        let held = self.detail.as_ref().is_some_and(|d| d.labels.iter().any(|l| l.name == candidate.name));
        self.label_draft = self.label_draft.toggle(&candidate.name.clone(), candidate, held);
        cx.notify();
    }

    fn markdown(&mut self, key: &str, text: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let cache_key = format!("{key}\0{text}");
        let pieces = self.markdown.entry(cache_key.clone()).or_insert_with(|| parse_message_blocks(text, MarkdownFlavor::ASSISTANT, false)).clone();
        let colors = ChatColors::get(cx);
        let k = cache_key.bytes().fold(1469598103934665603u64, |h, b| (h ^ u64::from(b)).wrapping_mul(1099511628211));
        let view = MarkdownView { key: k, pieces: &pieces, colors: &colors, user: false, streaming: false, piece_opacity: None, state: &self.markdown_state, actions: Rc::new(|_, _, _| {}) };
        render_markdown(&view, window, cx).into_any_element()
    }

    fn section(theme: &AppTheme, title: SharedString) -> gpui_kit::Div {
        div().flex().flex_col().gap(px(6.)).child(div().ui_text(theme, TextSize::Xs, weight::SEMIBOLD).text_color(theme.colors.muted_foreground).child(title))
    }
}

impl Render for IssueDetailPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let Some(detail) = self.detail.clone() else {
            let text = self.error.clone().unwrap_or_else(|| t!("Loading issue"));
            return div().id("issue-detail").size_full().p(px(20.)).bg(theme.colors.background()).text_color(theme.colors.muted_foreground).child(text);
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
                    .gap(px(10.))
                    .child(avatar(&theme, Some(&login), 24.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(4.))
                            .child(div().flex().gap(px(6.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(div().text_color(theme.colors.text).font_weight(weight::SEMIBOLD).child(SharedString::from(login))).child(relative_label(&comment.created_at, now)))
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
                    .gap(px(8.))
                    .pl(px(4.))
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
        let picker = self.labels_open.then(|| {
            let candidates = self.candidates.clone().map(|c| c.labels).unwrap_or_default();
            div()
                .flex()
                .flex_col()
                .p(px(4.))
                .rounded(theme.radii.md)
                .border_1()
                .border_color(theme.colors.border)
                .bg(theme.colors.popover())
                .children(candidates.into_iter().map(|c| {
                    let held = detail.labels.iter().any(|l| l.name == c.name);
                    let on = self.label_draft.is_selected(&c.name, held);
                    let name = c.name.clone();
                    div()
                        .id(SharedString::from(format!("issue-label-{name}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .h(px(26.))
                        .px(px(6.))
                        .rounded(px(4.))
                        .hover(|s| s.bg(theme.colors.accent))
                        .child(Icon::new(if on { IconName::Check } else { IconName::Plus }).size(IconSize::Xs).color(theme.colors.muted_foreground))
                        .child(label_chip(&theme, &c.name, c.color.as_deref()))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_label(c.clone(), cx)))
                }))
        });
        let reactions = detail.reactions.clone().unwrap_or_default();
        let mut reaction_row = div().flex().flex_wrap().gap(px(4.));
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
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(22.))
                    .px(px(7.))
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
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.colors.background())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .px(px(20.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .border_b_1()
                    .border_color(theme.colors.border)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .child(div().pt(px(3.)).child(Icon::new(state_icon).size(IconSize::Lg).color(state_tone)))
                            .child(div().flex_1().ui_text(&theme, TextSize::Lg, weight::SEMIBOLD).text_color(theme.colors.text).child(SharedString::from(detail.title.clone())))
                            .child(div().ui_text(&theme, TextSize::Lg, weight::NORMAL).text_color(theme.colors.muted_foreground).child(SharedString::from(detail.display_key.clone()))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                            .text_color(theme.colors.muted_foreground)
                            .children(state_menu)
                            .when(access.states.is_empty(), |el| el.child(div().flex().items_center().gap(px(4.)).px(px(8.)).h(px(20.)).rounded_full().bg(state_tone.opacity(0.14)).text_color(state_tone).child(SharedString::from(detail.state.name.clone()))))
                            .child(avatar(&theme, detail.author.as_ref().map(|a| a.login.as_str()), 16.))
                            .child(SharedString::from(detail.author.as_ref().map_or("ghost".into(), |a| a.login.clone())))
                            .child(relative_label(&detail.updated_at, now))
                            .child(div().flex_1())
                            .child(Button::new("issue-link-thread").size(ButtonSize::Sm).variant(ButtonVariant::Ghost).icon(IconName::MessageSquare).label(t!("Link to thread")).on_click(cx.listener(move |_, _, _, cx| cx.emit(IssueDetailEvent::LinkToThread(reference.clone()))))),
                    )
                    .when_some(access.blocked, |el, b| {
                        el.child(div().flex().items_center().gap(px(6.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.muted_foreground).child(Icon::new(IconName::TriangleAlert).size(IconSize::Xs)).child(t!("Read-only")).child(SharedString::new_static(b.message())))
                    })
                    .when_some(self.status.clone(), |el, s| el.child(div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.destructive()).child(s))),
            )
            .child(
                div()
                    .id("issue-detail-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(20.))
                            .p(px(20.))
                            .child(div().ui_text(&theme, TextSize::Sm, weight::NORMAL).text_color(theme.colors.text).child(body))
                            .child(reaction_row)
                            .child(
                                Self::section(&theme, t!("Labels"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .items_center()
                                            .gap(px(6.))
                                            .children(detail.labels.iter().map(|l| label_chip(&theme, &l.name, l.color.as_deref())))
                                            .when(access.labels, |el| {
                                                el.child(Button::new("issue-labels-edit").size(ButtonSize::Xs).variant(ButtonVariant::Ghost).icon(IconName::Pencil).label(if self.labels_open { t!("Save") } else { t!("Change labels") }).on_click(cx.listener(|this, _, _, cx| this.toggle_label_picker(cx))))
                                            }),
                                    )
                                    .children(picker),
                            )
                            .child(Self::section(&theme, t!("Assignees")).child(div().flex().flex_wrap().gap(px(8.)).children(detail.assignees.iter().map(|a| {
                                div().flex().items_center().gap(px(6.)).ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(theme.colors.text).child(avatar(&theme, Some(&a.login), 18.)).child(SharedString::from(a.login.clone()))
                            }))))
                            .when(!detail.linked_pull_requests.is_empty(), |el| {
                                el.child(Self::section(&theme, t!("Linked pull requests")).children(detail.linked_pull_requests.iter().map(|pr| {
                                    let (icon, tone, _) = pull_request_state_presentation(&theme, pr.state, pr.is_draft);
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                                        .text_color(theme.colors.text)
                                        .child(Icon::new(icon).size(IconSize::Sm).color(tone))
                                        .child(div().text_color(theme.colors.muted_foreground).child(format!("#{}", pr.number)))
                                        .child(SharedString::from(pr.title.clone()))
                                        .when(pr.closes_issue, |el| el.child(div().px(px(6.)).rounded_full().bg(theme.colors.muted).ui_text(&theme, TextSize::Xs3, weight::MEDIUM).child(t!("Closes this issue"))))
                                })))
                            })
                            .child(Self::section(&theme, t!("Activity")).gap(px(14.)).children(activity.into_iter().map(|(_, el)| el)))
                            .child(if access.comment {
                                div()
                                    .flex()
                                    .gap(px(6.))
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
