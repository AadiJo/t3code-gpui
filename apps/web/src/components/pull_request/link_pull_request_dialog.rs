//! "Link pull request" (`LinkPullRequestDialog.tsx`): attach a pull request to a thread by its
//! URL, or by its number for the thread's repository (`resolveLinkPullRequestInput`).

use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, component::input::{InputEvent, InputState}, div, prelude::*,
};
use t3code_client_runtime::change_request_links::{
    ChangeRequestUrlInput, SourceControlHostKind, change_request_url, parse_change_request_url,
};

use crate::appearance_fonts::{TextSize, weight};
use crate::components::branch_picker::parse_pull_request_reference;
use crate::components::ui::TextStyleExt as _;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{DialogSurface, Modal};
use crate::components::ui::input::TextInput;
use crate::i18n::t;
use crate::ActiveAppTheme as _;

/// A pull request the dialog resolved, as `thread.pull-request.link` takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLink {
    pub host: String,
    pub repository: String,
    pub number: u64,
    pub url: String,
}

/// The thread project's repository, so a bare number resolves (`pullRequestLinkProject`).
#[derive(Clone, Debug)]
pub struct LinkProject {
    pub kind: SourceControlHostKind,
    pub host: String,
    pub repository: String,
    pub remote_url: Option<String>,
    pub web_url: Option<String>,
}

impl LinkProject {
    /// The project's change request URL for `number`, when its host can write one.
    fn web_url(&self, number: u64) -> Option<String> {
        change_request_url(self.kind, ChangeRequestUrlInput {
            host: &self.host,
            repository: &self.repository,
            number,
            remote_url: self.remote_url.as_deref(),
            web_url: self.web_url.as_deref(),
        })
    }
}

/// `resolveLinkPullRequestInput`: `None` while the text is no reference, `Err` with the reason a
/// reference cannot be linked here.
pub fn resolve_link_input(reference: &str, project: Option<&LinkProject>) -> Option<Result<ResolvedLink, SharedString>> {
    let trimmed = reference.trim();
    let parsed = if parse_change_request_url(trimmed).is_some() { trimmed.to_owned() } else { parse_pull_request_reference(reference)? };
    if let Some((_, link)) = parse_change_request_url(&parsed) {
        return Some(Ok(ResolvedLink { host: link.host, repository: link.repository, number: link.number, url: parsed }));
    }
    let number = parsed.parse::<u64>().ok().filter(|n| *n >= 1)?;
    let Some(project) = project else {
        return Some(Err(t!("Paste the full pull request URL; this thread has no repository to resolve the number against.")));
    };
    let Some((url, link)) = project.web_url(number).and_then(|url| parse_change_request_url(&url).map(|(_, link)| (url, link))) else {
        return Some(Err(t!("Paste a full URL; this project's host has no known {changeRequest} URL.", changeRequest = "pull request")));
    };
    Some(Ok(ResolvedLink { host: link.host, repository: link.repository, number: link.number, url }))
}

/// Links the resolved pull request; the error is shown under the field.
pub type LinkPullRequest = Rc<dyn Fn(ResolvedLink, &mut App) -> Task<Result<(), SharedString>>>;

/// Sent when the dialog should close.
pub struct LinkPullRequestClosed;

/// The open dialog.
pub struct LinkPullRequestDialog {
    reference: Entity<InputState>,
    project: Option<LinkProject>,
    link: LinkPullRequest,
    dirty: bool,
    pending: bool,
    submit_error: Option<SharedString>,
    /// The field takes focus after the first frame; the modal focuses itself while it opens.
    focus_input: bool,
}

impl EventEmitter<LinkPullRequestClosed> for LinkPullRequestDialog {}

impl LinkPullRequestDialog {
    pub fn new(project: Option<LinkProject>, link: LinkPullRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let reference = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Pull request URL or #42")));
        cx.subscribe_in(&reference, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => {
                this.dirty = true;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.submit(cx),
            _ => {}
        })
        .detach();
        Self { reference, project, link, dirty: false, pending: false, submit_error: None, focus_input: true }
    }

    fn resolved(&self, cx: &App) -> Option<Result<ResolvedLink, SharedString>> {
        resolve_link_input(&self.reference.read(cx).value(), self.project.as_ref())
    }

    /// Closes unless a link is in flight (`pending ? undefined : onOpenChange`).
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if !self.pending {
            cx.emit(LinkPullRequestClosed);
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        let Some(Ok(link)) = self.resolved(cx) else {
            cx.notify();
            return;
        };
        if self.pending {
            return;
        }
        self.submit_error = None;
        self.pending = true;
        let task = (self.link)(link, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.pending = false;
                match result {
                    Ok(()) => cx.emit(LinkPullRequestClosed),
                    Err(error) => {
                        this.submit_error = Some(if error.is_empty() { t!("Could not link the pull request.") } else { error });
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Focusable for LinkPullRequestDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.reference.focus_handle(cx)
    }
}

impl Render for LinkPullRequestDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.focus_input) {
            cx.defer_in(window, |this, window, cx| this.reference.update(cx, |input, cx| input.focus(window, cx)));
        }
        let theme = cx.app_theme().clone();
        let c = &theme.colors;
        let value = self.reference.read(cx).value();
        let resolved = self.resolved(cx);
        let validation = if !self.dirty {
            None
        } else if value.trim().is_empty() {
            Some(t!("Paste a pull request URL or enter 123 / #123."))
        } else {
            match &resolved {
                None => Some(t!("Use a pull request URL, 123, or #123.")),
                Some(Err(error)) => Some(error.clone()),
                Some(Ok(_)) => None,
            }
        };
        let summary = match &resolved {
            Some(Ok(link)) => Some(format!("{}/{} #{}", link.host, link.repository, link.number)),
            _ => None,
        };
        let error = validation.or_else(|| self.submit_error.clone());
        let ready = matches!(resolved, Some(Ok(_))) && !self.pending;
        let weak = cx.weak_entity();
        let close = weak.clone();
        let submit = weak.clone();
        let surface = DialogSurface::new(t!("Link pull request"))
            .max_width_rem(36.)
            .description(t!("Attach a pull request to this thread by its URL, or by its number for this thread's repository."))
            .on_close(move |_, cx| {
                close.update(cx, |this, cx| this.close(cx)).ok();
            })
            .child(TextInput::new(&self.reference))
            .children(summary.map(|s| div().truncate().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.muted_foreground).child(s)))
            .children(error.map(|e| div().ui_text(&theme, TextSize::Xs, weight::NORMAL).text_color(c.destructive()).child(e)))
            .footer(
                Button::new("link-pr-cancel")
                    .variant(ButtonVariant::Outline)
                    .size(ButtonSize::Sm)
                    .label(t!("Cancel"))
                    .disabled(self.pending)
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |this, cx| this.close(cx)).ok();
                    }),
            )
            .footer(
                Button::new("link-pr-submit")
                    .size(ButtonSize::Sm)
                    .label(if self.pending { t!("Linking...") } else { t!("Link", context = "verb, button") })
                    .disabled(!ready)
                    .on_click(move |_, _, cx| {
                        submit.update(cx, |this, cx| this.submit(cx)).ok();
                    }),
            );
        let backdrop = cx.weak_entity();
        Modal::dialog("link-pull-request-dialog", div().id("link-pull-request-content").occlude().w_full().max_w(theme.rem(36.)).child(surface), move |_, cx| {
            backdrop.update(cx, |this, cx| this.close(cx)).ok();
        })
    }
}
