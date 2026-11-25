//! Provider updates across every machine. Ports `ProviderUpdatesAction.tsx`
//! (`useCheckProviderUpdates`, `useProviderUpdates`, `ProviderUpdatesAction`).
//!
//! [`ProviderUpdates`] is a GPUI global entity that notifies whenever something its answers
//! read changes: the connected environments, their live provider lists and their sessions'
//! permissions. Views observe it and read [`ProviderUpdates::machines`] in render.

use std::collections::HashMap;
use std::sync::Arc;

use futures::FutureExt as _;
use futures::future::Shared;
use gpui_kit::{App, AppContext as _, Context, Entity, EntityId, Global, IntoElement, Render, SharedString, Subscription, Task, Window, div, prelude::*};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::rpc::methods::{ServerRefreshProviders, ServerUpdateProvider};
use t3code_contracts::{AuthEnvironmentScope, ProviderDriverKind, ProviderInstanceId, ServerProviderUpdateInput, ServerRefreshProvidersPayload};

use crate::components::provider_update_launch_notification::{
    ProviderUpdateRun, one_click_update_candidates, provider_name, provider_update_run_toast_view,
};
use crate::components::server_update_action::ServerUpdateStore;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::components::ui::tooltip::{TooltipSurface, tooltip};
use crate::connection::ConnectionRuntime;
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::provider_instances::ServerProvider;
use crate::state::{LiveEnvironment, StateBridge};

/// One machine with providers "Update all" can update.
#[derive(Clone, Debug)]
pub struct ProviderUpdateMachine {
    pub environment_id: EnvironmentId,
    pub label: SharedString,
    pub candidates: Vec<Arc<ServerProvider>>,
}

impl ProviderUpdateMachine {
    /// The candidates' display names, e.g. `["Claude", "Codex"]`.
    pub fn provider_names(&self) -> Vec<SharedString> {
        self.candidates.iter().map(|c| SharedString::from(provider_name(&c.driver).to_owned())).collect()
    }
}

/// See the module docs.
pub struct ProviderUpdates {
    /// The settings header's "Update all" is running.
    updating_all: bool,
    /// The running model rediscovery; a repeat check joins it.
    checking: Option<Shared<Task<()>>>,
    environments: HashMap<EntityId, Subscription>,
    _subscriptions: Vec<Subscription>,
}

struct GlobalProviderUpdates(Entity<ProviderUpdates>);

impl Global for GlobalProviderUpdates {}

impl ProviderUpdates {
    /// The app's entity, created on first use.
    pub fn entity(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalProviderUpdates>() {
            return global.0.clone();
        }
        let entity = cx.new(Self::new);
        cx.set_global(GlobalProviderUpdates(entity.clone()));
        entity
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let updates = ServerUpdateStore::entity(cx);
        let mut subscriptions = vec![cx.observe(&updates, |_, _, cx| cx.notify())];
        if ConnectionRuntime::is_running(cx) {
            subscriptions.push(cx.observe(&ConnectionRuntime::state(cx), |_, _, cx| cx.notify()));
        }
        if StateBridge::is_running(cx) {
            let environments = StateBridge::environments(cx);
            subscriptions.push(cx.observe(&environments, |this, _, cx| {
                this.follow_environments(cx);
                cx.notify();
            }));
        }
        let mut this = Self { updating_all: false, checking: None, environments: HashMap::new(), _subscriptions: subscriptions };
        this.follow_environments(cx);
        this
    }

    /// Observes each environment entity, whose config carries the live provider list.
    fn follow_environments(&mut self, cx: &mut Context<Self>) {
        if !StateBridge::is_running(cx) {
            return;
        }
        let list = StateBridge::environments(cx).read(cx).list.clone();
        self.environments.retain(|id, _| list.iter().any(|env| env.entity_id() == *id));
        for env in list {
            self.environments.entry(env.entity_id()).or_insert_with(|| cx.observe(&env, |_, _, cx| cx.notify()));
        }
    }

    /// An environment's live providers, else the last config it received.
    pub fn providers(environment_id: &EnvironmentId, cx: &App) -> Vec<Arc<ServerProvider>> {
        let live = StateBridge::is_running(cx)
            .then(|| StateBridge::environments(cx).read(cx).get(environment_id, cx))
            .flatten()
            .and_then(|env: Entity<LiveEnvironment>| env.read(cx).config.clone());
        let config = live.or_else(|| ServerSettingsStore::config(cx, environment_id));
        config.map(|c| c.providers.iter().cloned().map(Arc::new).collect()).unwrap_or_default()
    }

    /// Connected environments whose session may manage providers, with their labels.
    fn manageable(cx: &mut App) -> Vec<(EnvironmentId, SharedString)> {
        if !ConnectionRuntime::is_running(cx) {
            return Vec::new();
        }
        let environments: Vec<_> = ConnectionRuntime::state(cx)
            .read(cx)
            .environments
            .iter()
            .filter(|env| env.is_connected())
            .map(|env| (env.id().clone(), SharedString::from(env.entry.label.clone())))
            .collect();
        environments.into_iter().filter(|(id, _)| ServerUpdateStore::grants(id, AuthEnvironmentScope::ProvidersManage, cx)).collect()
    }

    /// Machines with one-click provider updates (`useProviderUpdates().machines`).
    pub fn machines(cx: &mut App) -> Vec<ProviderUpdateMachine> {
        Self::manageable(cx)
            .into_iter()
            .filter_map(|(environment_id, label)| {
                let providers = Self::providers(&environment_id, cx);
                let candidates = one_click_update_candidates(&providers);
                (!candidates.is_empty()).then_some(ProviderUpdateMachine { environment_id, label, candidates })
            })
            .collect()
    }

