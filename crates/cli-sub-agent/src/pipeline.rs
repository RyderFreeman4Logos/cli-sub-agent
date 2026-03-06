//! Shared execution pipeline functions for CSA command handlers.
//!
//! This module extracts common patterns from run, review, and debate handlers:
//! - Config loading and recursion depth validation
//! - Executor building and tool installation checks
//! - Global slot acquisition with concurrency limits
//!
//! Session-bound execution lives in [`session_exec`]; result.toml contract
//! enforcement lives in [`result_contract`].

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::{error, warn};

use csa_config::{GlobalConfig, McpRegistry, ProjectConfig};
use csa_core::types::ToolName;
use csa_executor::{AcpMcpServerConfig, Executor};
use csa_hooks::{HookEvent, run_hooks_for_event};
use csa_process::{ExecutionResult, check_tool_installed};

#[path = "pipeline_prompt_guard.rs"]
pub(crate) mod prompt_guard;

#[path = "pipeline_result_contract.rs"]
mod result_contract;

#[path = "pipeline_session_exec.rs"]
mod session_exec;

// Re-export session execution API so callers keep using `crate::pipeline::*`.
pub(crate) use session_exec::{
    execute_with_session, execute_with_session_and_meta,
    execute_with_session_and_meta_with_parent_source,
};

pub(crate) const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 120;
pub(crate) const DEFAULT_LIVENESS_DEAD_SECONDS: u64 = csa_process::DEFAULT_LIVENESS_DEAD_SECS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentSessionSource {
    /// Use explicit `parent` argument when provided, otherwise fall back to
    /// inherited `CSA_SESSION_ID` from environment.
    ExplicitOrEnv,
    /// Only use explicit `parent` argument; never inherit `CSA_SESSION_ID`.
    ExplicitOnly,
}

pub(crate) fn resolve_idle_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_override: Option<u64>,
) -> u64 {
    cli_override
        .or_else(|| config.map(|cfg| cfg.resources.idle_timeout_seconds))
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECONDS)
}

pub(crate) fn resolve_liveness_dead_seconds(config: Option<&ProjectConfig>) -> u64 {
    config
        .and_then(|cfg| cfg.resources.liveness_dead_seconds)
        .unwrap_or(DEFAULT_LIVENESS_DEAD_SECONDS)
}

pub(crate) fn context_load_options_with_skips(
    skip_files: &[String],
) -> Option<csa_executor::ContextLoadOptions> {
    if skip_files.is_empty() {
        None
    } else {
        Some(csa_executor::ContextLoadOptions {
            skip_files: skip_files.to_vec(),
            ..Default::default()
        })
    }
}

/// Load ProjectConfig and GlobalConfig, validate recursion depth.
///
/// Returns `Some((project_config, global_config))` on success.
/// Returns `Ok(None)` if recursion depth exceeded (caller should exit with code 1).
/// Returns `Err` for config loading/parsing failures (caller should propagate).
pub(crate) fn load_and_validate(
    project_root: &Path,
    current_depth: u32,
) -> Result<Option<(Option<ProjectConfig>, GlobalConfig)>> {
    let config = ProjectConfig::load(project_root)?;

    let max_depth = config
        .as_ref()
        .map(|c| c.project.max_recursion_depth)
        .unwrap_or(5u32);

    if current_depth > max_depth {
        error!(
            "Max recursion depth ({}) exceeded. Current: {}. Do it yourself.",
            max_depth, current_depth
        );
        return Ok(None);
    }

    let global_config = GlobalConfig::load()?;
    Ok(Some((config, global_config)))
}

