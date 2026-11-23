//! "Checkout pull request" (`PullRequestThreadDialog.tsx`): paste a URL, a
//! checkout command or `#123`, see the resolved pull request, and start a
//! draft thread on its head branch in the main checkout or a new worktree.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Task, Window, component::input::{InputEvent, InputState}, div, px,
};
use t3code_contracts::{
    GitPreparePullRequestThreadInput, GitPreparePullRequestThreadResultPullRequest, GitPullRequestRefInput,
    PullRequestState, ThreadEnvMode, ThreadId,
};

use super::branch_picker::parse_pull_request_reference;
use super::git_actions_control_source::GitActionsSource;
use super::chat::thread_details_pr_row::pull_request_state_tone;
use super::ui::button::{Button, ButtonSize, ButtonVariant};
use super::ui::dialog::{DialogSurface, Modal};
use super::ui::input::TextInput;
use super::ui::spinner::{Spinner, SpinnerSize};
use super::ui::TextStyleExt as _;
use crate::appearance_fonts::{TextSize, weight};
use crate::i18n::{t, t_rich};
use crate::source_control_presentation::SourceControlPresentation;
use crate::theme_color::with_alpha;
use crate::ActiveAppTheme as _;

/// The dialog's result: the draft thread's branch and checkout.
#[derive(Clone, Debug, PartialEq)]
pub enum PullRequestThreadEvent {
    Prepared { branch: String, worktree_path: Option<String>, pull_request_url: String },
    Close,
}

/// The open dialog.
pub struct PullRequestThreadDialog {
    source: Arc<dyn GitActionsSource>,
    cwd: String,
    thread_id: Option<ThreadId>,
    presentation: SourceControlPresentation,
    reference: Entity<InputState>,
    dirty: bool,
    resolved: Option<GitPreparePullRequestThreadResultPullRequest>,
    resolving: bool,
    preparing: Option<ThreadEnvMode>,
    error: Option<SharedString>,
    _resolve: Option<Task<()>>,
}

impl EventEmitter<PullRequestThreadEvent> for PullRequestThreadDialog {}

