//! Post-execution processing for pipeline sessions.
//!
//! Handles session state updates, token tracking, result persistence,
//! structured output parsing, hooks, and memory capture after tool execution.

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use csa_config::{GlobalConfig, ProjectConfig};
use csa_executor::Executor;
use csa_hooks::{HookEvent, run_hooks_for_event};
use csa_session::{
    MetaSessionState, SessionArtifact, SessionResult, TokenUsage, ToolState,
    persist_structured_output, save_result, save_session,
};

use crate::memory_capture;
use crate::run_helpers::{is_compress_command, parse_token_usage};

/// All inputs needed for post-execution processing.
pub(crate) struct PostExecContext<'a> {
    pub executor: &'a Executor,
    pub prompt: &'a str,
    pub effective_prompt: &'a str,
    pub project_root: &'a Path,
    pub config: Option<&'a ProjectConfig>,
    pub global_config: Option<&'a GlobalConfig>,
    pub session_dir: PathBuf,
    pub sessions_root: String,
    pub execution_start_time: chrono::DateTime<chrono::Utc>,
    pub hooks_config: &'a csa_hooks::HooksConfig,
    pub memory_project_key: Option<String>,
    pub provider_session_id: Option<String>,
    pub events_count: u64,
    pub transcript_artifacts: Vec<SessionArtifact>,
}

/// Process the results of tool execution: update session, persist artifacts, fire hooks.
///
/// Returns the final `ExecutionResult` and metadata needed by the caller.
pub(crate) async fn process_execution_result(
    ctx: PostExecContext<'_>,
    session: &mut MetaSessionState,
    result: &csa_process::ExecutionResult,
) -> anyhow::Result<()> {
    let token_usage = parse_token_usage(&result.output);

    // Update session tool state
    update_tool_state(
        session,
        ctx.executor.tool_name(),
        &ctx.provider_session_id,
        result,
        &token_usage,
    );
    // Clear stale interruption markers once a run reaches post-exec.
    session.termination_reason = None;
    session.last_accessed = chrono::Utc::now();

    // Detect compress/compact commands: mark session as Available for reuse
    if result.exit_code == 0 && is_compress_command(ctx.prompt) {
        session.context_status.is_compacted = true;
        session.context_status.last_compacted_at = Some(chrono::Utc::now());
        match session.apply_phase_event(csa_session::PhaseEvent::Compressed) {
            Ok(()) => {
                info!(
                    session = %session.meta_session_id,
                    "Session compacted and marked Available for reuse"
                );
            }
            Err(e) => {
                warn!(
                    session = %session.meta_session_id,
                    error = %e,
                    "Skipping phase transition on compress"
                );
            }
        }
    }

    // Increment turn count
    session.turn_count += 1;

    // Update cumulative token usage
    update_cumulative_tokens(session, token_usage);

    // Write effective_prompt to input/ for audit trail
    write_prompt_audit(&ctx.session_dir, ctx.effective_prompt);

    // Write structured result
    let execution_end_time = chrono::Utc::now();
    let session_result = SessionResult {
        status: SessionResult::status_from_exit_code(result.exit_code),
        exit_code: result.exit_code,
        summary: result.summary.clone(),
        tool: ctx.executor.tool_name().to_string(),
        started_at: ctx.execution_start_time,
        completed_at: execution_end_time,
        events_count: ctx.events_count,
        artifacts: ctx.transcript_artifacts,
    };
    if let Err(e) = save_result(ctx.project_root, &session.meta_session_id, &session_result) {
        warn!("Failed to save session result: {}", e);
    }

    // Persist structured output sections from output.log markers
    persist_output_sections(&ctx.session_dir);

    // Save session
    save_session(session)?;

    // Fire PostRun and SessionComplete hooks, capture memory
    let hook_vars = std::collections::HashMap::from([
        ("session_id".to_string(), session.meta_session_id.clone()),
        (
            "session_dir".to_string(),
            ctx.session_dir.display().to_string(),
        ),
        ("sessions_root".to_string(), ctx.sessions_root.to_string()),
        ("tool".to_string(), ctx.executor.tool_name().to_string()),
        ("exit_code".to_string(), result.exit_code.to_string()),
    ]);

    // PostRun hook: fires after every tool execution
    crate::pipeline::run_pipeline_hook(HookEvent::PostRun, ctx.hooks_config, &hook_vars)?;

    // Memory capture
    let memory_config = ctx
        .config
        .map(|cfg| &cfg.memory)
        .filter(|m| !m.is_default())
        .or_else(|| ctx.global_config.map(|cfg| &cfg.memory));
    if let Some(memory_config) = memory_config {
        if let Err(e) = memory_capture::capture_session_memory(
            memory_config,
            &ctx.session_dir,
            ctx.memory_project_key.as_deref(),
            Some(ctx.executor.tool_name()),
            Some(session.meta_session_id.as_str()),
        )
        .await
        {
            warn!("Memory capture failed: {}", e);
        }
    }

    // SessionComplete hook: git-commits session artifacts
    if let Err(e) = run_hooks_for_event(HookEvent::SessionComplete, ctx.hooks_config, &hook_vars) {
        warn!("SessionComplete hook failed: {}", e);
    }

    Ok(())
}

