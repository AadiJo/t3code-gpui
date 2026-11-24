use std::time::Duration;

use serde::Deserialize;
use url::Url;

use super::{EnvironmentId, endpoint_url};
use crate::rpc::http::{RemoteRequestError, http_client, send_json};

/// The public `/.well-known/t3/environment` answer. Only the fields the
/// connection layer reads; unknown fields are ignored.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDescriptor {
    pub environment_id: EnvironmentId,
    pub label: String,
    #[serde(default)]
    pub server_version: String,
    /// Absent on servers that predate negotiation, which speak version 1.
    pub orchestration_protocol_version: Option<u32>,
    #[serde(default)]
    pub capabilities: DescriptorCapabilities,
}

/// Capability flags the connection layer gates on. Absent means unsupported.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DescriptorCapabilities {
    /// The host renews DPoP-bound access tokens, so pairing should use DPoP.
    #[serde(default)]
    pub access_token_renewal: bool,
    pub server_self_update: Option<String>,
    #[serde(default)]
    pub server_self_update_progress: bool,
    #[serde(default)]
    pub desktop_app_update: bool,
}

/// Reads the descriptor. It is public: no credential is sent, so a saved LAN
/// address that now belongs to another machine learns nothing.
pub async fn fetch_descriptor(
    http_base_url: &Url,
    timeout: Duration,
) -> Result<EnvironmentDescriptor, RemoteRequestError> {
    let url = endpoint_url(http_base_url, "/.well-known/t3/environment");
    let request = http_client()
        .get(url)
        .header(reqwest::header::CACHE_CONTROL, "no-store");
    send_json(request, timeout).await
}
