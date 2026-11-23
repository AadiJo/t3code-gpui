//! One environment's RPC session: the socket plus the `subscribeServerConfig`
//! stream that proves it ready (spec 01, section 5.1).

use std::borrow::Cow;
use std::sync::Arc;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use t3code_contracts::rpc::{RpcMethod, RpcStream};
use t3code_contracts::{AuthEnvironmentScope, rpc::methods};
use tokio::sync::{broadcast, watch};

use super::client::{CloseReason, FrameTap, RpcClient, RpcError};
use crate::connection::model::{ConnectionError, ConnectionHop, ConnectionVia, PreparedConnection, TransientReason};
use crate::connection::routes::ReportedEndpoint;
use crate::environment::EnvironmentId;

/// Server config opt-ins. Only opt in to events the state layer decodes.
/// `workspaceSnapshotsOnDemand` is always sent.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RpcSessionOptions {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub environment_themes: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub usage_limit_sources: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub usage_limits_command: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub provider_catalogs_on_demand: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub provider_catalog_digests: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub provider_status_deltas: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerConfigInput<'a> {
    #[serde(flatten)]
    options: &'a RpcSessionOptions,
    workspace_snapshots_on_demand: bool,
}

/// `subscribeServerConfig` with undecoded events, so the session can hand
/// them to the state layer, which decodes its own types.
struct SubscribeServerConfigRaw;

impl RpcMethod for SubscribeServerConfigRaw {
    const TAG: &'static str = "subscribeServerConfig";
    const SCOPE: AuthEnvironmentScope = <methods::SubscribeServerConfig as RpcMethod>::SCOPE;
    const CLIENT_GUARDED: bool = <methods::SubscribeServerConfig as RpcMethod>::CLIENT_GUARDED;
    type Payload = Box<RawValue>;
    type Success = Box<RawValue>;
    type Error = Box<RawValue>;
}

impl RpcStream for SubscribeServerConfigRaw {}

/// The bits of a config event the connection layer reads itself.
#[derive(Deserialize)]
struct ConfigEventHead<'a> {
    #[serde(rename = "type", borrow)]
    kind: Cow<'a, str>,
    config: Option<ConfigHead>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigHead {
    environment: EnvironmentHead,
    #[serde(default)]
    direct_endpoints: Vec<ReportedEndpoint>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnvironmentHead {
    environment_id: EnvironmentId,
}

/// A cold server with a large database can take a while to send its first snapshot.
pub const SYNCHRONIZATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);

/// A server config event as JSON, for the state layer to decode.
pub type RawConfigEvent = Arc<RawValue>;

/// A live, synchronized session. Dropping every clone closes the socket.
pub struct RpcSession {
    pub client: RpcClient,
    pub prepared: PreparedConnection,
    /// The first `snapshot` event, which made the session ready.
    pub initial_config: RawConfigEvent,
    /// Addresses the server reported for learned routes.
    pub direct_endpoints: Arc<[ReportedEndpoint]>,
    /// Every config event after the initial snapshot, including later snapshots.
    config_events: broadcast::Sender<RawConfigEvent>,
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
        let input = serde_json::value::to_raw_value(&ServerConfigInput { options, workspace_snapshots_on_demand: true })
            .expect("config input serializes");
        let mut stream = client.subscribe::<SubscribeServerConfigRaw>(input);
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
            let snapshot = chunk.into_iter().find_map(|event| {
                let head: ConfigEventHead<'_> = serde_json::from_str(event.get()).ok()?;
                let config = head.config.filter(|_| head.kind == "snapshot")?;
                Some((Arc::<RawValue>::from(event), config))
            });
            if let Some(snapshot) = snapshot {
                return Ok(snapshot);
            }
        } };
        let (initial, head) = tokio::time::timeout(SYNCHRONIZATION_TIMEOUT, synchronize)
            .await
            .map_err(|_| {
                ConnectionError::transient(
                    TransientReason::Timeout,
                    format!("{} did not send its configuration in time.", prepared.label),
                )
            })??;
        if head.environment.environment_id != prepared.environment_id {
            return Err(ConnectionError::environment_mismatch(&prepared.environment_id, &head.environment.environment_id));
        }
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
                                _ = config_events.send(Arc::from(event));
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
            initial_config: initial,
            direct_endpoints: head.direct_endpoints.into(),
            config_events,
            ended,
            reader: reader.abort_handle(),
            prepared,
        }))
    }

    /// Config events after the initial snapshot. A receiver that lags
    /// should reload from the next `snapshot` event.
    pub fn config_events(&self) -> broadcast::Receiver<RawConfigEvent> {
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

fn config_stream_error(label: &str, via: ConnectionVia, error: RpcError<Box<RawValue>>) -> ConnectionError {
    match error {
        RpcError::Closed(reason) => socket_error(label, via, &reason, true),
        RpcError::Failure(error) | RpcError::UnknownFailure(error) => {
            #[derive(Deserialize)]
            struct Tagged {
                #[serde(rename = "_tag")]
                tag: String,
                #[serde(default)]
                message: String,
            }
            let tagged: Option<Tagged> = serde_json::from_str(error.get()).ok();
            match tagged {
                Some(Tagged { tag, message }) if tag == "EnvironmentAuthorizationError" => ConnectionError::blocked(
                    crate::connection::model::BlockedReason::Permission,
                    message,
                )
                .at(ConnectionHop::Server)
                .via(via),
                Some(Tagged { message, .. }) => {
                    ConnectionError::transient(TransientReason::RemoteUnavailable, message).at(ConnectionHop::Server).via(via)
                }
                None => ConnectionError::transient(
                    TransientReason::RemoteUnavailable,
                    format!("{label} config subscription failed."),
                ),
            }
        }
        other => ConnectionError::transient(
            TransientReason::RemoteUnavailable,
            format!("{label} config subscription failed: {other}"),
        )
        .at(ConnectionHop::Server)
        .via(via),
    }
}
