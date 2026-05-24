use super::*;
use std::io::Write;

#[path = "session_cmds_daemon_wait_lock.rs"]
mod lock;
#[path = "session_cmds_daemon_wait_next_step.rs"]
mod next_step;
#[path = "session_cmds_daemon_wait_result.rs"]
mod result_loader;
pub(crate) use lock::try_acquire_session_wait_lock;
pub(crate) use next_step::synthesized_wait_next_step;
use result_loader::{load_completed_daemon_result_with_fallback, refresh_result_for_wait};

/// Exit code reserved for `csa session wait` memory warning early-exit.
pub(crate) const SESSION_WAIT_MEMORY_WARN_EXIT_CODE: i32 = 33;
const SESSION_WAIT_SUCCESS_EXIT_CODE: i32 = 0;
const SESSION_WAIT_FAILURE_EXIT_CODE: i32 = 1;
/// Healthy poll-cap exit when the session is still alive: callers should
/// process tokens (warming their KV cache) and re-wait. See #1439.
const SESSION_WAIT_KV_WARM_EXIT_CODE: i32 = 0;
/// Reserved for the rare case where the wait cap is reached but the session
/// daemon is no longer alive and no result.toml was produced.
const SESSION_WAIT_TIMEOUT_EXIT_CODE: i32 = 124;
const SESSION_WAIT_MEMORY_SAMPLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
pub(crate) struct WaitLoopTiming {
    pub(crate) poll_interval: std::time::Duration,
    pub(crate) memory_sample_interval: std::time::Duration,
}

impl Default for WaitLoopTiming {
    fn default() -> Self {
        Self {
            poll_interval: std::time::Duration::from_secs(1),
            memory_sample_interval: SESSION_WAIT_MEMORY_SAMPLE_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WaitBehavior {
    pub(crate) wait_timeout_secs: u64,
    pub(crate) memory_warn_mb: Option<u64>,
    pub(crate) timing: WaitLoopTiming,
}

impl WaitBehavior {
    fn new(wait_timeout_secs: u64, memory_warn_mb: Option<u64>) -> Self {
        Self {
            wait_timeout_secs,
            memory_warn_mb,
            timing: WaitLoopTiming::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitReconciliationOutcome {
    pub(crate) result_became_available: bool,
    pub(crate) synthetic: bool,
}

/// Wait for a daemon session to reach a terminal result and daemon exit.
/// Exits 0 on session success, 1 on terminal session failure, 124 when the
/// wait times out while the session is still active, and 33 for memory warnings.
#[cfg(test)]
pub(crate) fn handle_session_wait(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
) -> Result<i32> {
    handle_session_wait_with_memory_warn(session, cd, wait_timeout_secs, None)
}

pub(crate) fn handle_session_wait_with_memory_warn(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
    memory_warn_mb: Option<u64>,
) -> Result<i32> {
    handle_session_wait_with_hooks(
        session,
        cd,
        WaitBehavior::new(wait_timeout_secs, memory_warn_mb),
        |project_root, session_id, trigger| {
            let reconciled = crate::session_cmds::ensure_terminal_result_for_dead_active_session(
                project_root,
                session_id,
                trigger,
            )?;
            Ok(WaitReconciliationOutcome {
                result_became_available: reconciled.result_became_available(),
                synthetic: reconciled.synthesized_failure(),
            })
        },
        emit_wait_completion_signal,
    )
}

pub(crate) fn handle_session_wait_with_hooks<R, E>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
) -> Result<i32>
where
    R: for<'a, 'b, 'c> FnMut(&'a Path, &'b str, &'c str) -> Result<WaitReconciliationOutcome>,
    E: for<'a, 'b> FnMut(&'a str, &'b str, i32, bool, bool),
{
    let mut cached_memory_sampler: Option<csa_session::SessionTreeMemorySampler> = None;
    handle_session_wait_with_hooks_and_sampler(
        session,
        cd,
        wait_behavior,
        &mut reconcile_dead_active_session,
        &mut emit_completion_signal,
        |project_root, session_id| {
            if cached_memory_sampler.is_none() {
                cached_memory_sampler = Some(csa_session::SessionTreeMemorySampler::new(
                    project_root,
                    session_id,
                )?);
            }
            cached_memory_sampler
                .as_ref()
                .expect("cached sampler initialized above")
                .sample_rss_mb()
        },
        emit_wait_memory_warn_marker,
    )
}

pub(crate) fn handle_session_wait_with_hooks_and_sampler<R, E, S, M>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
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
    let memory_warn_mb = wait_behavior.memory_warn_mb.filter(|limit| *limit > 0);
    let mut next_memory_sample_at =
        memory_warn_mb.map(|_| start + wait_behavior.timing.memory_sample_interval);

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
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
            #[rustfmt::skip]
            let (completion_status, exit_code) = resolve_wait_completion_status_and_exit(completion.status.as_str(), completion.exit_code, synthetic, loaded_result.as_ref());
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

        if let Some(result) = load_completed_daemon_result_with_fallback(
            effective_root,
            &resolved.session_id,
            &session_dir,
            is_cross_project,
        )? {
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
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
            let streamed_output = stream_wait_output(&session_dir)?;
            emit_wait_next_step_if_needed(&session_dir)?;
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
                let streamed_output = stream_wait_output(&session_dir)?;
                emit_wait_next_step_if_needed(&session_dir)?;
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
                        std::time::Instant::now() + wait_behavior.timing.memory_sample_interval,
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
                            std::time::Instant::now() + wait_behavior.timing.memory_sample_interval,
                        );
                }
            }
        }

