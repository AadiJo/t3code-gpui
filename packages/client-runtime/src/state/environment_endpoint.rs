//! Typed calls to an environment's HTTP endpoints with this client's own session there
//! (`executeAuthenticatedEnvironmentHttpRequest`): the session's credential, renewed DPoP tokens
//! included, signs each request. Used by pairing codes, T3 Connect link state and access changes.

use std::time::Duration;

use serde::Serialize;
use t3code_contracts::http::{BodyEncoding, HttpEndpoint};

use crate::environment::endpoint_url;
use crate::preview::environment_link::EnvironmentLink;
use crate::rpc::http::{EnvironmentHttpError, RemoteRequestError, http_client};

/// Why an endpoint call failed.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EndpointError {
    #[error("The environment is not connected.")]
    NotConnected,
    #[error(transparent)]
    Request(#[from] RemoteRequestError),
}

impl EndpointError {
    /// The server refused it for this session's grants (`EnvironmentScopeRequiredError`,
    /// `EnvironmentOperationForbiddenError`).
    pub fn is_not_allowed(&self) -> bool {
        matches!(
            self,
            Self::Request(RemoteRequestError::Environment(
                EnvironmentHttpError::EnvironmentScopeRequiredError { .. } | EnvironmentHttpError::EnvironmentOperationForbiddenError { .. }
            ))
        )
    }
}

/// Calls `E` on `link`'s environment. `params` fill the path's `:name` segments; `body` is sent
/// as the endpoint declares (JSON or form). A success with no body decodes from `null`.
pub async fn call<E>(link: &EnvironmentLink, params: &[(&str, &str)], body: &E::Body, timeout: Duration) -> Result<E::Success, EndpointError>
where
    E: HttpEndpoint,
    E::Body: Serialize,
{
    let session = link.session(Duration::from_secs(10)).await.map_err(|_| EndpointError::NotConnected)?;
    let url = endpoint_url(&session.prepared.http_base_url, &super::environment_http::endpoint_path::<E>(params));
    let method = reqwest::Method::from_bytes(E::METHOD.as_str().as_bytes()).expect("HTTP methods are valid");
    let authorization =
        link.supervisor.http_authorization.borrow().clone().unwrap_or_else(|| session.prepared.http_authorization.clone());
    let mut request = authorization.apply(http_client().request(method, url.clone()), E::METHOD.as_str(), &url, link.auth.key());
    request = match E::BODY {
        BodyEncoding::Json => request.json(body),
        BodyEncoding::Form => request.form(body),
        BodyEncoding::None | BodyEncoding::Bytes => request,
    };
    let response = tokio::time::timeout(timeout, request.send())
        .await
        .map_err(|_| RemoteRequestError::Timeout)?
        .map_err(|error| RemoteRequestError::Network(error.to_string()))?;
    let status = response.status();
    let bytes = tokio::time::timeout(timeout, response.bytes())
        .await
        .map_err(|_| RemoteRequestError::Timeout)?
        .map_err(|error| RemoteRequestError::Network(error.to_string()))?;
    if status.is_success() {
        let bytes: &[u8] = if bytes.is_empty() { b"null" } else { &bytes };
        return serde_json::from_slice(bytes).map_err(|error| RemoteRequestError::InvalidJson(error.to_string()).into());
    }
    Err(match serde_json::from_slice::<EnvironmentHttpError>(&bytes) {
        Ok(error) => RemoteRequestError::Environment(error),
        Err(_) => RemoteRequestError::UndeclaredStatus {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).chars().take(512).collect(),
        },
    }
    .into())
}
