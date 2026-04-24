//! Pre-session hook support for transport-uniform prompt priming.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DEFAULT_PRE_SESSION_TIMEOUT_SECONDS: u64 = 10;

fn default_true() -> bool {
    true
}

const fn default_timeout_seconds() -> u64 {
    DEFAULT_PRE_SESSION_TIMEOUT_SECONDS
}

fn is_default_timeout_seconds(value: &u64) -> bool {
    *value == default_timeout_seconds()
}

/// Global-only `[hooks.pre_session]` configuration from
/// `~/.config/cli-sub-agent/config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreSessionHookConfig {
    /// Whether this hook is enabled when configured.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Shell command to run via `sh -c`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Optional tool-name filter (`codex`, `gemini-cli`, `claude-code`, ...).
    /// Empty or omitted means all transports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transports: Vec<String>,
    /// Timeout in seconds. Accept `timeout_secs` as a compatibility alias, but
    /// document `timeout_seconds` for the global config shape.
    #[serde(
        default = "default_timeout_seconds",
        alias = "timeout_secs",
        skip_serializing_if = "is_default_timeout_seconds"
    )]
    pub timeout_seconds: u64,
}

impl Default for PreSessionHookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            command: None,
            transports: Vec::new(),
            timeout_seconds: default_timeout_seconds(),
        }
    }
}

impl PreSessionHookConfig {
    /// Return true when this hook should run for the resolved tool transport.
    pub fn matches_transport(&self, transport: &str) -> bool {
        self.transports.is_empty() || self.transports.iter().any(|name| name == transport)
    }
}

/// Runtime context passed to a pre-session hook.
#[derive(Debug, Clone, Copy)]
pub struct PreSessionHookContext<'a> {
    pub session_id: &'a str,
    pub transport: &'a str,
    pub project_root: &'a str,
    pub working_dir: &'a str,
    pub user_prompt: &'a str,
}

#[derive(Debug)]
struct PreSessionHookOutput {
    stdout: String,
    stderr: String,
}

#[derive(Debug, Deserialize)]
struct GlobalHooksEnvelope {
    #[serde(default)]
    hooks: Option<GlobalHooksTable>,
}

#[derive(Debug, Deserialize)]
struct GlobalHooksTable {
    #[serde(default)]
    pre_session: Option<PreSessionHookConfig>,
}

/// Resolve the global config file that may contain `[hooks.pre_session]`.
pub fn global_pre_session_config_path() -> Option<PathBuf> {
    csa_config::paths::config_dir().map(|dir| dir.join("config.toml"))
}

/// Parse `[hooks.pre_session]` from a TOML string.
pub fn parse_pre_session_hook_config(
    content: &str,
) -> Result<Option<PreSessionHookConfig>, toml::de::Error> {
    let envelope: GlobalHooksEnvelope = toml::from_str(content)?;
    Ok(envelope.hooks.and_then(|hooks| hooks.pre_session))
}

/// Load `[hooks.pre_session]` from an explicit global config path.
pub fn load_pre_session_hook_config_from_path(path: &Path) -> Option<PreSessionHookConfig> {
    if !path.exists() {
        return None;
    }

    match std::fs::read_to_string(path) {
        Ok(content) => match parse_pre_session_hook_config(&content) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "Failed to parse pre_session hook config"
                );
                None
            }
        },
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "Failed to read pre_session hook config"
            );
            None
        }
    }
}

/// Load `[hooks.pre_session]` from the global CSA config.
pub fn load_global_pre_session_hook_config() -> Option<PreSessionHookConfig> {
    global_pre_session_config_path()
        .as_deref()
        .and_then(load_pre_session_hook_config_from_path)
}

/// Wrap hook stdout in the system reminder block used for prompt priming.
pub fn format_pre_session_reminder(stdout: &str) -> Option<String> {
    let content = stdout.trim();
    if content.is_empty() {
        return None;
    }
    Some(format!("<system-reminder>\n{content}\n</system-reminder>"))
}

/// Prepend hook stdout to the user prompt when stdout is non-empty.
pub fn prepend_pre_session_stdout(prompt: &str, stdout: &str) -> Option<String> {
    format_pre_session_reminder(stdout).map(|reminder| format!("{reminder}\n\n{prompt}"))
}

/// Run a pre-session hook opportunistically and return a prompt with injected
/// context when the hook succeeds and writes non-empty stdout.
pub fn run_pre_session_hook(
    config: &PreSessionHookConfig,
    context: &PreSessionHookContext<'_>,
) -> Option<String> {
    if !config.enabled {
        tracing::debug!("pre_session hook disabled");
        return None;
    }
    if !config.matches_transport(context.transport) {
        tracing::debug!(
            transport = context.transport,
            configured = ?config.transports,
            "pre_session hook skipped by transport filter"
        );
        return None;
    }

    let Some(command) = config
        .command
        .as_deref()
        .filter(|cmd| !cmd.trim().is_empty())
    else {
        tracing::warn!(
            "pre_session hook enabled but command is missing; continuing without injection"
        );
        return None;
    };

    match run_pre_session_hook_command(command, config.timeout_seconds, context) {
        Ok(output) => {
            if !output.stderr.trim().is_empty() {
                tracing::warn!(
                    stderr = %output.stderr.trim(),
                    "pre_session hook wrote to stderr"
                );
            }
            prepend_pre_session_stdout(context.user_prompt, &output.stdout)
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "pre_session hook failed; continuing without injection"
            );
            None
        }
    }
}

