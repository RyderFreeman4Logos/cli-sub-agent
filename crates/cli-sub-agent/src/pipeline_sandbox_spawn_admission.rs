//! Spawn-admission helpers derived from a resolved sandbox plan.

use csa_executor::ExecuteOptions;
use csa_resource::ResourceCapability;

/// Resource capability from the resolved sandbox plan used for spawn admission.
pub(crate) fn resource_capability_for_spawn_admission(
    options: &ExecuteOptions,
) -> ResourceCapability {
    options
        .sandbox
        .as_ref()
        .map_or(ResourceCapability::None, |sandbox| {
            sandbox.isolation_plan.resource
        })
}
