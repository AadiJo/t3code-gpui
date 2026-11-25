//! Saved environments, routes, and keeping each environment connected.

pub mod catalog;
pub mod compatibility;
pub mod credential_store;
pub mod driver;
pub mod errors;
pub mod model;
pub mod onboarding;
pub mod outdated_host_update;
pub mod profile_store;
pub mod pull_request_routing;
pub mod registry;
pub mod resolver;
pub mod routes;
pub mod server_update;
pub mod supervisor;

pub use catalog::{CatalogEntry, ConnectionCredential, ConnectionRoute};
pub use driver::ConnectionContext;
pub use model::{ConnectionError, ConnectionPhase, ConnectionStatus, PreparedConnection};
pub use registry::ConnectionRegistry;
pub use supervisor::{EnvironmentSupervisor, SupervisorHandle, SupervisorSignal};
