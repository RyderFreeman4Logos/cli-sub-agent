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

#[path = "pipeline_gate.rs"]
pub(crate) mod gate;

#[path = "pipeline_prompt_guard.rs"]
pub(crate) mod prompt_guard;

#[path = "pipeline_prompt_cache.rs"]
mod prompt_cache;

#[path = "pipeline_changed_paths.rs"]
pub(crate) mod changed_paths;

#[path = "pipeline_result_contract.rs"]
pub(crate) mod result_contract;

#[path = "pipeline_design_context.rs"]
pub(crate) mod design_context;

#[path = "pipeline_plan_context.rs"]
pub(crate) mod plan_context;

#[path = "pipeline_session_exec.rs"]
mod session_exec;

#[path = "pipeline_session_exec_failover.rs"]
mod session_exec_failover;

#[path = "pipeline_session_hooks.rs"]
mod session_hooks;

#[path = "pipeline_admitted_executor.rs"]
mod admitted_executor;
pub(crate) use admitted_executor::{AdmittedExecutor, DispatchExecutor};

#[path = "pipeline_catalog_admission.rs"]
mod catalog_admission;
use catalog_admission::validate_final_executor_identity;

#[path = "pipeline_model_failover_session.rs"]
pub(crate) mod model_failover_session;

#[path = "lefthook_auto_install.rs"]
pub(crate) mod lefthook_auto_install;

// Re-export session execution API so callers keep using `crate::pipeline::*`.
#[allow(unused_imports)]
pub(crate) use session_exec::{
    CleanRoomExecutionContract, CleanRoomExecutionLimits, execute_clean_room_session,
    execute_with_session, execute_with_session_and_meta_with_parent_source,
};
#[cfg(test)]
pub(crate) use session_exec::{
    CleanRoomSandboxInput, clean_room_execution_policy_effects, clean_room_runtime_prompt_for_test,
    execute_with_session_and_meta, resolve_clean_room_sandbox_options_with_capabilities,
};

pub(crate) const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 250;
pub(crate) const DEFAULT_LIVENESS_DEAD_SECONDS: u64 = csa_process::DEFAULT_LIVENESS_DEAD_SECS;
pub(crate) const DEFAULT_RESOURCES_INITIAL_RESPONSE_TIMEOUT_SECONDS: u64 = 120;
pub(crate) use csa_executor::DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS;
pub(crate) const DEFAULT_GEMINI_INITIAL_RESPONSE_TIMEOUT_SECONDS: u64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParentSessionSource {
    /// Use explicit `parent` argument when provided, otherwise fall back to
    /// inherited `CSA_SESSION_ID` from environment.
    ExplicitOrEnv,
    /// Only use explicit `parent` argument; never inherit `CSA_SESSION_ID`.
    ExplicitOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionCreationMode {
    /// Reuse the daemon-preassigned session ID when present. This is the
    /// normal top-level CLI behavior.
    DaemonManaged,
    /// Always allocate a fresh child session ID, even inside a daemon child.
    FreshChild,
}

pub(crate) fn resolve_idle_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_override: Option<u64>,
) -> u64 {
    cli_override
        .or_else(|| config.map(|cfg| cfg.resources.idle_timeout_seconds))
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECONDS)
}

pub(crate) fn promote_idle_timeout_for_explicit_wall_timeout(
    resolved_idle_timeout_seconds: u64,
    cli_idle_timeout: Option<u64>,
    wall_timeout: Option<u64>,
) -> u64 {
    if cli_idle_timeout.is_some() {
        return resolved_idle_timeout_seconds;
    }

    wall_timeout.map_or(resolved_idle_timeout_seconds, |timeout| {
        resolved_idle_timeout_seconds.max(timeout)
    })
}

pub(crate) fn resolve_effective_idle_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_idle_timeout: Option<u64>,
    wall_timeout: Option<u64>,
) -> u64 {
    let resolved_idle = resolve_idle_timeout_seconds(config, cli_idle_timeout);
    promote_idle_timeout_for_explicit_wall_timeout(resolved_idle, cli_idle_timeout, wall_timeout)
}

