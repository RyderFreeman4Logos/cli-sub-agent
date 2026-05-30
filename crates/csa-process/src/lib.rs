//! Process management: spawning, signal handling, and output capture.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::time::MissedTickBehavior;
use tracing::warn;
#[path = "lib_execution_result.rs"]
mod execution_result;
pub use execution_result::{ExecutionResult, model_completed_from_terminal_reason};
mod idle_watchdog;
use idle_watchdog::{
    idle_timeout_note, should_terminate_for_idle, should_terminate_for_initial_response,
};
mod persistent_rate_limit;
use persistent_rate_limit::PersistentRateLimitTracker;
#[path = "lib_output_helpers.rs"]
mod output_helpers;
#[path = "lib_subprocess_helpers.rs"]
mod subprocess_helpers;
mod tool_liveness;
mod workspace_boundary;
#[cfg(unix)]
pub use daemon_stderr::DEFAULT_STDERR_SPOOL_MAX_BYTES;
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
    parse_legacy_terminal_reason, resolve_actionable_failure_detail, resolve_heartbeat_interval,
    sanitize_opaque_object_payloads, spool_chunk,
};
#[cfg(test)]
use output_helpers::{last_non_empty_line, truncate_line};
pub use subprocess_helpers::check_tool_installed;
use subprocess_helpers::terminate_child_process_group;
use tool_liveness::record_spool_bytes_written;
pub use tool_liveness::{DEFAULT_LIVENESS_DEAD_SECS, ToolLiveness, write_fatal_error_markers};
#[cfg(test)]
use workspace_boundary::WORKSPACE_BOUNDARY_THRESHOLD_ENV;
use workspace_boundary::{note_workspace_boundary_threshold, resolve_workspace_boundary_threshold};

#[cfg(unix)]
pub mod daemon;
#[cfg(unix)]
pub mod daemon_stderr;

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

pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 250;
pub const DEFAULT_STDIN_WRITE_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_TERMINATION_GRACE_PERIOD_SECS: u64 = 5;
/// Default threshold for workspace-boundary rejection warn-hint emission.
///
/// Historically this threshold triggered an immediate kill, which produced
/// false positives whenever long-running employee sessions legitimately hit
/// a handful of sandbox-rejected reads (e.g. CSA state dirs under
/// `~/.local/state/cli-sub-agent/...`).  The detector now only emits a
/// one-shot hint on threshold crossing — see #981 for history.
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

#[path = "lib_wait_capture.rs"]
mod wait_capture;
pub use wait_capture::wait_and_capture_with_idle_timeout;
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

/// Poll whether a child process has exited using Tokio's built-in try_wait().
///
/// Sets `*consumed = true` on first success so callers never call try_wait()
/// again on an already-reaped child (which would return an ECHILD error).
fn poll_child_exited(child: &mut tokio::process::Child, consumed: &mut bool) -> bool {
    if *consumed {
        return true;
    }
    if matches!(child.try_wait(), Ok(Some(_))) {
        *consumed = true;
    }
    *consumed
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "lib_tests_boundary.rs"]
mod tests_boundary;
#[cfg(test)]
#[path = "lib_tests_compaction_death.rs"]
mod tests_compaction_death;
#[cfg(test)]
#[path = "lib_tests_heartbeat.rs"]
mod tests_heartbeat;
#[cfg(test)]
#[path = "lib_tests_retry.rs"]
mod tests_retry;
