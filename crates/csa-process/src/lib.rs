//! Process management: spawning, signal handling, and output capture.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tracing::warn;
mod idle_watchdog;
use idle_watchdog::should_terminate_for_idle;
#[path = "lib_output_helpers.rs"]
mod output_helpers;
#[path = "lib_subprocess_helpers.rs"]
mod subprocess_helpers;
mod tool_liveness;
pub use output_helpers::{
    CompressDecision, DEFAULT_SPOOL_KEEP_ROTATED, DEFAULT_SPOOL_MAX_BYTES, SpoolRotator,
    sanitize_spool_plan, should_compress_output,
};
#[cfg(test)]
use output_helpers::{DEFAULT_HEARTBEAT_SECS, HEARTBEAT_INTERVAL_ENV};
use output_helpers::{
    accumulate_and_flush_lines, accumulate_and_flush_stderr,
    append_actionable_detail_for_opaque_payload, drain_if_over_high_water, extract_summary,
    failure_summary, flush_line_buf, flush_stderr_buf, maybe_emit_heartbeat,
    resolve_actionable_failure_detail, resolve_heartbeat_interval, sanitize_opaque_object_payloads,
    spool_chunk,
};
#[cfg(test)]
use output_helpers::{last_non_empty_line, truncate_line};
pub use subprocess_helpers::check_tool_installed;
use subprocess_helpers::terminate_child_process_group;
use tool_liveness::record_spool_bytes_written;
pub use tool_liveness::{DEFAULT_LIVENESS_DEAD_SECS, ToolLiveness};

#[cfg(feature = "codex-pty-fork")]
pub mod pty_fork;

/// Controls whether stdout is forwarded to stderr in real-time.
///
/// By default, stdout is both buffered and forwarded to stderr with a
/// `[stdout] ` prefix, allowing callers to distinguish "thinking" from "hung".
/// Set to `BufferOnly` to suppress real-time streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamMode {
    /// Only buffer stdout; do not forward.
    BufferOnly,
    /// Buffer stdout AND forward each line to stderr with `[stdout] ` prefix (default).
    #[default]
    TeeToStderr,
}

/// Holds sandbox resources that must live as long as the child process.
///
/// # Signal semantics
///
/// - **`Cgroup`**: The child runs inside a systemd transient scope.  On drop,
///   [`CgroupScopeGuard`] calls `systemctl --user stop <scope>`, which sends
///   `SIGTERM` to **all** processes in the scope.  CSA should let systemd
///   handle cleanup rather than sending signals directly when a scope is active.
///
/// - **`Rlimit`**: `RLIMIT_NPROC` was applied in the child's `pre_exec`.
///   This is a marker variant indicating rlimit-based PID isolation is active.
///
/// - **`None`**: No sandbox active; signal handling is unchanged.
///
/// [`CgroupScopeGuard`]: csa_resource::cgroup::CgroupScopeGuard
pub enum SandboxHandle {
    /// cgroup scope guard -- dropped to stop the scope.
    Cgroup(csa_resource::cgroup::CgroupScopeGuard),
    /// Bubblewrap filesystem sandbox is active.
    Bwrap,
    /// Landlock LSM filesystem restrictions applied in child via `pre_exec`.
    Landlock,
    /// `RLIMIT_NPROC` was applied in child via `pre_exec`.
    Rlimit,
    /// No sandbox active.
    None,
}

/// Result of executing a command.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionResult {
    /// Combined stdout output.
    pub output: String,
    /// Captured stderr output.
    ///
    /// In `StreamMode::TeeToStderr`, stderr is also forwarded to parent stderr
    /// in real-time. In `StreamMode::BufferOnly`, stderr is captured only.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_output: String,
    /// Last non-empty line or truncated output (max 200 chars).
    pub summary: String,
    /// Exit code (1 if signal-killed).
    pub exit_code: i32,
}

impl ExecutionResult {
    /// Consolidate consecutive retry/quota-exhaustion messages in stderr to
    /// reduce noise for orchestrators.  Replaces N consecutive retry lines with
    /// a single summary, preserving the last message for context.
    pub fn consolidate_stderr_retries(&mut self) {
        if self.stderr_output.is_empty() {
            return;
        }

        let lines: Vec<&str> = self.stderr_output.lines().collect();
        let mut consolidated = String::with_capacity(self.stderr_output.len());
        let mut retry_count = 0u32;
        let mut last_retry_line = "";

        for line in &lines {
            if is_retry_noise(line) {
                retry_count += 1;
                last_retry_line = line;
            } else {
                flush_retries(&mut consolidated, retry_count, last_retry_line);
                retry_count = 0;
                last_retry_line = "";
                consolidated.push_str(line);
                consolidated.push('\n');
            }
        }
        flush_retries(&mut consolidated, retry_count, last_retry_line);

        self.stderr_output = consolidated;
    }
}

