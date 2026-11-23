//! TEMPORARY: the few RPC declarations `examples/e2e_connect.rs` uses.
//! Delete this module once `t3code_contracts` generates the real ones.

use serde::{Deserialize, Serialize};
use t3code_contracts::rpc::{RpcMethod, RpcStream};

/// `EnvironmentAuthorizationError`, which every RPC may fail with.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentAuthorizationError {
    pub message: String,
    pub required_scope: String,
}

/// `{}` on the wire.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Empty {}

/// `server.probe`: a unary round trip with an empty payload and result.
pub struct ServerProbe;

impl RpcMethod for ServerProbe {
    const TAG: &'static str = "server.probe";
    type Payload = Empty;
    type Success = Empty;
    type Error = EnvironmentAuthorizationError;
}

/// `orchestration.subscribeShell` input, the flags the e2e check sends.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeShellInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    pub request_completion_marker: bool,
}

/// One shell stream item, reduced to its discriminator.
#[derive(Deserialize, Debug)]
pub struct ShellStreamItemHead {
    pub kind: String,
}

/// `orchestration.subscribeShell`.
pub struct SubscribeShell;

impl RpcMethod for SubscribeShell {
    const TAG: &'static str = "orchestration.subscribeShell";
    type Payload = SubscribeShellInput;
    type Success = ShellStreamItemHead;
    type Error = EnvironmentAuthorizationError;
}

impl RpcStream for SubscribeShell {}