/// Resolve the initial-response timeout (seconds).
///
/// Priority: CLI override > project config > default (120s).
/// Returns `None` when explicitly disabled (set to 0 in config or CLI).
fn resolve_initial_response_timeout_with_default(
    config: Option<&ProjectConfig>,
    cli_override: Option<u64>,
    default_seconds: u64,
) -> Option<u64> {
    let raw = cli_override
        .or_else(|| config.and_then(|cfg| cfg.resources.initial_response_timeout_seconds));
    csa_executor::resolve_initial_response_timeout(raw, default_seconds)
}

#[cfg(test)]
pub(crate) fn resolve_initial_response_timeout_seconds(
    config: Option<&ProjectConfig>,
    cli_override: Option<u64>,
) -> Option<u64> {
    resolve_initial_response_timeout_with_default(
        config,
        cli_override,
        DEFAULT_RESOURCES_INITIAL_RESPONSE_TIMEOUT_SECONDS,
    )
}

fn per_tool_default(tool_name: &str) -> u64 {
    match tool_name {
        "codex" => DEFAULT_CODEX_INITIAL_RESPONSE_TIMEOUT_SECONDS,
        "gemini-cli" | "antigravity-cli" => DEFAULT_GEMINI_INITIAL_RESPONSE_TIMEOUT_SECONDS,
        _ => DEFAULT_RESOURCES_INITIAL_RESPONSE_TIMEOUT_SECONDS,
    }
}

pub(crate) fn resolve_initial_response_timeout_for_tool(
    config: Option<&ProjectConfig>,
    cli_initial_response_timeout: Option<u64>,
    cli_idle_timeout: Option<u64>,
    tool_name: &str,
) -> Option<u64> {
    if cli_idle_timeout.is_some() && cli_initial_response_timeout.is_none() {
        return None;
    }

    if let Some(cli_override) = cli_initial_response_timeout {
        return csa_executor::resolve_initial_response_timeout(
            Some(cli_override),
            per_tool_default(tool_name),
        );
    }

    let configured = config
        .and_then(|cfg| cfg.tool_initial_response_timeout_seconds(tool_name))
        .or_else(|| config.and_then(|cfg| cfg.resources.initial_response_timeout_seconds));

    resolve_initial_response_timeout_with_default(config, configured, per_tool_default(tool_name))
}