/// Load and merge MCP server registries from global + project config.
///
/// Returns a merged list of [`AcpMcpServerConfig`] ready for transport injection.
/// Global servers are included unless overridden by a project server with the same name.
pub(crate) fn resolve_mcp_servers(
    project_root: &Path,
    global_config: &GlobalConfig,
) -> Vec<AcpMcpServerConfig> {
    let global_servers = global_config.mcp_servers();

    let project_registry = match McpRegistry::load(project_root) {
        Ok(Some(registry)) => registry,
        Ok(None) => {
            // No project MCP config; use global servers only
            return global_servers
                .iter()
                .filter_map(config_to_acp_mcp)
                .collect();
        }
        Err(e) => {
            warn!("Failed to load project MCP registry: {e}");
            return global_servers
                .iter()
                .filter_map(config_to_acp_mcp)
                .collect();
        }
    };

    let merged = McpRegistry::merge(global_servers, &project_registry);
    merged
        .servers
        .iter()
        .filter_map(config_to_acp_mcp)
        .collect()
}

/// Convert `csa_config::McpServerConfig` to [`AcpMcpServerConfig`].
///
/// Only stdio transport servers can be injected into ACP sessions (tools
/// launch subprocesses directly). Remote transport servers are filtered out.
fn config_to_acp_mcp(cfg: &csa_config::McpServerConfig) -> Option<AcpMcpServerConfig> {
    match &cfg.transport {
        csa_config::McpTransport::Stdio {
            command, args, env, ..
        } => Some(AcpMcpServerConfig {
            name: cfg.name.clone(),
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
        }),
        _ => None,
    }
}

/// References to project and global config for executor building.
pub(crate) struct ConfigRefs<'a> {
    pub project: Option<&'a ProjectConfig>,
    pub global: Option<&'a GlobalConfig>,
}

/// Build executor and validate tool is installed and enabled.
///
/// Returns Executor on success.
/// Returns error if tool not installed or disabled in config.
///
/// When `enforce_tier` is `false`, tier whitelist and model-name checks are
/// skipped. Review and debate commands use this because they select tools for
/// heterogeneous evaluation, not for tier-controlled execution.
///
/// If the tool has `default_model` / `default_thinking` in project config, those
/// fill missing CLI values before falling back to the executor's internal defaults.
///
/// If the tool has a `thinking_lock` in project or global config, the locked
/// value silently overrides the effective thinking budget.
pub(crate) async fn build_and_validate_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking_budget: Option<&str>,
    configs: ConfigRefs<'_>,
    enforce_tier: bool,
    force_override_user_config: bool,
) -> Result<Executor> {
    let mut executor = crate::run_helpers::build_executor(
        tool,
        model_spec,
        model,
        thinking_budget,
        configs.project,
    )?;

    // Apply thinking lock: project config takes precedence over global.
    // When set, silently overrides the effective thinking budget (including
    // any project default or the one embedded in --model-spec).
    let tool_str = tool.as_str();
    let default_model_resolved: Option<String> = if model_spec.is_none() {
        configs.project.and_then(|cfg| {
            cfg.tool_default_model(tool_str)
                .map(|m| cfg.resolve_alias(m))
        })
    } else {
        None
    };
    let default_thinking_from_project = model_spec.is_none().then(|| {
        configs
            .project
            .and_then(|cfg| cfg.tool_default_thinking(tool_str))
    });
    let lock_from_project = configs.project.and_then(|c| c.thinking_lock(tool_str));
    let lock_from_global = configs.global.and_then(|g| g.thinking_lock(tool_str));
    if let Some(lock_str) = lock_from_project.or(lock_from_global) {
        let locked_budget = csa_executor::ThinkingBudget::parse(lock_str)?;
        executor.override_thinking_budget(locked_budget);
    }

    // Defense-in-depth: enforce tool enablement from user config
    if let Some(cfg) = configs.project {
        cfg.enforce_tool_enabled(executor.tool_name(), force_override_user_config)?;

        if enforce_tier {
            // Defense-in-depth: enforce tier whitelist at execution boundary
            cfg.enforce_tier_whitelist(executor.tool_name(), model_spec)?;
            let effective_model = model.or(default_model_resolved.as_deref());
            cfg.enforce_tier_model_name(executor.tool_name(), effective_model)?;
        }

        // Enforce thinking level is configured in tiers (unless force override).
        // Use the effective thinking level (after thinking_lock override), not the
        // original CLI value, to avoid rejecting locked values that differ from CLI.
        let effective_thinking = lock_from_project
            .or(lock_from_global)
            .or(thinking_budget)
            .or(default_thinking_from_project.flatten());
        if enforce_tier && !force_override_user_config {
            cfg.enforce_thinking_level(effective_thinking)?;
        }
    }

    // Check tool is installed
    if let Err(e) = check_tool_installed(executor.runtime_binary_name()).await {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{}", e);
    }
    ensure_tool_runtime_prerequisites(executor.tool_name()).await?;

    Ok(executor)
}

