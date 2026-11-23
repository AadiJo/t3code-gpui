//! One environment's RPC session: the socket plus the `subscribeServerConfig`
//! stream that proves it ready (spec 01, section 5.1).

use std::sync::Arc;

use futures::StreamExt;
use serde::Serialize;
use t3code_contracts::rpc::methods::SubscribeServerConfig;
use t3code_contracts::{ServerConfig, ServerConfigStreamEvent, SubscribeServerConfigError, SubscribeServerConfigPayload};
use tokio::sync::{broadcast, watch};

use super::client::{CloseReason, FrameTap, RpcClient, RpcError};
use crate::connection::model::{ConnectionError, ConnectionHop, ConnectionVia, PreparedConnection, TransientReason};
use crate::connection::routes::ReportedEndpoint;

/// Server config opt-ins. Only opt in to events the state layer decodes.
/// `workspaceSnapshotsOnDemand` is always sent.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcSessionOptions {
    pub environment_themes: bool,
    pub usage_limit_sources: bool,
    pub usage_limits_command: bool,
    pub provider_catalogs_on_demand: bool,
    pub provider_catalog_digests: bool,
    pub provider_status_deltas: bool,
}

impl RpcSessionOptions {
    fn payload(&self) -> SubscribeServerConfigPayload {
        let flag = |on: bool| on.then_some(true);
        SubscribeServerConfigPayload {
            environment_themes: flag(self.environment_themes),
            usage_limit_sources: flag(self.usage_limit_sources),
            usage_limits_command: flag(self.usage_limits_command),
            workspace_snapshots_on_demand: Some(true),
            provider_catalogs_on_demand: flag(self.provider_catalogs_on_demand),
            provider_catalog_digests: flag(self.provider_catalog_digests),
            provider_status_deltas: flag(self.provider_status_deltas),
        }
    }
}

/// A cold server with a large database can take a while to send its first snapshot.
pub const SYNCHRONIZATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);

/// A server config event after the initial snapshot.
pub type ConfigEvent = Arc<ServerConfigStreamEvent>;

/// A live, synchronized session. Dropping every clone closes the socket.
pub struct RpcSession {
    pub client: RpcClient,
    pub prepared: PreparedConnection,
    /// The config of the first `snapshot` event, which made the session ready.
    pub initial_config: Arc<ServerConfig>,
    /// Addresses the server reported for learned routes.
    pub direct_endpoints: Arc<[ReportedEndpoint]>,
    /// Every config event after the initial snapshot, including later snapshots.
    config_events: broadcast::Sender<ConfigEvent>,
    /// Set when the socket closes or the config stream ends.
    ended: watch::Receiver<Option<ConnectionError>>,
    /// The config reader task; it holds the socket, so it stops with the session.
    reader: tokio::task::AbortHandle,
}

