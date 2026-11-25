//! Asks whoever can approve to look at T3 Connect devices waiting on an environment. Ports
//! `ConnectDeviceApprovalCoordinator.tsx`: for each connected environment that requires device
//! approval and lets this session manage access, a toast per waiting device with Review, Deny
//! and Approve. A toast ends when its request does or expires (the server publishes nothing on
//! expiry); a failed Approve or Deny offers the request again.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::{App, Global, SharedString, Task};
use t3code_client_runtime::state::connect_device_approval::{approve_connect_device, revoke_pairing_link};
use t3code_contracts::{AuthAccessSnapshot, AuthEnvironmentScope, EnvironmentId};

use crate::components::settings::connections_settings_access::{check_session, grants};
use crate::components::settings::connections_settings_access_clients::{apply_access_event, describe_connect_device, follow_access};
use crate::components::settings::settings_layout::SettingsNavigator;
use crate::components::settings::settings_search::SettingsPath;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::connection::{CommandError, ConnectionRuntime};
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;

/// One environment being watched: its follower and the toast per waiting request.
struct Watched {
    _follow: Task<()>,
    /// Request id to (toast id, verification code and device it shows).
    toasts: HashMap<String, (u64, String)>,
}

/// The environments watched now.
#[derive(Default)]
pub struct ConnectDeviceApprovalCoordinator {
    watched: HashMap<EnvironmentId, Watched>,
    /// Environments whose session may not manage access, or is being checked.
    skipped: HashSet<EnvironmentId>,
}

impl Global for ConnectDeviceApprovalCoordinator {}

impl ConnectDeviceApprovalCoordinator {
    /// Starts following the environment list. Call once the connection runtime runs.
    pub fn init(cx: &mut App) {
        if !ConnectionRuntime::is_running(cx) || cx.has_global::<Self>() {
            return;
        }
        cx.set_global(Self::default());
        let state = ConnectionRuntime::state(cx);
        cx.observe(&state, |_, cx| Self::sync(cx)).detach();
        cx.observe_global::<ServerSettingsStore>(Self::sync).detach();
        Self::sync(cx);
    }

    /// The environments with a waiting device toast now, for scripted checks.
    pub fn pending(cx: &App) -> Vec<(EnvironmentId, String)> {
        cx.try_global::<Self>()
            .map(|c| c.watched.iter().flat_map(|(id, w)| w.toasts.keys().map(move |r| (id.clone(), r.clone()))).collect())
            .unwrap_or_default()
    }

    fn sync(cx: &mut App) {
        let state = ConnectionRuntime::state(cx);
        let wanted: Vec<EnvironmentId> = state
            .read(cx)
            .environments
            .iter()
            .filter(|env| env.is_connected())
            .filter(|env| ServerSettingsStore::config(cx, env.id()).is_some_and(|c| c.settings.require_connect_device_approval == Some(true)))
            .map(|env| env.id().clone())
            .collect();
        let gone: Vec<EnvironmentId> = cx.global::<Self>().watched.keys().filter(|id| !wanted.contains(id)).cloned().collect();
        for id in gone {
            if let Some(watched) = cx.global_mut::<Self>().watched.remove(&id) {
                for (toast, _) in watched.toasts.values() {
                    Toaster::dismiss(*toast, cx);
                }
            }
        }
        cx.global_mut::<Self>().skipped.retain(|id| wanted.contains(id));
        for id in wanted {
            let this = cx.global::<Self>();
            if this.watched.contains_key(&id) || this.skipped.contains(&id) {
                continue;
            }
            cx.global_mut::<Self>().skipped.insert(id.clone());
            let Some(check) = check_session(&id, cx) else { continue };
            cx.spawn(async move |cx| {
                let session = check.await;
                cx.update(|cx| {
                    if session.is_ok_and(|s| grants(&s, AuthEnvironmentScope::AccessWrite)) {
                        Self::watch(id, cx);
                    }
                });
            })
            .detach();
        }
    }

    fn watch(id: EnvironmentId, cx: &mut App) {
        let Some(link) = ConnectionRuntime::global(cx).environment_link(&id) else { return };
        cx.global_mut::<Self>().skipped.remove(&id);
        let mut events = follow_access(link, cx);
        let environment = id.clone();
        let follow = cx.spawn(async move |cx| {
            let mut snapshot: Option<AuthAccessSnapshot> = None;
            while let Some(event) = events.next().await {
                apply_access_event(&mut snapshot, event);
                if let Some(snapshot) = &snapshot {
                    let snapshot = snapshot.clone();
                    let environment = environment.clone();
                    cx.update(|cx| Self::show(&environment, &snapshot, cx));
                }
            }
        });
        cx.global_mut::<Self>().watched.insert(id, Watched { _follow: follow, toasts: HashMap::new() });
    }