async fn ensure_tool_runtime_prerequisites(tool_name: &str) -> Result<()> {
    if tool_name != "codex" {
        return Ok(());
    }
    if std::env::var("CSA_SKIP_BWRAP_PREFLIGHT").ok().as_deref() == Some("1") {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let has_bwrap = tokio::process::Command::new("which")
            .arg("bwrap")
            .output()
            .await
            .map(|out| out.status.success())
            .unwrap_or(false);
        if !has_bwrap {
            anyhow::bail!(
                "codex preflight failed: required runtime dependency 'bwrap' (bubblewrap) is missing.\n\
                 Install bubblewrap first, then re-run the command."
            );
        }
    }

    Ok(())
}

/// Acquire global concurrency slot for the executor.
///
/// Returns ToolSlot guard on success.
/// Returns error if all slots occupied (no failover here).
#[tracing::instrument(skip_all, fields(tool = %executor.tool_name()))]
pub(crate) fn acquire_slot(
    executor: &Executor,
    global_config: &GlobalConfig,
) -> Result<csa_lock::slot::ToolSlot> {
    let max_concurrent = global_config.max_concurrent(executor.tool_name());
    let slots_dir = GlobalConfig::slots_dir()?;

    match csa_lock::slot::try_acquire_slot(&slots_dir, executor.tool_name(), max_concurrent, None) {
        Ok(csa_lock::slot::SlotAcquireResult::Acquired(slot)) => Ok(slot),
        Ok(csa_lock::slot::SlotAcquireResult::Exhausted(status)) => {
            anyhow::bail!(
                "All {} slots for '{}' occupied ({}/{}). Try again later or use --tool to switch.",
                max_concurrent,
                executor.tool_name(),
                status.occupied,
                status.max_slots,
            )
        }
        Err(e) => anyhow::bail!(
            "Slot acquisition failed for '{}': {}",
            executor.tool_name(),
            e
        ),
    }
}

/// Execution result with the resolved CSA meta session ID used by this run.
pub(crate) struct SessionExecutionResult {
    pub execution: ExecutionResult,
    pub meta_session_id: String,
    pub provider_session_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryInjectionOptions {
    pub disabled: bool,
    pub query_override: Option<String>,
}

pub(crate) fn run_pipeline_hook(
    event: HookEvent,
    hooks_config: &csa_hooks::HooksConfig,
    variables: &std::collections::HashMap<String, String>,
) -> Result<()> {
    run_hooks_for_event(event, hooks_config, variables).map_err(|err| {
        anyhow::anyhow!("{event:?} hook failed and fail_policy=closed blocked execution: {err}")
    })
}

pub(crate) fn determine_project_root(cd: Option<&str>) -> Result<PathBuf> {
    let path = if let Some(cd_path) = cd {
        PathBuf::from(cd_path)
    } else {
        std::env::current_dir()?
    };

    Ok(path.canonicalize()?)
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_tests_thinking.rs"]
mod thinking_tests;

#[cfg(test)]
#[path = "pipeline_tests_prompt_guard.rs"]
mod prompt_guard_tests;
