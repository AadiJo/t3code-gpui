//! Helpers over provider instances (`providerInstance.ts`).

use crate::{ProviderDriverKind, ProviderInstanceId};

/// The id of a driver's default instance, which is the driver's own name (`defaultInstanceIdForDriver`).
pub fn default_instance_id_for_driver(driver: &ProviderDriverKind) -> ProviderInstanceId {
    ProviderInstanceId(driver.0.clone())
}
