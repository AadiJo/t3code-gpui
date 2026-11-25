//! What stands between the user and the wizard's steps (`mcp/mcpGate.ts`). Only `Open` lets
//! them go on: cloud apps need a public address, so the environment must answer on its
//! T3 Connect tunnel. This client runs no backend, so every environment is a saved machine
//! whose address and health come from T3 Connect's list.

use gpui_kit::SharedString;
use t3code_client_runtime::relay::RelayEndpoint;
use t3code_client_runtime::state::cloud_link::{is_managed_tunnel_active, t3_connect_mcp_url};
use t3code_contracts::{EnvironmentCloudLinkStateResult, EnvironmentConnectReachability, EnvironmentConnectReachabilityUnreachable};

use crate::connection::RelayAvailability;
use crate::i18n::t;

/// The gate.
#[derive(Clone, Debug, PartialEq)]
pub enum McpGate {
    Checking,
    SignIn,
    /// `relay`: T3 Connect reports the machine offline, or its list could not be read; both
    /// keep a retry. `None` when that list shows nothing wrong, so the user should reconnect.
    Disconnected { relay: Option<RelayProblem> },
    Update,
    NoRelayRead,
    Error { message: SharedString },
    TurnOn,
    Offline { reachability: Option<EnvironmentConnectReachabilityUnreachable> },
    Open { mcp_url: SharedString },
}

/// Why a disconnected machine can still be retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayProblem {
    Offline,
    Failed,
}

/// T3 Connect's machine list as the gate reads it.
#[derive(Clone, Copy, Debug)]
pub enum RelayListing<'a> {
    Loading,
    Failed,
    Listed { endpoint: Option<&'a RelayEndpoint>, availability: Option<RelayAvailability> },
}

/// A reachability check run for the gate: "checking" until it has an answer, or its answer
/// (`None` when the check failed).
#[derive(Clone, Debug)]
pub enum ReachabilityInput {
    Checking,
    Answer(Option<EnvironmentConnectReachability>),
}

/// Everything the gate reads.
pub struct McpGateInput<'a> {
    /// `None` until T3 Connect sign-in has loaded.
    pub signed_in: Option<bool>,
    pub connected: bool,
    /// The environment's `mcpApprovalCodes` capability; `None` until its config loads.
    pub approval_codes: Option<bool>,
    pub can_read_relay: bool,
    pub link_state: Option<&'a EnvironmentCloudLinkStateResult>,
    pub link_state_error: Option<SharedString>,
    pub relay: RelayListing<'a>,
    pub reachability: ReachabilityInput,
}

/// `resolveMcpGate` with `thisMachine: false`.
pub fn resolve_mcp_gate(input: McpGateInput<'_>) -> McpGate {
    let Some(signed_in) = input.signed_in else { return McpGate::Checking };
    if !signed_in {
        return McpGate::SignIn;
    }
    let relay = input.relay;
    if !input.connected {
        // T3 Connect checks only its own tunnels; a publish-only link's check always fails.
        let availability = match relay {
            RelayListing::Listed { endpoint: Some(endpoint), availability } if endpoint.provider_kind == "cloudflare_tunnel" => availability,
            _ => None,
        };
        // A refresh empties the list first, so wait for it instead of flashing "Connect first".
        if matches!(relay, RelayListing::Loading) || availability == Some(RelayAvailability::Checking) {
            return McpGate::Checking;
        }
        if matches!(relay, RelayListing::Failed) || availability == Some(RelayAvailability::Error) {
            return McpGate::Disconnected { relay: Some(RelayProblem::Failed) };
        }
        return McpGate::Disconnected { relay: (availability == Some(RelayAvailability::Offline)).then_some(RelayProblem::Offline) };
    }
    let Some(approval_codes) = input.approval_codes else { return McpGate::Checking };
    if !approval_codes {
        return McpGate::Update;
    }
    if !input.can_read_relay {
        return McpGate::NoRelayRead;
    }
    if let Some(message) = input.link_state_error {
        return McpGate::Error { message };
    }
    let Some(link_state) = input.link_state else { return McpGate::Checking };
    let endpoint = match relay {
        RelayListing::Listed { endpoint, .. } => endpoint,
        _ => None,
    };
    if !is_managed_tunnel_active(link_state, endpoint.map(|e| e.provider_kind.as_str())) {
        return McpGate::TurnOn;
    }
    let endpoint = match relay {
        RelayListing::Loading => return McpGate::Checking,
        RelayListing::Failed => return McpGate::Error { message: t!("Could not reach T3 Connect.") },
        RelayListing::Listed { endpoint: None, .. } => return McpGate::TurnOn,
        RelayListing::Listed { endpoint: Some(endpoint), .. } => endpoint,
    };
    let reachability = match input.reachability {
        ReachabilityInput::Checking => return McpGate::Checking,
        ReachabilityInput::Answer(answer) => answer,
    };
    let mcp_url = t3_connect_mcp_url(&endpoint.provider_kind, &endpoint.http_base_url);
    match (reachability, mcp_url) {
        (Some(EnvironmentConnectReachability::Reachable(_)), Some(mcp_url)) => McpGate::Open { mcp_url: mcp_url.into() },
        (Some(EnvironmentConnectReachability::Unreachable(unreachable)), _) => McpGate::Offline { reachability: Some(unreachable) },
        _ => McpGate::Offline { reachability: None },
    }
}
