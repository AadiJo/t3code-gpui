//! Updates a host this client cannot open a session with because it speaks an older
//! orchestration protocol. Ports `connection/outdatedHostUpdate.ts` and the command in
//! `state/outdatedServerUpdate.ts`.
//!
//! The normal session refuses such a host, so this opens a bare socket and calls only the
//! self-update RPCs, whose wire shape has not changed across protocol versions. Then it reads
//! the host's public descriptor every second until it answers on a compatible protocol, and
//! asks the supervisor to connect again.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use t3code_contracts::rpc::methods::{ServerUpdateServer, ServerUpdateServerWithProgress};
use t3code_contracts::{
    ServerSelfUpdateInput, ServerSelfUpdateMethod, ServerSelfUpdateProgressEvent,
    ServerSelfUpdateResult,
};
use url::Url;

use super::catalog::CatalogEntry;
use super::compatibility::ORCHESTRATION_PROTOCOL_VERSION;
use super::resolver::prepare_for_update;
use super::routes::usable_routes;
use super::server_update::{
    ServerUpdateFailure, ServerUpdateOutcome, ServerUpdateStage, ServerUpdateState, ServerUpdates, is_handoff_loss,
    rpc_failure,
};
use super::supervisor::{SupervisorHandle, SupervisorSignal};
use super::model::PreparedConnection;
use crate::authorization::RemoteAuthorization;
use crate::environment::{EnvironmentDescriptor, fetch_descriptor};
use crate::rpc::client::RpcClient;

