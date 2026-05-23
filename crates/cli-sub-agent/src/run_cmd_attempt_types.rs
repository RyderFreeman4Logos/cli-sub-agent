//! Data types for the `csa run` execution loop.
//!
//! Extracted from `run_cmd_attempt.rs` to stay under the 800-line monolith limit.

use csa_config::{GlobalConfig, ProjectConfig};
use csa_core::types::{OutputFormat, ToolName, ToolSelectionStrategy};
use csa_executor::ContextLoadOptions;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::pipeline::MemoryInjectionOptions;
use crate::run_cmd_fork::ForkResolution;
use crate::run_helpers_branch_guard::BranchGuardRuntime;

pub(crate) struct RunLoopRequest<'a> {
    pub(crate) strategy: ToolSelectionStrategy,
    pub(crate) initial_tool: ToolName,
    pub(crate) initial_model_spec: Option<String>,
    pub(crate) user_model_spec_explicit: bool,
    pub(crate) initial_model: Option<String>,
    pub(crate) runtime_fallback_candidates: Vec<ToolName>,
    pub(crate) project_root: &'a Path,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a GlobalConfig,
    pub(crate) prompt_text: &'a str,
    pub(crate) skill: Option<&'a str>,
    pub(crate) skill_session_tag: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) parent: Option<String>,
    pub(crate) output_format: OutputFormat,
    pub(crate) stream_mode: csa_process::StreamMode,
    pub(crate) thinking: Option<&'a str>,
    pub(crate) force: bool,
    pub(crate) force_override_user_config: bool,
    pub(crate) force_ignore_tier_setting: bool,
    pub(crate) no_failover: bool,
    pub(crate) fast_but_more_cost: bool,
    pub(crate) wait: bool,
    pub(crate) idle_timeout_seconds: u64,
    pub(crate) cli_idle_timeout: Option<u64>,
    pub(crate) cli_initial_response_timeout: Option<u64>,
    pub(crate) no_idle_timeout: bool,
    pub(crate) run_timeout_seconds: Option<u64>,
    pub(crate) run_started_at: Instant,
    pub(crate) is_fork: bool,
    pub(crate) is_auto_seed_fork: bool,
    /// Pre-resolved fork from `--fork-from-caller` (CSA-lite, #1432).
    /// When present, supplies the initial fork_resolution before the loop
    /// runs `resolve_fork()`; downstream prepend-to-prompt path picks up
    /// the extracted caller conversation prefix.
    pub(crate) caller_fork_resolution: Option<ForkResolution>,
    pub(crate) ephemeral: bool,
    pub(crate) fork_call: bool,
    pub(crate) session_arg: Option<String>,
    pub(crate) effective_session_arg: Option<String>,
    pub(crate) tier_auto_select: bool,
    pub(crate) failover_on_crash_enabled: bool,
    pub(crate) resolved_tier_name: Option<&'a str>,
    pub(crate) context_load_options: Option<&'a ContextLoadOptions>,
    pub(crate) memory_injection: MemoryInjectionOptions,
    pub(crate) pre_session_hook: Option<csa_hooks::PreSessionHookInvocation>,
    pub(crate) task_needs_edit: Option<bool>,
    pub(crate) no_fs_sandbox: bool,
    pub(crate) extra_writable: Vec<PathBuf>,
    pub(crate) extra_readable: Vec<PathBuf>,
    pub(crate) branch_guard: BranchGuardRuntime,
}

pub(crate) enum RunLoopCompletion {
    Exit(i32),
    Completed(Box<RunLoopOutcome>),
}

pub(crate) struct RunLoopOutcome {
    pub(crate) result: csa_process::ExecutionResult,
    pub(crate) current_tool: ToolName,
    pub(crate) executed_session_id: Option<String>,
    pub(crate) changed_paths: Option<Vec<String>>,
    pub(crate) fork_resolution: Option<ForkResolution>,
    /// Ordered list of tools skipped due to rate-limit/quota failover before this result.
    pub(crate) fallback_chain: csa_scheduler::FallbackChain,
}
