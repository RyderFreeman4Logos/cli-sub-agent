//! Hook command execution with template variable substitution.

use crate::config::HookConfig;
use crate::event::HookEvent;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Escape a string for safe shell usage by wrapping in single quotes.
///
/// Internal single quotes are escaped as '\'' (end quote, escaped quote, start quote).
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Substitute template variables in a command string using single-pass parsing.
///
/// Variables are specified as `{key}` and replaced with shell-escaped values.
/// Unrecognized placeholders are left as-is. Already-substituted content is never
/// re-scanned, preventing double-substitution attacks.
fn substitute_variables(template: &str, variables: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut key = String::new();
            let mut found_close = false;
            for inner_ch in chars.by_ref() {
                if inner_ch == '}' {
                    found_close = true;
                    break;
                }
                key.push(inner_ch);
            }
            if found_close {
                if let Some(value) = variables.get(&key) {
                    result.push_str(&shell_escape(value));
                } else {
                    // Keep unresolved placeholders as-is
                    result.push('{');
                    result.push_str(&key);
                    result.push('}');
                }
            } else {
                // Unclosed brace, keep as-is
                result.push('{');
                result.push_str(&key);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Execute a hook command with template variable substitution.
///
/// Variables are shell-escaped to prevent injection.
/// Command is run via `sh -c` with a configurable timeout.
///
/// Returns `Err` on spawn failure, non-zero exit, or timeout.
/// Callers should handle errors as best-effort (log and continue).
pub fn run_hook(
    event: HookEvent,
    config: &HookConfig,
    variables: &HashMap<String, String>,
) -> Result<()> {
    // Skip if disabled
    if !config.enabled {
        tracing::debug!(event = ?event, "Hook disabled, skipping");
        return Ok(());
    }

    // Determine command: explicit config or built-in default
    let template = match config.command.as_deref() {
        Some(cmd) => cmd,
        None => match event.builtin_command() {
            Some(cmd) => cmd,
            None => {
                tracing::debug!(event = ?event, "Hook has no command configured, skipping");
                return Ok(());
            }
        },
    };

    // Substitute variables
    let expanded_command = substitute_variables(template, variables);
    tracing::debug!(event = ?event, "Executing hook");

    // Execute via sh -c with timeout.
    // Suppress stdout/stderr to avoid polluting CLI output (e.g., --format json).
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&expanded_command)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Create new process group so timeout can kill the entire group,
    // not just the shell process (which would orphan its children).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn()?;

    let timeout = Duration::from_secs(config.timeout_secs);
    let start = Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => {
                if status.success() {
                    tracing::debug!(event = ?event, "Hook completed successfully");
                    return Ok(());
                } else {
                    let exit_code = status.code().unwrap_or(-1);
                    bail!("Hook {event:?} exited with code {exit_code}");
                }
            }
            None => {
                if start.elapsed() >= timeout {
                    // Kill the entire process group on timeout
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
                    bail!("Hook {event:?} timed out after {}s", config.timeout_secs);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Execute all hooks for an event, using the merged config.
///
/// This is a convenience wrapper around `run_hook` that handles the config lookup.
pub fn run_hooks_for_event(
    event: HookEvent,
    hooks_config: &crate::config::HooksConfig,
    variables: &HashMap<String, String>,
) -> Result<()> {
    let config = hooks_config.get_for_event(event);
    run_hook(event, &config, variables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_escape_safe_string() {
        assert_eq!(shell_escape("hello"), "'hello'");
        assert_eq!(shell_escape("hello-world"), "'hello-world'");
    }

    #[test]
    fn test_shell_escape_with_single_quote() {
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
        assert_eq!(shell_escape("don't"), "'don'\\''t'");
    }

    #[test]
    fn test_shell_escape_with_special_chars() {
        assert_eq!(shell_escape("$HOME"), "'$HOME'");
        assert_eq!(shell_escape("$(whoami)"), "'$(whoami)'");
        assert_eq!(shell_escape("`ls`"), "'`ls`'");
        assert_eq!(shell_escape("a;b"), "'a;b'");
    }

    #[test]
    fn test_substitute_variables() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "alice".to_string());
        vars.insert("id".to_string(), "123".to_string());

        let template = "echo {name} has id {id}";
        let result = substitute_variables(template, &vars);
        assert_eq!(result, "echo 'alice' has id '123'");
    }

    #[test]
    fn test_substitute_variables_with_injection_attempt() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "alice; rm -rf /".to_string());

        let template = "echo {name}";
        let result = substitute_variables(template, &vars);
        // Should be safely escaped
        assert_eq!(result, "echo 'alice; rm -rf /'");
    }

    #[test]
    fn test_substitute_no_double_substitution() {
        let mut vars = HashMap::new();
        vars.insert("name".to_string(), "{id}".to_string());
        vars.insert("id".to_string(), "INJECTED".to_string());

        let template = "echo {name}";
        let result = substitute_variables(template, &vars);
        // The value "{id}" should be shell-escaped, NOT re-substituted
        assert_eq!(result, "echo '{id}'");
    }

    #[test]
    fn test_substitute_unresolved_placeholder() {
        let vars = HashMap::new();
        let template = "echo {unknown}";
        let result = substitute_variables(template, &vars);
        assert_eq!(result, "echo {unknown}");
    }

    #[test]
    fn test_substitute_unclosed_brace() {
        let vars = HashMap::new();
        let template = "echo {unclosed";
        let result = substitute_variables(template, &vars);
        assert_eq!(result, "echo {unclosed");
    }

    #[test]
    fn test_run_hook_disabled() {
        let config = HookConfig {
            enabled: false,
            command: Some("echo test".to_string()),
            timeout_secs: 30,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_hook_simple_command() {
        let config = HookConfig {
            enabled: true,
            command: Some("echo 'hello world'".to_string()),
            timeout_secs: 30,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_hook_with_variables() {
        let config = HookConfig {
            enabled: true,
            command: Some("test -n {value}".to_string()),
            timeout_secs: 30,
        };
        let mut vars = HashMap::new();
        vars.insert("value".to_string(), "test123".to_string());

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_hook_nonzero_exit_returns_err() {
        let config = HookConfig {
            enabled: true,
            command: Some("exit 1".to_string()),
            timeout_secs: 30,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_hook_timeout() {
        let config = HookConfig {
            enabled: true,
            command: Some("sleep 10".to_string()),
            timeout_secs: 1,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }

    #[test]
    fn test_run_hook_builtin_command() {
        let config = HookConfig {
            enabled: true,
            command: None, // Use built-in
            timeout_secs: 30,
        };
        let mut vars = HashMap::new();
        vars.insert("session_id".to_string(), "test-session".to_string());
        vars.insert("sessions_root".to_string(), "/tmp".to_string());

        // SessionComplete has a built-in command with git
        // This will return Err because /tmp is not a git repo, which is expected
        let result = run_hook(HookEvent::SessionComplete, &config, &vars);
        // Best-effort: the function returns Err for non-zero exit,
        // callers handle it gracefully
        assert!(result.is_err());
    }

    #[test]
    fn test_run_hooks_for_event_empty_config() {
        let hooks_config = crate::config::HooksConfig::default();
        let vars = HashMap::new();

        // PreRun has no built-in and empty config means disabled by default
        let result = run_hooks_for_event(HookEvent::PreRun, &hooks_config, &vars);
        assert!(
            result.is_ok(),
            "Empty config + no-builtin event should be Ok (disabled)"
        );
    }

    #[test]
    fn test_run_hooks_for_event_builtin_event_empty_config() {
        let hooks_config = crate::config::HooksConfig::default();
        let mut vars = HashMap::new();
        vars.insert("session_id".to_string(), "test-id".to_string());
        vars.insert("sessions_root".to_string(), "/nonexistent".to_string());

        // SessionComplete has a built-in command; empty config still enables it
        let result = run_hooks_for_event(HookEvent::SessionComplete, &hooks_config, &vars);
        // Built-in command will fail (not a git repo), but the function should run
        assert!(
            result.is_err(),
            "Built-in hook should execute and fail on non-git dir"
        );
    }

    #[test]
    fn test_run_hook_no_command_no_builtin_skips() {
        // PreRun has no builtin_command, and no command configured => should skip
        let config = HookConfig {
            enabled: true,
            command: None,
            timeout_secs: 30,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(
            result.is_ok(),
            "Event with no command and no builtin should skip (Ok)"
        );
    }

    #[test]
    fn test_run_hook_missing_script_error() {
        let config = HookConfig {
            enabled: true,
            command: Some("/nonexistent/path/to/script_abc123.sh".to_string()),
            timeout_secs: 5,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        // sh -c will fail with non-zero exit when script doesn't exist
        assert!(
            result.is_err(),
            "Non-existent script should produce an error"
        );
    }

    #[test]
    fn test_run_hook_with_tempdir_script() {
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("hook.sh");
        std::fs::write(&script_path, "#!/bin/sh\nexit 0\n").unwrap();

        let config = HookConfig {
            enabled: true,
            command: Some(format!("sh {}", script_path.display())),
            timeout_secs: 10,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_ok(), "Valid script in tempdir should succeed");
    }

    #[test]
    fn test_run_hook_script_with_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("fail.sh");
        std::fs::write(&script_path, "#!/bin/sh\nexit 42\n").unwrap();

        let config = HookConfig {
            enabled: true,
            command: Some(format!("sh {}", script_path.display())),
            timeout_secs: 10,
        };
        let vars = HashMap::new();

        let result = run_hook(HookEvent::PreRun, &config, &vars);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("exited with code 42"),
            "Error should contain exit code, got: {err_msg}"
        );
    }

    #[test]
    fn test_substitute_variables_empty_template() {
        let vars = HashMap::new();
        let result = substitute_variables("", &vars);
        assert_eq!(result, "");
    }

    #[test]
    fn test_substitute_variables_no_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("key".to_string(), "value".to_string());
        let result = substitute_variables("echo hello world", &vars);
        assert_eq!(result, "echo hello world");
    }

    #[test]
    fn test_substitute_variables_empty_key() {
        let mut vars = HashMap::new();
        vars.insert(String::new(), "empty_key_value".to_string());
        let result = substitute_variables("echo {}", &vars);
        // {} has an empty key, which matches the empty-string key
        assert_eq!(result, "echo 'empty_key_value'");
    }
}