    /// Updates every candidate on every machine at once (`updateAll`). Toasts the result
    /// unless `quiet`.
    pub fn update_all(quiet: bool, cx: &mut App) -> Task<Vec<ProviderUpdateRun>> {
        let calls: Vec<_> = Self::machines(cx)
            .into_iter()
            .flat_map(|machine| {
                machine.candidates.into_iter().map(move |candidate| (machine.environment_id.clone(), machine.label.clone(), candidate))
            })
            .map(|(environment_id, machine_label, candidate)| {
                let input = ServerProviderUpdateInput {
                    provider: ProviderDriverKind::new(candidate.driver.as_str()),
                    target_version: None,
                    instance_id: Some(ProviderInstanceId::new(candidate.instance_id.as_str())),
                };
                let call = ServerSettingsStore::call::<ServerUpdateProvider>(&environment_id, input, cx);
                async move {
                    let result = call.await.map(|payload| payload.providers).map_err(|error| error.0);
                    ProviderUpdateRun {
                        environment_id,
                        machine_label,
                        driver: candidate.driver.clone(),
                        instance_id: candidate.instance_id.clone(),
                        result,
                    }
                }
            })
            .collect();
        cx.spawn(async move |cx| {
            let runs = futures::future::join_all(calls).await;
            if !quiet && let Some(view) = provider_update_run_toast_view(&runs) {
                let kind = if view.success { ToastKind::Success } else { ToastKind::Error };
                cx.update(|cx| Toaster::push(ToastData::new(kind, view.title).description(view.description), cx));
            }
            runs
        })
    }

    /// Rediscovers models on every connected machine this session manages, which also drops
    /// each server's cached latest versions (`useCheckProviderUpdates`). A check while one runs
    /// joins it. Toasts the failures unless `quiet`.
    pub fn check(quiet: bool, cx: &mut App) -> Shared<Task<()>> {
        let entity = Self::entity(cx);
        if let Some(running) = entity.read(cx).checking.clone() {
            return running;
        }
        let calls: Vec<_> = Self::manageable(cx)
            .into_iter()
            .map(|(id, label)| {
                let input = ServerRefreshProvidersPayload { refresh_models: Some(true), ..Default::default() };
                let call = ServerSettingsStore::call::<ServerRefreshProviders>(&id, input, cx);
                async move { call.await.err().map(|error| SharedString::from(format!("{label}: {}", error.0))) }
            })
            .collect();
        let weak = entity.downgrade();
        let task = cx
            .spawn(async move |cx| {
                let failures: Vec<SharedString> = futures::future::join_all(calls).await.into_iter().flatten().collect();
                cx.update(|cx| {
                    weak.update(cx, |this, cx| {
                        this.checking = None;
                        cx.notify();
                    })
                    .ok();
                    if !quiet && !failures.is_empty() {
                        let lines = failures.iter().map(SharedString::as_ref).collect::<Vec<_>>().join("\n");
                        Toaster::push(ToastData::new(ToastKind::Error, t!("Could not check provider updates")).description(lines), cx);
                    }
                });
            })
            .shared();
        entity.update(cx, |this, cx| {
            this.checking = Some(task.clone());
            cx.notify();
        });
        task
    }

    /// Whether a check is running.
    pub fn is_checking(cx: &App) -> bool {
        cx.try_global::<GlobalProviderUpdates>().is_some_and(|g| g.0.read(cx).checking.is_some())
    }
}

/// "Update all" in the Providers header (`ProviderUpdatesAction`): every machine's one-click
/// provider updates, listed per machine in its tooltip. Renders nothing when there are none.
pub struct ProviderUpdatesAction {
    _updates: Subscription,
}

struct GlobalProviderUpdatesAction(Entity<ProviderUpdatesAction>);

impl Global for GlobalProviderUpdatesAction {}

impl ProviderUpdatesAction {
    /// The app's button view, created on first use; the Providers page shows it.
    pub fn view(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalProviderUpdatesAction>() {
            return global.0.clone();
        }
        let view = cx.new(|cx| {
            let updates = ProviderUpdates::entity(cx);
            Self { _updates: cx.observe(&updates, |_, _, cx| cx.notify()) }
        });
        cx.set_global(GlobalProviderUpdatesAction(view.clone()));
        view
    }
}

impl Render for ProviderUpdatesAction {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let machines = ProviderUpdates::machines(cx);
        let entity = ProviderUpdates::entity(cx);
        let pending = entity.read(cx).updating_all;
        if machines.is_empty() && !pending {
            return div().into_any_element();
        }
        let summary: SharedString = machines
            .iter()
            .map(|machine| format!("{}: {}", machine.label, machine.provider_names().join(", ")))
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        div()
            .id("provider-updates-action")
            .tooltip(tooltip(TooltipSurface::new(summary)))
            .child(
                Button::new("provider-update-all")
                    .variant(ButtonVariant::GhostMuted)
                    .size(ButtonSize::Xs)
                    .label(if pending { t!("Updating…") } else { t!("Update all") })
                    .disabled(pending)
                    .on_click(move |_, _, cx| {
                        if entity.read(cx).updating_all {
                            return;
                        }
                        entity.update(cx, |this, cx| {
                            this.updating_all = true;
                            cx.notify();
                        });
                        let run = ProviderUpdates::update_all(false, cx);
                        let entity = entity.clone();
                        cx.spawn(async move |cx| {
                            run.await;
                            cx.update(|cx| {
                                entity.update(cx, |this, cx| {
                                    this.updating_all = false;
                                    cx.notify();
                                })
                            });
                        })
                        .detach();
                    }),
            )
            .into_any_element()
    }
}
