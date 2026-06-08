use csa_config::{EnforcementMode, ProjectConfig};
use csa_resource::{ResourceCapability, isolation_plan::IsolationPlan};
use tracing::warn;

pub(crate) fn default_off_allows_unsandboxed(
    enforcement: EnforcementMode,
    has_run_memory_override: bool,
) -> bool {
    matches!(enforcement, EnforcementMode::Off) && !has_run_memory_override
}

pub(crate) fn resolve_config_enforcement(
    cfg: &ProjectConfig,
    tool_name: &str,
    has_run_memory_override: bool,
) -> Result<Option<EnforcementMode>, String> {
    let enforcement = cfg.tool_enforcement_mode(tool_name);
    if !matches!(enforcement, EnforcementMode::Off) {
        return Ok(Some(enforcement));
    }
    if !has_run_memory_override {
        return Ok(None);
    }
    if explicit_resource_enforcement_mode(cfg, tool_name) == Some(EnforcementMode::Off) {
        return Err(format!(
            "--memory-max-mb cannot be used for tool '{tool_name}' because resource \
             sandbox enforcement_mode is explicitly \"off\". Set enforcement_mode = \
             \"best-effort\" or remove --memory-max-mb."
        ));
    }

    warn!(
        tool = %tool_name,
        "Auto-promoting resource enforcement_mode Off to BestEffort for per-run --memory-max-mb"
    );
    Ok(Some(EnforcementMode::BestEffort))
}

fn explicit_resource_enforcement_mode(
    cfg: &ProjectConfig,
    tool_name: &str,
) -> Option<EnforcementMode> {
    cfg.tools
        .get(tool_name)
        .and_then(|tool| tool.enforcement_mode)
        .or(cfg.resources.enforcement_mode)
}

pub(crate) fn capability_error_if_unenforced(
    tool_name: &str,
    has_run_memory_override: bool,
    resource_cap: ResourceCapability,
) -> Option<String> {
    if !has_run_memory_override || resource_cap == ResourceCapability::CgroupV2 {
        return None;
    }

    Some(format!(
        "--memory-max-mb requires cgroup v2 memory enforcement for tool '{tool_name}', \
         but detected resource capability is {resource_cap}. Run without --memory-max-mb \
         or enable cgroup v2 sandbox support."
    ))
}

pub(crate) fn plan_error_if_unenforced(
    tool_name: &str,
    has_run_memory_override: bool,
    plan: &IsolationPlan,
) -> Option<String> {
    if !has_run_memory_override || plan.resource == ResourceCapability::CgroupV2 {
        return None;
    }

    Some(format!(
        "--memory-max-mb requires cgroup v2 memory enforcement for tool '{tool_name}', \
         but the resolved isolation plan would use {} after sandbox compatibility adjustments.",
        plan.resource
    ))
}