impl Drop for RpcSession {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl RpcSession {
    /// Opens the socket and waits for the first config snapshot of the
    /// expected environment, at most [`SYNCHRONIZATION_TIMEOUT`]. `on_open`
    /// runs once the socket is open and synchronization starts.
    pub async fn connect(
        prepared: PreparedConnection,
        options: &RpcSessionOptions,
        tap: Option<FrameTap>,
        on_open: impl FnOnce(),
    ) -> Result<Arc<Self>, ConnectionError> {
        let via = prepared.via;
        let client = RpcClient::connect(prepared.socket_url.as_str(), tap)
            .await
            .map_err(|reason| socket_error(&prepared.label, via, &reason, false))?;
        on_open();
        let mut stream = client.subscribe::<SubscribeServerConfig>(options.payload());
        let synchronize = async { loop {
            let chunk = match stream.next().await {
                Some(Ok(chunk)) => chunk,
                Some(Err(error)) => return Err(config_stream_error(&prepared.label, via, error)),
                None => {
                    return Err(ConnectionError::transient(
                        TransientReason::RemoteUnavailable,
                        format!("{} config subscription ended.", prepared.label),
                    ));
                }
            };
            let snapshot = chunk.into_iter().find_map(|event| match event {
                ServerConfigStreamEvent::Snapshot(snapshot) => Some(snapshot.config),
                _ => None,
            });
            if let Some(config) = snapshot {
                return Ok(config);
            }
        } };
        let config = tokio::time::timeout(SYNCHRONIZATION_TIMEOUT, synchronize)
            .await
            .map_err(|_| {
                ConnectionError::transient(
                    TransientReason::Timeout,
                    format!("{} did not send its configuration in time.", prepared.label),
                )
            })??;
        if config.environment.environment_id != prepared.environment_id {
            return Err(ConnectionError::environment_mismatch(&prepared.environment_id, &config.environment.environment_id));
        }
        let direct_endpoints = config
            .direct_endpoints
            .iter()
            .flatten()
            .map(|endpoint| ReportedEndpoint {
                http_base_url: endpoint.http_base_url.clone(),
                kind: Some(endpoint.kind.as_str().to_owned()),
            })
            .collect();
        let (config_events, _) = broadcast::channel(64);
        let (ended_tx, ended) = watch::channel(None);
        let label = prepared.label.clone();
        let reader_client = client.clone();
        let reader_events = config_events.clone();
        let reader = tokio::spawn(async move {
            let client = reader_client;
            let config_events = reader_events;
            let ended = loop {
                tokio::select! {
                    item = stream.next() => match item {
                        Some(Ok(chunk)) => {
                            for event in chunk {
                                _ = config_events.send(Arc::new(event));
                            }
                        }
                        Some(Err(error)) => break config_stream_error(&label, via, error),
                        None => break ConnectionError::transient(
                            TransientReason::RemoteUnavailable,
                            format!("{label} config subscription ended."),
                        ),
                    },
                    // The socket closing ends the stream too; this just reports it with its reason.
                    reason = client.closed() => break socket_error(&label, via, &reason, true),
                }
            };
            _ = ended_tx.send(Some(ended));
        });
        Ok(Arc::new(Self {
            client,
            initial_config: Arc::new(config),
            direct_endpoints,
            config_events,
            ended,
            reader: reader.abort_handle(),
            prepared,
        }))
    }

    /// Config events after the initial snapshot. A receiver that lags
    /// should reload from the next `snapshot` event.
    pub fn config_events(&self) -> broadcast::Receiver<ConfigEvent> {
        self.config_events.subscribe()
    }

    /// Resolves when the session ends, with why.
    pub async fn ended(&self) -> ConnectionError {
        let mut ended = self.ended.clone();
        loop {
            if let Some(error) = ended.borrow_and_update().clone() {
                return error;
            }
            if ended.changed().await.is_err() {
                return ConnectionError::transient(TransientReason::Transport, "The session ended.");
            }
        }
    }
}

fn hop(via: ConnectionVia) -> ConnectionHop {
    match via {
        ConnectionVia::Relay => ConnectionHop::Tunnel,
        ConnectionVia::Direct => ConnectionHop::Server,
    }
}

fn socket_error(label: &str, via: ConnectionVia, reason: &CloseReason, opened: bool) -> ConnectionError {
    let detail = match reason {
        CloseReason::PingTimeout => format!(
            "{label} stopped responding for {}s.",
            super::protocol::PING_TIMEOUT.as_secs()
        ),
        CloseReason::UpgradeRejected { status: 426, .. } => {
            return ConnectionError::blocked(
                crate::connection::model::BlockedReason::Unsupported,
                format!("{label} speaks another orchestration protocol."),
            )
            .at(ConnectionHop::Server)
            .via(via);
        }
        _ if opened => format!("{label} disconnected: {reason}"),
        _ => format!("{label} could not establish a WebSocket connection: {reason}"),
    };
    ConnectionError::transient(TransientReason::Transport, detail).at(hop(via)).via(via)
}

fn config_stream_error(label: &str, via: ConnectionVia, error: RpcError<SubscribeServerConfigError>) -> ConnectionError {
    match error {
        RpcError::Closed(reason) => socket_error(label, via, &reason, true),
        RpcError::Failure(SubscribeServerConfigError::EnvironmentAuthorization(error)) => ConnectionError::blocked(
            crate::connection::model::BlockedReason::Permission,
            error.message,
        )
        .at(ConnectionHop::Server)
        .via(via),
        RpcError::Failure(error) => ConnectionError::transient(TransientReason::RemoteUnavailable, format!("{error:?}"))
            .at(ConnectionHop::Server)
            .via(via),
        other => ConnectionError::transient(
            TransientReason::RemoteUnavailable,
            format!("{label} config subscription failed: {other}"),
        )
        .at(ConnectionHop::Server)
        .via(via),
    }
}
