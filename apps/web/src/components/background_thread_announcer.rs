//! Screen-reader announcements for threads other than the open one. Ports
//! `backgroundThreadAnnouncement.ts` and `BackgroundThreadAnnouncer.tsx`.
//!
//! GPUI exposes accessibility roles and labels but no live-region politeness,
//! so [`BackgroundThreadAnnouncer`] renders the latest announcement as an
//! invisible `Status` node whose label changes; platforms that speak status
//! changes read it out.

use crate::i18n::t;
use std::collections::HashMap;

use gpui_kit::{
    AppContext as _, Context, Entity, Global, IntoElement, Render, SharedString, Window, div,
    prelude::*, px,
};

use crate::components::sidebar::model::{RunStatus, SubagentRequest};

/// A pending request, as announcements name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundRequest {
    Approval,
    Input,
}

/// The request and its id (`resolveBackgroundThreadRequest`). Both come from the
/// same source, so a subagent's request never takes the parent's id.
pub fn resolve_background_thread_request(
    has_pending_approvals: bool,
    has_pending_user_input: bool,
    pending_subagent_request: Option<SubagentRequest>,
    own_request_id: Option<&SharedString>,
) -> (Option<BackgroundRequest>, Option<SharedString>) {
    if has_pending_approvals {
        return (Some(BackgroundRequest::Approval), own_request_id.cloned());
    }
    if pending_subagent_request == Some(SubagentRequest::Approval) {
        return (Some(BackgroundRequest::Approval), None);
    }
    if has_pending_user_input {
        return (Some(BackgroundRequest::Input), own_request_id.cloned());
    }
    if pending_subagent_request == Some(SubagentRequest::UserInput) {
        return (Some(BackgroundRequest::Input), None);
    }
    (None, None)
}

/// One listed thread in a live environment (`BackgroundThreadSnapshot`).
#[derive(Clone, Debug)]
pub struct BackgroundThreadSnapshot {
    pub key: SharedString,
    pub title: SharedString,
    pub request: Option<BackgroundRequest>,
    /// `None` for a subagent's request, which the parent's shell only knows by kind.
    pub request_id: Option<SharedString>,
    pub run_id: Option<SharedString>,
    pub run_status: Option<RunStatus>,
    /// The run doing work now, which can be older than a queued run cancelled while it ran.
    pub active_run_id: Option<SharedString>,
    /// A snoozed thread stays quiet until its timer.
    pub snoozed: bool,
}

#[derive(Clone, Debug)]
struct MemoryEntry {
    request: Option<BackgroundRequest>,
    request_id: Option<SharedString>,
    /// The last run whose ending is known, so each run's ending is announced once.
    settled_run_id: Option<SharedString>,
}

/// What the announcer remembers between shell changes.
#[derive(Clone, Debug, Default)]
pub struct BackgroundThreadMemory(HashMap<SharedString, MemoryEntry>);

/// `THREAD_STATUS_TEXT` for a request.
fn request_text(request: BackgroundRequest) -> SharedString {
    match request {
        BackgroundRequest::Approval => t!("Needs approval"),
        BackgroundRequest::Input => t!("Needs your input"),
    }
}

/// How a run ended, spoken (`runOutcomeText`).
pub fn run_outcome_text(status: Option<RunStatus>) -> Option<SharedString> {
    Some(match status? {
        RunStatus::Completed => t!("Done"),
        RunStatus::Failed => t!("Failed"),
        RunStatus::Interrupted | RunStatus::Cancelled => t!("Stopped"),
        _ => return None,
    })
}

/// The next announcement (`nextBackgroundThreadAnnouncement`). First sightings
/// are a silent baseline, so opening the app or reconnecting reads nothing out.
pub fn next_background_thread_announcement(
    previous: &BackgroundThreadMemory,
    threads: &[BackgroundThreadSnapshot],
    open_thread_key: Option<&SharedString>,
) -> (BackgroundThreadMemory, Option<String>) {
    let mut memory = HashMap::with_capacity(threads.len());
    let mut announcements = Vec::new();
    for thread in threads {
        let before = previous.0.get(&thread.key);
        let outcome = run_outcome_text(thread.run_status);
        let settled = outcome.is_some() || thread.run_status == Some(RunStatus::RolledBack);
        memory.insert(
            thread.key.clone(),
            MemoryEntry {
                request: thread.request,
                request_id: thread.request_id.clone(),
                settled_run_id: if settled {
                    thread.run_id.clone()
                } else {
                    before.and_then(|before| before.settled_run_id.clone())
                },
            },
        );
        let Some(before) = before else { continue };
        if Some(&thread.key) == open_thread_key || thread.snoozed {
            continue;
        }
        if let Some(request) = thread.request
            && (thread.request != before.request || thread.request_id != before.request_id)
        {
            let status = request_text(request);
            announcements.push(t!("{status}: {title}", status = status.as_ref(), title = thread.title.as_ref()).to_string());
        }
        // While another run still works, the latest run's ending (a queued message
        // cancelled before it started) isn't the thread finishing.
        let another_run_working =
            thread.active_run_id.is_some() && thread.active_run_id != thread.run_id;
        if let Some(outcome) = outcome
            && thread.run_id != before.settled_run_id
            && !another_run_working
        {
            announcements.push(t!("{status}: {title}", status = outcome.as_ref(), title = thread.title.as_ref()).to_string());
        }
    }
    let announcement = (!announcements.is_empty()).then(|| announcements.join(". "));
    (BackgroundThreadMemory(memory), announcement)
}

/// The invisible region background announcements are spoken through. It sits
/// at the window root, outside the sidebar, so a collapsed sidebar does not
/// silence it. Render [`BackgroundThreadAnnouncer::view`] once per window.
#[derive(Default)]
pub struct BackgroundThreadAnnouncer {
    text: SharedString,
}

struct GlobalAnnouncer(Entity<BackgroundThreadAnnouncer>);

impl Global for GlobalAnnouncer {}

impl BackgroundThreadAnnouncer {
    /// Creates the app-wide announcer.
    pub fn init(cx: &mut gpui_kit::App) {
        let announcer = cx.new(|_| Self::default());
        cx.set_global(GlobalAnnouncer(announcer));
    }

    /// The announcer view.
    pub fn view(cx: &gpui_kit::App) -> Entity<Self> {
        cx.global::<GlobalAnnouncer>().0.clone()
    }

    /// Speaks `text`.
    pub fn announce(text: impl Into<SharedString>, cx: &mut gpui_kit::App) {
        let text = text.into();
        Self::view(cx).update(cx, |announcer, cx| {
            announcer.text = text;
            cx.notify();
        });
    }

    /// The last announcement, for checks and scenes.
    pub fn text(&self) -> &SharedString {
        &self.text
    }
}

impl Render for BackgroundThreadAnnouncer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("background-thread-announcer")
            .absolute()
            .size(px(0.))
            .overflow_hidden()
            .role(gpui_kit::Role::Status)
            .aria_label(self.text.clone())
    }
}
