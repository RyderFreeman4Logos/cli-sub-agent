use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use tracing::{info, warn};

use csa_core::types::{OutputFormat, ToolArg, ToolSelectionStrategy};
use csa_lock::SessionLock;
use csa_process::StreamMode;

use crate::pipeline;
use crate::run_cmd_caller_fork::resolve_fork_from_caller;
use crate::run_cmd_fork::try_auto_seed_fork;
use crate::run_cmd_model_pin::{
    RunModelPinInput, explicit_tool_no_failover_from_inherited_pin,
    inherited_model_pin_from_startup, resolve_handle_run_model_pin,
    validate_inherited_model_pin_allows_explicit_tool,
};
use crate::run_cmd_post::{
    handle_fork_call_resume, mark_seed_and_evict, update_fork_genealogy,
    write_fallback_chain_to_result_toml,
};
use crate::run_cmd_tool_selection::{
    resolve_last_session_selection, resolve_return_target_session_id, resolve_skill_and_prompt,
    resolve_tool_by_strategy_with_catalog,
};
use crate::run_helpers::{
    apply_compound_tier_selector_arg, compound_tier_selects_tool, is_routing_conflict,
    resolve_positional_stdin_sentinel, resolve_prompt_with_file, resolve_task_edit_requirement,
    tier_bypass_allowed, truncate_prompt, warn_if_tier_without_tool,
};
use crate::run_helpers_branch_guard::{
    BranchGuardRuntime, evaluate_and_emit_refusal, observe_branch_state,
};
use crate::startup_env::StartupSubtreeEnv;
#[path = "run_cmd_execute_post_exec_gate.rs"]
mod post_exec_gate;
#[path = "run_cmd_execute_resume_tier.rs"]
mod resume_tier;
#[path = "run_cmd_execute_reuse_hint.rs"]
mod reuse_hint;
#[path = "run_cmd_execute_routing.rs"]
mod routing;
#[path = "run_cmd_execute_cli_flags.rs"]
mod run_cli_flags;
#[path = "run_cmd_execute_context.rs"]
mod run_context;
#[path = "run_cmd_execute_output.rs"]
mod run_output;
#[path = "run_cmd_execute_skill_resume.rs"]
mod skill_resume;
#[path = "run_cmd_execute_tier_guard.rs"]
mod tier_guard;
use post_exec_gate::{
    PostExecGateApplyOptions, apply_post_exec_gate_after_success_with_runner,
    execute_post_exec_gate_command,
};
use resume_tier::infer_resume_tier_for_matching_tool;
use reuse_hint::emit_reusable_session_hint;
use routing::{
    RunModelSelectionFlags, resolve_primary_writer_spec_for_run, resolve_run_effective_tier,
    resolve_run_fallback_tier_name, resolve_run_no_failover, resolve_run_subtree_pin_selection,
    resolve_run_tier_context, resolve_run_tool_strategy,
};
use run_cli_flags::{
    resolve_return_target, warn_deprecated_session_flags,
    warn_if_fast_mode_has_no_codex_run_candidate,
};
use run_context::finalize_prompt_text;
use run_output::emit_run_result_output;
use skill_resume::maybe_auto_resume_interrupted_skill_session;
use tier_guard::{
    DirectToolTierGuardCtx, RunPreExecErrorCtx, RunTierBypassPersistCtx,
    enforce_direct_tool_tier_guard, enforce_run_tier_bypass_gate_or_persist,
};

use super::attempt::{RunLoopCompletion, RunLoopRequest, execute_run_loop};
use super::resume::{
    detect_effective_repo, resolve_run_timeout_seconds, skill_session_description,
};

#[allow(clippy::too_many_arguments)]
#[path = "run_cmd_execute_handle.rs"]
mod handle;
pub(crate) use handle::handle_run;

#[cfg(test)]
#[path = "run_cmd_execute_codex_no_failover_tests.rs"]
mod codex_no_failover_tests;

#[cfg(test)]
#[path = "run_cmd_execute_pre_exec_tests.rs"]
mod pre_exec_tests;

#[cfg(test)]
#[path = "run_cmd_execute_tests.rs"]
mod tests;
