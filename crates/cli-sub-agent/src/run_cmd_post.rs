//! Post-execution processing for `csa run`: fork-call resume, genealogy
//! update, and seed-session management.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.

use std::path::Path;

use anyhow::Result;
use tracing::{debug, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_session::{PhaseEvent, SessionPhase};

use crate::run_cmd_fork::{ForkResolution, load_child_return_packet};
use crate::run_cmd_tool_selection::resolve_slot_wait_timeout_seconds;

/// Handle the fork-call parent resume protocol after child execution completes.
///
/// Loads the child return packet, stores its reference in the parent session,
/// reacquires a slot for parent resume, and applies phase transitions.
pub(crate) fn handle_fork_call_resume(
    project_root: &Path,
    executed_session_id: Option<&str>,
    fork_call_parent_session_id: &str,
    current_tool: &ToolName,
    return_target_present: bool,
    config: Option<&ProjectConfig>,
    global_config: &csa_config::GlobalConfig,
) -> Result<()> {
    let child_session_id = executed_session_id
        .ok_or_else(|| anyhow::anyhow!("fork-call completed without child session id"))?;
    let (_return_packet, return_packet_ref) =
        load_child_return_packet(project_root, child_session_id)?;

    // Reload current state from disk to avoid clobbering concurrent parent updates.
    let mut parent_state = csa_session::load_session(project_root, fork_call_parent_session_id)?;
    parent_state.last_return_packet = Some(return_packet_ref);
    csa_session::save_session(&parent_state)?;

    // Reacquire a slot for parent resume work after child execution.
    // This is best-effort only; return-packet persistence is the critical path.
    let slots_dir = csa_config::GlobalConfig::slots_dir()?;
    let parent_tool_name = current_tool.as_str();
    let parent_timeout = std::time::Duration::from_secs(resolve_slot_wait_timeout_seconds(config));
    let _parent_resume_slot = match csa_lock::slot::acquire_slot_blocking(
        &slots_dir,
        parent_tool_name,
        global_config.max_concurrent(parent_tool_name),
        parent_timeout,
        Some(fork_call_parent_session_id),
    ) {
        Ok(slot) => Some(slot),
        Err(e) => {
            warn!(
                session = %fork_call_parent_session_id,
                error = %e,
                "Failed to reacquire parent slot during fork-call resume; continuing"
            );
            None
        }
    };

    if return_target_present {
        match parent_state.phase {
            SessionPhase::Available => {
                parent_state
                    .apply_phase_event(PhaseEvent::Resumed)
                    .map_err(anyhow::Error::msg)?;
            }
            SessionPhase::Active => {
                debug!(
                    session = %parent_state.meta_session_id,
                    "Parent already active; skipping Resumed transition"
                );
            }
            SessionPhase::Retired => {
                warn!(
                    session = %parent_state.meta_session_id,
                    "Parent session is retired; skipping auto-resume"
                );
            }
        }
    }

    csa_session::save_session(&parent_state)?;

    let return_packet = load_child_return_packet(project_root, child_session_id)?.0;
    info!(
        parent = %fork_call_parent_session_id,
        child = %child_session_id,
        status = ?return_packet.status,
        exit_code = return_packet.exit_code,
        "Stored return packet ref and completed fork-call parent resume"
    );
    Ok(())
}

/// Update fork genealogy fields on the executed session after execution completes.
pub(crate) fn update_fork_genealogy(
    project_root: &Path,
    executed_session_id: &str,
    fork_res: &ForkResolution,
    current_tool: &ToolName,
) {
    match csa_session::load_session(project_root, executed_session_id) {
        Ok(mut session) => {
            session.genealogy.fork_of_session_id = Some(fork_res.source_session_id.clone());
            session.genealogy.fork_provider_session_id =
                fork_res.source_provider_session_id.clone();
            if session.genealogy.parent_session_id.is_none() {
                session.genealogy.parent_session_id = Some(fork_res.source_session_id.clone());
            }
            // For native fork: store the forked provider session ID in
            // ToolState so future `--session` resumes can use it.
            if let Some(ref new_provider_id) = fork_res.provider_session_id {
                if let Some(tool_state) = session.tools.get_mut(current_tool.as_str()) {
                    tool_state.provider_session_id = Some(new_provider_id.clone());
                }
            }
            if let Err(e) = csa_session::save_session(&session) {
                warn!("Failed to update fork genealogy on session: {e}");
            } else {
                info!(
                    session = %session.meta_session_id,
                    fork_of = %fork_res.source_session_id,
                    "Updated session genealogy with fork fields"
                );
            }
        }
        Err(e) => {
            warn!("Failed to load session for fork genealogy update: {e}");
        }
    }
}

/// Outcome of rate-limit failover evaluation.
pub(crate) enum RateLimitAction {
    /// No rate limit detected; break with result.
    NoRateLimit,
    /// Rate limit detected but no failover possible; break with result.
    ExhaustedFailovers,
    /// Retry with a different tool.
    Retry {
        new_tool: ToolName,
        new_model_spec: Option<String>,
    },
}

/// Check for 429 rate-limit signals and decide whether to failover.
///
/// Returns `RateLimitAction` to drive `continue`/`break` in the caller loop.
#[allow(clippy::too_many_arguments)]
pub(crate) fn evaluate_rate_limit_failover(
    tool_name_str: &str,
    exec_result: &csa_process::ExecutionResult,
    attempts: usize,
    max_failover_attempts: usize,
    tried_tools: &mut Vec<String>,
    executed_session_id: Option<&str>,
    effective_session_arg: Option<&str>,
    ephemeral: bool,
    prompt_text: &str,
    project_root: &Path,
    config: Option<&ProjectConfig>,
) -> Result<RateLimitAction> {
    let rate_limit = match csa_scheduler::detect_rate_limit(
        tool_name_str,
        &exec_result.stderr_output,
        &exec_result.output,
        exec_result.exit_code,
    ) {
        Some(rl) => rl,
        None => return Ok(RateLimitAction::NoRateLimit),
    };

    info!(
        tool = %tool_name_str,
        pattern = %rate_limit.matched_pattern,
        attempt = attempts,
        max = max_failover_attempts,
        "Rate limit detected, attempting failover"
    );

    if attempts >= max_failover_attempts {
        warn!(
            "Max failover attempts ({}) reached, returning error",
            max_failover_attempts
        );
        return Ok(RateLimitAction::ExhaustedFailovers);
    }

    tried_tools.push(tool_name_str.to_string());

    // Prefer the actually-executed session (important for forks where
    // effective_session_arg starts as None) so decide_failover evaluates
    // the fork session's context, not the parent session.
    let failover_session_ref = executed_session_id.or(effective_session_arg);
    let session_state = if !ephemeral {
        failover_session_ref.and_then(|sid| {
            let sessions_dir = csa_session::get_session_root(project_root)
                .ok()?
                .join("sessions");
            let resolved_id = csa_session::resolve_session_prefix(&sessions_dir, sid).ok()?;
            csa_session::load_session(project_root, &resolved_id).ok()
        })
    } else {
        None
    };

    let task_needs_edit = crate::run_helpers::infer_task_edit_requirement(prompt_text)
        .or_else(|| config.map(|cfg| cfg.can_tool_edit_existing(tool_name_str)));

    let Some(cfg) = config else {
        return Ok(RateLimitAction::ExhaustedFailovers);
    };

    let action = csa_scheduler::decide_failover(
        tool_name_str,
        "default",
        task_needs_edit,
        session_state.as_ref(),
        tried_tools,
        cfg,
        &rate_limit.matched_pattern,
    );

    match action {
        csa_scheduler::FailoverAction::RetryInSession {
            new_tool,
            new_model_spec,
            session_id: _,
        }
        | csa_scheduler::FailoverAction::RetrySiblingSession {
            new_tool,
            new_model_spec,
        } => {
            info!(
                from = %tool_name_str,
                to = %new_tool,
                "Failing over to alternative tool"
            );
            let tool = crate::run_helpers::parse_tool_name(&new_tool)?;
            Ok(RateLimitAction::Retry {
                new_tool: tool,
                new_model_spec: Some(new_model_spec),
            })
        }
        csa_scheduler::FailoverAction::ReportError { reason, .. } => {
            warn!(reason = %reason, "Failover not possible, returning original result");
            Ok(RateLimitAction::ExhaustedFailovers)
        }
    }
}

/// Mark a successful non-fork session as a seed candidate and run LRU eviction
/// to retire excess seed sessions.
pub(crate) fn mark_seed_and_evict(
    project_root: &Path,
    session_id: &str,
    current_tool: &ToolName,
    config: Option<&ProjectConfig>,
) {
    match csa_session::load_session(project_root, session_id) {
        Ok(mut session) => {
            if !session.is_seed_candidate {
                session.is_seed_candidate = true;
                if let Err(e) = csa_session::save_session(&session) {
                    warn!("Failed to mark session as seed candidate: {e}");
                } else {
                    info!(
                        session = %session.meta_session_id,
                        tool = %current_tool.as_str(),
                        "Marked session as seed candidate"
                    );
                }
            }
        }
        Err(e) => {
            debug!(error = %e, "Failed to load session for seed marking");
        }
    }

    // LRU eviction: retire excess seed sessions for this tool x project
    let max_seeds = config.map(|c| c.session.max_seed_sessions).unwrap_or(2);
    match csa_scheduler::evict_excess_seeds(project_root, current_tool.as_str(), max_seeds) {
        Ok(retired) if !retired.is_empty() => {
            info!(
                count = retired.len(),
                tool = %current_tool.as_str(),
                "Evicted excess seed sessions"
            );
        }
        Err(e) => {
            debug!(error = %e, "Seed eviction check failed");
        }
        _ => {}
    }
}
