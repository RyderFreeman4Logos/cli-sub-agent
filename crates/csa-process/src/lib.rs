//! Process management: spawning, signal handling, and output capture.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use csa_resource::cgroup::SandboxConfig;
use csa_resource::sandbox::{SandboxCapability, detect_sandbox_capability};
mod idle_watchdog;
use idle_watchdog::should_terminate_for_idle;
#[path = "lib_output_helpers.rs"]
mod output_helpers;
mod tool_liveness;
#[cfg(test)]
use output_helpers::{DEFAULT_HEARTBEAT_SECS, HEARTBEAT_INTERVAL_ENV};
use output_helpers::{
    accumulate_and_flush_lines, accumulate_and_flush_stderr, extract_summary, failure_summary,
    flush_line_buf, flush_stderr_buf, maybe_emit_heartbeat, resolve_heartbeat_interval,
    spool_chunk,
};
#[cfg(test)]
use output_helpers::{last_non_empty_line, truncate_line};
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

pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
pub const DEFAULT_STDIN_WRITE_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_TERMINATION_GRACE_PERIOD_SECS: u64 = 5;
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
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            stdin_write_timeout: Duration::from_secs(DEFAULT_STDIN_WRITE_TIMEOUT_SECS),
            keep_stdin_open: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PreExecPolicy {
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

/// Spawn a tool process without waiting for it to complete.
///
/// - Spawns the command
/// - Captures stdout (piped)
/// - Captures stderr (piped, tee'd to parent stderr in `wait_and_capture`)
/// - Sets stdin mode:
///   - `Stdio::piped()` when `stdin_data` is provided
///   - `Stdio::null()` otherwise
/// - Isolates child in its own process group (via setsid)
/// - Enables kill_on_drop as safety net
/// - Returns the child process handle for PID access and later waiting
///
/// Use this when you need the PID before waiting (e.g., for resource monitoring).
/// Call `wait_and_capture()` after starting monitoring to complete execution.
pub async fn spawn_tool(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
) -> Result<tokio::process::Child> {
    spawn_tool_with_options(cmd, stdin_data, SpawnOptions::default()).await
}

/// Spawn a tool process with explicit spawn options.
pub async fn spawn_tool_with_options(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
) -> Result<tokio::process::Child> {
    spawn_tool_with_pre_exec(cmd, stdin_data, PreExecPolicy::Setsid, spawn_options).await
}

async fn spawn_tool_with_pre_exec(
    mut cmd: Command,
    stdin_data: Option<Vec<u8>>,
    pre_exec_policy: PreExecPolicy,
    spawn_options: SpawnOptions,
) -> Result<tokio::process::Child> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if stdin_data.is_some() || spawn_options.keep_stdin_open {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.kill_on_drop(true);

    // Isolate child in its own process group and optionally apply rlimits.
    // SAFETY: setsid() and setrlimit are async-signal-safe and run before exec.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(move || {
            libc::setsid();
            match pre_exec_policy {
                PreExecPolicy::Setsid => Ok(()),
                PreExecPolicy::Rlimits {
                    memory_max_mb,
                    pids_max,
                } => csa_resource::rlimit::apply_rlimits(memory_max_mb, pids_max)
                    .map_err(std::io::Error::other),
                PreExecPolicy::OomAdj => {
                    csa_resource::rlimit::apply_oom_score_adj().map_err(std::io::Error::other)
                }
            }
        });
    }
    #[cfg(not(unix))]
    let _ = pre_exec_policy;

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            let stdin_write_timeout = spawn_options.stdin_write_timeout;
            tokio::spawn(async move {
                match tokio::time::timeout(stdin_write_timeout, async {
                    stdin.write_all(&data).await?;
                    stdin.shutdown().await?;
                    Ok::<_, std::io::Error>(())
                })
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!("stdin write error: {}", e),
                    Err(_) => warn!(
                        timeout_secs = stdin_write_timeout.as_secs(),
                        "stdin write timed out"
                    ),
                }
            });
        } else {
            warn!("stdin was requested but no piped stdin handle was available");
        }
    }

    Ok(child)
}

