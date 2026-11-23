//! Authenticated HTTP reads of orchestration routes (`environmentHttpAuth.ts`): the session's
//! credential (renewed DPoP tokens included) and the orchestration protocol header.

use std::time::Duration;

use t3code_contracts::http::HttpEndpoint;

use crate::connection::compatibility::ORCHESTRATION_PROTOCOL_VERSION;
use crate::environment::endpoint_url;
use crate::rpc::RpcSession;
use crate::rpc::http::{EnvironmentHttpError, RemoteRequestError, http_client};

use super::environment::EnvironmentContext;

/// Header every orchestration route requires (`ct/environment.ts`).
pub const ORCHESTRATION_PROTOCOL_HEADER: &str = "x-t3-orchestration-protocol";

/// Fills `:name` segments of an endpoint path.
pub fn endpoint_path<E: HttpEndpoint>(params: &[(&str, &str)]) -> String {
    let mut path = E::PATH.to_owned();
    for (name, value) in params {
        let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
        path = path.replace(&format!(":{name}"), &encoded);
    }
    path
}

/// GETs `path` on the session's server and returns the body of a 2xx answer.
pub async fn get_bytes(
    context: &EnvironmentContext,
    session: &RpcSession,
    path: &str,
    query: &[(&str, String)],
    timeout: Duration,
) -> Result<Vec<u8>, RemoteRequestError> {
    let mut url = endpoint_url(&session.prepared.http_base_url, path);
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
    }
    let authorization = context
        .supervisor
        .http_authorization
        .borrow()
        .clone()
        .unwrap_or_else(|| session.prepared.http_authorization.clone());
    let request = http_client()
        .get(url.clone())
        .header(ORCHESTRATION_PROTOCOL_HEADER, ORCHESTRATION_PROTOCOL_VERSION.to_string());
    let request = authorization.apply(request, "GET", &url, context.auth.key());
    let response = tokio::time::timeout(timeout, request.send())
        .await
        .map_err(|_| RemoteRequestError::Timeout)?
        .map_err(|error| RemoteRequestError::Network(error.to_string()))?;
    let status = response.status();
    let body = tokio::time::timeout(timeout, response.bytes())
        .await
        .map_err(|_| RemoteRequestError::Timeout)?
        .map_err(|error| RemoteRequestError::Network(error.to_string()))?;
    if status.is_success() {
        return Ok(body.to_vec());
    }
    Err(match serde_json::from_slice::<EnvironmentHttpError>(&body) {
        Ok(error) => RemoteRequestError::Environment(error),
        Err(_) => RemoteRequestError::UndeclaredStatus {
            status: status.as_u16(),
            body: String::from_utf8_lossy(&body).chars().take(512).collect(),
        },
    })
}

/// Whether the server said the resource does not exist.
pub fn is_not_found(error: &RemoteRequestError) -> bool {
    matches!(error, RemoteRequestError::Environment(EnvironmentHttpError::EnvironmentResourceNotFoundError { .. }))
}
