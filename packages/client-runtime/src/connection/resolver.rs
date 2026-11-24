//! Prepares one route: authorize it, gate the protocol, build the socket URL.

use std::time::Duration;

use super::catalog::{ConnectionCredential, ConnectionRoute, ProfileAuthorization, TargetRoute};
use super::compatibility::{append_orchestration_protocol, protocol_compatibility_error};
use super::model::{BlockedReason, ConnectionError, ConnectionHop, PreparedConnection, TransientReason};
use super::routes::{credential_connection_id, route_id};
use crate::authorization::RemoteAuthorization;
use crate::environment::EnvironmentId;
use crate::rpc::protocol::FRAME_PARTS_QUERY_PARAM;

/// Authorizing one route (descriptor, tokens, relay mint, ticket) must
/// finish within this.
pub const PREPARE_TIMEOUT: Duration = Duration::from_secs(30);

/// Authorizes `route` for `environment_id` and returns a socket URL with a
/// fresh ticket, `orchestrationProtocol=2` and `frameParts=1`.
pub async fn prepare(
    auth: &RemoteAuthorization,
    environment_id: &EnvironmentId,
    label: &str,
    route: &ConnectionRoute,
) -> Result<PreparedConnection, ConnectionError> {
    tokio::time::timeout(PREPARE_TIMEOUT, prepare_route(auth, environment_id, label, route))
        .await
        .unwrap_or_else(|_| {
            Err(ConnectionError::transient(
                TransientReason::Timeout,
                format!("Timed out authorizing {label}."),
            ))
        })
}

async fn prepare_route(
    auth: &RemoteAuthorization,
    environment_id: &EnvironmentId,
    label: &str,
    route: &ConnectionRoute,
) -> Result<PreparedConnection, ConnectionError> {
    let authorized = match &route.target.route {
        TargetRoute::Relay => auth.authorize_relay(environment_id, None).await?,
        TargetRoute::Ssh { .. } => {
            return Err(ConnectionError::blocked(
                BlockedReason::Configuration,
                "SSH routes are not supported by this client.",
            )
            .at(ConnectionHop::Device));
        }
        TargetRoute::Bearer { connection_id } => {
            let profile = route.bearer_profile().ok_or_else(|| {
                ConnectionError::blocked(
                    BlockedReason::Configuration,
                    format!("Connection profile {connection_id} is unavailable."),
                )
                .at(ConnectionHop::Device)
            })?;
            if profile.environment_id != *environment_id {
                return Err(ConnectionError::environment_mismatch(environment_id, &profile.environment_id));
            }
            let (http, ws) = (&profile.http_base_url, &profile.ws_base_url);
            if profile.authorization == Some(ProfileAuthorization::T3Connect) {
                auth.authorize_relay(environment_id, Some((http, ws))).await?
            } else {
                let credential_id = credential_connection_id(connection_id);
                let credential = auth.credentials().get(credential_id).await.map_err(|error| {
                    ConnectionError::blocked(
                        BlockedReason::Configuration,
                        format!("Could not read saved credentials: {error}"),
                    )
                    .at(ConnectionHop::Device)
                })?;
                match credential {
                    Some(ConnectionCredential::Bearer { token }) => {
                        auth.authorize_bearer(environment_id, http, ws, &token).await?
                    }
                    Some(ConnectionCredential::Dpop { .. }) => {
                        auth.authorize_direct_dpop(environment_id, credential_id, http, ws).await?
                    }
                    // No sign-in for this address: an import whose credential was bound to
                    // the desktop app's key, or one that was lost. Presented like a host that
                    // refused this device's sign-in, which the web shows for the same route,
                    // so the row offers pairing again; nothing is sent to the host.
                    None => {
                        return Err(ConnectionError::blocked(
                            BlockedReason::Authentication,
                            format!("This device has no sign-in for {}. Pair this address again.", profile.http_base_url),
                        )
                        .at(ConnectionHop::Server));
                    }
                }
            }
        }
    };
    if let Some(error) = protocol_compatibility_error(&authorized.descriptor) {
        return Err(error);
    }
    let mut socket_url = authorized.socket_url;
    append_orchestration_protocol(&mut socket_url);
    socket_url.query_pairs_mut().append_pair(FRAME_PARTS_QUERY_PARAM, "1");
    let label = if authorized.descriptor.label.is_empty() { label.to_owned() } else { authorized.descriptor.label.clone() };
    Ok(PreparedConnection {
        environment_id: environment_id.clone(),
        label,
        route_id: route_id(&route.target).to_owned(),
        via: authorized.via,
        http_base_url: authorized.http_base_url,
        socket_url,
        http_authorization: authorized.http_authorization,
        descriptor: authorized.descriptor,
    })
}
