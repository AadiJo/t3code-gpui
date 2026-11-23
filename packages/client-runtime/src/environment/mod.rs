//! Environment identity, endpoint URLs and the public descriptor.

pub mod descriptor;
pub mod endpoint;

pub use descriptor::{DescriptorCapabilities, EnvironmentDescriptor, fetch_descriptor};
pub use endpoint::{endpoint_url, rpc_socket_url};

/// A T3 Code environment's stable id. Survives restarts and address changes.
pub use t3code_contracts::EnvironmentId;
