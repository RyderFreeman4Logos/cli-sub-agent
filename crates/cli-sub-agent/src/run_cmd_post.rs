//! Post-execution processing for `csa run`: fork-call resume, genealogy
//! update, and seed-session management.
//!
//! Extracted from `run_cmd.rs` to keep module sizes manageable.
//!
//! Failover evaluation (rate-limit / transport-error → tier failover) lives
//! in [`run_cmd_post_failover`](../run_cmd_post_failover/index.html) and is
//! re-exported below to keep existing call sites stable.

use std::path::Path;

use anyhow::Result;
use tracing::{debug, info, warn};

use csa_config::ProjectConfig;
use csa_core::types::ToolName;
use csa_scheduler::FallbackChain;
use csa_session::{PhaseEvent, SessionPhase, load_result, load_session, save_result, save_session};

use crate::run_cmd_fork::{ForkResolution, load_child_return_packet};
use crate::run_cmd_tool_selection::resolve_slot_wait_timeout_seconds;

#[path = "run_cmd_post_failover.rs"]
mod failover;
pub(crate) use failover::{
    ErrorRateLimitFailoverRequest, RateLimitAction, RateLimitFailoverRequest,
    detect_permanent_tool_exhaustion_result, detect_permanent_tool_exhaustion_text,
    evaluate_error_rate_limit_failover_with_catalog, evaluate_rate_limit_failover_with_catalog,
    format_tool_exhausted_summary, is_permanent_tool_exhaustion_error,
};
#[cfg(test)]
pub(crate) use failover::{
    evaluate_error_rate_limit_failover, evaluate_error_rate_limit_failover_with_global_config,
    evaluate_rate_limit_failover,
};

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
            SessionPhase::ToolExhausted => {
                warn!(
                    session = %parent_state.meta_session_id,
                    "Parent session is tool-exhausted; skipping auto-resume"
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
            if let Some(ref new_provider_id) = fork_res.provider_session_id
                && let Some(tool_state) = session.tools.get_mut(current_tool.as_str())
            {
                tool_state.provider_session_id = Some(new_provider_id.clone());
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

/// Overwrite a session's `result.toml` after a post-exec gate failure (#1486).
///
/// The result was written with status=success before the gate ran.
/// This function loads the existing result and overwrites `exit_code` and
/// `status` so orchestrators reading the session never observe false success.
pub(crate) fn overwrite_result_as_post_exec_gate_failure(
    project_root: &Path,
    session_id: &str,
    gate_summary: &str,
    gate_timeout: bool,
) {
    match load_result(project_root, session_id) {
        Ok(Some(mut result)) => {
            result.exit_code = 1;
            result.status = "failure".to_string();
            result.summary = gate_summary.to_string();
            result.gate_timeout = gate_timeout;
            if let Err(save_err) = save_result(project_root, session_id, &result) {
                warn!(
                    session = %session_id,
                    error = %save_err,
                    "Failed to overwrite result.toml after post-exec gate failure"
                );
            }
        }
        Ok(None) => {
            warn!(
                session = %session_id,
                "No result.toml to overwrite after post-exec gate failure"
            );
        }
        Err(load_err) => {
            warn!(
                session = %session_id,
                error = %load_err,
                "Failed to load result.toml for post-exec gate failure overwrite"
            );
        }
    }

    // Retire the session so it doesn't remain in Active state when daemon exits
    if let Err(retire_err) = retire_session_after_gate_failure(project_root, session_id) {
        warn!(
            session = %session_id,
            error = %retire_err,
            "Failed to retire session after post-exec gate failure"
        );
    }
}

pub(crate) fn record_post_exec_gate_timeout_advisory(project_root: &Path, session_id: &str) {
    record_post_exec_gate_success_warning(
        project_root,
        session_id,
        true,
        "gate timed out, verification incomplete, not a gate pass",
    );
}

pub(crate) fn record_post_exec_gate_skipped_by_flag(project_root: &Path, session_id: &str) {
    record_post_exec_gate_success_warning(
        project_root,
        session_id,
        false,
        "post-exec gate skipped by --no-post-exec-gate; external verification required",
    );
}

/// Record that a planning-mode run (e.g. `--skill mktd`) unexpectedly left dirty
/// tracked changes, so the post-exec gate is being run to verify them instead of
/// being skipped by the planning-only fast path. Surfaces the anomaly both in
/// logs and on the persisted result for audit.
pub(crate) fn record_post_exec_gate_planning_dirty_override(project_root: &Path, session_id: &str) {
    warn!(
        session = %session_id,
        "planning-mode run left dirty tracked changes; running post-exec gate to verify them"
    );
    record_post_exec_gate_success_warning(
        project_root,
        session_id,
        false,
        "planning-mode run left dirty tracked changes; post-exec gate run to verify them",
    );
}

fn record_post_exec_gate_success_warning(
    project_root: &Path,
    session_id: &str,
    gate_timeout: bool,
    warning: &str,
) {
    match load_result(project_root, session_id) {
        Ok(Some(mut result)) => {
            result.gate_timeout = result.gate_timeout || gate_timeout;
            if !result.warnings.iter().any(|existing| existing == warning) {
                result.warnings.push(warning.to_string());
            }
            if let Err(save_err) = save_result(project_root, session_id, &result) {
                warn!(
                    session = %session_id,
                    error = %save_err,
                    "Failed to record post-exec gate warning"
                );
            }
        }
        Ok(None) => {
            warn!(
                session = %session_id,
                "No result.toml to annotate with post-exec gate warning"
            );
        }
        Err(load_err) => {
            warn!(
                session = %session_id,
                error = %load_err,
                "Failed to load result.toml for post-exec gate warning"
            );
        }
    }
}

/// Retire a session after post-exec gate failure to prevent it from remaining in Active state.
/// This ensures that `csa session wait` will not poll indefinitely for a dead session.
pub(crate) fn retire_session_after_gate_failure(
    project_root: &Path,
    session_id: &str,
) -> anyhow::Result<()> {
    match load_session(project_root, session_id) {
        Ok(mut session) => {
            let phase_result = session.phase.transition(&PhaseEvent::Retired);
            match phase_result {
                Ok(new_phase) => {
                    session.phase = new_phase;
                    save_session(&session)?;
                    debug!(
                        session = %session_id,
                        phase = ?session.phase,
                        "Session retired after post-exec gate failure"
                    );
                }
                Err(transition_err) => {
                    warn!(
                        session = %session_id,
                        phase = ?session.phase,
                        error = %transition_err,
                        "Failed to transition session to Retired after gate failure; forcing to Retired"
                    );
                    // Force transition to Retired even if the normal transition fails
                    session.phase = SessionPhase::Retired;
                    save_session(&session)?;
                }
            }
        }
        Err(load_err) => {
            warn!(
                session = %session_id,
                error = %load_err,
                "Failed to load session when attempting to retire after gate failure"
            );
        }
    }
    Ok(())
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

/// Persist `fallback_chain` into the session's `result.toml` using the
/// typed `SessionResult` path so the field is visible via `csa session result --json`.
///
/// Loads the existing result, sets `fallback_chain`, and writes back atomically
/// via `save_result`. No-ops silently if the result is missing or `chain` is empty.
pub(crate) fn write_fallback_chain_to_result_toml(
    project_root: &Path,
    session_id: &str,
    chain: &FallbackChain,
) {
    if chain.is_empty() {
        return;
    }
    let mut result = match load_result(project_root, session_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            debug!(session = %session_id, "result.toml missing; skipping fallback_chain write");
            return;
        }
        Err(e) => {
            debug!(session = %session_id, error = %e, "Could not load result.toml for fallback_chain write");
            return;
        }
    };
    result.fallback_chain = Some(chain.to_vec());
    if let Err(e) = save_result(project_root, session_id, &result) {
        debug!(session = %session_id, error = %e, "Could not write result.toml with fallback_chain");
    } else {
        info!(
            session = %session_id,
            entries = chain.len(),
            "Wrote fallback_chain to result.toml"
        );
    }
}

#[cfg(test)]
#[path = "run_cmd_post_gate_failure_tests.rs"]
mod post_exec_gate_overwrite_tests;
