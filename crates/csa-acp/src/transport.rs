use std::time::Duration;
use std::{collections::HashMap, path::Path};

use csa_process::{DEFAULT_SPOOL_KEEP_ROTATED, DEFAULT_SPOOL_MAX_BYTES, ExecutionCancellation};

use crate::{
    client::SessionEvent,
    connection::{AcpConnection, PromptIoOptions},
    error::AcpResult,
    tool_output_compaction::ToolOutputCompactionConfig,
};

pub use crate::connection::PromptResult;

#[derive(Debug, Clone, Default)]
pub struct AcpSessionStart<'a> {
    pub system_prompt: Option<&'a str>,
    pub resume_session_id: Option<&'a str>,
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
    /// Provider-level session ID to fork from (creates a branching conversation).
    pub fork_session_id: Option<&'a str>,
    /// Resume at a specific message within the forked session.
    pub resume_at_message: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct AcpOutput {
    pub output: String,
    pub stderr: String,
    pub events: Vec<SessionEvent>,
    pub session_id: String,
    pub exit_code: i32,
    /// Raw ACP stop reason for the turn (`end_turn`, `max_tokens`, `cancelled`,
    /// `idle_timeout`, …). `None` when no terminal reason was recorded. Carried
    /// to the session-outcome classifier as the model-completion signal.
    pub exit_reason: Option<String>,
    pub metadata: crate::client::StreamingMetadata,
    /// Peak memory usage in MB from cgroup `memory.peak`.
    /// `None` when cgroup monitoring is unavailable.
    pub peak_memory_mb: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AcpOutputIoOptions<'a> {
    pub stream_stdout_to_stderr: bool,
    pub output_spool: Option<&'a Path>,
    pub spool_max_bytes: u64,
    pub keep_rotated_spool: bool,
    pub tool_output_compaction: Option<ToolOutputCompactionConfig>,
}

impl Default for AcpOutputIoOptions<'_> {
    fn default() -> Self {
        Self {
            stream_stdout_to_stderr: false,
            output_spool: None,
            spool_max_bytes: DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: DEFAULT_SPOOL_KEEP_ROTATED,
            tool_output_compaction: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcpRunOptions<'a> {
    pub idle_timeout: Duration,
    /// Shorter timeout for the period before the first response is received.
    /// When `Some`, the backend must produce at least one ACP notification
    /// within this window or the session is killed.
    pub initial_response_timeout: Option<Duration>,
    pub init_timeout: Duration,
    pub termination_grace_period: Duration,
    /// Pipeline cancellation shared with the spawned ACP process group.
    pub cancellation: Option<ExecutionCancellation>,
    pub io: AcpOutputIoOptions<'a>,
}

impl Default for AcpRunOptions<'_> {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(250),
            initial_response_timeout: None,
            init_timeout: Duration::from_secs(120),
            termination_grace_period: Duration::from_secs(5),
            cancellation: None,
            io: AcpOutputIoOptions::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcpSessionCreate<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub working_dir: &'a Path,
    pub env: &'a HashMap<String, String>,
    pub session_start: AcpSessionStart<'a>,
    pub init_timeout: Duration,
    pub termination_grace_period: Duration,
}

pub struct AcpSession {
    connection: AcpConnection,
    session_id: String,
}

impl AcpSession {
    pub async fn new(create: AcpSessionCreate<'_>) -> AcpResult<Self> {
        Self::new_with_cancellation(create, None).await
    }

