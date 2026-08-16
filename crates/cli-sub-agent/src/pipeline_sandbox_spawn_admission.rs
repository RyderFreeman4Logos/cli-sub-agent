//! Sandbox plan construction and spawn-admission helpers.

use csa_executor::ExecuteOptions;
use csa_resource::{
    ResourceCapability,
    isolation_plan::{IsolationPlan, IsolationPlanBuilder},
};

pub(super) fn build_isolation_plan(
    builder: IsolationPlanBuilder,
    tool_name: &str,
) -> Result<IsolationPlan, String> {
    builder.build().map_err(|error| {
        format!(
            "Failed to build isolation plan for tool '{tool_name}': {error}. \
             Repair the reported sandbox path or pass --no-fs-sandbox to disable filesystem isolation."
        )
    })
}

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
