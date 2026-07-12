use anyhow::Result;
use csa_config::{GlobalConfig, ProjectConfig};
use std::time::Instant;
use tracing::warn;

use super::attempt_exec::{
    AttemptExecution, EphemeralRunRequest, run_ephemeral_with_timeout,
    run_ephemeral_without_timeout, run_persistent_with_timeout, run_persistent_without_timeout,
};
use super::attempt_support::{
    CommitSkillWorkspaceGuard as Cg, allow_cross_tool_failover,
    capture_commit_skill_workspace_guard as capture_cg, codex_fast_mode_enabled as codex_fast,
    merge_run_loop_changed_paths as merge_changed,
    persist_fork_timeout_result_if_missing as persist_timeout,
    resolve_attempt_initial_response_timeout_seconds as initial_timeout,
    resolve_max_failover_attempts as max_failovers,
    resolve_runtime_fallback_enabled as runtime_fallback,
    restore_failed_commit_skill_workspace as restore_cg, strategy_is_explicit,
};
use super::resume::{emit_run_timeout, resolve_remaining_run_timeout};
use crate::pipeline;
use crate::run_cmd_fork::{ForkResolution, pre_create_native_fork_session, resolve_fork};
use crate::run_cmd_tool_selection::resolve_slot_wait_timeout_seconds;
use crate::run_helpers::parse_token_usage;

#[path = "run_cmd_attempt_types.rs"]
mod types;
use RunLoopCompletion::Exit;
pub(crate) use types::{RunLoopCompletion, RunLoopOutcome, RunLoopRequest};
#[path = "run_cmd_attempt_outcome.rs"]
mod outcome;
use outcome::{
    AttemptErrorAction, AttemptErrorRequest, AttemptErrorState, AttemptRetryState,
    PostAttemptAction, PostAttemptRequest, PostAttemptState, evaluate_post_attempt_retry,
    handle_attempt_error,
};
#[path = "run_cmd_attempt_slot.rs"]
mod slot;
use slot::{AttemptSlotOutcome, AttemptSlotRequest, acquire_attempt_slot};
#[path = "run_cmd_attempt_prompt.rs"]
mod prompt;
#[cfg(test)]
use prompt::resolve_attempt_subtree_model_pin_spec;
use prompt::{AttemptPromptRequest, build_attempt_prompt};
