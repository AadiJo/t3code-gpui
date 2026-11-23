//! The orchestration protocol gate: descriptor version and socket query.

use url::Url;

use super::model::{BlockedReason, ConnectionError};
use crate::environment::EnvironmentDescriptor;

/// The orchestration protocol this client speaks.
pub const ORCHESTRATION_PROTOCOL_VERSION: u32 = 2;

/// `None` when the server speaks our protocol. Servers that predate
/// negotiation omit the field and speak version 1.
pub fn protocol_compatibility_error(descriptor: &EnvironmentDescriptor) -> Option<ConnectionError> {
    let server = descriptor.orchestration_protocol_version.unwrap_or(1);
    if server == ORCHESTRATION_PROTOCOL_VERSION {
        return None;
    }
    if server > ORCHESTRATION_PROTOCOL_VERSION {
        return Some(ConnectionError::blocked(
            BlockedReason::Unsupported,
            format!(
                "This client is not supported by this server. Update your app or use a compatible release to connect to {}.",
                descriptor.label
            ),
        ));
    }
    let capabilities = &descriptor.capabilities;
    let mut error = ConnectionError::blocked(
        BlockedReason::Unsupported,
        format!("This client requires a newer server. Update T3 Code on {} to connect.", descriptor.label),
    );
    error.server_update_required = capabilities
        .server_self_update
        .as_deref()
        .is_some_and(|mode| mode != "desktop-managed" || capabilities.desktop_app_update);
    Some(error)
}

/// Adds `orchestrationProtocol=2` (required, else HTTP 426). No
/// `clientCapabilities`: this client does not decode secret references.
pub fn append_orchestration_protocol(socket_url: &mut Url) {
    socket_url
        .query_pairs_mut()
        .append_pair("orchestrationProtocol", &ORCHESTRATION_PROTOCOL_VERSION.to_string());
}
