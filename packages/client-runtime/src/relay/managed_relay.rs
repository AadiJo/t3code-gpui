//! The T3 Connect relay client (spec 02, section 4). The relay lists linked
//! environments and brokers one-time bootstrap credentials bound to this
//! install's DPoP key; it never carries app traffic.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use url::Url;

use super::dpop_proof::DpopKey;
use crate::connection::model::FailureCause;
use crate::environment::EnvironmentId;
use crate::rpc::http::{DpopFailureReason, http_client};

/// Every relay request times out after this.
pub const RELAY_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// A cached relay token is reused while it has more than this left.
const RELAY_TOKEN_REUSE_MARGIN_MS: i64 = 5_000;
/// The only public client ids are `t3-web` and `t3-mobile`.
const RELAY_CLIENT_ID: &str = "t3-web";

/// Supplies the signed-in user's Clerk session JWT from the `t3-relay`
/// template. Sign-in UI lives elsewhere; this runtime only consumes it.
pub trait CloudSession: Send + Sync {
    /// A fresh JWT, or `None` while signed out or still loading.
    fn relay_jwt(&self) -> BoxFuture<'_, Option<String>>;
}

/// A relay DPoP scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayScope {
    EnvironmentConnect,
    EnvironmentStatus,
}

impl RelayScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::EnvironmentConnect => "environment:connect",
            Self::EnvironmentStatus => "environment:status",
        }
    }
}

/// A typed error body from the relay.
#[derive(Deserialize, Debug, Clone, thiserror::Error)]
#[serde(tag = "_tag")]
pub enum RelayProtectedError {
    #[error("Relay authentication failed: {reason}")]
    RelayAuthInvalidError {
        reason: String,
        #[serde(rename = "dpopFailureReason")]
        dpop_failure_reason: Option<DpopFailureReason>,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("This environment is not available to this account through T3 Connect.")]
    RelayEnvironmentConnectNotAuthorizedError {
        reason: Option<String>,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("The environment's tunnel did not answer: {reason}")]
    RelayEnvironmentEndpointUnavailableError {
        reason: String,
        #[serde(rename = "endpointFailure")]
        endpoint_failure: Option<FailureCause>,
        #[serde(rename = "endpointStatus")]
        endpoint_status: Option<u16>,
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("The environment's tunnel timed out.")]
    RelayEnvironmentEndpointTimedOutError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("The environment link is unavailable.")]
    RelayEnvironmentLinkUnavailableError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("The environment link failed.")]
    RelayEnvironmentLinkFailedError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("T3 Connect had an internal error.")]
    RelayInternalError {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    /// Link proof and limit errors: the account cannot use this link.
    #[error("T3 Connect refused the environment link.")]
    #[serde(
        alias = "RelayEnvironmentLinkProofExpiredError",
        alias = "RelayEnvironmentLinkProofInvalidError",
        alias = "RelayEnvironmentLinkLimitExceededError"
    )]
    Other {
        #[serde(rename = "traceId")]
        trace_id: Option<String>,
    },
    #[error("T3 Connect returned an unknown error.")]
    #[serde(other)]
    Unknown,
}

impl RelayProtectedError {
    pub fn trace_id(&self) -> Option<&str> {
        match self {
            Self::RelayAuthInvalidError { trace_id, .. }
            | Self::RelayEnvironmentConnectNotAuthorizedError { trace_id, .. }
            | Self::RelayEnvironmentEndpointUnavailableError { trace_id, .. }
            | Self::RelayEnvironmentEndpointTimedOutError { trace_id }
            | Self::RelayEnvironmentLinkUnavailableError { trace_id }
            | Self::RelayEnvironmentLinkFailedError { trace_id }
            | Self::RelayInternalError { trace_id }
            | Self::Other { trace_id } => trace_id.as_deref(),
            Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum RelayError {
    #[error(transparent)]
    Protected(RelayProtectedError),
    #[error("{detail}")]
    Request { status: Option<u16>, detail: String },
    #[error("T3 Connect did not answer in time")]
    Timeout,
    #[error("the relay URL must be an https origin")]
    InvalidUrl,
    #[error("the relay granted scope {granted:?}")]
    ScopesUnexpected { granted: String },
}

impl RelayError {
    /// The relay rejected a cached access token; mint a new one and retry once.
    fn is_rejected_token(&self) -> bool {
        matches!(self, Self::Protected(RelayProtectedError::RelayAuthInvalidError { reason, .. }) if reason == "invalid_bearer")
    }
}

/// Where an environment's tunnel answers.
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelayEndpoint {
    pub http_base_url: Url,
    pub ws_base_url: Url,
    /// `manual`, `cloudflare_tunnel` or `t3_relay`.
    pub provider_kind: String,
}

/// One environment linked to the account.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RelayEnvironmentRecord {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub endpoint: RelayEndpoint,
    pub linked_at: String,
}

/// A one-time bootstrap credential for one environment, bound to our key.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RelayConnectResponse {
    pub environment_id: EnvironmentId,
    pub endpoint: RelayEndpoint,
    pub credential: String,
    pub expires_at: String,
}

/// `online` or `offline`, with the relay's reason when offline.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RelayEnvironmentStatus {
    pub environment_id: EnvironmentId,
    pub status: String,
    pub checked_at: String,
    pub error: Option<String>,
    pub endpoint_failure: Option<FailureCause>,
    pub offline_reason: Option<String>,
}

