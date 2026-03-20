//! Sandbox resolution and telemetry for the execution pipeline.
//!
//! Extracted from `pipeline.rs` to keep the main pipeline under the monolith limit.
//! Handles enforcement mode checking, capability detection, config resolution,
//! and first-turn telemetry recording.

use csa_config::ProjectConfig;
use csa_executor::{ExecuteOptions, SandboxContext};
use csa_process::StreamMode;
use csa_resource::isolation_plan::{
    EnforcementMode as ResourceEnforcementMode, IsolationPlanBuilder,
};
use csa_session::MetaSessionState;
use tracing::{info, warn};

/// Outcome of sandbox resolution — either enriched options or a fatal error string
/// (for `Required` mode with no capability).
pub(crate) enum SandboxResolution {
    /// Options ready (may or may not contain sandbox context).
    Ok(Box<ExecuteOptions>),
    /// Sandbox is required but no capability was detected; caller must bail.
    RequiredButUnavailable(String),
}

/// Resolve sandbox configuration from project config and enforcement mode.
///
/// Returns `SandboxResolution::Ok` with the options (possibly enriched with
/// `SandboxContext`) or `SandboxResolution::RequiredButUnavailable` when
/// enforcement is `Required` but the host lacks both cgroup v2 and setrlimit.
///
/// When `no_fs_sandbox` is `true`, filesystem isolation is forcibly disabled
/// regardless of config (equivalent to `enforcement_mode = "off"` for FS only).
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_sandbox_options(
    config: Option<&ProjectConfig>,
    tool_name: &str,
    session_id: &str,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    liveness_dead_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
) -> SandboxResolution {
    let default_resources = csa_config::ResourcesConfig::default();
    let stdin_write_timeout_seconds = config
        .map(|cfg| cfg.resources.stdin_write_timeout_seconds)
        .unwrap_or(default_resources.stdin_write_timeout_seconds);
    let acp_init_timeout_seconds = config
        .map(|cfg| cfg.acp.init_timeout_seconds)
        .unwrap_or(csa_config::AcpConfig::default().init_timeout_seconds);
    let termination_grace_period_seconds = config
        .map(|cfg| cfg.resources.termination_grace_period_seconds)
        .unwrap_or(default_resources.termination_grace_period_seconds);
    let mut execute_options = ExecuteOptions::new(stream_mode, idle_timeout_seconds)
        .with_liveness_dead_seconds(liveness_dead_seconds)
        .with_stdin_write_timeout_seconds(stdin_write_timeout_seconds)
        .with_acp_init_timeout_seconds(acp_init_timeout_seconds)
        .with_termination_grace_period_seconds(termination_grace_period_seconds)
        .with_initial_response_timeout_seconds(initial_response_timeout_seconds);

    let Some(cfg) = config else {
        // No project config — apply profile-based defaults for heavyweight tools.
        let defaults = csa_config::default_sandbox_for_tool(tool_name);
        execute_options = execute_options.with_setting_sources(defaults.setting_sources);

        if matches!(defaults.enforcement, csa_config::EnforcementMode::Off) {
            return SandboxResolution::Ok(Box::new(execute_options));
        }

        let Some(_memory_max_mb) = defaults.memory_max_mb else {
            return SandboxResolution::Ok(Box::new(execute_options));
        };

        let resource_cap = csa_resource::detect_resource_capability();
        let fs_cap = if no_fs_sandbox {
            csa_resource::FilesystemCapability::None
        } else {
            csa_resource::detect_filesystem_capability()
        };
        if matches!(resource_cap, csa_resource::ResourceCapability::None) {
            warn!(
                tool = tool_name,
                "No sandbox capability available; skipping enforcement for profile defaults"
            );
            return SandboxResolution::Ok(Box::new(execute_options));
        }

        // Build IsolationPlan via builder (BestEffort for profile defaults).
        let plan = IsolationPlanBuilder::new(ResourceEnforcementMode::BestEffort)
            .with_resource_capability(resource_cap)
            .with_filesystem_capability(fs_cap)
            .with_resource_limits(
                defaults.memory_max_mb,
                defaults.memory_swap_max_mb,
                None, // pids_max not available from profile defaults
            )
            .with_tool_defaults(
                tool_name,
                // No project root available from profile defaults; use cwd.
                &std::env::current_dir().unwrap_or_default(),
                // No session dir available; use a temporary placeholder.
                &std::env::temp_dir(),
            )
            .build()
            .expect("BestEffort IsolationPlan should never fail");

        execute_options = execute_options.with_sandbox(SandboxContext {
            isolation_plan: plan,
            tool_name: tool_name.to_string(),
            session_id: session_id.to_string(),
            best_effort: true, // Profile defaults always use best-effort
        });

        return SandboxResolution::Ok(Box::new(execute_options));
    };

    execute_options = execute_options.with_setting_sources(cfg.tool_setting_sources(tool_name));

    // Use per-tool enforcement mode (profile-aware) instead of global-only.
    let enforcement = cfg.tool_enforcement_mode(tool_name);
    if matches!(enforcement, csa_config::EnforcementMode::Off) {
        return SandboxResolution::Ok(Box::new(execute_options));
    }

    let Some(memory_max_mb) = cfg.sandbox_memory_max_mb(tool_name) else {
        if matches!(enforcement, csa_config::EnforcementMode::Required) {
            return SandboxResolution::RequiredButUnavailable(format!(
                "Sandbox enforcement is required for tool '{tool_name}' but no memory_max_mb is configured. \
                 Set resources.memory_max_mb or tools.{tool_name}.memory_max_mb in config."
            ));
        }
        info!(
            tool = %tool_name,
            enforcement = ?enforcement,
            "Sandbox enforcement active but no memory_max_mb configured; skipping isolation"
        );
        return SandboxResolution::Ok(Box::new(execute_options));
    };

    // Memory limit exists — detect capabilities and build IsolationPlan.
    let resource_cap = csa_resource::detect_resource_capability();

    // Resolve filesystem enforcement independently from resource enforcement.
    let fs_enforcement = if no_fs_sandbox {
        ResourceEnforcementMode::Off
    } else {
        let fs_config = &cfg.filesystem_sandbox;
        match fs_config
            .enforcement_mode
            .as_deref()
            .unwrap_or("best-effort")
        {
            "off" => ResourceEnforcementMode::Off,
            "required" => ResourceEnforcementMode::Required,
            _ => ResourceEnforcementMode::BestEffort,
        }
    };

    let fs_cap = if matches!(fs_enforcement, ResourceEnforcementMode::Off) {
        csa_resource::FilesystemCapability::None
    } else {
        csa_resource::detect_filesystem_capability()
    };

    // Map config enforcement mode to resource enforcement mode.
    let resource_enforcement = match enforcement {
        csa_config::EnforcementMode::Required => ResourceEnforcementMode::Required,
        csa_config::EnforcementMode::BestEffort => ResourceEnforcementMode::BestEffort,
        csa_config::EnforcementMode::Off => ResourceEnforcementMode::Off,
    };

    match enforcement {
        csa_config::EnforcementMode::Required => {
            if resource_cap == csa_resource::ResourceCapability::None {
                return SandboxResolution::RequiredButUnavailable(
                    "Sandbox required but no capability detected (no cgroup v2 or setrlimit). \
                     Set enforcement_mode = \"off\" or \"best-effort\" to proceed without isolation."
                        .to_string(),
                );
            }
        }
        csa_config::EnforcementMode::BestEffort => {
            if resource_cap == csa_resource::ResourceCapability::None {
                warn!(
                    tool = %tool_name,
                    "Sandbox configured but no capability detected; proceeding without isolation"
                );
            }
        }
        csa_config::EnforcementMode::Off => {} // already filtered above
    }

    // Resolve project root and session dir for tool-specific writable paths.
    let project_root = std::env::current_dir().unwrap_or_default();
    let session_dir = csa_session::manager::get_session_dir(&project_root, session_id)
        .unwrap_or_else(|_| std::env::temp_dir());

    let memory_swap_max_mb = cfg.sandbox_memory_swap_max_mb(tool_name);
    let pids_max = cfg.sandbox_pids_max();

    let mut builder = IsolationPlanBuilder::new(resource_enforcement)
        .with_filesystem_enforcement(fs_enforcement)
        .with_resource_capability(resource_cap)
        .with_filesystem_capability(fs_cap)
        .with_resource_limits(Some(memory_max_mb), memory_swap_max_mb, pids_max)
        .with_tool_defaults(tool_name, &project_root, &session_dir);

    // Apply extra writable paths from [filesystem_sandbox] config.
    if !no_fs_sandbox {
        let fs_config = &cfg.filesystem_sandbox;
        for path in &fs_config.extra_writable {
            builder = builder.with_writable_path(path.clone());
        }
        if let Some(tool_paths) = fs_config.tool_writable_overrides.get(tool_name) {
            for path in tool_paths {
                builder = builder.with_writable_path(path.clone());
            }
        }
    }

    let plan = builder.build();

    let plan = match plan {
        Ok(p) => p,
        Err(e) => {
            return SandboxResolution::RequiredButUnavailable(format!(
                "Failed to build isolation plan for tool '{tool_name}': {e}"
            ));
        }
    };

    info!(
        tool = %tool_name,
        enforcement = ?enforcement,
        resource_cap = %resource_cap,
        filesystem_cap = %fs_cap,
        memory_max_mb,
        "Sandbox isolation plan resolved"
    );

    execute_options = execute_options.with_sandbox(SandboxContext {
        isolation_plan: plan,
        tool_name: tool_name.to_string(),
        session_id: session_id.to_string(),
        best_effort: matches!(enforcement, csa_config::EnforcementMode::BestEffort),
    });

    SandboxResolution::Ok(Box::new(execute_options))
}

