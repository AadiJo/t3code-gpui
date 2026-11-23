//! Environment identity, endpoint URLs and the public descriptor.

pub mod descriptor;
pub mod endpoint;

pub use descriptor::{DescriptorCapabilities, EnvironmentDescriptor, fetch_descriptor};
pub use endpoint::{endpoint_url, rpc_socket_url};

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A T3 Code environment's stable id. Survives restarts and address changes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentId(pub Arc<str>);

impl EnvironmentId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for EnvironmentId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}
