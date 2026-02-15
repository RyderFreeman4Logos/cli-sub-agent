//! Prompt Guard: user-configurable shell scripts that inject text into prompts.
//!
//! Guards run before tool execution and their stdout is injected into the
//! `effective_prompt`. This enables "reverse prompt injection" — reminding
//! tools (including those without native hook systems) to follow AGENTS.md
//! rules like branch protection, timely commits, etc.
//!
//! ## Protocol
//!
//! Each guard script receives a JSON [`GuardContext`] on stdin and writes
//! injection text to stdout. Empty stdout means "nothing to inject".
//! Non-zero exit or timeout results in a warning and skip (never blocks).
//!
//! ## Configuration
//!
//! ```toml
//! [[prompt_guard]]
//! name = "branch-protection"
//! command = "/path/to/guard-branch.sh"
//! timeout_secs = 5
//!
//! [[prompt_guard]]
//! name = "commit-reminder"
//! command = "/path/to/remind-commit.sh"
//! timeout_secs = 10
//! ```

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Configuration entry for a single prompt guard script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptGuardEntry {
    /// Human-readable name for this guard (used in XML tag and logs).
    pub name: String,
    /// Shell command to execute (run via `sh -c`).
    pub command: String,
    /// Maximum execution time in seconds (default: 10).
    #[serde(default = "default_guard_timeout")]
    pub timeout_secs: u64,
}

fn default_guard_timeout() -> u64 {
    10
}

/// Result from executing a single prompt guard.
#[derive(Debug, Clone)]
pub struct PromptGuardResult {
    /// Guard name (from config).
    pub name: String,
    /// Captured stdout (injection text). Empty means no injection.
    pub output: String,
}

/// Context passed to guard scripts via stdin as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardContext {
    /// Absolute path to the project root directory.
    pub project_root: String,
    /// Current session ID (ULID).
    pub session_id: String,
    /// Tool name being executed (e.g., "codex", "claude-code").
    pub tool: String,
    /// Whether this is a resumed session (`--session` / `--last`).
    pub is_resume: bool,
    /// Current working directory.
    pub cwd: String,
}

/// Execute prompt guard scripts sequentially and collect results.
///
/// Each guard receives [`GuardContext`] as JSON on stdin. Stdout is captured
/// as injection text. Guards that fail (non-zero exit, timeout, spawn error)
/// are warned and skipped — they never block execution.
///
/// Returns results only for guards that produced non-empty output.
pub fn run_prompt_guards(
    guards: &[PromptGuardEntry],
    context: &GuardContext,
) -> Vec<PromptGuardResult> {
    if guards.is_empty() {
        return Vec::new();
    }

    let context_json = match serde_json::to_string(context) {
        Ok(json) => json,
        Err(e) => {
            tracing::warn!("Failed to serialize GuardContext: {e}");
            return Vec::new();
        }
    };

    let mut results = Vec::new();

    for guard in guards {
        match run_single_guard(guard, &context_json) {
            Ok(output) if !output.is_empty() => {
                results.push(PromptGuardResult {
                    name: guard.name.clone(),
                    output,
                });
            }
            Ok(_) => {
                tracing::debug!(guard = %guard.name, "Guard produced empty output, skipping");
            }
            Err(e) => {
                tracing::warn!(guard = %guard.name, "Guard failed, skipping: {e}");
            }
        }
    }

    results
}

/// Execute a single guard script, passing context JSON on stdin and capturing stdout.
fn run_single_guard(guard: &PromptGuardEntry, context_json: &str) -> anyhow::Result<String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&guard.command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // Create new process group for clean timeout kill.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn guard '{}': {e}", guard.name))?;

    // Write context JSON to stdin, then close it.
    if let Some(mut stdin) = child.stdin.take() {
        // Ignore write errors — script may have exited early.
        let _ = stdin.write_all(context_json.as_bytes());
        // stdin is dropped here, closing the pipe.
    }

    let timeout = Duration::from_secs(guard.timeout_secs);
    let start = Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => {
                if status.success() {
                    let output = if let Some(mut stdout) = child.stdout.take() {
                        use std::io::Read;
                        let mut buf = String::new();
                        stdout.read_to_string(&mut buf)?;
                        buf.trim().to_string()
                    } else {
                        String::new()
                    };
                    return Ok(output);
                } else {
                    let code = status.code().unwrap_or(-1);
                    anyhow::bail!("Guard '{}' exited with code {code}", guard.name);
                }
            }
            None => {
                if start.elapsed() >= timeout {
                    // Kill the entire process group on timeout.
                    #[cfg(unix)]
                    {
                        // SAFETY: kill() is async-signal-safe. Negative PID targets
                        // the entire process group created by process_group(0).
                        unsafe {
                            libc::kill(-(child.id() as i32), libc::SIGKILL);
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill();
                    }
                    let _ = child.wait(); // Reap zombie
                    anyhow::bail!(
                        "Guard '{}' timed out after {}s",
                        guard.name,
                        guard.timeout_secs
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Format guard results into XML blocks for prompt injection.
///
/// Returns `None` if all results are empty. Output values are XML-escaped.
///
/// Example output:
/// ```text
/// <prompt-guard name="branch-protection">
/// You are on branch main. Do NOT commit directly.
/// </prompt-guard>
/// <prompt-guard name="commit-reminder">
/// You have uncommitted changes. Commit before stopping.
/// </prompt-guard>
/// ```
pub fn format_guard_output(results: &[PromptGuardResult]) -> Option<String> {
    let non_empty: Vec<_> = results.iter().filter(|r| !r.output.is_empty()).collect();
    if non_empty.is_empty() {
        return None;
    }

    let mut buf = String::new();
    for (i, result) in non_empty.iter().enumerate() {
        if i > 0 {
            buf.push('\n');
        }
        buf.push_str(&format!(
            "<prompt-guard name=\"{}\">\n{}\n</prompt-guard>",
            xml_escape_attr(&result.name),
            xml_escape_text(&result.output),
        ));
    }

    Some(buf)
}

/// Escape a string for use in an XML attribute value (inside double quotes).
fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape a string for use as XML text content.
fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
