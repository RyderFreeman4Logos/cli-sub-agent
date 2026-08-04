//! Pre-session hook support for transport-uniform prompt priming.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

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

/// Per-command invocation state for `[hooks.pre_session]`.
///
/// Clones share the same fired flag so retries and multi-turn transport calls
/// for one `csa run`/`review`/`debate` command cannot inject the hook twice.
#[derive(Debug, Clone)]
pub struct PreSessionHookInvocation {
    config: PreSessionHookConfig,
    fired: Arc<AtomicBool>,
}

impl PreSessionHookInvocation {
    pub fn new(config: PreSessionHookConfig) -> Self {
        Self {
            config,
            fired: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn config(&self) -> &PreSessionHookConfig {
        &self.config
    }

    pub fn claim_first_fire(&self) -> bool {
        self.fired
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
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

#[derive(Debug)]
struct PreSessionCleanupError(String);

impl std::fmt::Display for PreSessionCleanupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PreSessionCleanupError {}

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

/// Load global hook config and wrap it in per-invocation state.
pub fn load_global_pre_session_hook_invocation() -> Option<PreSessionHookInvocation> {
    load_global_pre_session_hook_config().map(PreSessionHookInvocation::new)
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
/// context when the hook succeeds and writes non-empty stdout. Cleanup failures
/// are returned so callers cannot continue with a live hook process group.
pub async fn run_pre_session_hook(
    config: &PreSessionHookConfig,
    context: &PreSessionHookContext<'_>,
) -> Result<Option<String>> {
    run_pre_session_hook_with_cancellation(config, context, None).await
}

/// Run a pre-session hook while observing the owning execution's cancellation.
pub async fn run_pre_session_hook_with_cancellation(
    config: &PreSessionHookConfig,
    context: &PreSessionHookContext<'_>,
    cancellation: Option<&csa_process::ExecutionCancellation>,
) -> Result<Option<String>> {
    if cancellation.is_some_and(csa_process::ExecutionCancellation::is_cancelled) {
        bail!("pre_session hook cancelled");
    }
    if !config.enabled {
        tracing::debug!("pre_session hook disabled");
        return Ok(None);
    }
    if !config.matches_transport(context.transport) {
        tracing::debug!(
            transport = context.transport,
            configured = ?config.transports,
            "pre_session hook skipped by transport filter"
        );
        return Ok(None);
    }

    let Some(command) = config
        .command
        .as_deref()
        .filter(|cmd| !cmd.trim().is_empty())
    else {
        tracing::warn!(
            "pre_session hook enabled but command is missing; continuing without injection"
        );
        return Ok(None);
    };

    match run_pre_session_hook_command(command, config.timeout_seconds, context, cancellation).await
    {
        Ok(output) => {
            if !output.stderr.trim().is_empty() {
                tracing::warn!(
                    stderr = %output.stderr.trim(),
                    "pre_session hook wrote to stderr"
                );
            }
            Ok(prepend_pre_session_stdout(
                context.user_prompt,
                &output.stdout,
            ))
        }
        Err(error) => {
            if error.downcast_ref::<PreSessionCleanupError>().is_some()
                || cancellation.is_some_and(csa_process::ExecutionCancellation::is_cancelled)
            {
                return Err(error);
            }
            tracing::warn!(
                error = %error,
                "pre_session hook failed; continuing without injection"
            );
            Ok(None)
        }
    }
}

async fn run_pre_session_hook_command(
    command: &str,
    timeout_seconds: u64,
    context: &PreSessionHookContext<'_>,
    cancellation: Option<&csa_process::ExecutionCancellation>,
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
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| "failed to spawn pre_session hook")?;

    let prompt = context.user_prompt.as_bytes().to_vec();
    let stdin_writer = child.stdin.take().map(|mut stdin| {
        tokio::spawn(async move {
            // Hooks are allowed to ignore stdin. A fast command such as
            // `echo context` can close its stdin before this writer finishes;
            // that must not turn an otherwise successful hook into a failure.
            let _ = stdin.write_all(&prompt).await;
        })
    });

    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let mut stdout = child
        .stdout
        .take()
        .context("pre_session hook stdout pipe missing")?;
    let mut stderr = child
        .stderr
        .take()
        .context("pre_session hook stderr pipe missing")?;
    let stdout_reader = tokio::spawn(async move {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).await.map(|_| output)
    });
    let stderr_reader = tokio::spawn(async move {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).await.map(|_| output)
    });

    enum HookStop {
        Exited,
        Cancelled,
        TimedOut,
        WaitFailed(std::io::Error),
    }

    let cancellation_wait = async {
        if let Some(cancellation) = cancellation {
            cancellation.cancelled().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(cancellation_wait);
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let mut child_poll = tokio::time::interval(Duration::from_millis(10));
    let stop = loop {
        tokio::select! {
            biased;
            _ = &mut cancellation_wait => break HookStop::Cancelled,
            _ = &mut deadline => break HookStop::TimedOut,
            _ = child_poll.tick() => {
                match csa_process::inspect_child_without_reaping(&mut child) {
                    Ok(csa_process::ChildWaitState::Running) => {}
                    Ok(csa_process::ChildWaitState::Exited(_)) => break HookStop::Exited,
                    Err(error) => break HookStop::WaitFailed(error),
                }
            }
        }
    };

    let status = if matches!(&stop, HookStop::Exited) {
        csa_process::terminate_child_process_group(&mut child, Duration::ZERO)
            .await
            .context("failed to terminate completed pre_session hook process group")
            .map_err(|error| PreSessionCleanupError(format!("{error:#}")))?
    } else {
        let cleanup = csa_process::terminate_child_process_group(&mut child, Duration::ZERO)
            .await
            .context("failed to terminate stopped pre_session hook process group")
            .map_err(|error| PreSessionCleanupError(format!("{error:#}")));
        if let Some(stdin_writer) = stdin_writer {
            stdin_writer.abort();
            let _ = stdin_writer.await;
        }
        stdout_reader.abort();
        stderr_reader.abort();
        let _ = stdout_reader.await;
        let _ = stderr_reader.await;
        cleanup?;
        match stop {
            HookStop::Cancelled => bail!("pre_session hook cancelled"),
            HookStop::TimedOut => {
                tracing::warn!(
                    timeout_seconds = timeout.as_secs(),
                    "pre_session hook timed out; terminated process group"
                );
                bail!("pre_session hook timed out after {}s", timeout.as_secs());
            }
            HookStop::WaitFailed(error) => {
                return Err(error).context("failed while waiting for pre_session hook");
            }
            HookStop::Exited => unreachable!(),
        }
    };

    if let Some(stdin_writer) = stdin_writer {
        stdin_writer
            .await
            .with_context(|| "pre_session hook stdin writer task failed to join")?;
    }
    let stdout = read_hook_pipe(stdout_reader, "stdout").await?;
    let stderr = read_hook_pipe(stderr_reader, "stderr").await?;
    let stdout = String::from_utf8_lossy(&stdout).to_string();
    let stderr = String::from_utf8_lossy(&stderr).to_string();
    if !status.success() {
        let exit_code = status.code().unwrap_or(-1);
        bail!(
            "pre_session hook exited with code {exit_code}: {}",
            stderr.trim()
        );
    }
    Ok(PreSessionHookOutput { stdout, stderr })
}

async fn read_hook_pipe(
    reader: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    pipe_name: &str,
) -> Result<Vec<u8>> {
    reader
        .await
        .with_context(|| format!("pre_session hook {pipe_name} reader task failed to join"))?
        .with_context(|| format!("failed to read pre_session hook {pipe_name}"))
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

    #[tokio::test]
    async fn run_pre_session_hook_success_reads_prompt_from_stdin() {
        let config = PreSessionHookConfig {
            command: Some("read line; printf 'seen:%s\\n' \"$line\"".to_string()),
            timeout_seconds: 2,
            ..Default::default()
        };

        let injected = run_pre_session_hook(&config, &context("original prompt"))
            .await
            .expect("hook execution")
            .expect("hook should inject");

        assert!(injected.contains("seen:original prompt"));
        assert!(injected.ends_with("\n\noriginal prompt"));
    }

    #[tokio::test]
    async fn run_pre_session_hook_nonzero_skips_injection() {
        let config = PreSessionHookConfig {
            command: Some("echo nope >&2; exit 42".to_string()),
            timeout_seconds: 2,
            ..Default::default()
        };

        assert!(
            run_pre_session_hook(&config, &context("original prompt"))
                .await
                .expect("hook execution")
                .is_none()
        );
    }

    #[tokio::test]
    async fn run_pre_session_hook_timeout_skips_injection() {
        let config = PreSessionHookConfig {
            command: Some("sleep 2".to_string()),
            timeout_seconds: 1,
            ..Default::default()
        };

        assert!(
            run_pre_session_hook(&config, &context("original prompt"))
                .await
                .expect("hook execution")
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_pre_session_hook_large_stdout_does_not_deadlock() {
        let config = PreSessionHookConfig {
            command: Some("dd if=/dev/zero bs=70000 count=1 2>/dev/null | tr '\\0' x".to_string()),
            timeout_seconds: 2,
            ..Default::default()
        };

        let injected = run_pre_session_hook(&config, &context("original prompt"))
            .await
            .expect("hook execution")
            .expect("large hook stdout should inject");

        assert!(injected.starts_with("<system-reminder>\n"));
        assert!(injected.ends_with("\n\noriginal prompt"));
    }

    #[tokio::test]
    async fn run_pre_session_hook_missing_command_skips_injection() {
        let config = PreSessionHookConfig {
            command: None,
            ..Default::default()
        };

        assert!(
            run_pre_session_hook(&config, &context("original prompt"))
                .await
                .expect("hook execution")
                .is_none()
        );
    }
}
