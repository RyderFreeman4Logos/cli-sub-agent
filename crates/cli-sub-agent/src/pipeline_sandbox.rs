//! Sandbox resolution and telemetry for the execution pipeline.
//!
//! Handles enforcement mode checking, capability detection, config resolution,
//! and first-turn telemetry recording.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use csa_config::ProjectConfig;
use csa_executor::{ExecuteOptions, SandboxContext};
use csa_process::StreamMode;
use csa_resource::isolation_plan::{
    EnforcementMode as ResourceEnforcementMode, IsolationPlan, IsolationPlanBuilder,
};
use csa_session::MetaSessionState;
use serde::Serialize;
use tracing::{info, warn};

use crate::run_resource_overrides::RunResourceOverrides;

#[cfg(test)]
use crate::pipeline::{
    CleanRoomSandboxInput, resolve_clean_room_sandbox_options_with_capabilities,
};

#[path = "pipeline_sandbox_memory_balloon.rs"]
mod memory_balloon;
#[path = "pipeline_sandbox_memory_override.rs"]
mod memory_override;
#[path = "pipeline_sandbox_writable.rs"]
mod writable_sources;
use writable_sources::add_execution_env_writable_paths;

pub(crate) use memory_balloon::maybe_inflate_balloon;
#[cfg(test)]
pub(crate) use memory_balloon::should_skip_balloon_prewarm;

/// Outcome of sandbox resolution — either enriched options or a fatal error string
/// (for `Required` mode with no capability).
pub(crate) enum SandboxResolution {
    /// Options ready (may or may not contain sandbox context).
    Ok(Box<ExecuteOptions>),
    /// Sandbox is required but no capability was detected; caller must bail.
    RequiredButUnavailable(String),
}

/// Sandbox resolution inputs for one session spawn.
pub(crate) struct SandboxResolveInput<'a> {
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) tool_name: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) project_root: &'a Path,
    pub(crate) stream_mode: StreamMode,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) liveness_dead_seconds: u64,
    pub(crate) initial_response_timeout_seconds: Option<u64>,
    pub(crate) no_fs_sandbox: bool,
    pub(crate) allow_user_daemon_ipc: bool,
    pub(crate) readonly_project_root: bool,
    pub(crate) extra_writable: &'a [PathBuf],
    pub(crate) extra_readable: &'a [PathBuf],
    pub(crate) execution_env: Option<&'a HashMap<String, String>>,
}

fn resolve_session_dir_for_sandbox(project_root: &Path, session_id: &str) -> PathBuf {
    csa_session::manager::get_session_dir(project_root, session_id).unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("cli-sub-agent")
            .join("sessions")
            .join(session_id)
    })
}

pub(crate) fn validate_run_extra_writable_sources_exist(
    config: Option<&ProjectConfig>,
    project_root: &Path,
    no_fs_sandbox: bool,
    extra_writable: &[PathBuf],
) -> Result<(), String> {
    if no_fs_sandbox {
        return Ok(());
    }
    if !extra_writable.is_empty() {
        writable_sources::resolve_and_prepare_writable_sources(
            extra_writable,
            project_root,
            "--extra-writable",
        )?;
    }
    if let Some(cfg) = config
        && !cfg.filesystem_sandbox.extra_writable.is_empty()
    {
        writable_sources::resolve_config_extra_writable_sources(cfg, project_root)?;
    }
    Ok(())
}

/// Resolve sandbox configuration from project config and enforcement mode.
///
/// Returns `SandboxResolution::Ok` with the options (possibly enriched with
/// `SandboxContext`) or `SandboxResolution::RequiredButUnavailable` when
/// enforcement is `Required` but the host lacks both cgroup v2 and setrlimit.
///
/// When `no_fs_sandbox` is `true`, filesystem isolation is forcibly disabled
/// regardless of config (equivalent to `enforcement_mode = "off"` for FS only).
///
/// When `readonly_project_root` is `true`, the project root is mounted read-only
/// via bwrap `--ro-bind` instead of `--bind`. Used by review/debate to prevent
/// the tool from modifying project files.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_sandbox_options(
    config: Option<&ProjectConfig>,
    tool_name: &str,
    session_id: &str,
    project_root: &Path,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    liveness_dead_seconds: u64,
    initial_response_timeout_seconds: Option<u64>,
    no_fs_sandbox: bool,
    readonly_project_root: bool,
    extra_writable: &[PathBuf],
    extra_readable: &[PathBuf],
) -> SandboxResolution {
    resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config,
            tool_name,
            session_id,
            project_root,
            stream_mode,
            idle_timeout_seconds,
            liveness_dead_seconds,
            initial_response_timeout_seconds,
            no_fs_sandbox,
            allow_user_daemon_ipc: false,
            readonly_project_root,
            extra_writable,
            extra_readable,
            execution_env: None,
        },
        RunResourceOverrides::absent(),
    )
}

