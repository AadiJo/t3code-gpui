//! Helpers over server config (`server.ts`).

use crate::{EnvironmentMachineKind, ExecutionEnvironmentDescriptor, ServerSettings};

/// The machine icon of an environment (`resolveEnvironmentMachineKind`): the user's choice in
/// settings, else what the platform reports, else `Server`.
pub fn resolve_environment_machine_kind(
    environment: Option<&ExecutionEnvironmentDescriptor>,
    settings: Option<&ServerSettings>,
) -> EnvironmentMachineKind {
    settings
        .and_then(|s| s.environment_icon)
        .or_else(|| environment.and_then(|e| e.platform.machine))
        .unwrap_or(EnvironmentMachineKind::Server)
}
