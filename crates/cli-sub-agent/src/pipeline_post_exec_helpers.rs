//! Mechanical post-execution helpers split out of `pipeline_post_exec`.
//!
//! These are pure session-state accounting and tool-output utilities with no
//! coupling to the post-exec gate flow or the #161 outcome classifier; they
//! live here only to keep `pipeline_post_exec.rs` under the module token
//! budget. Re-exported privately by the parent module so existing call sites
//! (and the parent's test submodule) reach them unqualified.

use std::path::Path;

use tracing::{info, warn};

use csa_config::ProjectConfig;
use csa_executor::CODEX_EXEC_INITIAL_STALL_REASON;
use csa_session::{MetaSessionState, TokenUsage, ToolState, get_session_dir};

/// Whether `summary` is the codex-exec initial-stall sentinel: codex exited via
/// the stall watchdog (137) with the `{CODEX_EXEC_INITIAL_STALL_REASON}: no
/// stdout within …` marker. Recognised so the post-exec stall gate can treat it
/// as a CSA-own failure rather than a generic signal kill.
pub(super) fn is_codex_exec_initial_stall_summary(
    tool_name: &str,
    exit_code: i32,
    summary: &str,
) -> bool {
    tool_name == "codex"
        && exit_code == 137
        && summary.starts_with(&format!(
            "{CODEX_EXEC_INITIAL_STALL_REASON}: no stdout within "
        ))
        && summary.contains(" (effort=")
}

/// If tool output compression is enabled, persist the original output to
/// `{session_dir}/tool_outputs/` and replace `result.output` with a compact
/// placeholder.
pub(super) fn maybe_compress_tool_output(
    config: Option<&ProjectConfig>,
    project_root: &Path,
    session: &MetaSessionState,
    result: &mut csa_process::ExecutionResult,
) -> anyhow::Result<()> {
    let Some(cfg) = config else { return Ok(()) };
    if !cfg.session.tool_output_compression {
        return Ok(());
    }
    let threshold = cfg.session.tool_output_threshold_bytes;
    if let csa_process::CompressDecision::Compress {
        original_bytes,
        replacement: _,
    } = csa_process::should_compress_output(&result.output, threshold)
    {
        let session_dir = get_session_dir(project_root, &session.meta_session_id)?;
        let store = csa_session::tool_output_store::ToolOutputStore::new(&session_dir)?;
        let index = session.turn_count.saturating_sub(1);
        store.store(index, result.output.as_bytes())?;
        store.append_manifest(index, original_bytes as u64)?;
        info!(
            session = %session.meta_session_id,
            original_bytes,
            index,
            "Compressed tool output stored"
        );
        // Override generic placeholder with session-specific one for recoverability.
        result.output = format!(
            "[Tool output compressed: {original_bytes} bytes → csa session tool-output {} {index}]",
            session.meta_session_id
        );
    }
    Ok(())
}

/// Record this turn's tool invocation into `session.tools`: refresh the
/// provider session id, last summary/exit code, timestamp, and token usage,
/// inserting a fresh [`ToolState`] when the tool is seen for the first time.
pub(super) fn update_tool_state(
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
            tool_version: None,
            token_usage: token_usage.clone(),
        });
}

/// Fold this turn's `token_usage` into the session's cumulative totals and
/// update token-budget tracking (advisory soft/hard threshold warnings).
/// Missing per-field values must never zero out a previously recorded total.
pub(super) fn update_cumulative_tokens(
    session: &mut MetaSessionState,
    token_usage: Option<TokenUsage>,
) {
    let Some(new_usage) = token_usage else {
        return;
    };

    let cumulative = session
        .total_token_usage
        .get_or_insert(TokenUsage::default());
    accumulate_u64(&mut cumulative.input_tokens, new_usage.input_tokens);
    accumulate_u64(&mut cumulative.output_tokens, new_usage.output_tokens);
    accumulate_u64(
        &mut cumulative.reasoning_output_tokens,
        new_usage.reasoning_output_tokens,
    );
    accumulate_u64(&mut cumulative.total_tokens, new_usage.total_tokens);
    accumulate_f64(
        &mut cumulative.estimated_cost_usd,
        new_usage.estimated_cost_usd,
    );
    accumulate_u64(
        &mut cumulative.cache_read_input_tokens,
        new_usage.cache_read_input_tokens,
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

fn accumulate_u64(total: &mut Option<u64>, new_value: Option<u64>) {
    if let Some(value) = new_value {
        *total = Some(total.unwrap_or(0).saturating_add(value));
    }
}

fn accumulate_f64(total: &mut Option<f64>, new_value: Option<f64>) {
    if let Some(value) = new_value {
        *total = Some(total.unwrap_or(0.0) + value);
    }
}