    /// Opens, keeps and closes toasts to match the waiting requests.
    fn show(id: &EnvironmentId, snapshot: &AuthAccessSnapshot, cx: &mut App) {
        let label: SharedString = ConnectionRuntime::state(cx).read(cx).environment(id).map(|e| e.entry.label.clone()).unwrap_or_default().into();
        let now = jiff::Timestamp::now();
        let live: Vec<(String, String, SharedString, Duration)> = snapshot
            .pairing_links
            .iter()
            .filter_map(|link| {
                let request = link.connect_device_request.as_ref()?;
                let expires: jiff::Timestamp = link.expires_at.parse().ok()?;
                let remaining = expires.duration_since(now);
                (remaining.is_positive()).then(|| {
                    let device = describe_connect_device(request.client.as_ref());
                    (link.id.clone(), request.verification_code.clone(), device, Duration::from_secs_f64(remaining.as_secs_f64()))
                })
            })
            .collect();
        let Some(watched) = cx.global_mut::<Self>().watched.get_mut(id) else { return };
        let stale: Vec<String> = watched.toasts.keys().filter(|r| !live.iter().any(|(id, ..)| id == *r)).cloned().collect();
        let mut dismiss = Vec::new();
        for request in stale {
            if let Some((toast, _)) = watched.toasts.remove(&request) {
                dismiss.push(toast);
            }
        }
        let fresh: Vec<_> = live.into_iter().filter(|(request, code, device, _)| watched.toasts.get(request).is_none_or(|(_, key)| *key != format!("{code}:{device}"))).collect();
        for toast in dismiss {
            Toaster::dismiss(toast, cx);
        }
        for (request, code, device, remaining) in fresh {
            let (approve_id, deny_id) = (request.clone(), request.clone());
            let (environment_a, environment_d) = (id.clone(), id.clone());
            let data = ToastData::new(ToastKind::Info, t!("A device wants to connect to {environmentLabel}", environmentLabel = label.as_ref()))
                .description(t!("{device}. Approve only if it shows code {code}.", device = device.as_ref(), code = code.as_str()))
                .timeout(remaining)
                .secondary_action(t!("Review", context = "verb, opens the device in settings"), |_, cx| SettingsNavigator::open(SettingsPath::Access, cx))
                .secondary_action(t!("Deny"), move |_, cx| {
                    Self::settle(&environment_d, &deny_id, t!("Could not deny the device"), cx, |link, request| {
                        Box::pin(async move { revoke_pairing_link(&link, request).await.map_err(CommandError::from) })
                    })
                })
                .action(t!("Approve"), move |_, cx| {
                    Self::settle(&environment_a, &approve_id, t!("Could not approve the device"), cx, |link, request| {
                        Box::pin(async move { approve_connect_device(&link, request).await.map(|_| ()).map_err(CommandError::from) })
                    })
                });
            let previous = cx.global::<Self>().watched.get(id).and_then(|w| w.toasts.get(&request)).map(|(toast, _)| *toast);
            let toast = match previous {
                Some(toast) if Toaster::update(toast, data.clone(), cx) => toast,
                _ => Toaster::push(data, cx),
            };
            if let Some(watched) = cx.global_mut::<Self>().watched.get_mut(id) {
                watched.toasts.insert(request, (toast, format!("{code}:{device}")));
            }
        }
    }

    /// Closes the request's toast and runs `call`; a failure is reported and the request offered again.
    fn settle(
        id: &EnvironmentId,
        request: &str,
        failure: SharedString,
        cx: &mut App,
        call: impl FnOnce(t3code_client_runtime::preview::environment_link::EnvironmentLink, String) -> futures::future::BoxFuture<'static, Result<(), CommandError>> + Send + 'static,
    ) {
        let Some(link) = ConnectionRuntime::global(cx).environment_link(id) else { return };
        if let Some(watched) = cx.global_mut::<Self>().watched.get_mut(id)
            && let Some((toast, _)) = watched.toasts.get_mut(request)
        {
            let toast = *toast;
            // Forgetting the key lets the next snapshot offer the request again if it failed.
            watched.toasts.get_mut(request).map(|(_, key)| key.clear());
            Toaster::dismiss(toast, cx);
        }
        let task = ConnectionRuntime::global(cx).spawn(call(link, request.to_owned()), cx);
        cx.spawn(async move |cx| {
            if let Err(error) = task.await {
                cx.update(|cx| Toaster::push(ToastData::new(ToastKind::Error, failure).description(error.0), cx));
            }
        })
        .detach();
    }
}
