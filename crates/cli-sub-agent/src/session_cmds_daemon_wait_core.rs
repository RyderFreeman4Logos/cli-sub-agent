//! Core polling loop for `csa session wait`.
//!
//! Contains the main session wait logic with completion detection, memory sampling,
//! timeout detection, and reconciliation of dead sessions.
//!
//! Extracted from `session_cmds_daemon_wait.rs` to reduce module complexity.

use std::path::Path;

use anyhow::Result;

use super::completion::{
    SESSION_WAIT_FAILURE_EXIT_CODE, SESSION_WAIT_KV_WARM_EXIT_CODE,
    SESSION_WAIT_MEMORY_WARN_EXIT_CODE, SESSION_WAIT_TIMEOUT_EXIT_CODE,
    resolve_wait_completion_status_and_exit,
};
use super::types::{WaitExecutionOptions, WaitReconciliationOutcome};
use super::{
    emit_wait_terminal_output, load_completed_daemon_result_with_fallback, refresh_result_for_wait,
    try_acquire_session_wait_lock,
};
use crate::session_cmds::resolve_session_prefix_with_global_fallback;
use crate::session_cmds_daemon::{
    emit_failure_summary_for_empty_output, finalize_daemon_completion_if_present,
    load_daemon_completion_packet, session_has_terminal_process,
};

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
    R: FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome>,
    E: FnMut(&str, &str, i32, bool, bool),
    S: FnMut(&Path, &str) -> std::io::Result<u64>,
    M: FnMut(&str, u64, u64),
{
    let project_root = crate::pipeline::determine_project_root(cd.as_deref())?;
    let resolved = resolve_session_prefix_with_global_fallback(&project_root, &session)?;
    // For cross-project sessions, derive session_dir from the resolved sessions_dir
    let session_dir = resolved.sessions_dir.join(&resolved.session_id);
    let _wait_lock = match try_acquire_session_wait_lock(&session_dir)? {
        Some(lock) => lock,
        None => {
            eprintln!(
                "ERROR: another csa session wait is already running for session {} (lock held). The existing wait will notify you on completion. Do NOT re-issue.",
                resolved.session_id
            );
            return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
        }
    };

    // Use the foreign project root for cross-project sessions, local otherwise.
    let effective_root = resolved
        .foreign_project_root
        .as_deref()
        .unwrap_or(&project_root);
    let is_cross_project = resolved.foreign_project_root.is_some();

    let start = std::time::Instant::now();
    let memory_warn_mb = wait_options
        .behavior
        .memory_warn_mb
        .filter(|limit| *limit > 0);
    let mut next_memory_sample_at =
        memory_warn_mb.map(|_| start + wait_options.behavior.timing.memory_sample_interval);

    // Check if the session is Stale before entering the polling loop.
    // This prevents indefinite polling of sessions that have no live daemon process
    // and no progress for an extended period.
    if let Err(stale_err) = super::check_session_stale_before_wait(
        effective_root,
        &resolved.session_id,
        &session_dir,
        wait_options.behavior,
    ) {
        eprintln!(
            "Session {} appears stale: {}",
            resolved.session_id, stale_err
        );
        eprintln!(
            "Run `csa session result --session {}` for diagnostics.",
            resolved.session_id
        );
        return Ok(SESSION_WAIT_FAILURE_EXIT_CODE);
    }

    loop {
        if let Some(completion) = load_daemon_completion_packet(&session_dir)?
            && !session_has_terminal_process(&session_dir)
        {
            let refreshed_result = refresh_result_for_wait(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            );
            if let Err(err) = &refreshed_result {
                tracing::debug!(
                    session_id = %resolved.session_id,
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
                crate::session_cmds::retire_if_dead_with_result(
                    effective_root,
                    &resolved.session_id,
                    "session wait",
                )?;
            } else {
                loaded_result = finalize_daemon_completion_if_present(&session_dir)?;
                if loaded_result.is_none() {
                    let reconciled = reconcile_dead_active_session(
                        effective_root,
                        &resolved.session_id,
                        "session wait",
                    )?;
                    synthetic = reconciled.synthetic;
                    if reconciled.result_became_available {
                        loaded_result = load_completed_daemon_result_with_fallback(
                            effective_root,
                            &resolved.session_id,
                            &session_dir,
                            is_cross_project,
                        )?;
                    }
                }
            }
            if let Some(result) = loaded_result {
                let streamed_output = emit_wait_terminal_output(
                    &session_dir,
                    &resolved.session_id,
                    Some(&result),
                    wait_options.output_mode,
                )?;
                super::emit_wait_next_step_if_needed(&session_dir)?;
                #[rustfmt::skip]
                let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(completion.status.as_str(), completion.exit_code, synthetic, Some(&result));
                emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
                emit_completion_signal(
                    &resolved.session_id,
                    completion_status.as_ref(),
                    exit_code,
                    synthetic,
                    !streamed_output,
                );
                return Ok(exit_code);
            }

            if csa_process::ToolLiveness::is_alive(&session_dir) {
                tracing::debug!(
                    session_id = %resolved.session_id,
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

        if let Some(result) = load_completed_daemon_result_with_fallback(
            effective_root,
            &resolved.session_id,
            &session_dir,
            is_cross_project,
        )? {
            let streamed_output = emit_wait_terminal_output(
                &session_dir,
                &resolved.session_id,
                Some(&result),
                wait_options.output_mode,
            )?;
            super::emit_wait_next_step_if_needed(&session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, false, Some(&result));
            emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
            emit_completion_signal(
                &resolved.session_id,
                completion_status.as_ref(),
                exit_code,
                false,
                !streamed_output,
            );
            return Ok(exit_code);
        }

        // Synthesize terminal result for dead Active sessions.
        let reconciled =
            reconcile_dead_active_session(effective_root, &resolved.session_id, "session wait")?;
        if reconciled.result_became_available
            && let Some(result) = load_completed_daemon_result_with_fallback(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            )?
        {
            let streamed_output = emit_wait_terminal_output(
                &session_dir,
                &resolved.session_id,
                Some(&result),
                wait_options.output_mode,
            )?;
            super::emit_wait_next_step_if_needed(&session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, reconciled.synthetic, Some(&result));
            emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
            emit_completion_signal(
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

        if !session_has_terminal_process(&session_dir) {
            if let Some(result) = load_completed_daemon_result_with_fallback(
                effective_root,
                &resolved.session_id,
                &session_dir,
                is_cross_project,
            )? {
                let streamed_output = emit_wait_terminal_output(
                    &session_dir,
                    &resolved.session_id,
                    Some(&result),
                    wait_options.output_mode,
                )?;
                super::emit_wait_next_step_if_needed(&session_dir)?;
                #[rustfmt::skip]
                let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(result.status.as_str(), result.exit_code, false, Some(&result));
                emit_failure_summary_for_empty_output(&session_dir, streamed_output, false);
                emit_completion_signal(
                    &resolved.session_id,
                    completion_status.as_ref(),
                    exit_code,
                    false,
                    !streamed_output,
                );
                return Ok(exit_code);
            }
            if csa_process::ToolLiveness::is_alive(&session_dir) {
                // PID detection missed but filesystem liveness signals say alive;
                // continue polling so the timeout (exit 124) fires instead of exit 1.
                tracing::debug!(session_id = %resolved.session_id, "alive; no terminal PID");
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

        if let (Some(limit_mb), Some(sample_at)) = (memory_warn_mb, next_memory_sample_at)
            && std::time::Instant::now() >= sample_at
        {
            match sample_session_tree_rss_mb(effective_root, &resolved.session_id) {
                Ok(actual_rss_mb) => {
                    if actual_rss_mb > limit_mb {
                        emit_memory_warn_marker(&resolved.session_id, actual_rss_mb, limit_mb);
                        return Ok(SESSION_WAIT_MEMORY_WARN_EXIT_CODE);
                    }
                    next_memory_sample_at = Some(
                        std::time::Instant::now()
                            + wait_options.behavior.timing.memory_sample_interval,
                    );
                }
                Err(err) => {
                    tracing::debug!(
                        session_id = %resolved.session_id,
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
            let cd_arg = cd
                .as_ref()
                .map(|path| crate::daemon_caller_hints::format_cd_arg(Path::new(path)))
                .unwrap_or_default();
            let session_alive = session_has_terminal_process(&session_dir)
                || csa_process::ToolLiveness::is_alive(&session_dir);
            if session_alive {
                let wait_cmd = format!(
                    "csa session wait --session {}{}",
                    resolved.session_id, cd_arg
                );
                let wait_cmd_attr =
                    crate::daemon_caller_hints::escape_structured_comment_attr(&wait_cmd);
                // KV-warm exit: session still alive at the wait cap. See #1439.
                eprintln!(
                    "Session {} still running after {}s wait cap; returning so caller can warm its KV cache before re-waiting.",
                    resolved.session_id, wait_options.behavior.wait_timeout_secs,
                );
                eprintln!(
                    "<!-- CSA:SESSION_WAIT_KV_WARM session={} status=alive elapsed={}s action=re-wait cmd=\"{}\" -->",
                    resolved.session_id, elapsed, wait_cmd_attr,
                );
                eprintln!(
                    "<!-- CSA:CALLER_HINT action=\"retry_wait\" \
                     rule=\"Session is alive; this exit is healthy. Process this output NOW and generate tokens to warm your KV cache, \
                     then call {wait_cmd} again in a NEW Bash call. \
                     NEVER batch multiple session waits in one Bash call. \
                     If you background the wait (run_in_background: true), the completion task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top; that's pure redundancy and wastes tokens. \
                     FORBIDDEN: ls/cat/wc/grep on session-dir, state.toml reads, ps checks on daemon PID — \
                     any manual polling wastes caller tokens with zero benefit. \
                     FORBIDDEN: piping csa commands through 2>/dev/null. CSA errors on stderr are diagnostic — \
                     suppressing them hides invalid-argument errors and causes silent retry loops that waste thousands of tokens.\" -->",
                    wait_cmd = wait_cmd_attr,
                );
                let codex_hint = crate::process_tree::codex_yield_hint();
                if !codex_hint.is_empty() {
                    eprint!("{codex_hint}");
                }
                return Ok(SESSION_WAIT_KV_WARM_EXIT_CODE);
            }
            // Defensive: daemon gone with no result.toml (rare; earlier loop branches usually exit-1 first).
            eprintln!(
                "Timeout: session {} did not complete within {}s and no live daemon process remains.",
                resolved.session_id, wait_options.behavior.wait_timeout_secs,
            );
            let result_cmd = format!(
                "csa session result --session {}{}",
                resolved.session_id, cd_arg
            );
            let result_cmd_attr =
                crate::daemon_caller_hints::escape_structured_comment_attr(&result_cmd);
            eprintln!(
                "<!-- CSA:SESSION_WAIT_TIMEOUT session={} elapsed={}s status=dead cmd=\"{}\" -->",
                resolved.session_id, elapsed, result_cmd_attr,
            );
            return Ok(SESSION_WAIT_TIMEOUT_EXIT_CODE);
        }

        std::thread::sleep(wait_options.behavior.timing.poll_interval);
    }
}
