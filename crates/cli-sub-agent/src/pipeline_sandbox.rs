//! Sandbox resolution and telemetry for the execution pipeline.
//!
//! Extracted from `pipeline.rs` to keep the main pipeline under the monolith limit.
//! Handles enforcement mode checking, capability detection, config resolution,
//! and first-turn telemetry recording.

use csa_config::ProjectConfig;
use csa_executor::{ExecuteOptions, SandboxContext};
use csa_process::StreamMode;
use csa_session::MetaSessionState;
use tracing::{info, warn};

/// Outcome of sandbox resolution — either enriched options or a fatal error string
/// (for `Required` mode with no capability).
pub(crate) enum SandboxResolution {
    /// Options ready (may or may not contain sandbox context).
    Ok(ExecuteOptions),
    /// Sandbox is required but no capability was detected; caller must bail.
    RequiredButUnavailable(String),
}

/// Resolve sandbox configuration from project config and enforcement mode.
///
/// Returns `SandboxResolution::Ok` with the options (possibly enriched with
/// `SandboxContext`) or `SandboxResolution::RequiredButUnavailable` when
/// enforcement is `Required` but the host lacks both cgroup v2 and setrlimit.
pub(crate) fn resolve_sandbox_options(
    config: Option<&ProjectConfig>,
    tool_name: &str,
    session_id: &str,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    liveness_dead_seconds: u64,
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
        .with_termination_grace_period_seconds(termination_grace_period_seconds);

    let Some(cfg) = config else {
        // No project config — apply profile-based defaults for heavyweight tools.
        let defaults = csa_config::default_sandbox_for_tool(tool_name);
        execute_options = execute_options.with_setting_sources(defaults.setting_sources);

        if matches!(defaults.enforcement, csa_config::EnforcementMode::Off) {
            return SandboxResolution::Ok(execute_options);
        }

        let Some(memory_max_mb) = defaults.memory_max_mb else {
            return SandboxResolution::Ok(execute_options);
        };

        let sandbox_config = csa_resource::SandboxConfig {
            memory_max_mb,
            memory_swap_max_mb: defaults.memory_swap_max_mb,
            pids_max: None,
        };

        let capability = csa_resource::detect_sandbox_capability();
        if matches!(capability, csa_resource::SandboxCapability::None) {
            warn!(
                tool = tool_name,
                "No sandbox capability available; skipping enforcement for profile defaults"
            );
            return SandboxResolution::Ok(execute_options);
        }

        execute_options = execute_options.with_sandbox(SandboxContext {
            config: sandbox_config,
            tool_name: tool_name.to_string(),
            session_id: session_id.to_string(),
            best_effort: true, // Profile defaults always use best-effort
        });

        return SandboxResolution::Ok(execute_options);
    };

    execute_options = execute_options.with_setting_sources(cfg.tool_setting_sources(tool_name));

    // Use per-tool enforcement mode (profile-aware) instead of global-only.
    let enforcement = cfg.tool_enforcement_mode(tool_name);
    if matches!(enforcement, csa_config::EnforcementMode::Off) {
        return SandboxResolution::Ok(execute_options);
    }

    let Some(memory_max_mb) = cfg.sandbox_memory_max_mb(tool_name) else {
        if matches!(enforcement, csa_config::EnforcementMode::Required) {
            return SandboxResolution::RequiredButUnavailable(format!(
                "Sandbox enforcement is required for tool '{}' but no memory_max_mb is configured. \
                 Set resources.memory_max_mb or tools.{}.memory_max_mb in config.",
                tool_name, tool_name
            ));
        }
        info!(
            tool = %tool_name,
            enforcement = ?enforcement,
            "Sandbox enforcement active but no memory_max_mb configured; skipping isolation"
        );
        return SandboxResolution::Ok(execute_options);
    };

    // Memory limit exists — build sandbox config.
    let sandbox_config = csa_resource::SandboxConfig {
        memory_max_mb,
        memory_swap_max_mb: cfg.sandbox_memory_swap_max_mb(tool_name),
        pids_max: cfg.sandbox_pids_max(),
    };

    // Enforce capability requirements based on enforcement mode.
    let capability = csa_resource::detect_sandbox_capability();
    match enforcement {
        csa_config::EnforcementMode::Required => {
            if capability == csa_resource::SandboxCapability::None {
                return SandboxResolution::RequiredButUnavailable(
                    "Sandbox required but no capability detected (no cgroup v2 or setrlimit). \
                     Set enforcement_mode = \"off\" or \"best-effort\" to proceed without isolation."
                        .to_string(),
                );
            }
        }
        csa_config::EnforcementMode::BestEffort => {
            if capability == csa_resource::SandboxCapability::None {
                warn!(
                    tool = %tool_name,
                    "Sandbox configured but no capability detected; proceeding without isolation"
                );
            }
        }
        csa_config::EnforcementMode::Off => {} // already filtered above
    }

    info!(
        tool = %tool_name,
        enforcement = ?enforcement,
        capability = %capability,
        memory_max_mb,
        memory_swap_max_mb = ?sandbox_config.memory_swap_max_mb,
        pids_max = ?sandbox_config.pids_max,
        "Sandbox configuration resolved"
    );

    execute_options = execute_options.with_sandbox(SandboxContext {
        config: sandbox_config,
        tool_name: tool_name.to_string(),
        session_id: session_id.to_string(),
        best_effort: matches!(enforcement, csa_config::EnforcementMode::BestEffort),
    });

    SandboxResolution::Ok(execute_options)
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
    let available_swap = sys.free_swap();

    if !should_enable_balloon(available_swap, BALLOON_SIZE as u64) {
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

    let capability = csa_resource::detect_sandbox_capability();
    let mode = match capability {
        csa_resource::SandboxCapability::CgroupV2 => "cgroup",
        csa_resource::SandboxCapability::Setrlimit => "rlimit",
        csa_resource::SandboxCapability::None => "none",
    };
    let memory = execute_options
        .sandbox
        .as_ref()
        .map(|s| s.config.memory_max_mb);

    session.sandbox_info = Some(csa_session::SandboxInfo {
        mode: mode.to_string(),
        memory_max_mb: memory,
    });

    info!(
        session = %session.meta_session_id,
        sandbox_mode = mode,
        memory_max_mb = ?memory,
        "Sandbox telemetry recorded in session state"
    );
}

#[cfg(test)]
#[path = "pipeline_sandbox_tests.rs"]
mod tests;
