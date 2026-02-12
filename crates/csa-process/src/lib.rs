//! Process management: spawning, signal handling, and output capture.

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::warn;

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

/// Spawn a tool process without waiting for it to complete.
///
/// - Spawns the command
/// - Captures stdout (piped)
/// - Captures stderr (piped, tee'd to parent stderr in `wait_and_capture`)
/// - Isolates child in its own process group (via setsid)
/// - Enables kill_on_drop as safety net
/// - Returns the child process handle for PID access and later waiting
///
/// Use this when you need the PID before waiting (e.g., for resource monitoring).
/// Call `wait_and_capture()` after starting monitoring to complete execution.
pub async fn spawn_tool(mut cmd: Command) -> Result<tokio::process::Child> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
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

    cmd.spawn().context("Failed to spawn command")
}

/// Wait for a spawned child process and capture its output.
///
/// - Reads stdout until EOF
/// - Reads stderr in tee mode (forwards each line to parent stderr + accumulates)
/// - Waits for the process to exit
/// - Returns ExecutionResult with output, stderr_output, summary, and exit code
///
/// IMPORTANT: The child's stdout and stderr must be piped. This function will take
/// ownership of both handles.
pub async fn wait_and_capture(mut child: tokio::process::Child) -> Result<ExecutionResult> {
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take();

    let mut stdout_reader = BufReader::new(stdout);
    let mut output = String::new();
    let mut stdout_line = String::new();

    let mut stderr_output = String::new();

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
                            // Tee: forward to parent stderr in real-time
                            eprint!("{}", stderr_line);
                            stderr_output.push_str(&stderr_line);
                            stderr_line.clear();
                        }
                        Err(_) => stderr_done = true,
                    }
                }
            }
        }
    } else {
        // No stderr handle (shouldn't happen with spawn_tool, but handle gracefully)
        while let Ok(n) = stdout_reader.read_line(&mut stdout_line).await {
            if n == 0 {
                break;
            }
            output.push_str(&stdout_line);
            stdout_line.clear();
        }
    }

    let status = child.wait().await.context("Failed to wait for command")?;

    let exit_code = status.code().unwrap_or_else(|| {
        warn!("Process terminated by signal, using exit code 1");
        1
    });

    let summary = if exit_code == 0 {
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
    let child = spawn_tool(cmd).await?;
    wait_and_capture(child).await
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
mod tests {
    use super::*;

    #[test]
    fn test_extract_summary_empty() {
        assert_eq!(extract_summary(""), "");
    }

    #[test]
    fn test_extract_summary_single_line() {
        assert_eq!(extract_summary("Hello, world!"), "Hello, world!");
    }

    #[test]
    fn test_extract_summary_multi_line() {
        let input = "First line\nSecond line\nThird line";
        assert_eq!(extract_summary(input), "Third line");
    }

    #[test]
    fn test_extract_summary_with_empty_lines() {
        let input = "First line\n\nThird line\n\n";
        assert_eq!(extract_summary(input), "Third line");
    }

    #[test]
    fn test_extract_summary_long_line() {
        let long = "a".repeat(250);
        let summary = extract_summary(&long);
        assert_eq!(summary.chars().count(), 200);
        assert!(summary.ends_with("..."));
        assert_eq!(summary.strip_suffix("...").unwrap(), &long[..197]);
    }

    #[test]
    fn test_extract_summary_exactly_200_chars() {
        let exact = "a".repeat(200);
        let summary = extract_summary(&exact);
        assert_eq!(summary.chars().count(), 200);
        assert!(!summary.ends_with("..."));
    }

    #[test]
    fn test_extract_summary_multibyte_truncation() {
        // Create a string where truncation would fall in the middle of multi-byte UTF-8 characters.
        // Emoji character '🔥' is 4 bytes (F0 9F 94 A5 in UTF-8).
        // We need more than 200 characters total to trigger truncation.

        // Use 196 ASCII chars + 10 emoji chars = 206 chars total
        let mut long_line = "a".repeat(196);
        for _ in 0..10 {
            long_line.push('🔥');
        }

        // Total: 196 + 10 = 206 chars (but many more bytes due to emoji)
        assert_eq!(long_line.chars().count(), 206);

        // This should NOT panic, even with multi-byte characters
        let summary = extract_summary(&long_line);

        // Summary should be 197 chars + "..." = 200 chars total
        assert_eq!(summary.chars().count(), 200);
        assert!(summary.ends_with("..."));

        // The truncated part should be exactly 197 characters
        let content_without_ellipsis = summary.strip_suffix("...").unwrap();
        assert_eq!(content_without_ellipsis.chars().count(), 197);

        // Should have the first 196 'a' chars and the first emoji character
        assert!(content_without_ellipsis.starts_with(&"a".repeat(196)));
        assert!(content_without_ellipsis.ends_with('🔥'));
    }

    #[test]
    fn test_execution_result_construction() {
        let result = ExecutionResult {
            output: "test output".to_string(),
            stderr_output: String::new(),
            summary: "test summary".to_string(),
            exit_code: 0,
        };
        assert_eq!(result.output, "test output");
        assert_eq!(result.summary, "test summary");
        assert_eq!(result.exit_code, 0);
        assert!(result.stderr_output.is_empty());
    }

    #[tokio::test]
    async fn test_spawn_tool_returns_valid_child() {
        let mut cmd = Command::new("echo");
        cmd.arg("test");

        let child = spawn_tool(cmd).await.expect("Failed to spawn tool");
        let pid = child.id().expect("Child process has no PID");

        // PID should be a positive number
        assert!(pid > 0);

        // Clean up by waiting for the child
        let result = wait_and_capture(child)
            .await
            .expect("Failed to wait for child");
        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("test"));
    }

    #[tokio::test]
    async fn test_run_and_capture_still_works() {
        let mut cmd = Command::new("echo");
        cmd.arg("backward_compatible");

        let result = run_and_capture(cmd)
            .await
            .expect("run_and_capture should work");

        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("backward_compatible"));
    }

    #[tokio::test]
    async fn test_stderr_capture() {
        // Use bash -c to write to both stdout and stderr
        let mut cmd = Command::new("bash");
        cmd.args(["-c", "echo stdout_line && echo stderr_line >&2"]);

        let child = spawn_tool(cmd).await.expect("Failed to spawn");
        let result = wait_and_capture(child).await.expect("Failed to wait");

        assert_eq!(result.exit_code, 0);
        assert!(result.output.contains("stdout_line"));
        assert!(result.stderr_output.contains("stderr_line"));
    }

    // --- failure_summary tests ---

    #[test]
    fn test_failure_summary_prefers_stdout() {
        let summary = failure_summary("stdout error\n", "stderr error\n", 1);
        assert_eq!(summary, "stdout error");
    }

    #[test]
    fn test_failure_summary_falls_back_to_stderr() {
        let summary = failure_summary("", "stderr error message\n", 1);
        assert_eq!(summary, "stderr error message");
    }

    #[test]
    fn test_failure_summary_falls_back_to_stderr_when_stdout_whitespace_only() {
        let summary = failure_summary("  \n\n", "stderr msg\n", 42);
        assert_eq!(summary, "stderr msg");
    }

    #[test]
    fn test_failure_summary_exit_code_fallback() {
        let summary = failure_summary("", "", 137);
        assert_eq!(summary, "exit code 137");
    }

    #[test]
    fn test_failure_summary_exit_code_when_both_whitespace() {
        let summary = failure_summary("  \n", "  \n", 2);
        assert_eq!(summary, "exit code 2");
    }

    #[test]
    fn test_failure_summary_truncates_long_stderr() {
        let long_err = "e".repeat(250);
        let summary = failure_summary("", &long_err, 1);
        assert_eq!(summary.chars().count(), 200);
        assert!(summary.ends_with("..."));
    }

    #[tokio::test]
    async fn test_failed_command_uses_stderr_summary() {
        // Command that writes to stderr and exits non-zero
        let mut cmd = Command::new("bash");
        cmd.args(["-c", "echo 'fatal: something went wrong' >&2; exit 1"]);

        let child = spawn_tool(cmd).await.expect("Failed to spawn");
        let result = wait_and_capture(child).await.expect("Failed to wait");

        assert_eq!(result.exit_code, 1);
        assert_eq!(result.summary, "fatal: something went wrong");
    }

    #[tokio::test]
    async fn test_failed_command_exit_code_only() {
        // Command that produces no output and exits non-zero
        let mut cmd = Command::new("bash");
        cmd.args(["-c", "exit 42"]);

        let child = spawn_tool(cmd).await.expect("Failed to spawn");
        let result = wait_and_capture(child).await.expect("Failed to wait");

        assert_eq!(result.exit_code, 42);
        assert_eq!(result.summary, "exit code 42");
    }

    // --- helper function tests ---

    #[test]
    fn test_last_non_empty_line() {
        assert_eq!(last_non_empty_line(""), "");
        assert_eq!(last_non_empty_line("hello"), "hello");
        assert_eq!(last_non_empty_line("a\nb\nc\n"), "c");
        assert_eq!(last_non_empty_line("  \n  \n"), "");
        assert_eq!(last_non_empty_line("first\n\nlast\n\n"), "last");
    }

    #[test]
    fn test_truncate_line() {
        assert_eq!(truncate_line("short", 200), "short");
        assert_eq!(truncate_line("", 200), "");
        let long = "x".repeat(250);
        let result = truncate_line(&long, 200);
        assert_eq!(result.chars().count(), 200);
        assert!(result.ends_with("..."));
    }
}
