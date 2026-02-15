//! Process management: spawning, signal handling, and output capture.

use anyhow::{Context, Result};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tracing::warn;

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

/// Result of executing a command.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionResult {
    /// Combined stdout output.
    pub output: String,
    /// Captured stderr output (tee'd to parent stderr in real-time).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr_output: String,
    /// Last non-empty line or truncated output (max 200 chars).
    pub summary: String,
    /// Exit code (1 if signal-killed).
    pub exit_code: i32,
}

pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 300;
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(200);

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
    mut cmd: Command,
    stdin_data: Option<Vec<u8>>,
) -> Result<tokio::process::Child> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    if stdin_data.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::null());
    }
    cmd.kill_on_drop(true);

    // Isolate child in its own process group to prevent signal inheritance
    // and enable clean termination of the entire subprocess tree.
    // SAFETY: setsid() is async-signal-safe and we call it before exec,
    // so no Rust runtime state exists in the child yet.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            tokio::spawn(async move {
                match tokio::time::timeout(std::time::Duration::from_secs(30), async {
                    stdin.write_all(&data).await?;
                    stdin.shutdown().await?;
                    Ok::<_, std::io::Error>(())
                })
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!("stdin write error: {}", e),
                    Err(_) => warn!("stdin write timed out after 30s"),
                }
            });
        } else {
            warn!("stdin was requested but no piped stdin handle was available");
        }
    }

    Ok(child)
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
    )
    .await
}

/// Wait for a spawned child process, capturing output and enforcing idle-timeout.
///
/// The process is killed only when there is no stdout/stderr output for the full
/// `idle_timeout` duration.
pub async fn wait_and_capture_with_idle_timeout(
    mut child: tokio::process::Child,
    stream_mode: StreamMode,
    idle_timeout: Duration,
) -> Result<ExecutionResult> {
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take();

    let mut stdout_reader = BufReader::new(stdout);
    let mut output = String::new();
    let mut stdout_line = String::new();

    let mut stderr_output = String::new();
    let mut last_activity = Instant::now();
    let mut idle_timed_out = false;
    let timeout_note = format!(
        "idle timeout: no stdout/stderr output for {}s; process killed",
        idle_timeout.as_secs()
    );

    if let Some(stderr_handle) = stderr {
        // Tee mode: read stdout and stderr concurrently
        let mut stderr_reader = BufReader::new(stderr_handle);
        let mut stderr_line = String::new();

        let mut stdout_done = false;
        let mut stderr_done = false;

        while !stdout_done || !stderr_done {
            tokio::select! {
                result = stdout_reader.read_line(&mut stdout_line), if !stdout_done => {
                    match result {
                        Ok(0) => stdout_done = true,
                        Ok(_) => {
                            last_activity = Instant::now();
                            if stream_mode == StreamMode::TeeToStderr {
                                eprint!("[stdout] {}", stdout_line);
                            }
                            output.push_str(&stdout_line);
                            stdout_line.clear();
                        }
                        Err(_) => stdout_done = true,
                    }
                }
                result = stderr_reader.read_line(&mut stderr_line), if !stderr_done => {
                    match result {
                        Ok(0) => stderr_done = true,
                        Ok(_) => {
                            last_activity = Instant::now();
                            // Tee: forward to parent stderr in real-time
                            eprint!("{}", stderr_line);
                            stderr_output.push_str(&stderr_line);
                            stderr_line.clear();
                        }
                        Err(_) => stderr_done = true,
                    }
                }
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => {
                    if last_activity.elapsed() >= idle_timeout {
                        idle_timed_out = true;
                        warn!(timeout_secs = idle_timeout.as_secs(), "Killing child due to idle timeout");
                        kill_child_process_group(&mut child);
                        break;
                    }
                }
            }
        }
    } else {
        // No stderr handle (shouldn't happen with spawn_tool, but handle gracefully)
        loop {
            tokio::select! {
                result = stdout_reader.read_line(&mut stdout_line) => {
                    let n = match result {
                        Ok(v) => v,
                        Err(_) => break,
                    };
                    if n == 0 {
                        break;
                    }
                    last_activity = Instant::now();
                    if stream_mode == StreamMode::TeeToStderr {
                        eprint!("[stdout] {}", stdout_line);
                    }
                    output.push_str(&stdout_line);
                    stdout_line.clear();
                }
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => {
                    if last_activity.elapsed() >= idle_timeout {
                        idle_timed_out = true;
                        warn!(timeout_secs = idle_timeout.as_secs(), "Killing child due to idle timeout");
                        kill_child_process_group(&mut child);
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
    )
    .await
}

fn kill_child_process_group(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: kill() is async-signal-safe; negative PID targets the process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
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

/// Extract summary from output (last non-empty line, truncated to 200 chars).
fn extract_summary(output: &str) -> String {
    truncate_line(last_non_empty_line(output), 200)
}

/// Build summary for failed executions (exit_code != 0).
///
/// Priority chain:
/// 1. stdout last non-empty line (if present — some tools write errors to stdout)
/// 2. stderr last non-empty line (fallback for tools that write errors to stderr)
/// 3. `"exit code {N}"` (final fallback when both streams are empty)
fn failure_summary(stdout: &str, stderr: &str, exit_code: i32) -> String {
    let stdout_line = last_non_empty_line(stdout);
    if !stdout_line.is_empty() {
        return truncate_line(stdout_line, 200);
    }

    let stderr_line = last_non_empty_line(stderr);
    if !stderr_line.is_empty() {
        return truncate_line(stderr_line, 200);
    }

    format!("exit code {exit_code}")
}

/// Return the last non-empty line from the given text, or `""` if none.
fn last_non_empty_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// Truncate a line to `max_chars` characters, appending "..." if truncated.
fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().nth(max_chars).is_none() {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(max_chars - 3).collect();
        format!("{truncated}...")
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
