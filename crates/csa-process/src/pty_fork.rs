//! PTY-based fork for interactive CLI tools.
//!
//! `codex fork <SESSION_ID> [PROMPT]` launches an interactive TUI session that
//! forks an existing conversation.  Unlike `codex exec`, it does not run in
//! non-interactive mode — it requires a terminal (PTY).
//!
//! This module wraps the fork command in `script(1)` to provide a PTY, capture
//! output, and run headless.  It is gated behind `#[cfg(feature = "codex-pty-fork")]`.
//!
//! ## Platform notes
//!
//! - **Linux (GNU coreutils)**: `script -qefc '<cmd>' /dev/null`
//!   - `-q` quiet (no header/footer)
//!   - `-e` return child exit code
//!   - `-f` flush on each write
//!   - `-c <cmd>` run command instead of shell
//! - **macOS (BSD)**: `script -q /dev/null <cmd> <args...>`
//!   - BSD `script` puts the command after the typescript file, not via `-c`
//!
//! ## Why PTY?
//!
//! `codex fork` requires a terminal because it uses a TUI picker and interactive
//! input.  With `--dangerously-bypass-approvals-and-sandbox`, the approval
//! prompts are skipped, but the process still expects a PTY for rendering.
//! Wrapping in `script(1)` provides a pseudo-terminal without a real terminal.

use anyhow::{Context, Result};
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

/// Configuration for a PTY fork session.
#[derive(Debug, Clone)]
pub struct PtyForkConfig {
    /// The codex session ID (UUID) to fork from.
    pub session_id: String,
    /// Optional prompt to start the forked session with.
    pub prompt: Option<String>,
    /// Model override (passed as `-m <model>`).
    pub model: Option<String>,
    /// Working directory for the forked session.
    pub working_dir: Option<std::path::PathBuf>,
    /// Additional `codex -c key=value` config overrides.
    pub config_overrides: Vec<(String, String)>,
}

/// Result of a PTY fork execution.
#[derive(Debug, Clone)]
pub struct PtyForkResult {
    /// Captured stdout/PTY output.
    pub output: String,
    /// Exit code from the `script` wrapper.
    pub exit_code: i32,
}

/// Detect whether the system has GNU or BSD `script` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptVariant {
    /// GNU coreutils `script` (Linux): supports `-c <command>`.
    Gnu,
    /// BSD `script` (macOS): command goes after the typescript file.
    Bsd,
}

/// Detect the `script(1)` variant available on this system.
///
/// GNU script outputs "Usage: script [options] [file]" with `-c` option.
/// BSD script outputs "usage: script [-...] [file [command ...]]".
pub async fn detect_script_variant() -> Result<ScriptVariant> {
    let output = Command::new("script")
        .arg("--help")
        .output()
        .await
        .context("failed to run 'script --help'")?;

    // GNU `script --help` prints to stdout and exits 0.
    // BSD `script --help` may print to stderr and exit non-zero.
    let help_text = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::from_utf8_lossy(&output.stderr).to_string()
    };

    if help_text.contains("-c, --command") || help_text.contains("-c <command>") {
        Ok(ScriptVariant::Gnu)
    } else {
        Ok(ScriptVariant::Bsd)
    }
}

/// Build the `codex fork` command string for wrapping in `script(1)`.
///
/// Returns the full shell command as a string (for `script -c '...'`).
pub fn build_codex_fork_command(config: &PtyForkConfig) -> String {
    let mut parts = vec!["codex".to_string(), "fork".to_string()];

    // Session ID
    parts.push(config.session_id.clone());

    // Yolo mode for non-interactive execution
    parts.push("--dangerously-bypass-approvals-and-sandbox".to_string());

    // Disable alt-screen to get clean output
    parts.push("--no-alt-screen".to_string());

    // Model override
    if let Some(ref model) = config.model {
        parts.push("-m".to_string());
        parts.push(shell_escape(model));
    }

    // Config overrides
    for (key, value) in &config.config_overrides {
        parts.push("-c".to_string());
        parts.push(shell_escape(&format!("{key}={value}")));
    }

    // Working directory
    if let Some(ref dir) = config.working_dir {
        parts.push("-C".to_string());
        parts.push(shell_escape(&dir.to_string_lossy()));
    }

    // Prompt (must be last positional argument after session_id)
    if let Some(ref prompt) = config.prompt {
        parts.push(shell_escape(prompt));
    }

    parts.join(" ")
}

/// Build a `script(1)`-wrapped `Command` for the codex fork.
///
/// Returns a `tokio::process::Command` ready to be spawned via
/// [`spawn_tool`](super::spawn_tool) or similar.
pub fn build_pty_fork_command(config: &PtyForkConfig, variant: ScriptVariant) -> Command {
    let codex_cmd = build_codex_fork_command(config);

    debug!(
        codex_command = %codex_cmd,
        script_variant = ?variant,
        "building PTY fork command"
    );

    let mut cmd = Command::new("script");

    match variant {
        ScriptVariant::Gnu => {
            // GNU: script -qefc '<command>' /dev/null
            cmd.arg("-q") // quiet: no "Script started" header
                .arg("-e") // return child exit code
                .arg("-f") // flush after each write
                .arg("-c")
                .arg(&codex_cmd)
                .arg("/dev/null"); // typescript file (discarded)
        }
        ScriptVariant::Bsd => {
            // BSD: script -q /dev/null <command> <args...>
            cmd.arg("-q").arg("/dev/null");
            // For BSD, we use sh -c to run the full command string
            cmd.arg("sh").arg("-c").arg(&codex_cmd);
        }
    }

    cmd
}

