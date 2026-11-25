//! Access changes on an environment with this client's session there (`state/auth.ts`'s
//! commands): approve or deny a T3 Connect device waiting for approval, revoke a trusted
//! device, revoke one client or every other client, and create a pairing link.

use std::time::Duration;

use t3code_contracts::http::endpoints::{
    AuthApproveConnectDevice, AuthPairingCredential, AuthRevokeClient, AuthRevokeConnectDevice, AuthRevokeOtherClients, AuthRevokePairingLink,
};
use t3code_contracts::{
    AuthApproveConnectDeviceInput, AuthCreatePairingCredentialInput, AuthCreatePairingCredentialInputNoPurpose, AuthGrantScopes,
    AuthPairingCredentialResult, AuthRevokeClientSessionInput, AuthRevokeConnectDeviceInput, AuthRevokePairingLinkInput, AuthSessionId,
};

use super::environment_endpoint::{EndpointError, call};
use crate::preview::environment_link::EnvironmentLink;

const TIMEOUT: Duration = Duration::from_secs(15);

/// Approves the device waiting on pairing link `pairing_link_id`.
pub async fn approve_connect_device(link: &EnvironmentLink, pairing_link_id: String) -> Result<bool, EndpointError> {
    Ok(call::<AuthApproveConnectDevice>(link, &[], &AuthApproveConnectDeviceInput { pairing_link_id }, TIMEOUT).await?.approved)
}

/// Denies a waiting device, or revokes a pairing link: both remove the link.
pub async fn revoke_pairing_link(link: &EnvironmentLink, id: String) -> Result<(), EndpointError> {
    call::<AuthRevokePairingLink>(link, &[], &AuthRevokePairingLinkInput { id }, TIMEOUT).await.map(|_| ())
}

/// Revokes a trusted T3 Connect device by its key.
pub async fn revoke_connect_device(link: &EnvironmentLink, proof_key_thumbprint: String) -> Result<bool, EndpointError> {
    Ok(call::<AuthRevokeConnectDevice>(link, &[], &AuthRevokeConnectDeviceInput { proof_key_thumbprint }, TIMEOUT).await?.revoked)
}

/// Revokes one client session.
pub async fn revoke_client(link: &EnvironmentLink, session_id: AuthSessionId) -> Result<(), EndpointError> {
    call::<AuthRevokeClient>(link, &[], &AuthRevokeClientSessionInput { session_id }, TIMEOUT).await.map(|_| ())
}

/// Revokes every client session but this one.
pub async fn revoke_other_clients(link: &EnvironmentLink) -> Result<(), EndpointError> {
    call::<AuthRevokeOtherClients>(link, &[], &(), TIMEOUT).await.map(|_| ())
}

/// A one-time pairing link with `scopes`, labelled when `label` is set.
pub async fn create_pairing_link(link: &EnvironmentLink, label: Option<String>, scopes: AuthGrantScopes) -> Result<AuthPairingCredentialResult, EndpointError> {
    let input = AuthCreatePairingCredentialInput::NoPurpose(AuthCreatePairingCredentialInputNoPurpose { label, scopes: Some(scopes), ..Default::default() });
    call::<AuthPairingCredential>(link, &[], &input, TIMEOUT).await
}