/// Spawn a tool process with optional resource sandbox.
///
/// When `sandbox` is `Some`, the child process is wrapped in resource
/// isolation based on the host's detected capability:
///
/// - **CgroupV2**: The tool binary is launched inside a systemd transient
///   scope via `systemd-run --user --scope`.  A [`CgroupScopeGuard`] is
///   returned that stops the scope on drop.
///
/// - **Setrlimit**: `RLIMIT_NPROC` is applied in the child via `pre_exec`.
///
/// - **None capability**: Falls through to normal `spawn_tool` behavior.
///
/// When `sandbox` is `None`, this delegates directly to [`spawn_tool`] with
/// no overhead — behavior is identical to the unsandboxed path.
///
/// # Arguments
///
/// * `cmd` — The tool command to spawn.  For cgroup mode, this is rebuilt as
///   a child of `systemd-run`; for rlimit mode, `pre_exec` is added.
/// * `stdin_data` — Optional data to write to the child's stdin.
/// * `sandbox` — Resource limits to enforce.  `None` skips sandboxing.
/// * `tool_name` — Tool identifier for scope naming (e.g. "claude-code").
/// * `session_id` — Session identifier for scope naming.
///
/// [`CgroupScopeGuard`]: csa_resource::cgroup::CgroupScopeGuard
pub async fn spawn_tool_sandboxed(
    cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
    sandbox: Option<&SandboxConfig>,
    tool_name: &str,
    session_id: &str,
) -> Result<(tokio::process::Child, SandboxHandle)> {
    let Some(config) = sandbox else {
        let child = spawn_tool_with_options(cmd, stdin_data, spawn_options).await?;
        return Ok((child, SandboxHandle::None));
    };

    match detect_sandbox_capability() {
        SandboxCapability::CgroupV2 => {
            spawn_with_cgroup(
                cmd,
                stdin_data,
                spawn_options,
                config,
                tool_name,
                session_id,
            )
            .await
        }
        SandboxCapability::Setrlimit => {
            let memory_max_mb = config.memory_max_mb;
            let pids_max = config.pids_max.map(u64::from);

            let child = spawn_tool_with_pre_exec(
                cmd,
                stdin_data,
                PreExecPolicy::Rlimits {
                    memory_max_mb,
                    pids_max,
                },
                spawn_options,
            )
            .await?;

            Ok((child, SandboxHandle::Rlimit))
        }
        SandboxCapability::None => {
            debug!("no sandbox capability detected; applying OOM score adj as fallback");
            let child =
                spawn_tool_with_pre_exec(cmd, stdin_data, PreExecPolicy::OomAdj, spawn_options)
                    .await?;
            Ok((child, SandboxHandle::None))
        }
    }
}

