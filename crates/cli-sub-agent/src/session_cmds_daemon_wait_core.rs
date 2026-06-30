//! Core polling loop for `csa session wait`.
//!
//! Contains the main session wait logic with completion detection, memory sampling,
//! timeout detection, and reconciliation of dead sessions.
//!
//! Extracted from `session_cmds_daemon_wait.rs` to reduce module complexity.

use std::path::Path;

use anyhow::Result;

use super::completion::{
    SESSION_WAIT_FAILURE_EXIT_CODE, SESSION_WAIT_MEMORY_WARN_EXIT_CODE, emit_wait_cap_outcome,
    resolve_wait_completion_status_and_exit,
};
use super::liveness::{resume_handoff_blocks_target_reconcile, session_has_live_execution};
use super::registry_loss::{
    emit_registry_state_loss_or_missing_result, session_registry_phase_retired,
    session_registry_state_loss,
};
use super::target::resolve_wait_target;
use super::types::{WaitExecutionOptions, WaitReconciliationOutcome};
use super::{
    emit_wait_terminal_output, load_completed_daemon_result_with_fallback, refresh_result_for_wait,
    suppress_pending_tier_failover_result, try_acquire_session_wait_lock,
};
use crate::session_cmds::resolve_session_prefix_with_global_fallback;
use crate::session_cmds_daemon::{
    emit_failure_summary_for_empty_output, finalize_daemon_completion_if_present,
    load_daemon_completion_packet,
};

type ReconcileEmitter<'a> =
    Box<dyn FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome> + 'a>;
type CompletionSignalEmitter<'a> = Box<dyn FnMut(&str, &str, i32, bool, bool) + 'a>;
type MemorySampler<'a> = Box<dyn FnMut(&Path, &str) -> std::io::Result<u64> + 'a>;
type MemoryWarnEmitter<'a> = Box<dyn FnMut(&str, u64, u64) + 'a>;
type TerminalOutputEmitter<'a> = Box<
    dyn FnMut(
            &Path,
            &str,
            Option<&csa_session::SessionResult>,
            super::SessionWaitOutputMode,
        ) -> Result<bool>
        + 'a,
>;
type NextStepEmitter<'a> = Box<dyn FnMut(&Path) -> Result<()> + 'a>;

pub(crate) struct WaitEmitters<'a> {
    pub(crate) reconcile_dead_active_session: ReconcileEmitter<'a>,
    pub(crate) emit_completion_signal: CompletionSignalEmitter<'a>,
    pub(crate) sample_session_tree_rss_mb: MemorySampler<'a>,
    pub(crate) emit_memory_warn_marker: MemoryWarnEmitter<'a>,
    pub(crate) emit_terminal_output: TerminalOutputEmitter<'a>,
    pub(crate) emit_next_step: NextStepEmitter<'a>,
}

/// Core polling loop implementation for session wait.
///
/// Handles lock acquisition, session resolution, completion detection,
/// memory sampling, timeout detection, and signal emission.
pub(crate) fn handle_session_wait_with_hooks_and_sampler_output_mode<R, E, S, M>(
    session: String,
    cd: Option<String>,
    wait_options: WaitExecutionOptions,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
    mut sample_session_tree_rss_mb: S,
    mut emit_memory_warn_marker: M,
) -> Result<i32>
where
    R: for<'a, 'b, 'c> FnMut(&'a Path, &'b str, &'c str) -> Result<WaitReconciliationOutcome>,
    E: for<'a, 'b> FnMut(&'a str, &'b str, i32, bool, bool),
    S: for<'a, 'b> FnMut(&'a Path, &'b str) -> std::io::Result<u64>,
    M: for<'a> FnMut(&'a str, u64, u64),
{
    handle_session_wait_with_emitters(
        session,
        cd,
        wait_options,
        WaitEmitters {
            reconcile_dead_active_session: Box::new(&mut reconcile_dead_active_session),
            emit_completion_signal: Box::new(&mut emit_completion_signal),
            sample_session_tree_rss_mb: Box::new(&mut sample_session_tree_rss_mb),
            emit_memory_warn_marker: Box::new(&mut emit_memory_warn_marker),
            emit_terminal_output: Box::new(emit_wait_terminal_output),
            emit_next_step: Box::new(super::emit_wait_next_step_if_needed),
        },
    )
}

