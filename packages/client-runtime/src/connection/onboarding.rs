//! Pairing a new environment or route (spec 02, section 5).

use url::Url;

use super::catalog::{BearerProfile, ConnectionCredential, ConnectionProfile, ConnectionRoute, ConnectionTarget, TargetRoute};
use super::errors::remote_environment_error;
use super::model::{BlockedReason, ConnectionError};
use crate::authorization::RemoteAuthorization;
use crate::authorization::remote::{ConnectionMethod, IssuedTokenType, SubjectToken, exchange_token};
use crate::environment::{EnvironmentId, fetch_descriptor};
use crate::rpc::http::DEFAULT_REQUEST_TIMEOUT;

/// A parsed pairing link or host-plus-code: where to pair and the one-time
/// credential to spend.
#[derive(Clone, Debug)]
pub struct PairingRequest {
    pub http_base_url: Url,
    pub ws_base_url: Url,
    pub credential: String,
    /// The label a hosted pairing link suggests.
    pub label: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PairingInputError {
    #[error("the address is not a valid http(s) or ws(s) URL")]
    InvalidAddress,
    #[error("the pairing link has no token")]
    MissingToken,
}

/// Normalizes a backend address to `http(s)://host[:port]/` and its socket
/// twin. A bare host gets `https://`.
pub fn normalize_base_urls(input: &str) -> Result<(Url, Url), PairingInputError> {
    let input = input.trim();
    let with_scheme = if input.contains("://") { input.to_owned() } else { format!("https://{input}") };
    let mut url = Url::parse(&with_scheme).map_err(|_| PairingInputError::InvalidAddress)?;
    let (http, ws) = match url.scheme() {
        "http" | "ws" => ("http", "ws"),
        "https" | "wss" => ("https", "wss"),
        _ => return Err(PairingInputError::InvalidAddress),
    };
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    let mut http_url = url.clone();
    http_url.set_scheme(http).map_err(|_| PairingInputError::InvalidAddress)?;
    let mut ws_url = url;
    ws_url.set_scheme(ws).map_err(|_| PairingInputError::InvalidAddress)?;
    Ok((http_url, ws_url))
}

fn param(pairs: url::form_urlencoded::Parse<'_>, key: &str) -> Option<String> {
    pairs.into_iter().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).filter(|v| !v.is_empty())
}

/// Reads a pairing link: `https://host/pair#token=…` (or `?token=`), a hosted
/// link with `?host=<backend>&label=`, or `t3code[-channel]://pair?pairingUrl=…`.
pub fn parse_pairing_url(input: &str) -> Result<PairingRequest, PairingInputError> {
    let url = Url::parse(input.trim()).map_err(|_| PairingInputError::InvalidAddress)?;
    if (url.scheme() == "t3code" || url.scheme().starts_with("t3code-")) && url.host_str() == Some("pair") {
        let inner = param(url.query_pairs(), "pairingUrl").ok_or(PairingInputError::MissingToken)?;
        return parse_pairing_url(&inner);
    }
    let token = url
        .fragment()
        .and_then(|fragment| param(url::form_urlencoded::parse(fragment.as_bytes()), "token"))
        .or_else(|| param(url.query_pairs(), "token"))
        .ok_or(PairingInputError::MissingToken)?;
    let label = param(url.query_pairs(), "label");
    let backend = param(url.query_pairs(), "host").unwrap_or_else(|| url.as_str().to_owned());
    let (http_base_url, ws_base_url) = normalize_base_urls(&backend)?;
    Ok(PairingRequest { http_base_url, ws_base_url, credential: token, label })
}

/// Host typed by the user plus a pairing code.
pub fn parse_host_and_code(host: &str, code: &str) -> Result<PairingRequest, PairingInputError> {
    let (http_base_url, ws_base_url) = normalize_base_urls(host)?;
    let credential = code.trim().to_owned();
    if credential.is_empty() {
        return Err(PairingInputError::MissingToken);
    }
    Ok(PairingRequest { http_base_url, ws_base_url, credential, label: None })
}

/// A paired route, ready to save.
#[derive(Clone, Debug)]
pub struct PairedRoute {
    pub environment_id: EnvironmentId,
    pub label: String,
    pub route: ConnectionRoute,
    pub credential: ConnectionCredential,
}

/// Pairs: reads the descriptor and checks it before spending the one-time
/// credential, then exchanges it. Uses DPoP when the host renews tokens.
/// `expected` is set when adding a route to a known environment.
pub async fn pair(
    auth: &RemoteAuthorization,
    request: &PairingRequest,
    expected: Option<&EnvironmentId>,
) -> Result<PairedRoute, ConnectionError> {
    let descriptor = fetch_descriptor(&request.http_base_url, DEFAULT_REQUEST_TIMEOUT)
        .await
        .map_err(|error| remote_environment_error(&error, ConnectionMethod::Direct))?;
    if let Some(expected) = expected
        && descriptor.environment_id != *expected
    {
        return Err(ConnectionError::environment_mismatch(expected, &descriptor.environment_id));
    }
    // An outdated server is still saved so it can be updated; a newer one is refused.
    if descriptor.orchestration_protocol_version.unwrap_or(1) > super::compatibility::ORCHESTRATION_PROTOCOL_VERSION {
        return Err(super::compatibility::protocol_compatibility_error(&descriptor).expect("newer protocol is an error"));
    }
    let dpop = descriptor.capabilities.access_token_renewal.then(|| auth.key());
    let issued = exchange_token(&request.http_base_url, SubjectToken::Bootstrap(&request.credential), dpop, auth.presentation())
        .await
        .map_err(|error| remote_environment_error(&error, ConnectionMethod::Direct))?;
    let credential = match issued.token_type {
        IssuedTokenType::DPoP if dpop.is_some() => ConnectionCredential::Dpop {
            expires_at_epoch_ms: issued.expires_at_epoch_ms(),
            access_token: issued.access_token,
        },
        IssuedTokenType::Bearer => ConnectionCredential::Bearer { token: issued.access_token },
        IssuedTokenType::DPoP => {
            return Err(ConnectionError::blocked(
                BlockedReason::Configuration,
                "The environment issued a DPoP token this client did not ask for.",
            ));
        }
    };
    let label = request.label.clone().unwrap_or_else(|| descriptor.label.clone());
    let origin = request.http_base_url.origin().ascii_serialization();
    let connection_id = format!("bearer:{}:{origin}", descriptor.environment_id);
    let route = ConnectionRoute {
        target: ConnectionTarget {
            environment_id: descriptor.environment_id.clone(),
            label: label.clone(),
            route: TargetRoute::Bearer { connection_id: connection_id.clone() },
        },
        profile: Some(ConnectionProfile::Bearer(BearerProfile {
            connection_id,
            environment_id: descriptor.environment_id.clone(),
            label: label.clone(),
            http_base_url: request.http_base_url.clone(),
            ws_base_url: request.ws_base_url.clone(),
            learned: false,
            authorization: None,
            network: None,
        })),
    };
    Ok(PairedRoute { environment_id: descriptor.environment_id, label, route, credential })
}
