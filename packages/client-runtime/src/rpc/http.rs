//! HTTP plumbing shared by environment and relay requests: one TLS config,
//! one connection pool, typed error bodies.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Default timeout for auth and descriptor requests.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One rustls config with the ring provider and webpki roots, shared by the
/// HTTP client and the WebSocket connector so neither picks its own provider.
pub fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// The process-wide HTTP client. It sends no `Origin` and keeps no cookies,
/// as a native client should (spec 01, section 1.3).
pub fn http_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .use_preconfigured_tls((*tls_config()).clone())
                .connect_timeout(DEFAULT_REQUEST_TIMEOUT)
                .build()
                .expect("HTTP client builds")
        })
        .clone()
}

/// `reason` on `EnvironmentAuthInvalidError`.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthInvalidReason {
    MissingCredential,
    InvalidCredential,
    #[serde(other)]
    Other,
}

/// Why the server rejected a DPoP proof.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DpopFailureReason {
    TimeWindow,
    KeyMismatch,
    RequestMismatch,
    TokenMismatch,
    Replay,
    InvalidProof,
    #[serde(other)]
    Other,
}

/// A typed JSON error body from an environment's HTTP API.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "_tag")]
pub enum EnvironmentHttpError {
    EnvironmentRequestInvalidError {
        reason: String,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    EnvironmentAuthInvalidError {
        reason: AuthInvalidReason,
        #[serde(rename = "dpopFailureReason")]
        dpop_failure_reason: Option<DpopFailureReason>,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    EnvironmentScopeRequiredError {
        #[serde(rename = "requiredScope")]
        required_scope: String,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    EnvironmentConnectApprovalPendingError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    EnvironmentOperationForbiddenError {
        reason: String,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    EnvironmentResourceNotFoundError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    EnvironmentInternalError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
}

impl EnvironmentHttpError {
    pub fn trace_id(&self) -> Option<&str> {
        match self {
            Self::EnvironmentRequestInvalidError { trace_id, .. }
            | Self::EnvironmentAuthInvalidError { trace_id, .. }
            | Self::EnvironmentScopeRequiredError { trace_id, .. }
            | Self::EnvironmentConnectApprovalPendingError { trace_id }
            | Self::EnvironmentOperationForbiddenError { trace_id, .. }
            | Self::EnvironmentResourceNotFoundError { trace_id }
            | Self::EnvironmentInternalError { trace_id } => trace_id.as_deref(),
        }
    }
}

/// How an HTTP request to an environment failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RemoteRequestError {
    #[error("the request timed out")]
    Timeout,
    #[error("the request did not reach the server: {0}")]
    Network(String),
    #[error("the server refused the request: {0:?}")]
    Environment(EnvironmentHttpError),
    #[error("the server answered HTTP {status}")]
    UndeclaredStatus { status: u16, body: String },
    #[error("the server's answer did not decode: {0}")]
    InvalidJson(String),
}

/// Sends `request` with `timeout` and decodes a JSON success body, or the
/// environment's typed error body.
pub async fn send_json<T: DeserializeOwned>(
    request: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<T, RemoteRequestError> {
    let response = tokio::time::timeout(timeout, request.send())
        .await
        .map_err(|_| RemoteRequestError::Timeout)?
        .map_err(|error| {
            if error.is_timeout() {
                RemoteRequestError::Timeout
            } else {
                RemoteRequestError::Network(error.to_string())
            }
        })?;
    let status = response.status();
    let body = tokio::time::timeout(timeout, response.bytes())
        .await
        .map_err(|_| RemoteRequestError::Timeout)?
        .map_err(|error| RemoteRequestError::Network(error.to_string()))?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|error| RemoteRequestError::InvalidJson(error.to_string()));
    }
    Err(match serde_json::from_slice::<EnvironmentHttpError>(&body) {
        Ok(error) => RemoteRequestError::Environment(error),
        Err(_) => RemoteRequestError::UndeclaredStatus {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).chars().take(512).collect(),
        },
    })
}