fn run_pre_session_hook_command(
    command: &str,
    timeout_seconds: u64,
    context: &PreSessionHookContext<'_>,
) -> Result<PreSessionHookOutput> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(context.working_dir)
        .env("CSA_SESSION_ID", context.session_id)
        .env("CSA_TRANSPORT", context.transport)
        .env("CSA_PROJECT_ROOT", context.project_root)
        .env("CSA_WORKING_DIR", context.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| "failed to spawn pre_session hook")?;

    let stdin = child.stdin.take();
    let prompt = context.user_prompt.as_bytes().to_vec();
    let stdin_writer = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            // Hooks are allowed to ignore stdin. A fast command such as
            // `echo context` can close its stdin before this writer finishes;
            // that must not turn an otherwise successful hook into a failure.
            let _ = stdin.write_all(&prompt);
        }
    });

    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let start = Instant::now();

    loop {
        match child.try_wait()? {
            Some(status) => {
                stdin_writer
                    .join()
                    .map_err(|_| anyhow::anyhow!("pre_session hook stdin writer panicked"))?;
                let output = child.wait_with_output()?;
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if !status.success() {
                    let exit_code = status.code().unwrap_or(-1);
                    bail!(
                        "pre_session hook exited with code {exit_code}: {}",
                        stderr.trim()
                    );
                }
                return Ok(PreSessionHookOutput { stdout, stderr });
            }
            None => {
                if start.elapsed() >= timeout {
                    #[cfg(unix)]
                    {
                        // SAFETY: negative PID targets the process group created
                        // with process_group(0) for this child.
                        unsafe {
                            libc::kill(-(child.id() as i32), libc::SIGKILL);
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                    let _ = stdin_writer.join();
                    bail!("pre_session hook timed out after {}s", timeout.as_secs());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context<'a>(prompt: &'a str) -> PreSessionHookContext<'a> {
        PreSessionHookContext {
            session_id: "01TESTSESSION",
            transport: "codex",
            project_root: "/project",
            working_dir: "/",
            user_prompt: prompt,
        }
    }

    #[test]
    fn parse_pre_session_hook_config_from_global_toml() {
        let parsed = parse_pre_session_hook_config(
            r#"
[hooks.pre_session]
command = "mempal timeline --language en --limit 30"
enabled = true
transports = ["codex", "gemini-cli"]
timeout_seconds = 10
"#,
        )
        .expect("parse")
        .expect("pre_session config");

        assert!(parsed.enabled);
        assert_eq!(
            parsed.command.as_deref(),
            Some("mempal timeline --language en --limit 30")
        );
        assert_eq!(parsed.transports, vec!["codex", "gemini-cli"]);
        assert_eq!(parsed.timeout_seconds, 10);
    }

    #[test]
    fn parse_pre_session_hook_config_accepts_timeout_secs_alias() {
        let parsed = parse_pre_session_hook_config(
            r#"
[hooks.pre_session]
command = "echo hook"
timeout_secs = 7
"#,
        )
        .expect("parse")
        .expect("pre_session config");

        assert_eq!(parsed.timeout_seconds, 7);
    }

    #[test]
    fn transport_filter_empty_matches_all() {
        let config = PreSessionHookConfig::default();

        assert!(config.matches_transport("codex"));
        assert!(config.matches_transport("gemini-cli"));
    }

    #[test]
    fn transport_filter_matches_exact_transport_only() {
        let config = PreSessionHookConfig {
            transports: vec!["gemini-cli".to_string()],
            ..Default::default()
        };

        assert!(config.matches_transport("gemini-cli"));
        assert!(!config.matches_transport("codex"));
    }

    #[test]
    fn prepends_hook_stdout_as_system_reminder() {
        let prompt = prepend_pre_session_stdout("user task", "primed context\n").expect("inject");

        assert_eq!(
            prompt,
            "<system-reminder>\nprimed context\n</system-reminder>\n\nuser task"
        );
    }

    #[test]
    fn empty_hook_stdout_skips_injection() {
        assert!(prepend_pre_session_stdout("user task", "\n \t").is_none());
    }

    #[test]
    fn run_pre_session_hook_success_reads_prompt_from_stdin() {
        let config = PreSessionHookConfig {
            command: Some("read line; printf 'seen:%s\\n' \"$line\"".to_string()),
            timeout_seconds: 2,
            ..Default::default()
        };

        let injected =
            run_pre_session_hook(&config, &context("original prompt")).expect("hook should inject");

        assert!(injected.contains("seen:original prompt"));
        assert!(injected.ends_with("\n\noriginal prompt"));
    }

    #[test]
    fn run_pre_session_hook_nonzero_skips_injection() {
        let config = PreSessionHookConfig {
            command: Some("echo nope >&2; exit 42".to_string()),
            timeout_seconds: 2,
            ..Default::default()
        };

        assert!(run_pre_session_hook(&config, &context("original prompt")).is_none());
    }

    #[test]
    fn run_pre_session_hook_timeout_skips_injection() {
        let config = PreSessionHookConfig {
            command: Some("sleep 2".to_string()),
            timeout_seconds: 1,
            ..Default::default()
        };

        assert!(run_pre_session_hook(&config, &context("original prompt")).is_none());
    }

    #[test]
    fn run_pre_session_hook_missing_command_skips_injection() {
        let config = PreSessionHookConfig {
            command: None,
            ..Default::default()
        };

        assert!(run_pre_session_hook(&config, &context("original prompt")).is_none());
    }
}