impl PullRequestThreadDialog {
    pub fn new(
        source: Arc<dyn GitActionsSource>,
        cwd: String,
        thread_id: Option<ThreadId>,
        presentation: SourceControlPresentation,
        initial_reference: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let term = presentation.terminology.term_kind().as_str();
        let initial = initial_reference.to_owned();
        let reference = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("{term, select, mr {MR} cr {change request} other {PR}} URL, checkout command, or #42", term = term))
                .default_value(initial)
        });
        cx.subscribe_in(&reference, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.dirty = true;
                this.resolve(cx);
            }
        })
        .detach();
        let mut dialog = Self {
            source,
            cwd,
            thread_id,
            presentation,
            reference,
            dirty: !initial_reference.is_empty(),
            resolved: None,
            resolving: false,
            preparing: None,
            error: None,
            _resolve: None,
        };
        dialog.resolve(cx);
        dialog
    }

    fn parsed(&self, cx: &gpui_kit::App) -> Option<String> {
        parse_pull_request_reference(&self.reference.read(cx).value())
    }

    /// Resolves the typed reference after a short pause (`git.resolvePullRequest`).
    fn resolve(&mut self, cx: &mut Context<Self>) {
        self.resolved = None;
        self.error = None;
        let Some(reference) = self.parsed(cx) else {
            self.resolving = false;
            self._resolve = None;
            cx.notify();
            return;
        };
        self.resolving = true;
        let request = self.source.resolve_pull_request(GitPullRequestRefInput { cwd: self.cwd.clone(), reference });
        self._resolve = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let result = request.await;
            this.update(cx, |this, cx| {
                this.resolving = false;
                match result {
                    Ok(r) => this.resolved = Some(r.pull_request),
                    Err(error) => this.error = Some(error.0),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn prepare(&mut self, mode: ThreadEnvMode, cx: &mut Context<Self>) {
        let Some(reference) = self.parsed(cx) else { return };
        if self.resolved.is_none() || self.preparing.is_some() {
            return;
        }
        let term = self.presentation.terminology.term_kind().as_str();
        let request = self.source.prepare_pull_request_thread(GitPreparePullRequestThreadInput {
            cwd: self.cwd.clone(),
            reference,
            mode,
            thread_id: (mode == ThreadEnvMode::Worktree).then(|| self.thread_id.clone()).flatten(),
        });
        self.preparing = Some(mode);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                this.preparing = None;
                match result {
                    Ok(prepared) => cx.emit(PullRequestThreadEvent::Prepared {
                        branch: prepared.branch,
                        worktree_path: prepared.worktree_path,
                        pull_request_url: prepared.pull_request.url,
                    }),
                    Err(error) => {
                        this.error = Some(if error.0.is_empty() {
                            t!("Failed to prepare {term, select, mr {merge request} cr {change request} other {pull request}} thread.", term = term)
                        } else {
                            error.0
                        })
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for PullRequestThreadDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let term = self.presentation.terminology.term_kind().as_str();
        let value = self.reference.read(cx).value();
        let validation = if !self.dirty {
            None
        } else if value.trim().is_empty() {
            Some(t!(
                "Paste a {term, select, mr {merge request} cr {change request} other {pull request}} URL, checkout command, or enter 123 / #123.",
                term = term
            ))
        } else if self.parsed(cx).is_none() {
            Some(t!(
                "Use a {term, select, mr {merge request} cr {change request} other {pull request}} URL, checkout command, 123, or #123.",
                term = term
            ))
        } else {
            None
        };
        let error = validation.or_else(|| self.error.clone());
        let resolved = self.resolved.clone().map(|pr| {
            let (_, tone) = pull_request_state_tone(&theme, pr.state, false);
            let state = match pr.state {
                PullRequestState::Open => "Open",
                PullRequestState::Closed => "Closed",
                PullRequestState::Merged => "Merged",
            };
            div()
                .rounded(theme.radii.xl)
                .border_1()
                .border_color(with_alpha(c.border, 0.7))
                .bg(with_alpha(c.muted, 0.24))
                .p(px(12.))
                .flex()
                .items_start()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(div().truncate().font_weight(weight::MEDIUM).child(pr.title.clone()))
                        .child(
                            div().truncate().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(
                                t_rich!("#{number} ·{\" \"}{head} to{\" \"}{base}", number = pr.number, head = pr.head_branch.as_str(), base = pr.base_branch.as_str())
                                    .plain(),
                            ),
                        ),
                )
                .child(div().flex_none().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(tone).child(state))
        });
        let ready = self.resolved.is_some() && !self.resolving && self.preparing.is_none();
        let surface = DialogSurface::new(t!(
            "Checkout {term, select, mr {merge request} cr {change request} other {pull request}}",
            term = term
        ))
        .description(t!(
            "Resolve a {provider} {term, select, mr {merge request} cr {change request} other {pull request}}, then create the draft thread in the main repo or in a dedicated worktree.",
            provider = self.presentation.provider_name.as_ref(),
            term = term
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div().ui_text(&theme, TextSize::Xs, weight::MEDIUM).child(t!(
                        "{term, select, mr {merge request} cr {change request} other {pull request}}",
                        context = "field label, capitalized by CSS",
                        term = term
                    )),
                )
                .child(TextInput::new(&self.reference)),
        )
        .children(resolved)
        .children(self.resolving.then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .ui_text(&theme, TextSize::Xs, weight::NORMAL)
                .text_color(c.muted_foreground)
                .child(Spinner::new(SpinnerSize::Sm))
                .child(t!("Resolving {term, select, mr {merge request} cr {change request} other {pull request}}...", term = term))
        }))
        .children(error.map(|e| div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(e)))
        .footer(
            Button::new("pr-cancel")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .label(t!("Cancel"))
                .disabled(self.preparing.is_some())
                .on_click(cx.listener(|_, _, _, cx| cx.emit(PullRequestThreadEvent::Close))),
        )
        .footer(
            Button::new("pr-local")
                .variant(ButtonVariant::Outline)
                .size(ButtonSize::Sm)
                .label(if self.preparing == Some(ThreadEnvMode::Local) {
                    t!("Preparing local...")
                } else {
                    t!("Local", context = "checkout in the main repo")
                })
                .disabled(!ready)
                .on_click(cx.listener(|this, _, _, cx| this.prepare(ThreadEnvMode::Local, cx))),
        )
        .footer(
            Button::new("pr-worktree")
                .size(ButtonSize::Sm)
                .label(if self.preparing == Some(ThreadEnvMode::Worktree) {
                    t!("Preparing worktree...")
                } else {
                    t!("Worktree", context = "checkout in a new git worktree")
                })
                .disabled(!ready)
                .on_click(cx.listener(|this, _, _, cx| this.prepare(ThreadEnvMode::Worktree, cx))),
        );
        let this = cx.entity();
        Modal::dialog("pull-request-thread-dialog", surface, move |_, cx| {
            this.update(cx, |this, cx| {
                if this.preparing.is_none() {
                    cx.emit(PullRequestThreadEvent::Close);
                }
            })
        })
    }
}