fn update_tool_state(
    session: &mut MetaSessionState,
    tool_name: &str,
    provider_session_id: &Option<String>,
    result: &csa_process::ExecutionResult,
    token_usage: &Option<TokenUsage>,
) {
    session
        .tools
        .entry(tool_name.to_string())
        .and_modify(|t| {
            if let Some(session_id) = provider_session_id {
                t.provider_session_id = Some(session_id.clone());
            }
            t.last_action_summary = result.summary.clone();
            t.last_exit_code = result.exit_code;
            t.updated_at = chrono::Utc::now();

            if let Some(usage) = token_usage {
                t.token_usage = Some(usage.clone());
            }
        })
        .or_insert_with(|| ToolState {
            provider_session_id: provider_session_id.clone(),
            last_action_summary: result.summary.clone(),
            last_exit_code: result.exit_code,
            updated_at: chrono::Utc::now(),
            token_usage: token_usage.clone(),
        });
}

fn update_cumulative_tokens(session: &mut MetaSessionState, token_usage: Option<TokenUsage>) {
    let Some(new_usage) = token_usage else {
        return;
    };

    let cumulative = session
        .total_token_usage
        .get_or_insert(TokenUsage::default());
    cumulative.input_tokens =
        Some(cumulative.input_tokens.unwrap_or(0) + new_usage.input_tokens.unwrap_or(0));
    cumulative.output_tokens =
        Some(cumulative.output_tokens.unwrap_or(0) + new_usage.output_tokens.unwrap_or(0));
    cumulative.total_tokens =
        Some(cumulative.total_tokens.unwrap_or(0) + new_usage.total_tokens.unwrap_or(0));
    cumulative.estimated_cost_usd = Some(
        cumulative.estimated_cost_usd.unwrap_or(0.0) + new_usage.estimated_cost_usd.unwrap_or(0.0),
    );

    // Update token budget tracking
    if let Some(ref mut budget) = session.token_budget {
        let tokens_used = new_usage.total_tokens.unwrap_or(0);
        budget.record_usage(tokens_used);
        if budget.is_hard_exceeded() {
            warn!(
                session = %session.meta_session_id,
                used = budget.used,
                allocated = budget.allocated,
                "Token budget hard threshold reached — advisory only"
            );
        } else if budget.is_soft_exceeded() {
            warn!(
                session = %session.meta_session_id,
                used = budget.used,
                allocated = budget.allocated,
                remaining = budget.remaining(),
                "Token budget soft threshold reached"
            );
        }
    }
}

fn write_prompt_audit(session_dir: &Path, effective_prompt: &str) {
    let input_dir = session_dir.join("input");
    if input_dir.exists() {
        let prompt_path = input_dir.join("prompt.txt");
        if let Err(e) = fs::write(&prompt_path, effective_prompt) {
            warn!("Failed to write prompt to input/: {}", e);
        }
    }
}

fn persist_output_sections(session_dir: &Path) {
    let output_log_path = session_dir.join("output.log");
    if output_log_path.exists() {
        match fs::read_to_string(&output_log_path) {
            Ok(output_log) => {
                if let Err(e) = persist_structured_output(session_dir, &output_log) {
                    warn!("Failed to persist structured output: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to read output.log for structured output: {}", e);
            }
        }
    }
}