/// Spawn inside a systemd cgroup scope.
///
/// Builds a `systemd-run --user --scope` command that wraps the original
/// tool command, then spawns it via [`spawn_tool`].
async fn spawn_with_cgroup(
    original_cmd: Command,
    stdin_data: Option<Vec<u8>>,
    spawn_options: SpawnOptions,
    config: &SandboxConfig,
    tool_name: &str,
    session_id: &str,
) -> Result<(tokio::process::Child, SandboxHandle)> {
    // Build systemd-run wrapper: `systemd-run --user --scope ... -- <tool> <args>`
    let scope_cmd = csa_resource::cgroup::create_scope_command(tool_name, session_id, config);

    // Convert std::process::Command to tokio::process::Command and append
    // the original program + args after the "--" separator.
    let mut tokio_cmd = Command::from(scope_cmd);
    tokio_cmd.arg(original_cmd.as_std().get_program());
    tokio_cmd.args(original_cmd.as_std().get_args());

    // Propagate environment from the original command.
    let envs: Vec<_> = original_cmd
        .as_std()
        .get_envs()
        .filter_map(|(k, v)| v.map(|val| (k.to_owned(), val.to_owned())))
        .collect();
    for (key, val) in &envs {
        tokio_cmd.env(key, val);
    }

    if let Some(dir) = original_cmd.as_std().get_current_dir() {
        tokio_cmd.current_dir(dir);
    }

    let child = spawn_tool_with_options(tokio_cmd, stdin_data, spawn_options).await?;
    let guard = csa_resource::cgroup::CgroupScopeGuard::new(tool_name, session_id);

    debug!(
        scope = %guard.scope_name(),
        pid = child.id(),
        "spawned tool inside cgroup scope"
    );

    Ok((child, SandboxHandle::Cgroup(guard)))
}

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
    wait_and_capture_with_idle_timeout(
        child,
        stream_mode,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        Duration::from_secs(DEFAULT_LIVENESS_DEAD_SECS),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
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
pub async fn wait_and_capture_with_idle_timeout(
    mut child: tokio::process::Child,
    stream_mode: StreamMode,
    idle_timeout: Duration,
    liveness_dead_timeout: Duration,
    termination_grace_period: Duration,
    output_spool: Option<&Path>,
) -> Result<ExecutionResult> {
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take();

    // Open spool file for incremental crash-safe output.
    let mut spool_file = output_spool.and_then(|path| {
        use std::fs::OpenOptions;
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(f) => Some(f),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "Failed to open output spool file");
                None
            }
        }
    });
    let session_dir = output_spool.and_then(Path::parent);
    let mut stderr_spool_file = session_dir.and_then(|dir| {
        use std::fs::OpenOptions;
        let path = dir.join("stderr.log");
        OpenOptions::new().create(true).append(true).open(path).ok()
    });

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
    let mut idle_timed_out = false;
    let timeout_note = format!(
        "idle timeout: no stdout/stderr output for {}s; liveness false for {}s; process killed",
        idle_timeout.as_secs(),
        liveness_dead_timeout.as_secs()
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
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stdout_buf[..n]);
                            // Spool to disk for crash recovery
                            spool_chunk(&mut spool_file, &stdout_buf[..n]);
                            accumulate_and_flush_lines(
                                &chunk,
                                &mut stdout_line_buf,
                                &mut output,
                                stream_mode,
                            );
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
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stderr_buf[..n]);
                            spool_chunk(&mut stderr_spool_file, &stderr_buf[..n]);
                            accumulate_and_flush_stderr(
                                &chunk,
                                &mut stderr_line_buf,
                                &mut stderr_output,
                                stream_mode,
                            );
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
                    maybe_emit_heartbeat(
                        heartbeat_interval,
                        execution_start,
                        last_activity,
                        &mut last_heartbeat,
                        idle_timeout,
                    );
                    if should_terminate_for_idle(
                        &mut last_activity,
                        idle_timeout,
                        liveness_dead_timeout,
                        session_dir,
                        &mut liveness_dead_since,
                        &mut next_liveness_poll_at,
                    ) {
                        idle_timed_out = true;
                        warn!(
                            timeout_secs = idle_timeout.as_secs(),
                            liveness_dead_timeout_secs = liveness_dead_timeout.as_secs(),
                            "Killing child due to idle timeout after liveness polling"
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
                            last_activity = Instant::now();
                            last_heartbeat = last_activity;
                            liveness_dead_since = None;
                            next_liveness_poll_at = None;
                            let chunk = String::from_utf8_lossy(&stdout_buf[..n]);
                            spool_chunk(&mut spool_file, &stdout_buf[..n]);
                            accumulate_and_flush_lines(
                                &chunk,
                                &mut stdout_line_buf,
                                &mut output,
                                stream_mode,
                            );
                        }
                        Err(_) => {
                            flush_line_buf(&mut stdout_line_buf, &mut output, stream_mode);
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => {
                    maybe_emit_heartbeat(
                        heartbeat_interval,
                        execution_start,
                        last_activity,
                        &mut last_heartbeat,
                        idle_timeout,
                    );
                    if should_terminate_for_idle(
                        &mut last_activity,
                        idle_timeout,
                        liveness_dead_timeout,
                        session_dir,
                        &mut liveness_dead_since,
                        &mut next_liveness_poll_at,
                    ) {
                        idle_timed_out = true;
                        warn!(
                            timeout_secs = idle_timeout.as_secs(),
                            liveness_dead_timeout_secs = liveness_dead_timeout.as_secs(),
                            "Killing child due to idle timeout after liveness polling"
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
    }

    let summary = if idle_timed_out {
        timeout_note
    } else if exit_code == 0 {
        extract_summary(&output)
    } else {
        failure_summary(&output, &stderr_output, exit_code)
    };

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
    wait_and_capture_with_idle_timeout(
        child,
        stream_mode,
        Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        Duration::from_secs(DEFAULT_LIVENESS_DEAD_SECS),
        Duration::from_secs(DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
        None,
    )
    .await
}

async fn terminate_child_process_group(
    child: &mut tokio::process::Child,
    termination_grace_period: Duration,
) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: kill() is async-signal-safe; negative PID targets the process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            tokio::time::sleep(termination_grace_period).await;
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            // SAFETY: kill() is async-signal-safe; negative PID targets the process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.start_kill();
            return;
        }
    }

    let _ = child.start_kill();
}

/// Check if a tool is installed by attempting to locate it.
///
/// Uses `which` command on Unix systems.
pub async fn check_tool_installed(executable: &str) -> Result<()> {
    let output = Command::new("which")
        .arg(executable)
        .output()
        .await
        .context("Failed to execute 'which' command")?;

    if !output.status.success() {
        anyhow::bail!("Tool '{}' is not installed or not in PATH", executable);
    }

    Ok(())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "lib_tests_heartbeat.rs"]
mod tests_heartbeat;