/// Build a PTY fork command with optional environment setup.
///
/// This is the high-level entry point. It detects the `script` variant,
/// builds the wrapped command, and optionally sets working directory and
/// environment variables.
pub async fn prepare_pty_fork(
    config: &PtyForkConfig,
    env_vars: &[(&str, &str)],
    output_spool: Option<&Path>,
) -> Result<Command> {
    let variant = detect_script_variant().await?;
    let mut cmd = build_pty_fork_command(config, variant);

    // Set working directory
    if let Some(ref dir) = config.working_dir {
        cmd.current_dir(dir);
    }

    // Apply environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    // Strip Claude Code env vars (same as main CSA spawn path)
    cmd.env_remove("CLAUDECODE");
    cmd.env_remove("CLAUDE_CODE_ENTRYPOINT");

    if let Some(spool_path) = output_spool {
        debug!(spool = %spool_path.display(), "output spool configured for PTY fork");
    }

    Ok(cmd)
}

/// Shell-escape a string for safe inclusion in a command line.
///
/// Wraps in single quotes and escapes embedded single quotes.
fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If the string contains no special characters, return as-is
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/')
    {
        return s.to_string();
    }
    // Wrap in single quotes, escaping any embedded single quotes
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_escape_simple() {
        assert_eq!(shell_escape("hello"), "hello");
        assert_eq!(shell_escape("hello-world"), "hello-world");
        assert_eq!(shell_escape("/usr/bin/codex"), "/usr/bin/codex");
    }

    #[test]
    fn test_shell_escape_special_chars() {
        assert_eq!(shell_escape("hello world"), "'hello world'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
        assert_eq!(shell_escape(""), "''");
    }

    #[test]
    fn test_build_codex_fork_command_minimal() {
        let config = PtyForkConfig {
            session_id: "abc-123-def".to_string(),
            prompt: None,
            model: None,
            working_dir: None,
            config_overrides: vec![],
        };
        let cmd = build_codex_fork_command(&config);
        assert!(cmd.contains("codex fork abc-123-def"));
        assert!(cmd.contains("--dangerously-bypass-approvals-and-sandbox"));
        assert!(cmd.contains("--no-alt-screen"));
    }

    #[test]
    fn test_build_codex_fork_command_with_prompt() {
        let config = PtyForkConfig {
            session_id: "abc-123".to_string(),
            prompt: Some("fix the auth bug".to_string()),
            model: Some("o3".to_string()),
            working_dir: None,
            config_overrides: vec![],
        };
        let cmd = build_codex_fork_command(&config);
        assert!(cmd.contains("codex fork abc-123"));
        assert!(cmd.contains("-m o3"));
        assert!(cmd.contains("'fix the auth bug'"));
    }

    #[test]
    fn test_build_codex_fork_command_with_config_overrides() {
        let config = PtyForkConfig {
            session_id: "session-1".to_string(),
            prompt: None,
            model: None,
            working_dir: Some(std::path::PathBuf::from("/tmp/project")),
            config_overrides: vec![("model_reasoning_effort".to_string(), "high".to_string())],
        };
        let cmd = build_codex_fork_command(&config);
        assert!(cmd.contains("-c 'model_reasoning_effort=high'"));
        assert!(cmd.contains("-C /tmp/project"));
    }

    #[test]
    fn test_build_pty_fork_command_gnu() {
        let config = PtyForkConfig {
            session_id: "test-session".to_string(),
            prompt: Some("hello".to_string()),
            model: None,
            working_dir: None,
            config_overrides: vec![],
        };
        let cmd = build_pty_fork_command(&config, ScriptVariant::Gnu);
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_string_lossy();
        assert_eq!(program, "script");

        let args: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"-q".to_string()));
        assert!(args.contains(&"-e".to_string()));
        assert!(args.contains(&"-f".to_string()));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"/dev/null".to_string()));
    }

    #[test]
    fn test_build_pty_fork_command_bsd() {
        let config = PtyForkConfig {
            session_id: "test-session".to_string(),
            prompt: None,
            model: None,
            working_dir: None,
            config_overrides: vec![],
        };
        let cmd = build_pty_fork_command(&config, ScriptVariant::Bsd);
        let std_cmd = cmd.as_std();
        let program = std_cmd.get_program().to_string_lossy();
        assert_eq!(program, "script");

        let args: Vec<String> = std_cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"-q".to_string()));
        assert!(args.contains(&"/dev/null".to_string()));
        assert!(args.contains(&"sh".to_string()));
        assert!(args.contains(&"-c".to_string()));
    }

    #[tokio::test]
    async fn test_detect_script_variant_runs() {
        // This test verifies that detect_script_variant doesn't panic.
        // On Linux CI, it should detect GNU. On macOS, BSD.
        let result = detect_script_variant().await;
        assert!(result.is_ok(), "script variant detection should not fail");
        let variant = result.unwrap();
        // On Linux, expect GNU
        #[cfg(target_os = "linux")]
        assert_eq!(variant, ScriptVariant::Gnu);
    }

    #[test]
    fn test_prompt_with_special_chars_is_escaped() {
        let config = PtyForkConfig {
            session_id: "s1".to_string(),
            prompt: Some("what's the bug in auth.rs?".to_string()),
            model: None,
            working_dir: None,
            config_overrides: vec![],
        };
        let cmd = build_codex_fork_command(&config);
        // The prompt should be properly escaped with single quotes
        assert!(cmd.contains("'what'\\''s the bug in auth.rs?'"));
    }
}
