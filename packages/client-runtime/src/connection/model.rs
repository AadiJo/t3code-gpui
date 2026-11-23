//! Connection attempt errors, prepared connections, and supervisor status.

use std::sync::Arc;
use std::time::SystemTime;

use url::Url;

use crate::authorization::HttpAuthorization;
use crate::environment::{EnvironmentDescriptor, EnvironmentId};

/// The access token is renewed this long before it expires.
pub const DPOP_ACCESS_TOKEN_REFRESH_SKEW_MS: i64 = 60_000;

/// A failure that retrying may fix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientReason {
    Network,
    Timeout,
    Transport,
    EndpointUnavailable,
    RelayUnavailable,
    RemoteUnavailable,
    /// The T3 Connect sign-in has not loaded yet.
    SessionPending,
    /// The host is asking its owner to approve this device.
    ApprovalPending,
}

/// A failure that needs the user: retrying alone will not fix it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedReason {
    Authentication,
    Configuration,
    Permission,
    /// The server speaks another orchestration protocol. Same on every route.
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionErrorKind {
    Transient(TransientReason),
    Blocked(BlockedReason),
}

/// The part of the path to an environment that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionHop {
    Device,
    Relay,
    Tunnel,
    Server,
}

/// Which kind of route the failure happened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionVia {
    Relay,
    Direct,
}

/// Why a hop failed, when the code that saw the failure knows. Decodes the
/// relay's `endpointFailure` literals; an unknown literal is `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    TunnelDown,
    OriginUnreachable,
    Dns,
    Tls,
    Refused,
    Timeout,
    InvalidResponse,
    ClockSkew,
    NotLinked,
    RelayError,
    TunnelPending,
    TunnelNotEnabled,
    ApprovalPending,
    #[serde(other)]
    Unknown,
}

/// Why one connection attempt failed, with where it failed when known.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{detail}")]
pub struct ConnectionError {
    pub kind: ConnectionErrorKind,
    /// English text for the user.
    pub detail: String,
    pub hop: Option<ConnectionHop>,
    pub via: Option<ConnectionVia>,
    pub failure_cause: Option<FailureCause>,
    pub http_status: Option<u16>,
    pub trace_id: Option<String>,
    /// The `XXXX-XXXX` code shown while the host approves this device.
    pub verification_code: Option<String>,
    /// The host can update itself to a compatible version.
    pub server_update_required: bool,
}

impl ConnectionError {
    pub fn transient(reason: TransientReason, detail: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::Transient(reason), detail)
    }

    pub fn blocked(reason: BlockedReason, detail: impl Into<String>) -> Self {
        Self::new(ConnectionErrorKind::Blocked(reason), detail)
    }

    fn new(kind: ConnectionErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            hop: None,
            via: None,
            failure_cause: None,
            http_status: None,
            trace_id: None,
            verification_code: None,
            server_update_required: false,
        }
    }

    pub fn at(mut self, hop: ConnectionHop) -> Self {
        self.hop = Some(hop);
        self
    }

    pub fn via(mut self, via: ConnectionVia) -> Self {
        self.via = Some(via);
        self
    }

    pub fn cause(mut self, cause: FailureCause) -> Self {
        self.failure_cause = Some(cause);
        self
    }

    pub fn trace(mut self, trace_id: Option<&str>) -> Self {
        self.trace_id = trace_id.map(str::to_owned);
        self
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self.kind, ConnectionErrorKind::Blocked(_))
    }

    pub fn is_unsupported(&self) -> bool {
        self.kind == ConnectionErrorKind::Blocked(BlockedReason::Unsupported)
    }

    pub fn is_approval_pending(&self) -> bool {
        self.kind == ConnectionErrorKind::Transient(TransientReason::ApprovalPending)
    }

    /// The host answered and refused this device's credential; pairing
    /// again fixes it. A clock-skew refusal is not one.
    pub fn is_host_credential_refusal(&self) -> bool {
        matches!(
            self.kind,
            ConnectionErrorKind::Blocked(BlockedReason::Authentication | BlockedReason::Permission)
        ) && self.hop == Some(ConnectionHop::Server)
            && self.failure_cause != Some(FailureCause::ClockSkew)
    }

    pub fn environment_mismatch(expected: &EnvironmentId, actual: &EnvironmentId) -> Self {
        Self::blocked(
            BlockedReason::Configuration,
            format!("Connected environment {actual} does not match {expected}."),
        )
    }
}

/// Everything needed to open one environment's socket over one route.
#[derive(Clone, Debug)]
pub struct PreparedConnection {
    pub environment_id: EnvironmentId,
    pub label: String,
    /// The route this connection uses, by [`crate::connection::routes::route_id`].
    pub route_id: String,
    pub via: ConnectionVia,
    pub http_base_url: Url,
    /// Carries a fresh single-use ticket; open it promptly.
    pub socket_url: Url,
    /// Authorizes HTTP requests to this environment while the session lives.
    pub http_authorization: HttpAuthorization,
    pub descriptor: Arc<EnvironmentDescriptor>,
}

/// What the platform reports about the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NetworkStatus {
    #[default]
    Unknown,
    Offline,
    Online,
}

/// Where a supervisor is in its connect loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionPhase {
    /// Not wanted: disabled, or not yet asked to connect.
    Available,
    Offline,
    Connecting,
    Backoff,
    Connected,
    /// Waiting for the user, see `last_failure`.
    Blocked,
}

/// Progress inside a `Connecting` attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptStage {
    Preparing,
    Opening,
    Synchronizing,
}

/// One environment's connection state, published through a `watch` channel.
#[derive(Clone, Debug)]
pub struct ConnectionStatus {
    pub desired: bool,
    pub network: NetworkStatus,
    pub phase: ConnectionPhase,
    pub stage: Option<AttemptStage>,
    /// 1-based attempt number since the last success that stayed up 30 s.
    pub attempt: u32,
    /// Increments each time a session is established.
    pub generation: u64,
    pub last_failure: Option<ConnectionError>,
    pub retry_at: Option<SystemTime>,
    /// The connected route, while connected.
    pub active_route_id: Option<String>,
    /// While connected: saved routes whose host refused this device's credential.
    pub refused_route_ids: Arc<[String]>,
}

impl ConnectionStatus {
    /// The initial state: nothing wanted yet.
    pub fn available() -> Self {
        Self {
            desired: false,
            network: NetworkStatus::Unknown,
            phase: ConnectionPhase::Available,
            stage: None,
            attempt: 0,
            generation: 0,
            last_failure: None,
            retry_at: None,
            active_route_id: None,
            refused_route_ids: Arc::new([]),
        }
    }
}