pub(crate) fn resolve_effective_initial_response_timeout_for_tool(
    config: Option<&ProjectConfig>,
    cli_initial_response_timeout: Option<u64>,
    cli_idle_timeout: Option<u64>,
    wall_timeout: Option<u64>,
    tool_name: &str,
) -> Option<u64> {
    let resolved = resolve_initial_response_timeout_for_tool(
        config,
        cli_initial_response_timeout,
        cli_idle_timeout,
        tool_name,
    );

    if cli_initial_response_timeout.is_some() {
        return resolved;
    }

    match (resolved, wall_timeout) {
        (Some(response_timeout), Some(timeout)) => Some(response_timeout.max(timeout)),
        _ => resolved,
    }
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

/// Immutable configuration snapshot used by command handlers after recursion-depth validation.
pub(crate) type LoadedConfig = (
    Option<ProjectConfig>,
    GlobalConfig,
    csa_config::EffectiveModelCatalog,
    Option<csa_config::ProjectConvergenceCompletionPolicy>,
);

/// Load configuration and validate recursion depth.
///
/// Returns [`LoadedConfig`] on success.
/// Returns `Ok(None)` if recursion depth exceeded (caller should exit with code 1).
/// Returns `Err` for config loading/parsing failures (caller should propagate).
pub(crate) fn load_and_validate(
    project_root: &Path,
    current_depth: u32,
) -> Result<Option<LoadedConfig>> {
    let effective = csa_config::EffectiveConfig::load(project_root)?;
    let config = effective.project;
    let global_config = effective.global;
    let project_completion_policy = effective.project_convergence_completion;

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

    Ok(Some((
        config,
        global_config,
        effective.model_catalog,
        project_completion_policy,
    )))
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
#[derive(Clone, Copy)]
pub(crate) struct ConfigRefs<'a> {
    pub project: Option<&'a ProjectConfig>,
    pub global: Option<&'a GlobalConfig>,
    pub model_catalog: Option<&'a csa_config::EffectiveModelCatalog>,
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
/// When `apply_tool_defaults` is `true`, `default_model` / `default_thinking`
/// from project config fill missing CLI values before falling back to the
/// executor's internal defaults.
///
/// If the tool has a `thinking_lock` in project or global config, the locked
/// value silently overrides the effective thinking budget.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_and_validate_executor(
    tool: &ToolName,
    model_spec: Option<&str>,
    model: Option<&str>,
    thinking_budget: Option<&str>,
    configs: ConfigRefs<'_>,
    enforce_tier: bool,
    force_override_user_config: bool,
    apply_tool_defaults: bool,
) -> Result<AdmittedExecutor> {
    let shipped_catalog;
    let model_catalog = if let Some(catalog) = configs.model_catalog {
        catalog
    } else {
        shipped_catalog = csa_config::EffectiveModelCatalog::shipped()?;
        &shipped_catalog
    };
    let mut executor = crate::run_helpers::build_executor(
        tool,
        model_spec,
        model,
        thinking_budget,
        configs.project,
        apply_tool_defaults,
    )?;

    // Apply thinking lock: project config takes precedence over global.
    // When set, silently overrides the effective thinking budget (including
    // any project default or the one embedded in --model-spec).
    let tool_str = tool.as_str();
    let default_model_resolved: Option<String> = if apply_tool_defaults && model_spec.is_none() {
        configs.project.and_then(|cfg| {
            cfg.tool_default_model(tool_str)
                .map(|m| cfg.resolve_alias(m))
        })
    } else {
        None
    };
    let default_thinking_from_project = (apply_tool_defaults && model_spec.is_none()).then(|| {
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
            let effective_model = crate::run_helpers::model_name_for_tier_validation(
                model.or(default_model_resolved.as_deref()),
            );
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

    let final_model_request = model.map(|requested| {
        configs
            .project
            .map(|cfg| cfg.resolve_alias(requested))
            .unwrap_or_else(|| requested.to_string())
    });
    let final_model_request = final_model_request
        .as_deref()
        .or(default_model_resolved.as_deref());
    let validated_identity = validate_final_executor_identity(
        &executor,
        model_spec,
        final_model_request,
        model_catalog,
    )?;

    #[cfg(test)]
    let assume_tool_binaries_available =
        crate::run_helpers::assume_tool_binaries_available_for_tests();
    #[cfg(not(test))]
    let assume_tool_binaries_available = false;

    if executor.tool_name() == "openai-compat" {
        let model_hint = model_spec.or(model).or(default_model_resolved.as_deref());
        let extra_env = configs.global.and_then(|cfg| {
            cfg.build_execution_env(
                executor.tool_name(),
                csa_config::ExecutionEnvOptions::default(),
            )
        });
        let availability = crate::run_helpers::tool_runtime_availability_with_env(
            executor.tool_name(),
            configs.project,
            model_hint,
            extra_env.as_ref(),
        );
        if let crate::run_helpers::ToolBinaryAvailability::Missing { hint, .. } = availability {
            anyhow::bail!("OpenAI-compat is not configured.\n\n{hint}");
        }
    } else if !assume_tool_binaries_available
        && let Err(e) = check_tool_installed(executor.runtime_binary_name()).await
    {
        error!(
            "Tool '{}' is not installed.\n\n{}\n\nOr disable it in .csa/config.toml:\n  [tools.{}]\n  enabled = false",
            executor.tool_name(),
            executor.install_hint(),
            executor.tool_name()
        );
        anyhow::bail!("{e}");
    }
    Ok(AdmittedExecutor::new(
        executor,
        validated_identity.resolved_model_spec,
        validated_identity.catalog_admission,
    ))
}

async fn ensure_tool_runtime_prerequisites(
    tool_name: &str,
    filesystem_capability: csa_resource::FilesystemCapability,
) -> Result<()> {
    if tool_name != "codex" {
        return Ok(());
    }
    if !matches!(
        filesystem_capability,
        csa_resource::FilesystemCapability::Bwrap
    ) {
        return Ok(());
    }
    if std::env::var("CSA_SKIP_BWRAP_PREFLIGHT").ok().as_deref() == Some("1") {
        return Ok(());
    }

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

    Ok(())
}

fn resolved_filesystem_capability(
    execute_options: &csa_executor::ExecuteOptions,
) -> csa_resource::FilesystemCapability {
    execute_options
        .sandbox
        .as_ref()
        .map_or(csa_resource::FilesystemCapability::None, |ctx| {
            ctx.isolation_plan.filesystem
        })
}

/// Canonical primary-failure / status reason for pre-provider slot capacity exhaustion.
pub(crate) const SLOT_UNAVAILABLE_REASON: &str = "slot_unavailable";

/// Acquire global concurrency slot for the executor.
///
/// Returns ToolSlot guard on success.
/// Returns error if all slots occupied (no failover here).
///
/// Recovery guidance stays generic: callers may pin a tool with
/// `--no-failover` / force-ignore, so the message must not imply that
/// switching tools is always valid (#2718).
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
                "All {} slots for '{}' occupied ({}/{}). Retry later, free slots with `csa gc`, or wait for an in-flight session to finish.",
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

/// Detect local tool-slot capacity exhaustion in error text (pre-provider).
pub(crate) fn is_slot_unavailable_error_text(error_text: &str) -> bool {
    let lower = error_text.to_ascii_lowercase();
    if lower.contains(SLOT_UNAVAILABLE_REASON) {
        return true;
    }
    let mentions_slots = lower.contains("slots for") || lower.contains("slot");
    let exhausted =
        lower.contains("occupied") || lower.contains("exhaust") || lower.contains("unavailable");
    mentions_slots && exhausted && (lower.contains("all ") || lower.contains("slot_unavailable"))
}

/// Execution result with the resolved CSA meta session ID used by this run.
#[derive(Debug)]
pub(crate) struct SessionExecutionResult {
    pub execution: ExecutionResult,
    pub meta_session_id: String,
    pub provider_session_id: Option<String>,
    /// Paths changed by this session when git snapshots were available.
    pub changed_paths: Option<Vec<String>>,
    /// Whether post-run commit policy observed HEAD advancing for this run.
    pub commit_created: Option<bool>,
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
    if let Err(err) = run_hooks_for_event(event, hooks_config, variables) {
        if event.is_gatekeeping() {
            return Err(anyhow::anyhow!(
                "{event:?} hook failed and fail_policy=closed blocked execution: {err}"
            ));
        }
        tracing::warn!("{event:?} hook failed (observational, continuing): {err}");
    }
    Ok(())
}

fn load_runtime_hooks(project_root: &std::path::Path) -> csa_hooks::HooksConfig {
    csa_hooks::load_hooks_config(
        csa_session::get_session_root(project_root)
            .ok()
            .map(|r| r.join("hooks.toml"))
            .as_deref(),
        csa_hooks::global_hooks_path().as_deref(),
        None,
    )
}

fn hook_variables(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Capture stdout from an observational hook and return it to the caller.
///
/// This is used for hooks whose output is part of the orchestration contract,
/// such as `post_review` emitting `CSA:NEXT_STEP` directives. Failures remain
/// observational and therefore degrade to an empty string.
pub(crate) fn capture_observational_hook_output(
    event: HookEvent,
    pairs: &[(&str, &str)],
    project_root: &std::path::Path,
) -> String {
    let hooks_config = load_runtime_hooks(project_root);
    let variables = hook_variables(pairs);
    let hook_config = hooks_config.get_for_event(event);

    match csa_hooks::run_hook_capturing(event, &hook_config, &variables) {
        Ok(output) => output,
        Err(err) => {
            warn!(
                "{event:?} hook failed while capturing output (observational, continuing): {err}"
            );
            String::new()
        }
    }
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
#[path = "pipeline_admitted_executor_tests.rs"]
mod admitted_executor_tests;

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_tests_clean_room_execution.rs"]
mod clean_room_execution_tests;

#[cfg(test)]
#[path = "pipeline_tests_clean_room_integration.rs"]
mod clean_room_integration_tests;

#[cfg(test)]
#[path = "pipeline_tests_thinking.rs"]
mod thinking_tests;

#[cfg(test)]
#[path = "pipeline_tests_prompt_guard.rs"]
mod prompt_guard_tests;

#[cfg(test)]
#[path = "pipeline_tests_git_guard_env.rs"]
mod git_guard_env_tests;

#[cfg(test)]
#[path = "pipeline_tests_preflight.rs"]
mod preflight_tests;

#[cfg(test)]
#[path = "pipeline_tests_session_cleanup.rs"]
mod session_cleanup_tests;