#[derive(Deserialize)]
struct ListEnvironmentsResponse {
    environments: Vec<RelayEnvironmentRecord>,
}

#[derive(Deserialize)]
struct RelayTokenResponse {
    access_token: String,
    expires_in: i64,
    scope: String,
}

struct CachedRelayToken {
    account_id: String,
    scopes: Vec<RelayScope>,
    access_token: String,
    expires_at_ms: i64,
}

/// Talks to one relay with this install's DPoP key.
pub struct RelayClient {
    /// The relay origin, no trailing slash. Also the token `resource`.
    origin: String,
    key: DpopKey,
    tokens: Mutex<Vec<CachedRelayToken>>,
}

#[derive(Serialize)]
struct RelayTokenForm<'a> {
    grant_type: &'static str,
    subject_token: &'a str,
    subject_token_type: &'static str,
    requested_token_type: &'static str,
    resource: &'a str,
    scope: &'a str,
    client_id: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectRequest<'a> {
    client_key_thumbprint: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<&'a str>,
}

impl RelayClient {
    /// `relay_url` must be an https origin with no path, query or credentials.
    pub fn new(relay_url: &str, key: DpopKey) -> Result<Self, RelayError> {
        let url = Url::parse(relay_url.trim()).map_err(|_| RelayError::InvalidUrl)?;
        let path_ok = url.path().chars().all(|c| c == '/');
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !path_ok
        {
            return Err(RelayError::InvalidUrl);
        }
        Ok(Self { origin: url.origin().ascii_serialization(), key, tokens: Mutex::default() })
    }

