//! Thread errors for the banner (`ChatView`'s `localServerError` / `localDraftError` and the
//! session's dismissed banner keys): a send failure recorded by the view key of the thread or
//! draft it belongs to, the server's last error otherwise, and the prompt a failed send gives back.

use std::collections::{HashMap, HashSet};

use gpui_kit::{App, AppContext as _, Entity, Global, SharedString};

use crate::components::chat::thread_error_banner::ThreadErrorBannerInput;
use crate::components::sidebar::model::ThreadKey;
use crate::state::StateBridge;

/// The window's thread errors. Views observe [`ThreadErrors::entity`].
#[derive(Default)]
pub struct ThreadErrors {
    /// Send failures by view key (`draft:<id>` or the thread's scoped key).
    local: HashMap<SharedString, SharedString>,
    /// `getThreadErrorBannerKey`s dismissed this session.
    dismissed: HashSet<String>,
    /// A failed send's prompt, for the composer showing that key (`useComposerSend` restores it).
    restore: Option<(SharedString, SharedString)>,
}

struct Holder(Entity<ThreadErrors>);

impl Global for Holder {}

impl ThreadErrors {
    /// The shared model, created on first use.
    pub fn entity(cx: &mut App) -> Entity<ThreadErrors> {
        if let Some(holder) = cx.try_global::<Holder>() {
            return holder.0.clone();
        }
        let entity = cx.new(|_| ThreadErrors::default());
        cx.set_global(Holder(entity.clone()));
        entity
    }

    /// Records (or with `None` clears) the send error of `key` (`setThreadError`).
    pub fn set(key: &SharedString, error: Option<SharedString>, cx: &mut App) {
        Self::entity(cx).update(cx, |this, cx| {
            let changed = match error.filter(|e| !e.trim().is_empty()) {
                Some(error) => this.local.insert(key.clone(), error.clone()).as_ref() != Some(&error),
                None => this.local.remove(key).is_some(),
            };
            if changed {
                cx.notify();
            }
        });
    }

    /// A send to `key` failed: shows `error` and gives `prompt` back to its composer.
    pub fn send_failed(key: &SharedString, error: SharedString, prompt: SharedString, cx: &mut App) {
        Self::set(key, Some(error), cx);
        Self::entity(cx).update(cx, |this, cx| {
            this.restore = (!prompt.trim().is_empty()).then(|| (key.clone(), prompt));
            cx.notify();
        });
    }

    /// The prompt a failed send left for `key`, taken once.
    pub fn take_restore(key: &SharedString, cx: &mut App) -> Option<SharedString> {
        Self::entity(cx).update(cx, |this, _| match &this.restore {
            Some((k, _)) if k == key => this.restore.take().map(|(_, text)| text),
            _ => None,
        })
    }

    /// The banner for view `key`: its send error, else the server's last error for `thread`,
    /// unless dismissed this session.
    pub fn banner(key: &SharedString, thread: Option<&ThreadKey>, cx: &mut App) -> Option<ThreadErrorBannerInput> {
        let (error, source, usage_limit, code) = Self::current(key, thread, cx)?;
        let entity = Self::entity(cx);
        let dismissed = entity.read(cx).dismissed.contains(&banner_key(key, &error, source.as_deref()));
        (!dismissed).then_some(ThreadErrorBannerInput { error, usage_limit, code })
    }

    /// Dismisses `key`'s banner: clears its send error and hides this message for the session.
    pub fn dismiss(key: &SharedString, thread: Option<&ThreadKey>, cx: &mut App) {
        let current = Self::current(key, thread, cx);
        Self::entity(cx).update(cx, |this, cx| {
            this.local.remove(key);
            if let Some((error, source, _, _)) = current {
                this.dismissed.insert(banner_key(key, &error, source.as_deref()));
            }
            cx.notify();
        });
    }

    /// (error, source, usage limit, code): the local error, else the server row's.
    fn current(key: &SharedString, thread: Option<&ThreadKey>, cx: &mut App) -> Option<(SharedString, Option<String>, bool, Option<SharedString>)> {
        if let Some(local) = Self::entity(cx).read(cx).local.get(key).cloned() {
            return Some((local, None, false, None));
        }
        // The fixture world has no live state.
        if !cx.has_global::<StateBridge>() {
            return None;
        }
        let shell = StateBridge::shell(cx);
        let row = shell.read(cx).row(thread?)?;
        let error = row.source.last_error.clone().filter(|e| !e.trim().is_empty())?;
        let usage_limit = matches!(row.source.last_error_class, Some(t3code_contracts::OrchestrationV2ProviderFailureClass::UsageLimit));
        Some((
            error.into(),
            row.source.latest_run_id.as_ref().map(|r| r.to_string()),
            usage_limit,
            row.source.last_error_code.clone().map(SharedString::from),
        ))
    }
}

/// `getThreadErrorBannerKey`.
fn banner_key(key: &str, error: &str, source: Option<&str>) -> String {
    format!("{key}\u{0}{}\u{0}{error}", source.unwrap_or_default())
}
