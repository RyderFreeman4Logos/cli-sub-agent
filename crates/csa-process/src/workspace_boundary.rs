const WORKSPACE_BOUNDARY_ERROR_THRESHOLD: usize = 20;
pub(crate) const WORKSPACE_BOUNDARY_THRESHOLD_ENV: &str = "CSA_WORKSPACE_BOUNDARY_THRESHOLD";

pub(crate) fn resolve_workspace_boundary_threshold() -> usize {
    std::env::var(WORKSPACE_BOUNDARY_THRESHOLD_ENV)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(WORKSPACE_BOUNDARY_ERROR_THRESHOLD)
}

pub(crate) fn workspace_boundary_hint(threshold: usize) -> String {
    format!(
        "[csa-notice] Workspace boundary rejections have crossed {threshold}. \
         Refocus on paths inside the project root; CSA state/cache and tool-internal \
         directories are inspectable via `csa session logs` / `csa session result` \
         from the orchestrator, not via direct filesystem reads from inside this session.\n"
    )
}