/// A v1 host restarting into v2 runs migrations before its descriptor answers again.
const RESTART_TIMEOUT: Duration = Duration::from_secs(4 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

fn compatible(descriptor: &EnvironmentDescriptor) -> bool {
    descriptor.orchestration_protocol_version.unwrap_or(1) == ORCHESTRATION_PROTOCOL_VERSION
}

/// Authorizes the first usable route that answers, without the protocol gate.
async fn reach(auth: &RemoteAuthorization, entry: &CatalogEntry) -> Option<PreparedConnection> {
    for route in usable_routes(entry) {
        if let Ok(prepared) = prepare_for_update(auth, &entry.environment_id, &entry.label, route).await {
            return Some(prepared);
        }
    }
    None
}

/// Updates the outdated host of `entry` to `input.target_version` (`updateOutdatedHost`), with
/// `from_version` from its descriptor when known, and records the state in `updates`.
pub async fn update_outdated_host(
    updates: &Arc<ServerUpdates>,
    auth: &RemoteAuthorization,
    entry: &CatalogEntry,
    supervisor: Option<SupervisorHandle>,
    input: ServerSelfUpdateInput,
    from_version: Option<String>,
) -> ServerUpdateOutcome {
    let Some(claim) = updates.claim(&entry.environment_id) else { return ServerUpdateOutcome::Skipped };
    let target_version = input.target_version.clone();
    let from_version = from_version.unwrap_or_else(|| target_version.clone());
    let mut stage = ServerUpdateStage::Downloading;
    let running = |stage| ServerUpdateState::Running {
        stage,
        from_version: from_version.clone(),
        target_version: target_version.clone(),
    };
    claim.set(running(stage));
    let result = async {
        let prepared = reach(auth, entry).await.ok_or_else(|| ServerUpdateFailure::Reason(format!("Could not reach {}.", entry.label)))?;
        let descriptor = prepared.descriptor.clone();
        // A host updated by hand is already compatible, and a bare socket would be refused with
        // 426, so there is nothing to update.
        if compatible(&descriptor) {
            return Ok(ServerSelfUpdateResult {
                target_version: descriptor.server_version.clone(),
                method: ServerSelfUpdateMethod::Respawn,
                update_id: None,
                desktop_update_token: None,
            });
        }
        let capabilities = &descriptor.capabilities;
        let method = match capabilities.server_self_update.as_deref() {
            Some("boot-service") => ServerSelfUpdateMethod::BootService,
            Some("respawn") => ServerSelfUpdateMethod::Respawn,
            Some("desktop-managed") if capabilities.desktop_app_update => ServerSelfUpdateMethod::DesktopApp,
            _ => return Err(ServerUpdateFailure::CannotSelfUpdate { host: descriptor.label.clone() }),
        };
        let client = RpcClient::connect(prepared.socket_url.as_str(), None)
            .await
            .map_err(|error| ServerUpdateFailure::Reason(error.to_string()))?;
        let in_place = method != ServerSelfUpdateMethod::DesktopApp;
        let result = if descriptor_reports_progress(&descriptor) {
            let mut stream = client.subscribe::<ServerUpdateServerWithProgress>(input.clone());
            let (mut terminal, mut received_progress, mut ended) = (None, false, None);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(events) => {
                        for event in events {
                            match event {
                                ServerSelfUpdateProgressEvent::Progress(progress) => {
                                    received_progress = true;
                                    stage = match progress.stage {
                                        t3code_contracts::ServerSelfUpdateProgressStage::Installing => ServerUpdateStage::Installing,
                                        _ => ServerUpdateStage::Downloading,
                                    };
                                    claim.set(running(stage));
                                }
                                ServerSelfUpdateProgressEvent::Complete(complete) => terminal = Some(complete.result),
                                ServerSelfUpdateProgressEvent::Unknown(_) => {}
                            }
                        }
                    }
                    Err(error) => {
                        ended = Some(error);
                        break;
                    }
                }
            }
            match (terminal, ended) {
                (Some(result), None) => result,
                (Some(result), Some(error)) if is_handoff_loss(&error) => result,
                (None, Some(error)) if is_handoff_loss(&error) && received_progress && in_place => ServerSelfUpdateResult {
                    target_version: target_version.clone(),
                    method: method.clone(),
                    update_id: None,
                    desktop_update_token: None,
                },
                (_, Some(error)) => return Err(rpc_failure(&error)),
                (None, None) => return Err(ServerUpdateFailure::ProgressIncomplete { version: target_version.clone() }),
            }
        } else {
            match client.call::<ServerUpdateServer>(input.clone()).await {
                Ok(result) => result,
                Err(error) if is_handoff_loss(&error) && in_place => ServerSelfUpdateResult {
                    target_version: target_version.clone(),
                    method: method.clone(),
                    update_id: None,
                    desktop_update_token: None,
                },
                Err(error) => return Err(rpc_failure(&error)),
            }
        };
        client.close();
        stage = ServerUpdateStage::Resuming;
        claim.set(running(stage));
        let mut urls: Vec<Url> = vec![prepared.http_base_url.clone()];
        for url in entry.routes.iter().filter_map(|route| route.bearer_profile().map(|profile| profile.http_base_url.clone())) {
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
        let resumed = tokio::time::timeout(RESTART_TIMEOUT, async {
            loop {
                for url in &urls {
                    if let Ok(descriptor) = fetch_descriptor(url, POLL_INTERVAL * 3).await
                        && descriptor.environment_id == entry.environment_id
                        && compatible(&descriptor)
                    {
                        return descriptor;
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
        .map_err(|_| ServerUpdateFailure::DidNotComeBack { host: descriptor.label.clone() })?;
        Ok(ServerSelfUpdateResult { target_version: resumed.server_version, ..result })
    }
    .await;
    match result {
        Ok(result) => {
            if let Some(supervisor) = supervisor {
                supervisor.signal(SupervisorSignal::Retry);
            }
            claim.settle(ServerUpdateState::Idle);
            ServerUpdateOutcome::Updated(result)
        }
        Err(failure) => {
            claim.settle(ServerUpdateState::Failed {
                stage,
                from_version: from_version.clone(),
                target_version: target_version.clone(),
                failure: failure.clone(),
            });
            ServerUpdateOutcome::Failed(failure)
        }
    }
}

/// The descriptor's `serverSelfUpdateProgress`, which the connection layer's descriptor type
/// does not keep.
fn descriptor_reports_progress(descriptor: &EnvironmentDescriptor) -> bool {
    descriptor.capabilities.server_self_update_progress
}