/// Conditionally inflate and immediately deflate a memory balloon for claude-code.
///
/// Pre-warms RAM by `mmap`-ing a large anonymous mapping with `MAP_POPULATE`, forcing
/// the kernel to swap out other processes.  The balloon is dropped (deflated) right
/// away so the freed physical pages are available for the tool process about to launch.
pub(crate) fn maybe_inflate_balloon(tool_name: &str) {
    use csa_resource::memory_balloon::{MemoryBalloon, should_enable_balloon};

    if tool_name != "claude-code" {
        return;
    }

    const BALLOON_SIZE: usize = 1024 * 1024 * 1024; // 1 GiB
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let available_memory = sys.available_memory();
    let available_swap = sys.free_swap();

    if !should_enable_balloon(available_memory, available_swap, BALLOON_SIZE as u64) {
        return;
    }

    match MemoryBalloon::inflate(BALLOON_SIZE) {
        Ok(balloon) => {
            info!(
                size_mb = BALLOON_SIZE / 1024 / 1024,
                "Memory balloon inflated — deflating immediately"
            );
            drop(balloon);
        }
        Err(e) => {
            // Balloon is an optimisation; failure is non-fatal.
            warn!(error = %e, "Memory balloon inflation failed; continuing without pre-warming");
        }
    }
}

