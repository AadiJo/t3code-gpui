//! The running "update everything". Ports `updateEverythingRun.tsx` and the `run` of
//! `useUpdateEverything.ts` for a client that hosts no server: provider updates finish first
//! (servers stage meanwhile), then every server updates, each after its own stage. The run
//! lives in a global entity so it keeps going while the sidebar redraws, and so the capsule,
//! its retry and its dismiss all act on the same state.
//!
//! The web's saved result across an app relaunch and its countdown to install the app exist
//! only for the desktop app's own update, which this client does not run.

use std::collections::HashMap;

use futures::FutureExt as _;
use gpui_kit::{App, AppContext as _, Entity, Global, SharedString, Task};
use t3code_client_runtime::environment::EnvironmentId;
use t3code_contracts::ServerSelfUpdateInput;
use t3code_contracts::rpc::methods::ServerUpdateServer;

use super::update_everything_logic::{
    ProviderMachine, UpdateEverythingRun, UpdateRunMachine, UpdateRunPhase, UpdateRunResult, close_finished_update_run,
    collect_update_everything_result, update_everything_toast_view,
};
use crate::components::provider_update_launch_notification::{ProviderUpdateOutcome, provider_update_run_outcome};
use crate::components::provider_updates_action::ProviderUpdates;
use crate::components::server_update_action::{ServerUpdateStore, ServerUpdateTarget, failure_text};
use t3code_client_runtime::connection::server_update::ServerUpdateOutcome;
use crate::components::ui::toast::{ToastData, ToastKind, Toaster};
use crate::hooks::use_settings::ServerSettingsStore;
use crate::i18n::t;
use crate::version_skew::resolve_version_mismatch;

fn now() -> u64 {
    crate::timestamp_format::now_ms().max(0) as u64
}

/// The run, if one is going or settled with failures. Notifies on every change.
#[derive(Default)]
pub struct UpdateRunStore {
    run: Option<UpdateEverythingRun>,
}

struct GlobalUpdateRun(Entity<UpdateRunStore>);

impl Global for GlobalUpdateRun {}

impl UpdateRunStore {
    /// The app's entity, created on first use.
    pub fn entity(cx: &mut App) -> Entity<Self> {
        if let Some(global) = cx.try_global::<GlobalUpdateRun>() {
            return global.0.clone();
        }
        let entity = cx.new(|_| Self::default());
        cx.set_global(GlobalUpdateRun(entity.clone()));
        entity
    }

    /// The run now (`useUpdateEverythingRun`).
    pub fn run(cx: &App) -> Option<UpdateEverythingRun> {
        cx.try_global::<GlobalUpdateRun>().and_then(|g| g.0.read(cx).run.clone())
    }

    /// `setUpdateEverythingRun`.
    fn set(cx: &mut App, update: impl FnOnce(Option<UpdateEverythingRun>) -> Option<UpdateEverythingRun>) {
        Self::entity(cx).update(cx, |this, cx| {
            this.run = update(this.run.take());
            cx.notify();
        });
    }

    /// `updateRunMachines`: changes each machine, then ends a run that finished.
    fn update_machines(cx: &mut App, update: impl Fn(&mut UpdateRunMachine)) {
        Self::set(cx, |run| {
            let mut run = run?;
            run.machines.iter_mut().for_each(update);
            close_finished_update_run(run)
        });
    }

    /// `settleUpdateRunMachine`: the machine is done, or failed with `failure`. A machine whose
    /// server updated owes no server update any more; `provider_failure` replaces the recorded
    /// one when given.
    pub fn settle_machine(
        environment_id: &EnvironmentId,
        failure: Option<SharedString>,
        server_updated: bool,
        provider_failure: Option<Option<SharedString>>,
        cx: &mut App,
    ) {
        Self::update_machines(cx, |machine| {
            if &machine.environment_id != environment_id {
                return;
            }
            machine.result = Some(if failure.is_none() { UpdateRunResult::Done } else { UpdateRunResult::Failed });
            machine.failure = failure.clone();
            if server_updated {
                machine.server = None;
            }
            if let Some(provider_failure) = &provider_failure {
                machine.provider_failure = provider_failure.clone();
            }
        });
    }

    /// `dismissSettledUpdateRunMachines`: drops every settled row.
    pub fn dismiss_settled(cx: &mut App) {
        Self::set(cx, |run| {
            let mut run = run?;
            run.machines.retain(|m| m.result.is_none());
            close_finished_update_run(run)
        });
    }

    /// `retryFailedUpdateRunServers`: updates each failed server again. A server that came back
    /// on its target by itself is done; the host would reject a same-version update.
    pub fn retry_failed(cx: &mut App) {
        let failed: Vec<(ServerUpdateTarget, Option<SharedString>)> = Self::run(cx)
            .map(|run| {
                run.machines
                    .into_iter()
                    .filter(|m| m.result == Some(UpdateRunResult::Failed))
                    .filter_map(|m| m.server.map(|server| (server, m.provider_failure)))
                    .collect()
            })
            .unwrap_or_default();
        let mut pending = Vec::new();
        for (target, provider_failure) in failed {
            let version = ServerSettingsStore::config(cx, &target.environment_id).map(|c| c.environment.server_version.clone());
            if version.is_some() && resolve_version_mismatch(version.as_deref(), &target.target_version).is_none() {
                Self::settle_machine(&target.environment_id, provider_failure, true, None, cx);
            } else {
                pending.push((target, provider_failure));
            }
        }
        let started_at = now();
        Self::update_machines(cx, |machine| {
            if pending.iter().any(|(t, _)| t.environment_id == machine.environment_id) {
                machine.result = None;
                machine.failure = None;
                machine.started_at = started_at;
            }
        });
        for (target, provider_failure) in pending {
            let id = target.environment_id.clone();
            let update = ServerUpdateStore::update(target, true, cx);
            cx.spawn(async move |cx| {
                let outcome = update.await;
                cx.update(|cx| {
                    let (failure, updated) = match &outcome {
                        ServerUpdateOutcome::Updated(_) => (provider_failure, true),
                        ServerUpdateOutcome::Failed(failure) => (Some(failure_text(failure)), false),
                        ServerUpdateOutcome::Skipped => (Some(t!("Could not start the update")), false),
                    };
                    Self::settle_machine(&id, failure, updated, None, cx);
                });
            })
            .detach();
        }
    }