    async fn new_with_cancellation(
        create: AcpSessionCreate<'_>,
        cancellation: Option<&ExecutionCancellation>,
    ) -> AcpResult<Self> {
        let AcpSessionCreate {
            command,
            args,
            working_dir,
            env,
            session_start,
            init_timeout,
            termination_grace_period,
        } = create;
        if cancellation.is_some_and(ExecutionCancellation::is_cancelled) {
            return Err(crate::error::AcpError::Cancelled);
        }
        let connection = AcpConnection::spawn_with_options(
            command,
            args,
            working_dir,
            env,
            crate::connection::AcpConnectionOptions {
                init_timeout,
                termination_grace_period,
            },
        )
        .await?;
        let setup = async {
            cancel_acp_step(&connection, cancellation, connection.initialize()).await?;

            // Inject fork metadata into the meta map when present.
            let meta = build_session_meta(
                session_start.meta.clone(),
                session_start.fork_session_id,
                session_start.resume_at_message,
            );

            if let Some(resume_id) = session_start.resume_session_id {
                tracing::debug!(resume_session_id = resume_id, "loading ACP session");
                match cancel_acp_step(
                    &connection,
                    cancellation,
                    connection.load_session(resume_id, Some(working_dir)),
                )
                .await
                {
                    Ok(id) => {
                        tracing::debug!(session_id = %id, "Resumed ACP session");
                        Ok(id)
                    }
                    Err(error) if matches!(error, crate::error::AcpError::Cancelled) => Err(error),
                    Err(error) => {
                        tracing::warn!(
                            resume_session_id = resume_id,
                            error = %error,
                            "Failed to resume ACP session, creating new session"
                        );
                        cancel_acp_step(
                            &connection,
                            cancellation,
                            connection.new_session(
                                session_start.system_prompt,
                                Some(working_dir),
                                meta.clone(),
                            ),
                        )
                        .await
                    }
                }
            } else {
                tracing::debug!("creating new ACP session");
                cancel_acp_step(
                    &connection,
                    cancellation,
                    connection.new_session(session_start.system_prompt, Some(working_dir), meta),
                )
                .await
            }
        }
        .await;
        let session_id = match setup {
            Ok(session_id) => session_id,
            Err(error) if matches!(error, crate::error::AcpError::Cancelled) => return Err(error),
            Err(error) => return cleanup_after_acp_error(&connection, error).await,
        };

        Ok(Self {
            connection,
            session_id,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn connection(&self) -> &AcpConnection {
        &self.connection
    }

    /// Fork this session via CLI and switch to the forked session.
    ///
    /// After forking, prompts on this `AcpSession` will target the new forked session.
    /// The original provider session remains intact (branching, not moving).
    ///
    /// Only supported for `claude-code`. Returns error for other tools.
    pub async fn fork_session(&mut self, tool_name: &str) -> AcpResult<String> {
        let new_session_id = self
            .connection
            .fork_and_load_session(&self.session_id, tool_name, None)
            .await?;
        let old_session_id = std::mem::replace(&mut self.session_id, new_session_id.clone());
        tracing::info!(
            old_session = %old_session_id,
            new_session = %new_session_id,
            "AcpSession forked to new provider session"
        );
        Ok(new_session_id)
    }

    pub async fn prompt(&self, prompt: &str) -> AcpResult<PromptResult> {
        self.connection
            .prompt(&self.session_id, prompt, Duration::from_secs(250), None)
            .await
    }

    pub async fn prompt_with_idle_timeout(
        &self,
        prompt: &str,
        idle_timeout: Duration,
        initial_response_timeout: Option<Duration>,
    ) -> AcpResult<PromptResult> {
        self.connection
            .prompt(
                &self.session_id,
                prompt,
                idle_timeout,
                initial_response_timeout,
            )
            .await
    }

    pub async fn prompt_with_idle_timeout_and_io(
        &self,
        prompt: &str,
        idle_timeout: Duration,
        initial_response_timeout: Option<Duration>,
        io: PromptIoOptions<'_>,
    ) -> AcpResult<PromptResult> {
        self.connection
            .prompt_with_io(
                &self.session_id,
                prompt,
                idle_timeout,
                initial_response_timeout,
                io,
            )
            .await
    }
}

pub async fn run_prompt(
    command: &str,
    args: &[String],
    working_dir: &Path,
    env: &HashMap<String, String>,
    session_start: AcpSessionStart<'_>,
    prompt: &str,
    idle_timeout: Duration,
) -> AcpResult<AcpOutput> {
    run_prompt_with_io(
        command,
        args,
        working_dir,
        env,
        session_start,
        prompt,
        AcpRunOptions {
            idle_timeout,
            initial_response_timeout: None,
            init_timeout: Duration::from_secs(120),
            termination_grace_period: Duration::from_secs(5),
            cancellation: None,
            io: AcpOutputIoOptions::default(),
        },
    )
    .await
}

pub async fn run_prompt_with_io(
    command: &str,
    args: &[String],
    working_dir: &Path,
    env: &HashMap<String, String>,
    session_start: AcpSessionStart<'_>,
    prompt: &str,
    options: AcpRunOptions<'_>,
) -> AcpResult<AcpOutput> {
    let session = AcpSession::new_with_cancellation(
        AcpSessionCreate {
            command,
            args,
            working_dir,
            env,
            session_start,
            init_timeout: options.init_timeout,
            termination_grace_period: options.termination_grace_period,
        },
        options.cancellation.as_ref(),
    )
    .await?;
    let result = match cancel_acp_step(
        session.connection(),
        options.cancellation.as_ref(),
        session.prompt_with_idle_timeout_and_io(
            prompt,
            options.idle_timeout,
            options.initial_response_timeout,
            PromptIoOptions {
                stream_stdout_to_stderr: options.io.stream_stdout_to_stderr,
                output_spool: options.io.output_spool,
                spool_max_bytes: options.io.spool_max_bytes,
                keep_rotated_spool: options.io.keep_rotated_spool,
                tool_output_compaction: options.io.tool_output_compaction,
            },
        ),
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return cleanup_after_acp_error(session.connection(), error).await,
    };

    // ACP processes may stay alive across prompts. If the prompt itself succeeded
    // (no error above), a still-running process is normal — default to exit_code=0.
    // Only report the actual exit code when the process has already exited (e.g., crash).
    let mut exit_code = match session.connection().exit_code().await {
        Ok(exit_code) => exit_code.unwrap_or(0),
        Err(error) => return cleanup_after_acp_error(session.connection(), error).await,
    };
    let mut stderr = session.connection().stderr();
    if result.timed_out {
        exit_code = 137;
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        let is_initial = result.exit_reason.as_deref() == Some("initial_response_timeout");
        let timeout_secs = if is_initial {
            options
                .initial_response_timeout
                .unwrap_or(options.idle_timeout)
                .as_secs()
        } else {
            options.idle_timeout.as_secs()
        };
        let label = if is_initial {
            "initial response timeout"
        } else {
            "idle timeout"
        };
        stderr.push_str(&format!(
            "{label}: no ACP events/stderr for {timeout_secs}s; process killed",
        ));
        stderr.push('\n');
    }

    // This helper owns a one-prompt connection; no caller can reuse it after return.
    session.connection().kill().await?;

    Ok(AcpOutput {
        output: result.output,
        stderr,
        events: result.events,
        session_id: session.session_id().to_string(),
        exit_code,
        exit_reason: result.exit_reason,
        metadata: result.metadata,
        peak_memory_mb: None,
    })
}

async fn cleanup_after_acp_error<T>(
    connection: &AcpConnection,
    error: crate::error::AcpError,
) -> AcpResult<T> {
    connection.kill().await?;
    Err(error)
}

async fn cancel_acp_step<T>(
    connection: &AcpConnection,
    cancellation: Option<&ExecutionCancellation>,
    step: impl std::future::Future<Output = AcpResult<T>>,
) -> AcpResult<T> {
    let Some(cancellation) = cancellation else {
        return step.await;
    };
    tokio::select! {
        result = step => result,
        _ = cancellation.cancelled() => {
            connection.kill().await?;
            Err(crate::error::AcpError::Cancelled)
        }
    }
}

/// Merge fork metadata into the session meta map. Returns the (possibly new) meta.
fn build_session_meta(
    base: Option<serde_json::Map<String, serde_json::Value>>,
    fork_session_id: Option<&str>,
    resume_at_message: Option<&str>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    if fork_session_id.is_none() && resume_at_message.is_none() {
        return base;
    }
    let mut meta = base.unwrap_or_default();
    if let Some(id) = fork_session_id {
        meta.insert(
            "fork_session_id".to_string(),
            serde_json::Value::String(id.to_string()),
        );
    }
    if let Some(msg) = resume_at_message {
        meta.insert(
            "resume_at_message".to_string(),
            serde_json::Value::String(msg.to_string()),
        );
    }
    Some(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    async fn assert_dead_or_zombie(pid: i32) {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
            "ACP descendant must be terminated before return; stat={stat}"
        );
    }

    #[cfg(unix)]
    fn acp_process_fixture(pid_file: &Path, ready_file: &Path, prompt_error: bool) -> String {
        let prompt_response = if prompt_error {
            r#"printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"prompt failed"}}\n' "$id""#
        } else {
            r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id""#
        };
        r#"
trap '' TERM
sh -c 'trap "" TERM; echo $$ > "__PID__"; : > "__READY__"; while :; do sleep 1; done' &
while [ ! -e "__READY__" ]; do sleep 0.01; done
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":[ ]*\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}}\n' "$id"
      ;;
    *'"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"fixture-session"}}\n' "$id"
      ;;
    *'"session/load"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      ;;
    *'"session/prompt"'*)
      __PROMPT_RESPONSE__
      ;;
  esac
done
"#
        .replace("__PID__", &pid_file.display().to_string())
        .replace("__READY__", &ready_file.display().to_string())
        .replace("__PROMPT_RESPONSE__", prompt_response)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn acp_completion_outcomes_terminate_term_resistant_descendants() {
        for (name, resume_session_id, prompt_error) in [
            ("new-success", None, false),
            ("resumed-success", Some("resume-session"), false),
            ("resumed-error", Some("resume-session"), true),
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let pid_file = temp.path().join(format!("{name}.pid"));
            let ready_file = temp.path().join(format!("{name}.ready"));
            let args = vec![
                "-c".to_string(),
                acp_process_fixture(&pid_file, &ready_file, prompt_error),
            ];
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                run_prompt_with_io(
                    "sh",
                    &args,
                    temp.path(),
                    &HashMap::new(),
                    AcpSessionStart {
                        resume_session_id,
                        ..Default::default()
                    },
                    "prompt",
                    AcpRunOptions {
                        idle_timeout: Duration::from_secs(1),
                        init_timeout: Duration::from_secs(1),
                        termination_grace_period: Duration::from_millis(20),
                        ..Default::default()
                    },
                ),
            )
            .await
            .expect("ACP outcome must return bounded");

            if prompt_error {
                assert!(
                    matches!(&result, Err(crate::error::AcpError::PromptFailed(_))),
                    "{name}: result={result:?}"
                );
            } else {
                let output = result.expect("successful ACP prompt");
                assert_eq!(output.exit_reason.as_deref(), Some("end_turn"), "{name}");
            }
            let pid = std::fs::read_to_string(&pid_file)
                .expect("descendant publishes pid after TERM trap")
                .trim()
                .parse()
                .expect("numeric descendant pid");
            assert_dead_or_zombie(pid).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_initialize_terminates_term_resistant_descendant() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("descendant.pid");
        let args = vec![
            "-c".to_string(),
            format!(
                "trap '' TERM; sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do sleep 1; done' & while [ ! -s \"{}\" ]; do sleep 0.01; done; printf 'not-json\\n'; exec 1>&- 2>&-; while :; do sleep 1; done",
                pid_file.display(),
                pid_file.display()
            ),
        ];
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            AcpSession::new_with_cancellation(
                AcpSessionCreate {
                    command: "sh",
                    args: &args,
                    working_dir: temp.path(),
                    env: &HashMap::new(),
                    session_start: AcpSessionStart::default(),
                    init_timeout: Duration::from_millis(100),
                    termination_grace_period: Duration::from_millis(20),
                },
                None,
            ),
        )
        .await;
        let error = match result.expect("malformed setup must return bounded") {
            Ok(_) => panic!("malformed initialization must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            crate::error::AcpError::InitializationFailed(_)
        ));
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("descendant publishes pid")
            .trim()
            .parse()
            .expect("numeric descendant pid");
        assert_dead_or_zombie(pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_during_acp_step_terminates_before_returning_cancelled() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pid_file = temp.path().join("descendant.pid");
        let args = vec![
            "-c".to_string(),
            format!(
                "trap '' TERM; sh -c 'trap \"\" TERM; echo $$ > \"{}\"; while :; do :; done' & while :; do :; done",
                pid_file.display()
            ),
        ];
        let connection = AcpConnection::spawn_with_options(
            "sh",
            &args,
            temp.path(),
            &HashMap::new(),
            crate::connection::AcpConnectionOptions {
                init_timeout: Duration::from_secs(1),
                termination_grace_period: Duration::from_millis(20),
            },
        )
        .await
        .expect("spawn ACP fixture");
        let cancellation = ExecutionCancellation::new();
        let canceller = cancellation.clone();
        let ready_pid_file = pid_file.clone();
        tokio::spawn(async move {
            while !ready_pid_file.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            canceller.cancel();
        });
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            cancel_acp_step(
                &connection,
                Some(&cancellation),
                std::future::pending::<AcpResult<()>>(),
            ),
        )
        .await
        .expect("cancellation must return bounded")
        .expect_err("cancelled ACP step must fail");
        assert!(matches!(error, crate::error::AcpError::Cancelled));
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("descendant publishes pid")
            .trim()
            .parse()
            .expect("numeric descendant pid");
        assert_dead_or_zombie(pid).await;
    }

    #[test]
    fn test_acp_session_start_default_has_no_fork_fields() {
        let start = AcpSessionStart::default();
        assert!(start.fork_session_id.is_none());
        assert!(start.resume_at_message.is_none());
    }

    #[test]
    fn test_build_session_meta_passthrough_when_no_fork() {
        let result = build_session_meta(None, None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_session_meta_passthrough_existing_when_no_fork() {
        let mut base = serde_json::Map::new();
        base.insert(
            "key".to_string(),
            serde_json::Value::String("val".to_string()),
        );
        let result = build_session_meta(Some(base.clone()), None, None);
        assert_eq!(result, Some(base));
    }

    #[test]
    fn test_build_session_meta_injects_fork_session_id() {
        let result = build_session_meta(None, Some("fork-123"), None);
        let meta = result.expect("should have meta");
        assert_eq!(meta["fork_session_id"], "fork-123");
        assert!(!meta.contains_key("resume_at_message"));
    }

    #[test]
    fn test_build_session_meta_injects_resume_at_message() {
        let result = build_session_meta(None, None, Some("msg-456"));
        let meta = result.expect("should have meta");
        assert_eq!(meta["resume_at_message"], "msg-456");
        assert!(!meta.contains_key("fork_session_id"));
    }

    #[test]
    fn test_build_session_meta_injects_both_fork_fields() {
        let mut base = serde_json::Map::new();
        base.insert("existing".to_string(), serde_json::Value::Bool(true));

        let result = build_session_meta(Some(base), Some("fork-123"), Some("msg-456"));
        let meta = result.expect("should have meta");
        assert_eq!(meta["fork_session_id"], "fork-123");
        assert_eq!(meta["resume_at_message"], "msg-456");
        assert_eq!(meta["existing"], true);
    }
}
