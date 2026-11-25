//! An environment's T3 Connect link as it reports it (`cloud/linkEnvironment.ts`): its link
//! state, its reachability check, and whether its tunnel and MCP URL count
//! (`cloud/t3ConnectState.ts`). Each call runs on the environment with this client's session.

use std::time::Duration;

use t3code_contracts::http::endpoints::{ConnectCheckReachability, ConnectLinkState};
use t3code_contracts::{EnvironmentCloudLinkStateResult, EnvironmentConnectReachability};

use super::environment_endpoint::{EndpointError, call};
use crate::preview::environment_link::EnvironmentLink;
use crate::rpc::http::RemoteRequestError;

/// Linking can install the relay client and start a managed tunnel before it answers.
const CLOUD_LINK_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// `CloudEnvironmentLinkError`: the English sentence the fork shows, with the server's detail.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct CloudLinkError(pub String);

fn failure(message: &str, error: EndpointError) -> CloudLinkError {
    // The connect group answers `EnvironmentHttp*Error { message }` (`findEnvironmentCloudApiError`).
    #[derive(serde::Deserialize)]
    struct Message {
        message: String,
    }
    let detail = match &error {
        EndpointError::Request(RemoteRequestError::UndeclaredStatus { body, .. }) => {
            serde_json::from_str::<Message>(body).ok().map(|m| m.message)
        }
        EndpointError::NotConnected => return CloudLinkError("The environment is not connected.".into()),
        _ => None,
    };
    CloudLinkError(match detail {
        Some(detail) => format!("{message}: {detail}"),
        None => format!("{message}."),
    })
}

/// `GET /api/connect/link-state`.
pub async fn read_link_state(link: &EnvironmentLink) -> Result<EnvironmentCloudLinkStateResult, CloudLinkError> {
    call::<ConnectLinkState>(link, &[], &(), CLOUD_LINK_REQUEST_TIMEOUT)
        .await
        .map_err(|error| failure("Could not read environment cloud link state", error))
}

/// `POST /api/connect/reachability`: runs the environment's check now; the link state then
/// reports it.
pub async fn check_reachability(link: &EnvironmentLink) -> Result<EnvironmentConnectReachability, CloudLinkError> {
    call::<ConnectCheckReachability>(link, &[], &(), CLOUD_LINK_REQUEST_TIMEOUT)
        .await
        .map_err(|error| failure("Could not check T3 Connect", error))
}

/// `isManagedTunnelActive`: a managed tunnel runs and the relay (when it says) agrees it is one.
/// Stale tunnel config after a publish-only switch must not count.
pub fn is_managed_tunnel_active(state: &EnvironmentCloudLinkStateResult, relay_provider_kind: Option<&str>) -> bool {
    let local = state.managed_tunnel_active.unwrap_or(state.linked);
    state.link_revoked != Some(true) && local && relay_provider_kind.is_none_or(|kind| kind == "cloudflare_tunnel")
}

/// `t3ConnectMcpUrl` for a saved machine: its tunnel's https origin from T3 Connect plus `/mcp`.
/// Only a tunnel counts (a publish-only link registers its local address as `manual`), and only
/// plain DNS names pass, since the URL ends up in shell commands.
pub fn t3_connect_mcp_url(provider_kind: &str, http_base_url: &url::Url) -> Option<String> {
    if provider_kind != "cloudflare_tunnel" || http_base_url.scheme() != "https" {
        return None;
    }
    let host = http_base_url.host_str()?;
    let labels: Vec<&str> = host.split('.').collect();
    let plain = labels.len() > 1 && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    plain.then(|| format!("{}/mcp", http_base_url.origin().ascii_serialization()))
}

/// The time a check ran, for ordering checks (`checkedAt` is ISO 8601, so it sorts as text).
pub fn checked_at(reachability: &EnvironmentConnectReachability) -> Option<&str> {
    match reachability {
        EnvironmentConnectReachability::Reachable(r) => Some(&r.checked_at),
        EnvironmentConnectReachability::Unreachable(r) => Some(&r.checked_at),
        EnvironmentConnectReachability::Unknown(_) => None,
    }
}

/// `afterFailedCheck`: what a failed check request leaves. `prior` is the link state before it,
/// `latest` a read after it. The server may still have checked, so a result newer than `prior`
/// counts; otherwise only a later server check does (`since`), and with nothing read, a retry.
pub fn after_failed_check(
    prior: Option<&EnvironmentCloudLinkStateResult>,
    latest: Option<&EnvironmentCloudLinkStateResult>,
) -> (Option<EnvironmentConnectReachability>, Option<String>) {
    let fresh = latest.and_then(|l| l.reachability.as_ref());
    let known = prior.and_then(|p| p.reachability.as_ref());
    if let (Some(fresh), Some(_)) = (fresh, prior) {
        let newer = match known {
            None => true,
            Some(known) => checked_at(fresh) > checked_at(known),
        };
        if newer {
            return (Some(fresh.clone()), checked_at(fresh).map(str::to_owned));
        }
    }
    let since = latest.or(prior).map(|baseline| baseline.reachability.as_ref().and_then(checked_at).unwrap_or("").to_owned());
    (None, since)
}
