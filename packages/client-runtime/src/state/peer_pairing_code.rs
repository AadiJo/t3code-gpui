//! One-use pairing codes minted on an environment with the user's own session there
//! (`state/peerPairingCode.ts`): a code for linking, so a link can be answered without fetching
//! a code by hand, and a code that approves one app's sign-in at a set level.

use std::time::Duration;

use t3code_contracts::http::endpoints::AuthPairingCredential;
use t3code_contracts::{
    AuthCreatePairingCredentialInput, AuthCreatePairingCredentialInputNoPurpose, AuthCreatePairingCredentialInputPeerLink,
    AuthMcpClientAccess, AuthPairingCredentialResult, AuthPairingCredentialResultPurpose,
};

use super::environment_endpoint::call;
use crate::preview::environment_link::EnvironmentLink;

const MINT_TIMEOUT: Duration = Duration::from_secs(8);
const PEER_PAIRING_CODE_LABEL: &str = "T3 Code link";
const MCP_APPROVAL_CODE_LABEL: &str = "App sign-in";

/// Minting failed; the user can still paste a code from the other environment.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PeerPairingCodeError {
    #[error("Your session there cannot create pairing codes. Paste a code from there instead.")]
    NotAllowed,
    #[error("Could not get a pairing code from there. Paste a code from there instead.")]
    Unavailable,
}

async fn mint(link: &EnvironmentLink, payload: AuthCreatePairingCredentialInput) -> Result<AuthPairingCredentialResult, PeerPairingCodeError> {
    call::<AuthPairingCredential>(link, &[], &payload, MINT_TIMEOUT)
        .await
        .map_err(|error| if error.is_not_allowed() { PeerPairingCodeError::NotAllowed } else { PeerPairingCodeError::Unavailable })
}

/// A hidden code that approves one app's sign-in at `access`, and only that. Servers without
/// `mcpApprovalCodes` ignore the level, so check the capability first.
pub async fn mint_mcp_approval_code(link: &EnvironmentLink, access: AuthMcpClientAccess) -> Result<AuthPairingCredentialResult, PeerPairingCodeError> {
    mint(
        link,
        AuthCreatePairingCredentialInput::NoPurpose(AuthCreatePairingCredentialInputNoPurpose {
            label: Some(MCP_APPROVAL_CODE_LABEL.into()),
            access: Some(access),
            ..Default::default()
        }),
    )
    .await
}

/// A code for linking another environment to this one at `access`. Each call mints its own: a
/// code is one-use, so two overlapping answers must never share one.
pub async fn mint_peer_pairing_code(link: &EnvironmentLink, access: AuthMcpClientAccess) -> Result<String, PeerPairingCodeError> {
    let session = link.session(Duration::from_secs(10)).await.map_err(|_| PeerPairingCodeError::Unavailable)?;
    if session.initial_config.environment.capabilities.peer_link_pairing_codes != Some(true) {
        return Err(PeerPairingCodeError::Unavailable);
    }
    let minted = mint(
        link,
        AuthCreatePairingCredentialInput::PeerLink(AuthCreatePairingCredentialInputPeerLink {
            label: Some(PEER_PAIRING_CODE_LABEL.into()),
            scopes: None,
            access,
        }),
    )
    .await?;
    let peer_link = matches!(minted.purpose, Some(AuthPairingCredentialResultPurpose::PeerLink));
    if !peer_link || minted.access != Some(access) {
        return Err(PeerPairingCodeError::Unavailable);
    }
    Ok(minted.credential)
}
