//! Process management: spawning, signal handling, and output capture.

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::warn;

/// Result of executing a command.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// Combined stdout output.
    pub output: String,
    /// Last non-empty line or truncated output (max 200 chars).
    pub summary: String,
    /// Exit code (1 if signal-killed).
    pub exit_code: i32,
}

/// Execute a command and capture output.
///
/// - Spawns the command
/// - Captures stdout (piped)
/// - Stderr passes through to parent (inherit)
/// - Waits for completion
/// - Returns ExecutionResult with output, summary, and exit code
pub async fn run_and_capture(mut cmd: Command) -> Result<ExecutionResult> {
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::inherit());

    let mut child = cmd.spawn().context("Failed to spawn command")?;

    let stdout = child.stdout.take().context("Failed to capture stdout")?;

    let mut reader = BufReader::new(stdout);
    let mut output = String::new();
    let mut line = String::new();

    while let Ok(n) = reader.read_line(&mut line).await {
        if n == 0 {
            break;
        }
        output.push_str(&line);
        line.clear();
    }

    let status = child.wait().await.context("Failed to wait for command")?;

    let exit_code = status.code().unwrap_or_else(|| {
        warn!("Process terminated by signal, using exit code 1");
        1
    });

    let summary = extract_summary(&output);

    Ok(ExecutionResult {
        output,
        summary,
        exit_code,
    })
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
    let last_line = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");

    if last_line.len() <= 200 {
        last_line.to_string()
    } else {
        format!("{}...", &last_line[..197])
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
        assert_eq!(summary.len(), 200);
        assert!(summary.ends_with("..."));
        assert_eq!(&summary[..197], &long[..197]);
    }

    #[test]
    fn test_extract_summary_exactly_200_chars() {
        let exact = "a".repeat(200);
        let summary = extract_summary(&exact);
        assert_eq!(summary.len(), 200);
        assert!(!summary.ends_with("..."));
    }

    #[test]
    fn test_execution_result_construction() {
        let result = ExecutionResult {
            output: "test output".to_string(),
            summary: "test summary".to_string(),
            exit_code: 0,
        };
        assert_eq!(result.output, "test output");
        assert_eq!(result.summary, "test summary");
        assert_eq!(result.exit_code, 0);
    }
}