    pub fn key(&self) -> &DpopKey {
        &self.key
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("{}{path}", self.origin)).expect("relay origin plus path parses")
    }

    /// `GET /v1/environments`: the account's linked environments.
    pub async fn list_environments(&self, clerk_jwt: &str) -> Result<Vec<RelayEnvironmentRecord>, RelayError> {
        let request = http_client()
            .get(self.url("/v1/environments"))
            .header("authorization", format!("Bearer {clerk_jwt}"));
        let response: ListEnvironmentsResponse = send_relay(request).await?;
        Ok(response.environments)
    }

    /// `POST /v1/environments/:id/connect`: a bootstrap credential for our key.
    pub async fn connect_environment(
        &self,
        clerk_jwt: &str,
        environment_id: &EnvironmentId,
        device_id: Option<&str>,
    ) -> Result<RelayConnectResponse, RelayError> {
        let url = self.url(&format!("/v1/environments/{environment_id}/connect"));
        let body = ConnectRequest { client_key_thumbprint: self.key.thumbprint(), device_id };
        self.with_relay_token(clerk_jwt, RelayScope::EnvironmentConnect, |token| {
            http_client()
                .post(url.clone())
                .header("authorization", format!("DPoP {token}"))
                .header("dpop", self.key.proof("POST", &url, Some(token)))
                .json(&body)
        })
        .await
    }

    /// `POST /v1/environments/:id/status`: whether the tunnel answers.
    pub async fn environment_status(
        &self,
        clerk_jwt: &str,
        environment_id: &EnvironmentId,
    ) -> Result<RelayEnvironmentStatus, RelayError> {
        let url = self.url(&format!("/v1/environments/{environment_id}/status"));
        self.with_relay_token(clerk_jwt, RelayScope::EnvironmentStatus, |token| {
            http_client()
                .post(url.clone())
                .header("authorization", format!("DPoP {token}"))
                .header("dpop", self.key.proof("POST", &url, Some(token)))
        })
        .await
    }

    /// Runs a DPoP request with a cached or fresh relay token. A token the
    /// relay rejects is dropped and the request retried once with a new one.
    async fn with_relay_token<T: DeserializeOwned>(
        &self,
        clerk_jwt: &str,
        scope: RelayScope,
        build: impl Fn(&str) -> reqwest::RequestBuilder,
    ) -> Result<T, RelayError> {
        let token = self.access_token(clerk_jwt, scope).await?;
        match send_relay(build(&token)).await {
            Err(error) if error.is_rejected_token() => {
                self.tokens.lock().await.retain(|cached| cached.access_token != token);
                let token = self.access_token(clerk_jwt, scope).await?;
                send_relay(build(&token)).await
            }
            result => result,
        }
    }

    /// A relay DPoP access token for `scope`, cached per account.
    async fn access_token(&self, clerk_jwt: &str, scope: RelayScope) -> Result<String, RelayError> {
        let account_id = jwt_subject(clerk_jwt).unwrap_or_default();
        let mut tokens = self.tokens.lock().await;
        let now = crate::authorization::remote::now_epoch_ms();
        tokens.retain(|cached| cached.expires_at_ms > now + RELAY_TOKEN_REUSE_MARGIN_MS);
        if !account_id.is_empty()
            && let Some(cached) = tokens
                .iter()
                .find(|cached| cached.account_id == account_id && cached.scopes.contains(&scope))
        {
            return Ok(cached.access_token.clone());
        }
        let url = self.url("/v1/client/dpop-token");
        let form = RelayTokenForm {
            grant_type: "urn:ietf:params:oauth:grant-type:token-exchange",
            subject_token: clerk_jwt,
            subject_token_type: "urn:ietf:params:oauth:token-type:jwt",
            requested_token_type: "urn:ietf:params:oauth:token-type:access_token",
            resource: &self.origin,
            scope: scope.as_str(),
            client_id: RELAY_CLIENT_ID,
        };
        let request = http_client()
            .post(url.clone())
            .header("dpop", self.key.proof("POST", &url, None))
            .form(&form);
        let response: RelayTokenResponse = send_relay(request).await?;
        if response.scope.split_whitespace().collect::<Vec<_>>() != [scope.as_str()] {
            return Err(RelayError::ScopesUnexpected { granted: response.scope });
        }
        if !account_id.is_empty() {
            tokens.push(CachedRelayToken {
                account_id,
                scopes: vec![scope],
                access_token: response.access_token.clone(),
                expires_at_ms: now + response.expires_in * 1000,
            });
        }
        Ok(response.access_token)
    }
}

/// The `sub` claim of a JWT, read without verifying it (only a cache key).
pub fn jwt_subject(jwt: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Claims {
        sub: Option<String>,
    }
    let payload = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice::<Claims>(&bytes).ok()?.sub.filter(|sub| !sub.is_empty())
}

async fn send_relay<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, RelayError> {
    let response = tokio::time::timeout(RELAY_REQUEST_TIMEOUT, request.send())
        .await
        .map_err(|_| RelayError::Timeout)?
        .map_err(|error| RelayError::Request { status: None, detail: error.to_string() })?;
    let status = response.status();
    let body = tokio::time::timeout(RELAY_REQUEST_TIMEOUT, response.bytes())
        .await
        .map_err(|_| RelayError::Timeout)?
        .map_err(|error| RelayError::Request { status: Some(status.as_u16()), detail: error.to_string() })?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|error| RelayError::Request {
            status: Some(status.as_u16()),
            detail: format!("T3 Connect's answer did not decode: {error}"),
        });
    }
    Err(match serde_json::from_slice::<RelayProtectedError>(&body) {
        Ok(error) => RelayError::Protected(error),
        Err(_) => RelayError::Request {
            status: Some(status.as_u16()),
            detail: format!("T3 Connect answered HTTP {status}"),
        },
    })
}