fn flush_retries(buf: &mut String, count: u32, last_line: &str) {
    match count {
        0 => {}
        1 => {
            buf.push_str(last_line);
            buf.push('\n');
        }
        n => {
            buf.push_str(&format!("[{n} retry messages consolidated] {last_line}\n"));
        }
    }
}

fn is_retry_noise(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    // gemini-cli specific: "Attempt N failed: You have exhausted your capacity ... Retrying after Xms..."
    if l.contains("attempt") && l.contains("failed") && l.contains("retrying after") {
        return true;
    }
    // gemini-cli quota: "exhausted your capacity ... quota will reset"
    if l.contains("exhausted your capacity") && l.contains("quota will reset") {
        return true;
    }
    false
}

pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
pub const DEFAULT_STDIN_WRITE_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_TERMINATION_GRACE_PERIOD_SECS: u64 = 5;
const WORKSPACE_BOUNDARY_ERROR_THRESHOLD: usize = 3;
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Spawn-time process control options.
#[derive(Debug, Clone, Copy)]
pub struct SpawnOptions {
    /// Max duration allowed for writing prompt payload to child stdin.
    pub stdin_write_timeout: Duration,
    /// Keep child stdin piped open even when `stdin_data` is `None`.
    ///
    /// Use this for long-lived interactive processes (e.g. JSON-RPC over stdio)
    /// that require writable stdin beyond initial spawn.
    pub keep_stdin_open: bool,
    /// Maximum spool file size before rotating to `*.rotated`.
    pub spool_max_bytes: u64,
    /// Preserve the rotated spool file after execution completes.
    pub keep_rotated_spool: bool,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            stdin_write_timeout: Duration::from_secs(DEFAULT_STDIN_WRITE_TIMEOUT_SECS),
            keep_stdin_open: false,
            spool_max_bytes: DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: DEFAULT_SPOOL_KEEP_ROTATED,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PreExecPolicy {
    /// Call `setsid()` only — no additional resource enforcement.
    Setsid,
    /// Call `setsid()` + apply `RLIMIT_NPROC` (and ignore memory_max_mb).
    Rlimits {
        memory_max_mb: u64,
        pids_max: Option<u64>,
    },
    /// Call `setsid()` + raise OOM score.  Used when no cgroup or rlimit is
    /// available, as a last-resort signal to the kernel OOM killer.
    OomAdj,
}

#[path = "lib_spawn.rs"]
mod spawn;
pub use spawn::{spawn_tool, spawn_tool_sandboxed, spawn_tool_with_options};

/// Wait for a spawned child process and capture its output.
///
/// - Reads stdout until EOF
/// - Reads stderr in tee mode (forwards each line to parent stderr + accumulates)
/// - When `stream_mode` is [`StreamMode::TeeToStderr`], also forwards each stdout
///   line to stderr with a `[stdout] ` prefix for real-time observability
/// - Waits for the process to exit
/// - Returns ExecutionResult with output, stderr_output, summary, and exit code
///
/// IMPORTANT: The child's stdout and stderr must be piped. This function will take
/// ownership of both handles.
pub async fn wait_and_capture(
    child: tokio::process::Child,
    stream_mode: StreamMode,
) -> Result<ExecutionResult> {
    let spawn_options = SpawnOptions::default();
    wait_and_capture_with_idle_timeout(
        child,
        stream_mode,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        Duration::from_secs(DEFAULT_LIVENESS_DEAD_SECS),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        spawn_options,
        None,
    )
    .await
}

/// Wait for a spawned child process, capturing output and enforcing idle-timeout.
///
/// The process is killed only when there is no stdout/stderr output for the full
/// `idle_timeout` duration.
///
/// When `output_spool` is `Some`, each stdout chunk is also written to the given
/// file path with an explicit flush after each write.  This ensures partial output
/// survives OOM kills or other ungraceful terminations — the caller can recover
/// output from the spool file even if this function never returns.
#[expect(
    clippy::too_many_arguments,
    reason = "timeout params are flat for caller convenience"
)]
pub async fn wait_and_capture_with_idle_timeout(
    mut child: tokio::process::Child,
    stream_mode: StreamMode,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    termination_grace_period: Duration,
    output_spool: Option<&Path>,
    spawn_options: SpawnOptions,
    initial_response_timeout: Option<Duration>,
) -> Result<ExecutionResult> {
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take();

    // Open spool file for incremental crash-safe output.
    let mut spool_file = None;
    if let Some(path) = output_spool {
        match SpoolRotator::open(
            path,
            spawn_options.spool_max_bytes,
            spawn_options.keep_rotated_spool,
        ) {
            Ok(rotator) => {
                spool_file = Some(rotator);
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to open output spool file");
            }
        }
    }
    let session_dir = output_spool.and_then(Path::parent);
    let mut stderr_spool_file = None;
    if let Some(dir) = session_dir {
        let path = dir.join("stderr.log");
        match SpoolRotator::open(
            &path,
            spawn_options.spool_max_bytes,
            spawn_options.keep_rotated_spool,
        ) {
            Ok(rotator) => {
                stderr_spool_file = Some(rotator);
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to open stderr spool file"
                );
            }
        }
    }

    // Use byte-level reads instead of read_line() to detect partial output
    // (e.g., progress bars with \r, streaming dots without \n). This prevents
    // false idle-timeout kills when the subprocess actively produces data that
    // never forms a complete line.
    const READ_BUF_SIZE: usize = 4096;
    let mut stdout_reader = BufReader::new(stdout);
    let mut output = String::new();
    let mut stdout_line_buf = String::new();

    let mut stderr_output = String::new();
    let execution_start = Instant::now();
    let mut last_activity = Instant::now();
    let mut last_heartbeat = execution_start;
    let heartbeat_interval = resolve_heartbeat_interval();
    let mut liveness_dead_since: Option<Instant> = None;
    let mut next_liveness_poll_at: Option<Instant> = None;
    let mut received_first_output = false;
    let mut idle_timed_out = false;
    let mut workspace_boundary_timed_out = false;
    let mut workspace_boundary_error_hits = 0usize;
    // timeout_note is set lazily when idle timeout fires, so that it reflects
    // whether the initial_response_timeout or the normal idle_timeout was active.
    let mut timeout_note = String::new();
    let workspace_boundary_note = format!(
        "workspace boundary timeout: detected {WORKSPACE_BOUNDARY_ERROR_THRESHOLD} repeated boundary errors ('Path not in workspace'); process killed"
    );

    if let Some(stderr_handle) = stderr {
        // Tee mode: read stdout and stderr concurrently via byte-level reads
        let mut stderr_reader = BufReader::new(stderr_handle);
        let mut stderr_line_buf = String::new();

        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut stdout_buf = [0u8; READ_BUF_SIZE];
        let mut stderr_buf = [0u8; READ_BUF_SIZE];

        while !stdout_done || !stderr_done {
            tokio::select! {
                result = stdout_reader.read(&mut stdout_buf), if !stdout_done => {
                    match result {
                        Ok(0) => {
                            // EOF — flush any remaining partial line
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            stdout_done = true;
                        }
                        Ok(n) => {
                            received_first_output = true;
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stdout_buf[..n]);
                            // Spool to disk for crash recovery
                            spool_chunk(&mut spool_file, &stdout_buf[..n]);
                            if let (Some(dir), Some(spool)) = (session_dir, spool_file.as_ref()) {
                                record_spool_bytes_written(dir, spool.bytes_written());
                            }
                            workspace_boundary_error_hits += accumulate_and_flush_lines(
                                &chunk,
                                &mut stdout_line_buf,
                                &mut output,
                                stream_mode,
                            );
                            drain_if_over_high_water(&mut output);
                            if workspace_boundary_error_hits >= WORKSPACE_BOUNDARY_ERROR_THRESHOLD {
                                workspace_boundary_timed_out = true;
                                warn!(
                                    hits = workspace_boundary_error_hits,
                                    threshold = WORKSPACE_BOUNDARY_ERROR_THRESHOLD,
                                    "Killing child due to repeated workspace boundary errors"
                                );
                                terminate_child_process_group(&mut child, termination_grace_period).await;
                                break;
                            }
                        }
                        Err(_) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            stdout_done = true;
                        }
                    }
                }
                result = stderr_reader.read(&mut stderr_buf), if !stderr_done => {
                    match result {
                        Ok(0) => {
                            flush_stderr_buf(
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
                            stderr_done = true;
                        }
                        Ok(n) => {
                            // NOTE: Do NOT set received_first_output here.
                            // Only stdout counts as "first output" — stderr
                            // often contains diagnostic banners (e.g. systemd-run's
                            // "Running scope as unit...") that should not reset
                            // the initial_response_timeout.
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stderr_buf[..n]);
                            spool_chunk(&mut stderr_spool_file, &stderr_buf[..n]);
                            workspace_boundary_error_hits += accumulate_and_flush_stderr(
                                &chunk,
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
                            drain_if_over_high_water(&mut stderr_output);
                            if workspace_boundary_error_hits >= WORKSPACE_BOUNDARY_ERROR_THRESHOLD {
                                workspace_boundary_timed_out = true;
                                warn!(
                                    hits = workspace_boundary_error_hits,
                                    threshold = WORKSPACE_BOUNDARY_ERROR_THRESHOLD,
                                    "Killing child due to repeated workspace boundary errors"
                                );
                                terminate_child_process_group(&mut child, termination_grace_period).await;
                                break;
                            }
                        }
                        Err(_) => {
                            flush_stderr_buf(
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
                            stderr_done = true;
                        }
                    }
                }
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => {
                    let effective_idle = if !received_first_output {
                        initial_response_timeout.unwrap_or(idle_timeout)
                    } else {
                        idle_timeout
                    };
                    maybe_emit_heartbeat(
                        heartbeat_interval,
                        execution_start,
                        last_activity,
                        &mut last_heartbeat,
                        effective_idle,
                    );
                    // Skip liveness polling for initial-response timeout:
                    // kill immediately once elapsed time exceeds the threshold.
                    let should_kill = if !received_first_output && initial_response_timeout.is_some() {
                        last_activity.elapsed() >= effective_idle
                    } else {
                        should_terminate_for_idle(
                            &mut last_activity,
                            effective_idle,
                            liveness_dead_timeout,
                            session_dir,
                            &mut liveness_dead_since,
                            &mut next_liveness_poll_at,
                        )
                    };
                    if should_kill {
                        idle_timed_out = true;
                        let timeout_kind = if !received_first_output && initial_response_timeout.is_some() {
                            "initial_response_timeout"
                        } else {
                            "idle timeout"
                        };
                        timeout_note = if !received_first_output && initial_response_timeout.is_some() {
                            format!(
                                "{timeout_kind}: no stdout output for {}s; process killed immediately (no liveness polling)",
                                effective_idle.as_secs(),
                            )
                        } else {
                            format!(
                                "{timeout_kind}: no stdout/stderr output for {}s; liveness false for {}s; process killed",
                                effective_idle.as_secs(),
                                liveness_dead_timeout.as_secs(),
                            )
                        };
                        warn!(
                            timeout_secs = effective_idle.as_secs(),
                            timeout_kind,
                            "Killing child due to {timeout_kind}"
                        );
                        terminate_child_process_group(&mut child, termination_grace_period).await;
                        break;
                    }
                }
            }
        }
    } else {
        // No stderr handle (shouldn't happen with spawn_tool, but handle gracefully)
        let mut stdout_buf = [0u8; READ_BUF_SIZE];
        loop {
            tokio::select! {
                result = stdout_reader.read(&mut stdout_buf) => {
                    match result {
                        Ok(0) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            break;
                        }
                        Ok(n) => {
                            received_first_output = true;
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stdout_buf[..n]);
                            spool_chunk(&mut spool_file, &stdout_buf[..n]);
                            if let (Some(dir), Some(spool)) = (session_dir, spool_file.as_ref()) {
                                record_spool_bytes_written(dir, spool.bytes_written());
                            }
                            workspace_boundary_error_hits += accumulate_and_flush_lines(
                                &chunk,
                                &mut stdout_line_buf,
                                &mut output,
                                stream_mode,
                            );
                            drain_if_over_high_water(&mut output);
                            if workspace_boundary_error_hits >= WORKSPACE_BOUNDARY_ERROR_THRESHOLD {
                                workspace_boundary_timed_out = true;
                                warn!(
                                    hits = workspace_boundary_error_hits,
                                    threshold = WORKSPACE_BOUNDARY_ERROR_THRESHOLD,
                                    "Killing child due to repeated workspace boundary errors"
                                );
                                terminate_child_process_group(&mut child, termination_grace_period).await;
                                break;
                            }
                        }
                        Err(_) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => {
                    let effective_idle = if !received_first_output {
                        initial_response_timeout.unwrap_or(idle_timeout)
                    } else {
                        idle_timeout
                    };
                    maybe_emit_heartbeat(
                        heartbeat_interval,
                        execution_start,
                        last_activity,
                        &mut last_heartbeat,
                        effective_idle,
                    );
                    // Skip liveness polling for initial-response timeout:
                    // kill immediately once elapsed time exceeds the threshold.
                    let should_kill = if !received_first_output && initial_response_timeout.is_some() {
                        last_activity.elapsed() >= effective_idle
                    } else {
                        should_terminate_for_idle(
                            &mut last_activity,
                            effective_idle,
                            liveness_dead_timeout,
                            session_dir,
                            &mut liveness_dead_since,
                            &mut next_liveness_poll_at,
                        )
                    };
                    if should_kill {
                        idle_timed_out = true;
                        let timeout_kind = if !received_first_output && initial_response_timeout.is_some() {
                            "initial_response_timeout"
                        } else {
                            "idle timeout"
                        };
                        timeout_note = if !received_first_output && initial_response_timeout.is_some() {
                            format!(
                                "{timeout_kind}: no stdout output for {}s; process killed immediately (no liveness polling)",
                                effective_idle.as_secs(),
                            )
                        } else {
                            format!(
                                "{timeout_kind}: no stdout/stderr output for {}s; liveness false for {}s; process killed",
                                effective_idle.as_secs(),
                                liveness_dead_timeout.as_secs(),
                            )
                        };
                        warn!(
                            timeout_secs = effective_idle.as_secs(),
                            timeout_kind,
                            "Killing child due to {timeout_kind}"
                        );
                        terminate_child_process_group(&mut child, termination_grace_period).await;
                        break;
                    }
                }
            }
        }
    }

    let status = child.wait().await.context("Failed to wait for command")?;

    let mut exit_code = status.code().unwrap_or_else(|| {
        warn!("Process terminated by signal, using exit code 1");
        1
    });
    if idle_timed_out {
        exit_code = 137;
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&timeout_note);
        stderr_output.push('\n');
    } else if workspace_boundary_timed_out {
        exit_code = 125;
        if !stderr_output.is_empty() && !stderr_output.ends_with('\n') {
            stderr_output.push('\n');
        }
        stderr_output.push_str(&workspace_boundary_note);
        stderr_output.push('\n');
    }

    let summary = if idle_timed_out {
        timeout_note
    } else if workspace_boundary_timed_out {
        workspace_boundary_note
    } else if exit_code == 0 {
        extract_summary(&output)
    } else {
        failure_summary(&output, &stderr_output, exit_code)
    };
    let output = sanitize_opaque_object_payloads(&output);
    let mut stderr_output = sanitize_opaque_object_payloads(&stderr_output);
    let actionable_detail = resolve_actionable_failure_detail(&summary, exit_code);
    stderr_output = append_actionable_detail_for_opaque_payload(&stderr_output, &actionable_detail);

    // Ensure spool artifacts do not keep raw opaque payload markers after a
    // successful capture cycle. Preserve previous turns by rewriting only the
    // segment appended during this run.
    let output_spool_plan = spool_file.take().map(|rotator| rotator.finalize());
    let stderr_spool_plan = stderr_spool_file.take().map(|rotator| rotator.finalize());
    if let Some(plan_result) = output_spool_plan {
        match plan_result {
            Ok(plan) => {
                if let Err(e) = sanitize_spool_plan(plan, None) {
                    warn!(error = %e, "Failed to sanitize output spool tail");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to finalize output spool file");
            }
        }
    }
    if let Some(plan_result) = stderr_spool_plan {
        match plan_result {
            Ok(plan) => {
                if let Err(e) = sanitize_spool_plan(plan, Some(&actionable_detail)) {
                    warn!(error = %e, "Failed to sanitize stderr spool tail");
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to finalize stderr spool file");
            }
        }
    }

    Ok(ExecutionResult {
        output,
        stderr_output,
        summary,
        exit_code,
    })
}

/// Execute a command and capture output.
///
/// - Spawns the command
/// - Captures stdout (piped)
/// - Stderr passes through to parent (inherit)
/// - Waits for completion
/// - Returns ExecutionResult with output, summary, and exit code
///
/// This is a convenience function that combines `spawn_tool()` and `wait_and_capture()`.
/// Use `spawn_tool()` directly if you need the PID before waiting (e.g., for monitoring).
pub async fn run_and_capture(cmd: Command) -> Result<ExecutionResult> {
    run_and_capture_with_stdin(cmd, None, StreamMode::BufferOnly).await
}

/// Execute a command and capture output, optionally writing prompt data to stdin.
#[tracing::instrument(skip_all)]
pub async fn run_and_capture_with_stdin(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
    stream_mode: StreamMode,
) -> Result<ExecutionResult> {
    let child = spawn_tool(cmd, stdin_data).await?;
    let spawn_options = SpawnOptions::default();
    wait_and_capture_with_idle_timeout(
        child,
        stream_mode,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        Duration::from_secs(DEFAULT_LIVENESS_DEAD_SECS),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
        spawn_options,
        None,
    )
    .await
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "lib_tests_heartbeat.rs"]
mod tests_heartbeat;
#[cfg(test)]
#[path = "lib_tests_retry.rs"]
mod tests_retry;