pub(crate) fn resolve_sandbox_options_with_overrides(
    input: SandboxResolveInput<'_>,
    resource_overrides: RunResourceOverrides,
) -> SandboxResolution {
    let SandboxResolveInput {
        config,
        tool_name,
        session_id,
        project_root,
        stream_mode,
        idle_timeout_seconds,
        liveness_dead_seconds,
        initial_response_timeout_seconds,
        no_fs_sandbox,
        allow_user_daemon_ipc,
        readonly_project_root,
        extra_writable,
        extra_readable,
        execution_env,
    } = input;
    let has_run_memory_override = resource_overrides.has_memory_max_override();

    let default_resources = csa_config::ResourcesConfig::default();
    let stdin_write_timeout_seconds = config
        .map(|cfg| cfg.resources.stdin_write_timeout_seconds)
        .unwrap_or(default_resources.stdin_write_timeout_seconds);
    let acp_init_timeout_seconds = config
        .map(|cfg| cfg.acp.init_timeout_seconds)
        .unwrap_or(csa_config::AcpConfig::default().init_timeout_seconds);
    let acp_crash_max_attempts = config.map_or_else(
        || csa_config::ExecutionConfig::default().resolved_acp_crash_max_attempts(),
        |cfg| cfg.execution.resolved_acp_crash_max_attempts(),
    );
    let termination_grace_period_seconds = config
        .map(|cfg| cfg.resources.termination_grace_period_seconds)
        .unwrap_or(default_resources.termination_grace_period_seconds);
    let mut execute_options = ExecuteOptions::new(stream_mode, idle_timeout_seconds)
        .with_acp_crash_max_attempts(acp_crash_max_attempts)
        .with_liveness_dead_seconds(liveness_dead_seconds)
        .with_stdin_write_timeout_seconds(stdin_write_timeout_seconds)
        .with_acp_init_timeout_seconds(acp_init_timeout_seconds)
        .with_termination_grace_period_seconds(termination_grace_period_seconds)
        .with_initial_response_timeout_seconds(initial_response_timeout_seconds);

    let Some(cfg) = config else {
        // No project config — apply profile-based defaults for heavyweight tools.
        let defaults = csa_config::default_sandbox_for_tool(tool_name);
        execute_options = execute_options.with_setting_sources(defaults.setting_sources);

        if memory_override::default_off_allows_unsandboxed(
            defaults.enforcement,
            has_run_memory_override,
        ) {
            return SandboxResolution::Ok(Box::new(execute_options));
        }

        let Some(memory_max_mb) = resource_overrides.resolve_memory_max_mb(None, tool_name) else {
            return SandboxResolution::Ok(Box::new(execute_options));
        };

        let resource_cap = csa_resource::detect_resource_capability();
        let fs_cap = if no_fs_sandbox {
            csa_resource::FilesystemCapability::None
        } else {
            csa_resource::detect_filesystem_capability()
        };
        if let Some(message) = memory_override::capability_error_if_unenforced(
            tool_name,
            has_run_memory_override,
            resource_cap,
        ) {
            return SandboxResolution::RequiredButUnavailable(message);
        }
        if matches!(resource_cap, csa_resource::ResourceCapability::None) {
            warn!(
                tool = tool_name,
                "No sandbox capability available; skipping enforcement for profile defaults"
            );
            return SandboxResolution::Ok(Box::new(execute_options));
        }

        // Build IsolationPlan via builder (BestEffort for profile defaults).
        let session_dir = resolve_session_dir_for_sandbox(project_root, session_id);
        let tool_state_dirs = csa_config::default_tool_state_dirs();
        let mut builder = IsolationPlanBuilder::new(ResourceEnforcementMode::BestEffort)
            .with_resource_capability(resource_cap)
            .with_filesystem_capability(fs_cap)
            .with_resource_limits(
                Some(memory_max_mb),
                defaults.memory_swap_max_mb,
                None, // pids_max not available from profile defaults
            )
            .with_tool_defaults_and_state_dirs(
                tool_name,
                project_root,
                &session_dir,
                Some(&tool_state_dirs),
            )
            .with_readonly_project_root(readonly_project_root);
        if allow_user_daemon_ipc {
            builder = builder.with_user_daemon_ipc();
        }

        // CSA runtime writable paths.
        if !no_fs_sandbox {
            builder = match add_execution_env_writable_paths(builder, execution_env, project_root) {
                Ok(builder) => builder,
                Err(message) => return SandboxResolution::RequiredButUnavailable(message),
            };
            if let Ok(project_state_root) = csa_session::manager::get_session_root(project_root) {
                builder = builder.with_writable_path(project_state_root);
            }
            if let Ok(slots) = csa_config::GlobalConfig::slots_dir() {
                builder = builder.with_writable_path(slots);
            }
            // CLI --extra-writable / --expose-readable (no-config path).
            if !extra_writable.is_empty() {
                let resolved = match writable_sources::resolve_and_prepare_writable_sources(
                    extra_writable,
                    project_root,
                    "--extra-writable",
                ) {
                    Ok(paths) => paths,
                    Err(message) => {
                        return SandboxResolution::RequiredButUnavailable(message);
                    }
                };
                for path in resolved {
                    builder = builder.with_writable_path(path);
                }
            }
            if !extra_readable.is_empty() {
                if let Err(e) = csa_resource::isolation_plan::validate_readable_paths(
                    extra_readable,
                    project_root,
                ) {
                    return SandboxResolution::RequiredButUnavailable(format!(
                        "--expose-readable validation failed: {e}"
                    ));
                }
                for path in extra_readable {
                    builder = builder.with_readable_path(path.clone());
                }
            }
        }

        let plan = builder
            .build()
            .expect("BestEffort IsolationPlan should never fail");
        if let Some(message) =
            memory_override::plan_error_if_unenforced(tool_name, has_run_memory_override, &plan)
        {
            return SandboxResolution::RequiredButUnavailable(message);
        }
        if allow_user_daemon_ipc
            && let Err(message) = write_user_daemon_ipc_audit_artifact(&session_dir, &plan)
        {
            return SandboxResolution::RequiredButUnavailable(message);
        }

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
    let enforcement = match memory_override::resolve_config_enforcement(
        cfg,
        tool_name,
        has_run_memory_override,
    ) {
        Ok(Some(enforcement)) => enforcement,
        Ok(None) => {
            return SandboxResolution::Ok(Box::new(execute_options));
        }
        Err(message) => return SandboxResolution::RequiredButUnavailable(message),
    };

    let Some(memory_max_mb) = resource_overrides.resolve_memory_max_mb(Some(cfg), tool_name) else {
        if matches!(enforcement, csa_config::EnforcementMode::Required) {
            return SandboxResolution::RequiredButUnavailable(format!(
                "Sandbox enforcement is required for tool '{tool_name}' but no memory_max_mb is configured. \
                 Set --memory-max-mb, resources.memory_max_mb, or tools.{tool_name}.memory_max_mb."
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
    if let Some(message) = memory_override::capability_error_if_unenforced(
        tool_name,
        has_run_memory_override,
        resource_cap,
    ) {
        return SandboxResolution::RequiredButUnavailable(message);
    }

    // Resolve filesystem enforcement independently from resource enforcement.
    // tool_fs_enforcement_mode already handles the full priority chain:
    //   tool-level > safety-net auto-promote > global [filesystem_sandbox].
    let fs_enforcement = if no_fs_sandbox {
        ResourceEnforcementMode::Off
    } else {
        let effective_mode = cfg
            .tool_fs_enforcement_mode(tool_name)
            .unwrap_or_else(|| "best-effort".to_string());
        match effective_mode.as_str() {
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
    let session_dir = resolve_session_dir_for_sandbox(project_root, session_id);

    let memory_swap_max_mb = cfg.sandbox_memory_swap_max_mb(tool_name);
    let pids_max = cfg.sandbox_pids_max();

    // Per-tool filesystem sandbox: check for REPLACE-semantics writable paths.
    let per_tool_writable = if !no_fs_sandbox {
        match writable_sources::resolve_per_tool_writable_sources(cfg, tool_name, project_root) {
            Ok(paths) => paths,
            Err(message) => {
                return SandboxResolution::RequiredButUnavailable(message);
            }
        }
    } else {
        None
    };
    let per_tool_readable = if !no_fs_sandbox {
        cfg.sandbox_readable_paths(tool_name)
    } else {
        None
    };

    // When per-tool writable paths are set, project root becomes read-only
    // (the per-tool paths provide fine-grained write access instead).
    let effective_readonly = readonly_project_root || per_tool_writable.is_some();

    let mut builder = IsolationPlanBuilder::new(resource_enforcement)
        .with_filesystem_enforcement(fs_enforcement)
        .with_resource_capability(resource_cap)
        .with_filesystem_capability(fs_cap)
        .with_resource_limits(Some(memory_max_mb), memory_swap_max_mb, pids_max)
        .with_tool_defaults_and_state_dirs(
            tool_name,
            project_root,
            &session_dir,
            Some(&cfg.tool_state_dirs),
        )
        .with_readonly_project_root(effective_readonly)
        .with_soft_limit_percent(cfg.resources.soft_limit_percent)
        .with_memory_monitor_interval(cfg.resources.memory_monitor_interval_seconds);
    if allow_user_daemon_ipc {
        builder = builder.with_user_daemon_ipc();
    }

    // CSA runtime paths must survive per-tool REPLACE semantics so fork-call
    // session creation and slot locks still work.
    if !no_fs_sandbox {
        builder = match add_execution_env_writable_paths(builder, execution_env, project_root) {
            Ok(builder) => builder,
            Err(message) => return SandboxResolution::RequiredButUnavailable(message),
        };
        if let Ok(project_state_root) = csa_session::manager::get_session_root(project_root) {
            builder = builder.with_writable_path(project_state_root);
        }
        if let Ok(slots) = csa_config::GlobalConfig::slots_dir() {
            builder = builder.with_writable_path(slots);
        }
    }

    if !no_fs_sandbox {
        if let Some(ref paths) = per_tool_writable {
            for path in paths {
                builder = builder.with_writable_path(path.clone());
            }
        } else {
            // No per-tool override — apply global extra_writable paths.
            if !cfg.filesystem_sandbox.extra_writable.is_empty() {
                let resolved = match writable_sources::resolve_config_extra_writable_sources(
                    cfg,
                    project_root,
                ) {
                    Ok(paths) => paths,
                    Err(message) => {
                        return SandboxResolution::RequiredButUnavailable(message);
                    }
                };
                for path in resolved {
                    builder = builder.with_writable_path(path);
                }
            }
        }

        if let Some(ref paths) = per_tool_readable {
            if let Err(e) =
                csa_resource::isolation_plan::validate_readable_paths(paths, project_root)
            {
                return SandboxResolution::RequiredButUnavailable(format!(
                    "Per-tool readable_paths validation failed for '{tool_name}': {e}"
                ));
            }
            for path in paths {
                builder = builder.with_readable_path(path.clone());
            }
        }
    }

    // CLI --extra-writable paths: always appended (APPEND semantics, not REPLACE).
    if !no_fs_sandbox && !extra_writable.is_empty() {
        let resolved = match writable_sources::resolve_and_prepare_writable_sources(
            extra_writable,
            project_root,
            "--extra-writable",
        ) {
            Ok(paths) => paths,
            Err(message) => {
                return SandboxResolution::RequiredButUnavailable(message);
            }
        };
        for path in resolved {
            builder = builder.with_writable_path(path);
        }
    }

    // CLI --expose-readable paths: always appended after config resolution.
    if !no_fs_sandbox && !extra_readable.is_empty() {
        if let Err(e) =
            csa_resource::isolation_plan::validate_readable_paths(extra_readable, project_root)
        {
            return SandboxResolution::RequiredButUnavailable(format!(
                "--expose-readable validation failed: {e}"
            ));
        }
        for path in extra_readable {
            builder = builder.with_readable_path(path.clone());
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
    if let Some(message) =
        memory_override::plan_error_if_unenforced(tool_name, has_run_memory_override, &plan)
    {
        return SandboxResolution::RequiredButUnavailable(message);
    }
    if allow_user_daemon_ipc
        && let Err(message) = write_user_daemon_ipc_audit_artifact(&session_dir, &plan)
    {
        return SandboxResolution::RequiredButUnavailable(message);
    }

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

include!("pipeline_sandbox_telemetry.rs");
#[cfg(test)]
#[path = "pipeline_sandbox_writable_tests.rs"]
mod writable_tests;

#[cfg(test)]
#[path = "pipeline_sandbox_cargo_target_tests.rs"]
mod cargo_target_tests;

#[cfg(test)]
#[path = "pipeline_sandbox_memory_override_tests.rs"]
mod memory_override_tests;

#[cfg(test)]
#[path = "pipeline_sandbox_tests.rs"]
mod tests;