/// Record sandbox telemetry in session state (first turn only).
///
/// If sandbox options are present and `session.sandbox_info` is still `None`,
/// detects the active capability and writes a `SandboxInfo` snapshot.
pub(crate) fn record_sandbox_telemetry(
    execute_options: &ExecuteOptions,
    session: &mut MetaSessionState,
) {
    if execute_options.sandbox.is_none() || session.sandbox_info.is_some() {
        return;
    }

    let capability = csa_resource::detect_resource_capability();
    let mode = match capability {
        csa_resource::ResourceCapability::CgroupV2 => "cgroup",
        csa_resource::ResourceCapability::Setrlimit => "rlimit",
        csa_resource::ResourceCapability::None => "none",
    };
    let memory: Option<u64> = execute_options
        .sandbox
        .as_ref()
        .and_then(|ctx| ctx.isolation_plan.memory_max_mb);

    // Capture filesystem isolation mode from the isolation plan.
    let fs_mode = execute_options
        .sandbox
        .as_ref()
        .map(|ctx| match ctx.isolation_plan.filesystem {
            csa_resource::FilesystemCapability::Bwrap => "bwrap".to_string(),
            csa_resource::FilesystemCapability::Landlock => "landlock".to_string(),
            csa_resource::FilesystemCapability::None => "none".to_string(),
        });

    session.sandbox_info = Some(csa_session::SandboxInfo {
        mode: mode.to_string(),
        memory_max_mb: memory,
        filesystem_mode: fs_mode.clone(),
    });

    info!(
        session = %session.meta_session_id,
        sandbox_mode = mode,
        memory_max_mb = ?memory,
        filesystem_mode = ?fs_mode,
        "Sandbox telemetry recorded in session state"
    );
}

#[cfg(test)]
#[path = "pipeline_sandbox_tests.rs"]
mod tests;
