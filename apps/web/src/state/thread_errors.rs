//! Thread errors for the banner (`ChatView`'s `localServerError` / `localDraftError` and the
//! session's dismissed banner keys): a send failure recorded by the view key of the thread or
//! draft it belongs to, the server's last error otherwise, and the prompt a failed send gives back.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use t3code_shared::thread_projection::ThreadProjection;

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
        let thread = thread?;
        let row = shell.read(cx).row(thread)?;
        let error = row.source.last_error.clone().filter(|e| !e.trim().is_empty())?;
        let usage_limit = matches!(row.source.last_error_class, Some(t3code_contracts::OrchestrationV2ProviderFailureClass::UsageLimit));
        // The open thread's projection names the failure item the error came from; a row the
        // window does not show has no banner to key.
        let latest_run = row.source.latest_run_id.clone();
        let threads = StateBridge::threads(cx);
        let source = threads
            .read(cx)
            .active()
            .filter(|live| live.read(cx).key() == thread)
            .and_then(|live| live.read(cx).projection().cloned())
            .and_then(|projection| error_source(&projection, latest_run.as_ref()));
        Some((
            error.into(),
            source,
            usage_limit,
            row.source.last_error_code.clone().map(SharedString::from),
        ))
    }
}

/// `getThreadErrorBannerKey`.
fn banner_key(key: &str, error: &str, source: Option<&str>) -> String {
    format!("{key}\u{0}{}\u{0}{error}", source.unwrap_or_default())
}

/// `latestRootProviderFailureItem`: the newest failed error item of a failed run's root node.
fn root_failure(run: &t3code_contracts::OrchestrationV2Run, projection: &ThreadProjection) -> Option<Arc<t3code_contracts::OrchestrationV2TurnItem>> {
    use t3code_contracts::{OrchestrationV2RunStatus as RunStatus, OrchestrationV2TurnItem as Item, OrchestrationV2TurnItemStatus as ItemStatus};
    if run.status != RunStatus::Failed {
        return None;
    }
    let mut latest: Option<(&Arc<Item>, i64, u64, &str)> = None;
    for item in projection.turn_items.iter() {
        let Item::Error(error) = &**item else { continue };
        if error.status != ItemStatus::Failed || error.run_id.as_ref() != Some(&run.id) || error.node_id != run.root_node_id {
            continue;
        }
        let at = t3code_client_runtime::state::models::parse_ms(&error.updated_at).unwrap_or_default();
        let newer = latest.is_none_or(|(_, l_at, l_ord, l_id)| at > l_at || (at == l_at && (error.ordinal > l_ord || (error.ordinal == l_ord && error.id.as_str() > l_id))));
        if newer {
            latest = Some((item, at, error.ordinal, error.id.as_str()));
        }
    }
    latest.map(|(item, ..)| Arc::clone(item))
}

/// `threadErrorSource`: `<failure item id>@<completedAt>` of the failure the error came from, so a
/// dismissed banner returns only for a new failure.
fn error_source(projection: &ThreadProjection, latest_run_id: Option<&t3code_contracts::RunId>) -> Option<String> {
    use t3code_contracts::{OrchestrationV2RunStatus as RunStatus, OrchestrationV2TurnItem as Item};
    let item_source = |item: &Item| match item {
        Item::Error(error) => Some(format!("{}@{}", error.id.as_str(), error.completed_at.as_deref().unwrap_or_default())),
        _ => None,
    };
    let message = |item: &Item| match item {
        Item::Error(error) => Some(error.failure.message.clone()),
        _ => None,
    };
    let session_error = projection
        .provider_sessions
        .iter()
        .rev()
        .find(|session| session.provider_instance_id == projection.thread.provider_instance_id)
        .and_then(|session| session.last_error.clone());
    let latest_run = latest_run_id.and_then(|id| projection.runs.iter().find(|run| &run.id == id));
    let failure = latest_run.and_then(|run| root_failure(run, projection));
    if let Some(item) = &failure
        && (session_error.is_none() || session_error == message(item))
    {
        return item_source(item);
    }
    let session_error = session_error?;
    let ended = projection.runs.iter().filter(|run| matches!(run.status, RunStatus::Failed | RunStatus::Completed)).max_by_key(|run| run.ordinal)?;
    let ended_failure = match &failure {
        Some(item) if matches!(&**item, Item::Error(error) if error.run_id.as_ref() == Some(&ended.id)) => Some(Arc::clone(item)),
        _ => root_failure(ended, projection),
    }?;
    (message(&ended_failure).as_deref() == Some(session_error.as_str())).then(|| item_source(&ended_failure)).flatten()
}