    /// Starts the run over `servers` and `provider_machines` (`useUpdateEverything().run`),
    /// and toasts how it ended. Fails nothing: each machine's outcome lands in its row.
    pub fn start(servers: Vec<ServerUpdateTarget>, provider_machines: Vec<ProviderMachine>, cx: &mut App) -> Task<()> {
        let started_at = now();
        let mut machines: Vec<UpdateRunMachine> = provider_machines
            .iter()
            .map(|m| UpdateRunMachine {
                environment_id: m.environment_id.clone(),
                label: m.label.clone(),
                updates_providers: true,
                server: None,
                started_at,
                result: None,
                failure: None,
                provider_failure: None,
            })
            .collect();
        // A machine keeps the place its provider row took, as the web's `Map.set` does.
        for server in &servers {
            let machine = UpdateRunMachine {
                environment_id: server.environment_id.clone(),
                label: server.server_label.clone(),
                updates_providers: false,
                server: Some(server.clone()),
                started_at,
                result: None,
                failure: None,
                provider_failure: None,
            };
            match machines.iter_mut().find(|m| m.environment_id == server.environment_id) {
                Some(existing) => *existing = UpdateRunMachine { updates_providers: existing.updates_providers, ..machine },
                None => machines.push(machine),
            }
        }
        Self::set(cx, |_| Some(UpdateEverythingRun { phase: UpdateRunPhase::Providers, machines }));

        // Servers that can stage download their release while providers update.
        let mut staged = HashMap::new();
        if !provider_machines.is_empty() {
            for server in &servers {
                let config = ServerSettingsStore::config(cx, &server.environment_id);
                if config.is_some_and(|c| c.environment.capabilities.server_update_staging == Some(true)) {
                    let input = ServerSelfUpdateInput {
                        target_version: server.target_version.to_string(),
                        channel: None,
                        target_database_migration: None,
                        stage_only: Some(true),
                    };
                    let stage = ServerSettingsStore::call::<ServerUpdateServer>(&server.environment_id, input, cx);
                    staged.insert(server.environment_id.clone(), stage.map(|_| ()));
                }
            }
        }
        let providers = if provider_machines.is_empty() { Task::ready(Vec::new()) } else { ProviderUpdates::update_all(true, cx) };
        cx.spawn(async move |cx| {
            let runs = providers.await;
            let outcomes: Vec<ProviderUpdateOutcome> = runs.iter().map(provider_update_run_outcome).collect();
            let provider_failures: HashMap<EnvironmentId, SharedString> = runs
                .iter()
                .zip(&outcomes)
                .filter_map(|(run, outcome)| match outcome {
                    ProviderUpdateOutcome::Failed { line } => Some((run.environment_id.clone(), line.clone())),
                    ProviderUpdateOutcome::Updated { .. } => None,
                })
                .collect();
            let server_tasks = cx.update(|cx| {
                let provider_only: Vec<EnvironmentId> = Self::run(cx)
                    .map(|run| run.machines.into_iter().filter(|m| m.server.is_none()).map(|m| m.environment_id).collect())
                    .unwrap_or_default();
                for id in provider_only {
                    Self::settle_machine(&id, provider_failures.get(&id).cloned(), false, None, cx);
                }
                let servers_started_at = now();
                Self::set(cx, |run| {
                    let mut run = run?;
                    run.phase = UpdateRunPhase::Servers;
                    for machine in run.machines.iter_mut().filter(|m| m.server.is_some()) {
                        machine.started_at = servers_started_at;
                    }
                    Some(run)
                });
                servers
                    .into_iter()
                    .map(|server| {
                        let stage = staged.remove(&server.environment_id);
                        let provider_failure = provider_failures.get(&server.environment_id).cloned();
                        cx.spawn(async move |cx| {
                            if let Some(stage) = stage {
                                stage.await;
                            }
                            let label = server.server_label.clone();
                            let id = server.environment_id.clone();
                            let outcome = cx.update(|cx| ServerUpdateStore::update(server, true, cx)).await;
                            cx.update(|cx| {
                                let failure = match &outcome {
                                    ServerUpdateOutcome::Failed(failure) => Some(failure_text(failure)),
                                    _ => provider_failure.clone(),
                                };
                                let updated = !matches!(outcome, ServerUpdateOutcome::Failed(_));
                                Self::settle_machine(&id, failure, updated, Some(provider_failure), cx);
                            });
                            (label, outcome)
                        })
                    })
                    .collect::<Vec<_>>()
            });
            let finished = futures::future::join_all(server_tasks).await;
            cx.update(|cx| {
                Self::set(cx, |run| run.and_then(close_finished_update_run));
                let result = collect_update_everything_result(&finished, &outcomes);
                if let Some((success, title, description)) = update_everything_toast_view(&result) {
                    let toast = ToastData::new(if success { ToastKind::Success } else { ToastKind::Error }, title);
                    Toaster::push(if let Some(d) = description { toast.description(d) } else { toast }, cx);
                }
            });
        })
    }
}
