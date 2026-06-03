use super::*;
use chrono::Utc;
use csa_config::GlobalConfig;
use std::time::{Duration, Instant, SystemTime};

#[path = "session_cmds_daemon_wait_completion.rs"]
mod completion;
#[path = "session_cmds_daemon_wait_core.rs"]
mod core;
#[path = "session_cmds_daemon_wait_lock.rs"]
mod lock;
#[path = "session_cmds_daemon_wait_next_step.rs"]
mod next_step;
#[path = "session_cmds_daemon_wait_result.rs"]
mod result_loader;
#[path = "session_cmds_daemon_wait_summary.rs"]
mod summary;
#[path = "session_cmds_daemon_wait_types.rs"]
mod types;
// Re-export the memory warning exit code constant for other modules
#[allow(unused_imports)]
pub(crate) use completion::SESSION_WAIT_MEMORY_WARN_EXIT_CODE;
#[cfg(test)]
pub(crate) use completion::resolve_wait_completion_status_and_exit;
pub(crate) use lock::try_acquire_session_wait_lock;
pub(crate) use next_step::synthesized_wait_next_step;
use result_loader::{load_completed_daemon_result_with_fallback, refresh_result_for_wait};
use summary::emit_wait_terminal_output;
#[cfg(test)]
pub(crate) use summary::render_wait_result_summary;
use types::WaitExecutionOptions;
#[cfg(test)]
pub(crate) use types::WaitLoopTiming;
pub(crate) use types::{SessionWaitOutputMode, WaitBehavior, WaitReconciliationOutcome};

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

#[cfg(test)]
pub(crate) fn handle_session_wait_with_memory_warn(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
    memory_warn_mb: Option<u64>,
) -> Result<i32> {
    handle_session_wait_with_options(
        session,
        cd,
        wait_timeout_secs,
        memory_warn_mb,
        SessionWaitOutputMode::from_flags(false, false),
    )
}

pub(crate) fn handle_session_wait_with_options(
    session: String,
    cd: Option<String>,
    wait_timeout_secs: u64,
    memory_warn_mb: Option<u64>,
    output_mode: SessionWaitOutputMode,
) -> Result<i32> {
    handle_session_wait_with_hooks_output_mode(
        session,
        cd,
        WaitBehavior::new(wait_timeout_secs, memory_warn_mb),
        output_mode,
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

#[cfg(test)]
pub(crate) fn handle_session_wait_with_hooks<R, E>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    reconcile_dead_active_session: R,
    emit_completion_signal: E,
) -> Result<i32>
where
    R: for<'a, 'b, 'c> FnMut(&'a Path, &'b str, &'c str) -> Result<WaitReconciliationOutcome>,
    E: for<'a, 'b> FnMut(&'a str, &'b str, i32, bool, bool),
{
    handle_session_wait_with_hooks_output_mode(
        session,
        cd,
        wait_behavior,
        SessionWaitOutputMode::from_flags(false, false),
        reconcile_dead_active_session,
        emit_completion_signal,
    )
}

