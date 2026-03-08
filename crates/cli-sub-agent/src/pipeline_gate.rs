//! Pre-review quality gate: detect and run pre-commit hooks or custom gate
//! commands before `csa review` / `csa debate` to catch lint/test failures early.
//!
//! Detection order (strict):
//! 1. Explicit `gate_command` from project config
//! 2. `git config core.hooksPath` → `<hooksPath>/pre-commit`
//! 3. No gate found → skip with debug log
//!
//! When `CSA_DEPTH > 0`, the gate is skipped entirely to prevent recursion.

use std::path::Path;
use std::process::Stdio;

use anyhow::Result;
use tokio::process::Command;
use tracing::{debug, warn};

use csa_config::GateMode;

/// Result of running a quality gate command.
#[derive(Debug, Clone)]
pub(crate) struct GateResult {
    /// The command that was executed.
    pub command: String,
    /// Process exit code (None if killed by signal).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Whether the gate was skipped (depth > 0, no gate found, etc.).
    pub skipped: bool,
    /// Human-readable reason when skipped.
    pub skip_reason: Option<String>,
}

impl GateResult {
    fn skipped(reason: &str) -> Self {
        Self {
            command: String::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            skipped: true,
            skip_reason: Some(reason.to_string()),
        }
    }

    /// Whether the gate passed (either skipped or exit code 0).
    pub fn passed(&self) -> bool {
        self.skipped || self.exit_code == Some(0)
    }
}

/// Evaluate the pre-review quality gate.
///
/// Returns `GateResult` describing what happened. The caller decides whether
/// to abort based on `gate_mode` and the result.
///
/// # Gate detection order
/// 1. `gate_command` from config (explicit override)
/// 2. `git config core.hooksPath` → `<path>/pre-commit` (if executable)
/// 3. No gate found → skip
///
/// # Recursion guard
/// When `CSA_DEPTH > 0`, the gate is always skipped to prevent recursive
/// quality checks when CSA spawns sub-agents.
pub(crate) async fn evaluate_quality_gate(
    project_root: &Path,
    gate_command: Option<&str>,
    gate_timeout_secs: u64,
    gate_mode: &GateMode,
) -> Result<GateResult> {
    // Recursion guard: skip when running as a sub-agent
    let depth: u32 = std::env::var("CSA_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if depth > 0 {
        debug!(depth, "Skipping quality gate (CSA_DEPTH > 0)");
        return Ok(GateResult::skipped("CSA_DEPTH > 0 (sub-agent)"));
    }

    // Step 1: Resolve gate command
    let resolved_command = match gate_command {
        Some(cmd) => {
            debug!(command = cmd, "Using explicit gate_command from config");
            cmd.to_string()
        }
        None => match detect_git_hooks_pre_commit(project_root).await? {
            Some(cmd) => {
                debug!(command = %cmd, "Detected pre-commit hook via core.hooksPath");
                cmd
            }
            None => {
                debug!("No quality gate found; skipping");
                return Ok(GateResult::skipped(
                    "no gate command configured or detected",
                ));
            }
        },
    };

    // Step 2: Execute the gate command
    execute_gate_command(
        &resolved_command,
        project_root,
        gate_timeout_secs,
        gate_mode,
    )
    .await
}

/// Detect pre-commit hook via `git config core.hooksPath`.
///
/// Returns the shell command to execute the pre-commit hook, or None if
/// `core.hooksPath` is not set or the pre-commit script doesn't exist.
///
/// Intentionally does NOT fall back to `.git/hooks/` — this supports
/// monorepo/submodule shared hooks configurations.
async fn detect_git_hooks_pre_commit(project_root: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["config", "core.hooksPath"])
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await?;

    if !output.status.success() {
        // core.hooksPath is not set
        return Ok(None);
    }

    let hooks_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if hooks_path.is_empty() {
        return Ok(None);
    }

    // Resolve relative paths against project root
    let hooks_dir = if Path::new(&hooks_path).is_absolute() {
        std::path::PathBuf::from(&hooks_path)
    } else {
        project_root.join(&hooks_path)
    };

    let pre_commit = hooks_dir.join("pre-commit");
    if pre_commit.exists() {
        // Return the path as a shell command
        Ok(Some(pre_commit.display().to_string()))
    } else {
        debug!(
            path = %pre_commit.display(),
            "core.hooksPath set but pre-commit script not found"
        );
        Ok(None)
    }
}

/// Execute a gate command with timeout and process group management.
///
/// Reuses the same patterns from `csa-hooks/src/runner.rs`:
/// - `sh -c` execution
/// - `process_group(0)` for clean kill
/// - Negative-PID signal propagation on timeout
async fn execute_gate_command(
    command: &str,
    project_root: &Path,
    timeout_secs: u64,
    gate_mode: &GateMode,
) -> Result<GateResult> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Create new process group for clean timeout kill.
    // tokio::process::Command::process_group(0) calls setsid in the child,
    // allowing timeout to kill the entire group via negative PID.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd.spawn()?;
    let timeout = std::time::Duration::from_secs(timeout_secs);

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let exit_code = output.status.code();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            let result = GateResult {
                command: command.to_string(),
                exit_code,
                stdout,
                stderr,
                skipped: false,
                skip_reason: None,
            };

            if !result.passed() {
                match gate_mode {
                    GateMode::Monitor => {
                        warn!(
                            exit_code = ?exit_code,
                            "Quality gate failed (monitor mode, not blocking)"
                        );
                    }
                    GateMode::CriticalOnly | GateMode::Full => {
                        // Caller will handle abort based on gate_mode
                    }
                }
            }

            Ok(result)
        }
        Ok(Err(e)) => {
            anyhow::bail!("Quality gate command failed to execute: {e}");
        }
        Err(_elapsed) => {
            // Timeout: kill the process group
            // Note: the child has already been consumed by wait_with_output,
            // but the process group may still have orphaned children.
            // In practice, tokio drops the child handle which sends SIGKILL.
            warn!(
                timeout_secs,
                command, "Quality gate timed out after {timeout_secs}s"
            );
            Ok(GateResult {
                command: command.to_string(),
                exit_code: None,
                stdout: String::new(),
                stderr: format!("Quality gate timed out after {timeout_secs}s"),
                skipped: false,
                skip_reason: None,
            })
        }
    }
}

#[cfg(test)]
#[path = "pipeline_gate_tests.rs"]
mod tests;