        let elapsed = start.elapsed().as_secs();
        if elapsed >= wait_behavior.wait_timeout_secs {
            let cd_arg = cd
                .as_ref()
                .map(|path| format!(" --cd '{}'", path))
                .unwrap_or_default();
            let session_alive = session_has_terminal_process(&session_dir)
                || csa_process::ToolLiveness::is_alive(&session_dir);
            if session_alive {
                // KV-warm exit: session still alive at the wait cap. See #1439.
                eprintln!(
                    "Session {} still running after {}s wait cap; returning so caller can warm its KV cache before re-waiting.",
                    resolved.session_id, wait_behavior.wait_timeout_secs,
                );
                eprintln!(
                    "<!-- CSA:SESSION_WAIT_KV_WARM session={} status=alive elapsed={}s action=re-wait cmd=\"csa session wait --session {}{}\" -->",
                    resolved.session_id, elapsed, resolved.session_id, cd_arg,
                );
                eprintln!(
                    "<!-- CSA:CALLER_HINT action=\"retry_wait\" \
                     rule=\"Session is alive; this exit is healthy. Process this output NOW and generate tokens to warm your KV cache, \
                     then call 'csa session wait --session {sid}{cd}' again in a NEW Bash call. \
                     NEVER batch multiple session waits in one Bash call. \
                     If you background the wait (run_in_background: true), the completion task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top; that's pure redundancy and wastes tokens. \
                     FORBIDDEN: ls/cat/wc/grep on session-dir, state.toml reads, ps checks on daemon PID — \
                     any manual polling wastes caller tokens with zero benefit.\" -->",
                    sid = resolved.session_id,
                    cd = cd
                        .as_ref()
                        .map(|p| format!(" --cd '{p}'"))
                        .unwrap_or_default(),
                );
                return Ok(SESSION_WAIT_KV_WARM_EXIT_CODE);
            }
            // Defensive: daemon gone with no result.toml (rare; earlier loop branches usually exit-1 first).
            eprintln!(
                "Timeout: session {} did not complete within {}s and no live daemon process remains.",
                resolved.session_id, wait_behavior.wait_timeout_secs,
            );
            eprintln!(
                "<!-- CSA:SESSION_WAIT_TIMEOUT session={} elapsed={}s status=dead cmd=\"csa session result --session {}{}\" -->",
                resolved.session_id, elapsed, resolved.session_id, cd_arg,
            );
            return Ok(SESSION_WAIT_TIMEOUT_EXIT_CODE);
        }

        std::thread::sleep(wait_behavior.timing.poll_interval);
    }
}

fn stream_wait_output(session_dir: &std::path::Path) -> Result<bool> {
    let stdout_log = session_dir.join("stdout.log");
    if !stdout_log.is_file() {
        return Ok(false);
    }
    let raw = std::fs::read(&stdout_log)?;
    let Some(rendered) = crate::codex_transcript_filter::render_codex_or_plain_output(&raw) else {
        return Ok(false);
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(rendered.as_bytes())?;
    let bytes = rendered.len() as u64;
    stdout.flush()?;
    Ok(bytes > 0)
}

fn emit_wait_next_step_if_needed(session_dir: &Path) -> Result<()> {
    if let Some(directive) = synthesized_wait_next_step(session_dir)? {
        println!("{directive}");
    }
    Ok(())
}

fn resolve_wait_completion_status_and_exit<'a>(
    fallback_status: &'a str,
    fallback_exit_code: i32,
    synthetic: bool,
    real_result: Option<&'a csa_session::SessionResult>,
) -> (Cow<'a, str>, i32) {
    if synthetic {
        return (Cow::Borrowed("failure"), SESSION_WAIT_FAILURE_EXIT_CODE);
    }
    real_result.map_or_else(
        || {
            (
                Cow::Borrowed(fallback_status),
                terminal_result_wait_exit_code(fallback_status, fallback_exit_code),
            )
        },
        |result| {
            (
                Cow::Borrowed(result.status.as_str()),
                terminal_result_wait_exit_code(result.status.as_str(), result.exit_code),
            )
        },
    )
}

fn terminal_result_wait_exit_code(status: &str, exit_code: i32) -> i32 {
    if matches!(status, "success" | "retired") && exit_code == 0 {
        SESSION_WAIT_SUCCESS_EXIT_CODE
    } else {
        SESSION_WAIT_FAILURE_EXIT_CODE
    }
}

fn emit_wait_completion_signal(
    session_id: &str,
    status: &str,
    exit_code: i32,
    synthetic: bool,
    _mirror_to_stdout: bool,
) {
    let signal = format!(
        "<!-- CSA:SESSION_WAIT_COMPLETED session={} status={} exit={} synthetic={} -->",
        session_id, status, exit_code, synthetic
    );
    println!("{signal}");
    eprintln!("{signal}");
    eprintln!(
        "<!-- CSA:CALLER_HINT action=\"next_session\" \
         rule=\"If you have more sessions to wait for, call each 'csa session wait' \
         in a SEPARATE Bash call. NEVER batch. \
         Generate tokens between waits to keep your KV cache warm. \
         If you background the wait (run_in_background: true), the completion task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top; that's pure redundancy and wastes tokens. \
         FORBIDDEN: ls/cat/wc/grep on session-dir, state.toml reads, ps checks on daemon PID — \
         any manual polling wastes caller tokens with zero benefit.\" -->"
    );
}

fn emit_wait_memory_warn_marker(session_id: &str, rss_mb: u64, limit_mb: u64) {
    println!(
        "<!-- CSA:MEMORY_WARN session={} rss_mb={} limit_mb={} -->",
        session_id, rss_mb, limit_mb
    );
}