fn handle_session_wait_with_hooks_output_mode<R, E>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    output_mode: SessionWaitOutputMode,
    mut reconcile_dead_active_session: R,
    mut emit_completion_signal: E,
) -> Result<i32>
where
    R: for<'a, 'b, 'c> FnMut(&'a Path, &'b str, &'c str) -> Result<WaitReconciliationOutcome>,
    E: for<'a, 'b> FnMut(&'a str, &'b str, i32, bool, bool),
{
    let mut cached_memory_sampler: Option<csa_session::SessionTreeMemorySampler> = None;
    handle_session_wait_with_hooks_and_sampler_output_mode(
        session,
        cd,
        WaitExecutionOptions {
            behavior: wait_behavior,
            output_mode,
        },
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

#[cfg(test)]
pub(crate) fn handle_session_wait_with_hooks_and_sampler<R, E, S, M>(
    session: String,
    cd: Option<String>,
    wait_behavior: WaitBehavior,
    reconcile_dead_active_session: R,
    emit_completion_signal: E,
    sample_session_tree_rss_mb: S,
    emit_memory_warn_marker: M,
) -> Result<i32>
where
    R: FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome>,
    E: FnMut(&str, &str, i32, bool, bool),
    S: FnMut(&Path, &str) -> std::io::Result<u64>,
    M: FnMut(&str, u64, u64),
{
    handle_session_wait_with_hooks_and_sampler_output_mode(
        session,
        cd,
        WaitExecutionOptions {
            behavior: wait_behavior,
            output_mode: SessionWaitOutputMode::from_flags(false, false),
        },
        reconcile_dead_active_session,
        emit_completion_signal,
        sample_session_tree_rss_mb,
        emit_memory_warn_marker,
    )
}

fn handle_session_wait_with_hooks_and_sampler_output_mode<R, E, S, M>(
    session: String,
    cd: Option<String>,
    wait_options: WaitExecutionOptions,
    reconcile_dead_active_session: R,
    emit_completion_signal: E,
    sample_session_tree_rss_mb: S,
    emit_memory_warn_marker: M,
) -> Result<i32>
where
    R: FnMut(&Path, &str, &str) -> Result<WaitReconciliationOutcome>,
    E: FnMut(&str, &str, i32, bool, bool),
    S: FnMut(&Path, &str) -> std::io::Result<u64>,
    M: FnMut(&str, u64, u64),
{
    core::handle_session_wait_with_hooks_and_sampler_output_mode(
        session,
        cd,
        wait_options,
        reconcile_dead_active_session,
        emit_completion_signal,
        sample_session_tree_rss_mb,
        emit_memory_warn_marker,
    )
}
fn emit_wait_next_step_if_needed(session_dir: &Path) -> Result<()> {
    if let Some(directive) = synthesized_wait_next_step(session_dir)? {
        println!("{directive}");
    }
    Ok(())
}

fn emit_wait_completion_signal(
    session_id: &str,
    status: &str,
    exit_code: i32,
    synthetic: bool,
    mirror_to_stdout: bool,
) {
    let signal = format!(
        "<!-- CSA:SESSION_WAIT_COMPLETED session={} status={} exit={} synthetic={} -->",
        session_id, status, exit_code, synthetic
    );
    if mirror_to_stdout {
        println!("{signal}");
    }
    eprintln!("{signal}");
    eprintln!(
        "<!-- CSA:CALLER_HINT action=\"next_session\" \
         rule=\"If you have more sessions to wait for, call each 'csa session wait' \
         in a SEPARATE Bash call. NEVER batch. \
         Generate tokens between waits to keep your KV cache warm. \
         If you background the wait (run_in_background: true), the completion task-notification IS your wake signal — do NOT stack ScheduleWakeup, /loop, or sleep loops on top; that's pure redundancy and wastes tokens. \
         FORBIDDEN: ls/cat/wc/grep on session-dir, state.toml reads, ps checks on daemon PID — \
         any manual polling wastes caller tokens with zero benefit. \
         FORBIDDEN: piping csa commands through 2>/dev/null. CSA errors on stderr are diagnostic — \
         suppressing them hides invalid-argument errors and causes silent retry loops that waste thousands of tokens.\" -->"
    );
    let codex_hint = crate::process_tree::codex_yield_hint();
    if !codex_hint.is_empty() {
        eprint!("{codex_hint}");
    }
}

/// Check if a session is stale before starting to wait.
/// Returns Err if the session is stale (daemon not running, no recent progress).
fn check_session_stale_before_wait(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    wait_behavior: WaitBehavior,
) -> anyhow::Result<()> {
    // Load the session to check its phase and last_accessed time.
    // Only flag truly stale sessions (Active phase, no daemon, no recent progress).
    // Sessions with an existing result.toml are NOT stale — they completed, and the
    // main polling loop will return the result correctly.
    match load_session_for_stale_precheck(project_root, session_id, session_dir, wait_behavior) {
        Ok(session) => {
            // Only check Active sessions for staleness
            if matches!(session.phase, csa_session::SessionPhase::Active) {
                // If there's already a terminal result (or a result file exists but
                // fails to parse), let the main loop handle it rather than flagging
                // as stale. The session completed normally; parse errors are handled
                // downstream with better diagnostics.
                if super::daemon_completion_exists(session_dir)
                    && !session_has_terminal_process(session_dir)
                {
                    return Ok(());
                }
                match csa_session::load_result(project_root, session_id) {
                    Ok(Some(_)) => return Ok(()),
                    Err(_) => return Ok(()), // result file exists but unparseable
                    Ok(None) => {}           // no result yet
                }

                let stale_threshold_seconds =
                    GlobalConfig::resolve_session_wait_long_poll_seconds().saturating_mul(2);
                let now = Utc::now();
                let elapsed = now.signed_duration_since(session.last_accessed);

                if elapsed > chrono::Duration::seconds(stale_threshold_seconds as i64) {
                    if session_has_terminal_process(session_dir) {
                        return Ok(());
                    }
                    return Err(anyhow::anyhow!(
                        "daemon not running, no recent progress ({}s > {}s threshold)",
                        elapsed.num_seconds(),
                        stale_threshold_seconds
                    ));
                }
            }
        }
        Err(load_err) => {
            return Err(anyhow::anyhow!("failed to load session: {}", load_err));
        }
    }

    Ok(())
}

fn load_session_for_stale_precheck(
    project_root: &Path,
    session_id: &str,
    session_dir: &Path,
    wait_behavior: WaitBehavior,
) -> anyhow::Result<csa_session::MetaSessionState> {
    let state_path = session_dir.join("state.toml");
    let retry_until = startup_state_retry_deadline(
        &state_path,
        session_dir,
        Duration::from_secs(wait_behavior.wait_timeout_secs),
    );

    loop {
        match csa_session::load_session(project_root, session_id) {
            Ok(session) => return Ok(session),
            Err(load_err) => {
                let Some(deadline) = retry_until else {
                    return Err(load_err);
                };
                if Instant::now() >= deadline || !state_load_error_can_be_initializing(&state_path)
                {
                    return Err(load_err);
                }

                tracing::debug!(
                    session_id,
                    state_path = %state_path.display(),
                    "Session state is still initializing; retrying stale precheck load"
                );
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(wait_behavior.timing.poll_interval),
                );
            }
        }
    }
}

fn startup_state_retry_deadline(
    state_path: &Path,
    session_dir: &Path,
    retry_window: Duration,
) -> Option<Instant> {
    if retry_window.is_zero() {
        return None;
    }

    let modified = state_retry_modified_time(state_path, session_dir)?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    if age > retry_window {
        return None;
    }

    Some(Instant::now() + retry_window.saturating_sub(age))
}

fn state_retry_modified_time(state_path: &Path, session_dir: &Path) -> Option<SystemTime> {
    match std::fs::metadata(state_path) {
        Ok(metadata) => metadata.modified().ok(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::metadata(session_dir).ok()?.modified().ok()
        }
        Err(_) => None,
    }
}

fn state_load_error_can_be_initializing(state_path: &Path) -> bool {
    match std::fs::read_to_string(state_path) {
        Ok(_) => true,
        Err(err) => err.kind() == std::io::ErrorKind::NotFound,
    }
}

fn emit_wait_memory_warn_marker(session_id: &str, rss_mb: u64, limit_mb: u64) {
    println!(
        "<!-- CSA:MEMORY_WARN session={} rss_mb={} limit_mb={} -->",
        session_id, rss_mb, limit_mb
    );
}