pub(crate) fn handle_session_wait_with_emitters(
    session: String,
    cd: Option<String>,
    wait_options: WaitExecutionOptions,
    mut emitters: WaitEmitters<'_>,
) -> Result<i32> {
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    // For cross-project sessions, derive session_dir from the resolved sessions_dir
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);
    // Use the foreign project root for cross-project sessions, local otherwise.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let is_cross_project = resolved.foreign_project_root.is_some();
    let mut wait_target = resolve_wait_target(effective_root, &resolved.session_id, &session_dir)?;
    let worktree_lock_root = super::resolve_session_wait_worktree_lock_root(effective_root);
    let mut wait_lock = match try_acquire_session_wait_lock(&wait_target.session_dir)? {
        Some(lock) => lock,
        None => {
            eprintln!(
                "ERROR: another csa session wait is already running for session {} (lock held). The existing wait will notify you on completion. Do NOT re-issue.",
                wait_target.session_id
            );
            return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
        }
    };

    let start = std::time::Instant::now();
    let memory_warn_mb = wait_options
        .behavior
        .memory_warn_mb
        .filter(|limit| *limit > 0);
    let mut next_memory_sample_at =
        memory_warn_mb.map(|_| start + wait_options.behavior.timing.memory_sample_interval);
    let handoff_blocks_initial_stale_precheck = resume_handoff_blocks_target_reconcile(
        wait_target.follows_resume_target,
        &session_dir,
        &wait_target.session_dir,
    );

    // Check if the session is Stale before entering the polling loop.
    // This prevents indefinite polling of sessions that have no live daemon process
    // and no progress for an extended period.
    //
    // A resume wrapper may write resume-target.toml before the target session has
    // target-local liveness. While the wrapper daemon is still alive, its process
    // owns that bootstrap window, so target-local silence is not stale yet.
    if !handoff_blocks_initial_stale_precheck
        && let Err(stale_err) = super::check_session_stale_before_wait(
            effective_root,
            &wait_target.session_id,
            &wait_target.session_dir,
            wait_options.behavior,
            worktree_lock_root.as_deref(),
            &[resolved.session_id.as_str(), &wait_target.session_id],
        )
    {
        if !crate::session_observability::emit_session_registry_state_loss_diagnostic(
            effective_root,
            &wait_target.session_id,
            &wait_target.session_dir,
        ) {
            eprintln!(
                "Session {} appears stale: {}",
                wait_target.session_id, stale_err
            );
            eprintln!(
                "Run `csa session result --session {}` for diagnostics.",
                wait_target.session_id
            );
        }
        return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
    }

    loop {
        if !wait_target.follows_resume_target {
            let updated_target =
                resolve_wait_target(effective_root, &resolved.session_id, &session_dir)?;
            if updated_target.follows_resume_target {
                let updated_lock = match try_acquire_session_wait_lock(&updated_target.session_dir)?
                {
                    Some(lock) => lock,
                    None => {
                        eprintln!(
                            "ERROR: another csa session wait is already running for session {} (lock held). The existing wait will notify you on completion. Do NOT re-issue.",
                            updated_target.session_id
                        );
                        return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
                    }
                };
                drop(wait_lock);
                wait_lock = updated_lock;
                wait_target = updated_target;
            }
        }

        let result_session_id = wait_target.session_id.as_str();
        let display_session_id = if wait_target.follows_resume_target {
            resolved.session_id.as_str()
        } else {
            result_session_id
        };
        let result_session_dir = &wait_target.session_dir;
        let session_live = session_has_live_execution(
            worktree_lock_root.as_deref(),
            result_session_dir,
            &resolved.session_id,
            result_session_id,
        );
        let handoff_blocks_target_reconcile = resume_handoff_blocks_target_reconcile(
            wait_target.follows_resume_target,
            &session_dir,
            result_session_dir,
        );
        let result_registry_state_loss =
            session_registry_state_loss(effective_root, result_session_id, result_session_dir);
        let result_uses_direct_session_dir = is_cross_project || result_registry_state_loss;
        let result_session_is_retired = !result_registry_state_loss
            && session_registry_phase_retired(effective_root, result_session_id);
        let session_live = session_live && !result_session_is_retired;
        let completion_packet = load_daemon_completion_packet(&session_dir)?;
        if let Some(completion) = completion_packet
            .filter(|completion| completion.is_legacy_complete_marker() || !session_live)
        {
            let refreshed_result = refresh_result_for_wait(
                effective_root,
                result_session_id,
                result_session_dir,
                result_uses_direct_session_dir,
            );
            if let Err(err) = &refreshed_result {
                tracing::debug!(
                    session_id = %result_session_id,
                    error = %err,
                    "Failed to refresh result after daemon completion packet"
                );
            }
            let refreshed_result = refreshed_result.ok().flatten();
            let mut synthetic = false;
            let refreshed_result_available = refreshed_result.is_some();
            // result.toml is the authoritative session artifact; trust it over the daemon
            // completion packet (which records the daemon process exit, not the session
            // outcome). The daemon may exit non-zero after writing a successful result.toml
            // (e.g., post-write cleanup failure, parent SIGTERM), so prior mtime-based
            // filtering caused #1442 false failures. See #1442.
            let mut loaded_result = refreshed_result;
            if refreshed_result_available {
                if result_registry_state_loss {
                    tracing::debug!(
                        session_id = %result_session_id,
                        "Skipping session retirement because registry state is unavailable"
                    );
                } else {
                    crate::session_cmds::retire_if_dead_with_result(
                        effective_root,
                        result_session_id,
                        "session wait",
                    )?;
                }
            } else {
                loaded_result = load_completed_daemon_result_with_fallback(
                    effective_root,
                    result_session_id,
                    result_session_dir,
                    result_uses_direct_session_dir,
                )?;
            }
            if loaded_result.is_none() {
                loaded_result = finalize_daemon_completion_if_present(result_session_dir)?
                    .and_then(|result| {
                        suppress_pending_tier_failover_result(
                            result_session_id,
                            result_session_dir,
                            result,
                        )
                    });
                if loaded_result.is_none()
                    && !handoff_blocks_target_reconcile
                    && result_registry_state_loss
                {
                    emit_registry_state_loss_or_missing_result(
                        effective_root,
                        result_session_id,
                        result_session_dir,
                    );
                    return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
                }
                if loaded_result.is_none() && !handoff_blocks_target_reconcile {
                    let reconciled = (emitters.reconcile_dead_active_session)(
                        effective_root,
                        result_session_id,
                        "session wait",
                    )?;
                    synthetic = reconciled.synthetic;
                    if reconciled.result_became_available {
                        loaded_result = load_completed_daemon_result_with_fallback(
                            effective_root,
                            result_session_id,
                            result_session_dir,
                            result_uses_direct_session_dir,
                        )?;
                    }
                }
            }
            if let Some(mut result) = loaded_result {
                super::summary::reconcile_repaired_review_pass_result_status(
                    result_session_dir,
                    &mut result,
                );
                let streamed_output = (emitters.emit_terminal_output)(
                    result_session_dir,
                    display_session_id,
                    Some(&result),
                    wait_options.output_mode,
                )?;
                (emitters.emit_next_step)(result_session_dir)?;
                #[rustfmt::skip]
                let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(completion.status.as_str(), completion.exit_code, synthetic, Some(&result));
                emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
                (emitters.emit_completion_signal)(
                    &resolved.session_id,
                    completion_status.as_ref(),
                    exit_code,
                    synthetic,
                    !streamed_output,
                );
                return Ok(exit_code);
            }

            if session_live
                || csa_process::ToolLiveness::is_alive(result_session_dir)
                || handoff_blocks_target_reconcile
            {
                tracing::debug!(
                    session_id = %result_session_id,
                    completion_status = %completion.status,
                    completion_exit_code = completion.exit_code,
                    "Daemon completion packet exists but no authoritative result is available yet; continuing wait"
                );
            } else {
                eprintln!(
                    "Session {} has a daemon completion packet but no terminal result.toml.",
                    resolved.session_id,
                );
                eprintln!(
                    "Run `csa session result --session {}` for diagnostics.",
                    resolved.session_id
                );
                return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
            }
        }

        let completed_result = if session_live {
            None
        } else {
            load_completed_daemon_result_with_fallback(
                effective_root,
                result_session_id,
                result_session_dir,
                result_uses_direct_session_dir,
            )?
        };
        if let Some(mut result) = completed_result {
            super::summary::reconcile_repaired_review_pass_result_status(
                result_session_dir,
                &mut result,
            );
            let streamed_output = (emitters.emit_terminal_output)(
                result_session_dir,
                display_session_id,
                Some(&result),
                wait_options.output_mode,
            )?;
            (emitters.emit_next_step)(result_session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, false, Some(&result));
            emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
            (emitters.emit_completion_signal)(
                &resolved.session_id,
                completion_status.as_ref(),
                exit_code,
                false,
                !streamed_output,
            );
            return Ok(exit_code);
        }

        if !session_live && !handoff_blocks_target_reconcile {
            if result_registry_state_loss {
                emit_registry_state_loss_or_missing_result(
                    effective_root,
                    result_session_id,
                    result_session_dir,
                );
                return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
            }
            // Synthesize terminal result for dead Active sessions.
            let reconciled = (emitters.reconcile_dead_active_session)(
                effective_root,
                result_session_id,
                "session wait",
            )?;
            let reconciled_result = if reconciled.result_became_available {
                load_completed_daemon_result_with_fallback(
                    effective_root,
                    result_session_id,
                    result_session_dir,
                    result_uses_direct_session_dir,
                )?
            } else {
                None
            };
            if let Some(mut result) = reconciled_result {
                super::summary::reconcile_repaired_review_pass_result_status(
                    result_session_dir,
                    &mut result,
                );
                let streamed_output = (emitters.emit_terminal_output)(
                    result_session_dir,
                    display_session_id,
                    Some(&result),
                    wait_options.output_mode,
                )?;
                (emitters.emit_next_step)(result_session_dir)?;
                #[rustfmt::skip]
                let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, reconciled.synthetic, Some(&result));
                emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
                (emitters.emit_completion_signal)(
                    &resolved.session_id,
                    completion_status.as_ref(),
                    exit_code,
                    reconciled.synthetic,
                    !streamed_output,
                );
                if reconciled.synthetic && !streamed_output {
                    eprintln!(
                        "Session {} reached a synthesized terminal result because no live daemon process remained.",
                        resolved.session_id,
                    );
                }
                return Ok(exit_code);
            }
        }

        if !session_live {
            if let Some(mut result) = load_completed_daemon_result_with_fallback(
                effective_root,
                result_session_id,
                result_session_dir,
                result_uses_direct_session_dir,
            )? {
                super::summary::reconcile_repaired_review_pass_result_status(
                    result_session_dir,
                    &mut result,
                );
                let streamed_output = (emitters.emit_terminal_output)(
                    result_session_dir,
                    display_session_id,
                    Some(&result),
                    wait_options.output_mode,
                )?;
                (emitters.emit_next_step)(result_session_dir)?;
                #[rustfmt::skip]
                let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, false, Some(&result));
                emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
                (emitters.emit_completion_signal)(
                    &resolved.session_id,
                    completion_status.as_ref(),
                    exit_code,
                    false,
                    !streamed_output,
                );
                return Ok(exit_code);
            }
            if csa_process::ToolLiveness::is_alive(result_session_dir) {
                // PID detection missed but filesystem liveness signals say alive;
                // continue polling so the timeout (exit 124) fires instead of exit 1.
                tracing::debug!(session_id = %result_session_id, "alive; no terminal PID");
            } else if handoff_blocks_target_reconcile {
                tracing::debug!(
                    wrapper_session_id = %resolved.session_id,
                    target_session_id = %result_session_id,
                    "resume wrapper still owns target handoff; continuing wait without target reconciliation"
                );
            } else {
                eprintln!(
                    "Session {} has no live daemon process and no terminal result packet.",
                    resolved.session_id,
                );
                eprintln!(
                    "Run `csa session result --session {}` for diagnostics.",
                    resolved.session_id
                );
                return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
            }
        }

        let memory_sample_limit_mb = match (memory_warn_mb, next_memory_sample_at) {
            (Some(limit_mb), Some(sample_at)) if std::time::Instant::now() >= sample_at => {
                Some(limit_mb)
            }
            _ => None,
        };
        if let Some(limit_mb) = memory_sample_limit_mb {
            match (emitters.sample_session_tree_rss_mb)(effective_root, result_session_id) {
                Ok(actual_rss_mb) => {
                    if actual_rss_mb > limit_mb {
                        (emitters.emit_memory_warn_marker)(
                            result_session_id,
                            actual_rss_mb,
                            limit_mb,
                        );
                        return Ok(SESSION_WAIT_MEMORY_WARN_EXIT_CODE);
                    }
                    next_memory_sample_at = Some(
                        std::time::Instant::now()
                            + wait_options.behavior.timing.memory_sample_interval,
                    );
                }
                Err(err) => {
                    tracing::debug!(
                        session_id = %result_session_id,
                        error = %err,
                        "Session wait memory sampler failed; will retry"
                    );
                    next_memory_sample_at = (err.kind() != std::io::ErrorKind::Unsupported)
                        .then_some(
                            std::time::Instant::now()
                                + wait_options.behavior.timing.memory_sample_interval,
                        );
                }
            }
        }

        let elapsed = start.elapsed().as_secs();
        if elapsed >= wait_options.behavior.wait_timeout_secs {
            let session_alive = session_has_live_execution(
                worktree_lock_root.as_deref(),
                result_session_dir,
                &resolved.session_id,
                result_session_id,
            ) || csa_process::ToolLiveness::is_alive(result_session_dir)
                || handoff_blocks_target_reconcile;
            let session_alive = session_alive && !result_session_is_retired;
            return Ok(emit_wait_cap_outcome(
                &resolved.session_id,
                cd.as_deref(),
                wait_options.behavior.wait_timeout_secs,
                elapsed,
                session_alive,
            ));
        }

        std::thread::sleep(wait_options.behavior.timing.poll_interval);
    }
}
