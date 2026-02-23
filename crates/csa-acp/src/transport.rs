use std::time::Duration;
use std::{collections::HashMap, path::Path};

use crate::{
    client::SessionEvent,
    connection::{AcpConnection, PromptIoOptions},
    error::AcpResult,
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
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AcpOutputIoOptions<'a> {
    pub stream_stdout_to_stderr: bool,
    pub output_spool: Option<&'a Path>,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpRunOptions<'a> {
    pub idle_timeout: Duration,
    pub init_timeout: Duration,
    pub termination_grace_period: Duration,
    pub io: AcpOutputIoOptions<'a>,
}

impl Default for AcpRunOptions<'_> {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(300),
            init_timeout: Duration::from_secs(60),
            termination_grace_period: Duration::from_secs(5),
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
        let AcpSessionCreate {
            command,
            args,
            working_dir,
            env,
            session_start,
            init_timeout,
            termination_grace_period,
        } = create;
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
        connection.initialize().await?;

        // Inject fork metadata into the meta map when present.
        let meta = build_session_meta(
            session_start.meta.clone(),
            session_start.fork_session_id,
            session_start.resume_at_message,
        );

        let session_id = if let Some(resume_id) = session_start.resume_session_id {
            tracing::debug!(resume_session_id = resume_id, "loading ACP session");
            match connection.load_session(resume_id, Some(working_dir)).await {
                Ok(id) => {
                    tracing::debug!(session_id = %id, "Resumed ACP session");
                    id
                }
                Err(error) => {
                    tracing::warn!(
                        resume_session_id = resume_id,
                        error = %error,
                        "Failed to resume ACP session, creating new session"
                    );
                    connection
                        .new_session(session_start.system_prompt, Some(working_dir), meta.clone())
                        .await?
                }
            }
        } else {
            tracing::debug!("creating new ACP session");
            connection
                .new_session(session_start.system_prompt, Some(working_dir), meta)
                .await?
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
            .prompt(&self.session_id, prompt, Duration::from_secs(300))
            .await
    }

    pub async fn prompt_with_idle_timeout(
        &self,
        prompt: &str,
        idle_timeout: Duration,
    ) -> AcpResult<PromptResult> {
        self.connection
            .prompt(&self.session_id, prompt, idle_timeout)
            .await
    }

    pub async fn prompt_with_idle_timeout_and_io(
        &self,
        prompt: &str,
        idle_timeout: Duration,
        io: PromptIoOptions<'_>,
    ) -> AcpResult<PromptResult> {
        self.connection
            .prompt_with_io(&self.session_id, prompt, idle_timeout, io)
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
            init_timeout: Duration::from_secs(60),
            termination_grace_period: Duration::from_secs(5),
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
    let has_resume_session = session_start.resume_session_id.is_some();
    let session = AcpSession::new(AcpSessionCreate {
        command,
        args,
        working_dir,
        env,
        session_start,
        init_timeout: options.init_timeout,
        termination_grace_period: options.termination_grace_period,
    })
    .await?;
    let result = session
        .prompt_with_idle_timeout_and_io(
            prompt,
            options.idle_timeout,
            PromptIoOptions {
                stream_stdout_to_stderr: options.io.stream_stdout_to_stderr,
                output_spool: options.io.output_spool,
            },
        )
        .await?;

    // ACP processes may stay alive across prompts. If the prompt itself succeeded
    // (no error above), a still-running process is normal — default to exit_code=0.
    // Only report the actual exit code when the process has already exited (e.g., crash).
    let mut exit_code = session.connection().exit_code().await?.unwrap_or(0);
    let mut stderr = session.connection().stderr();
    if result.timed_out {
        exit_code = 137;
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str(&format!(
            "idle timeout: no ACP events/stderr for {}s; process killed",
            options.idle_timeout.as_secs()
        ));
        stderr.push('\n');
    }

    // Kill ACP process immediately for single-prompt usage (no session resumption).
    // In session mode (resume_session_id is Some), the process stays alive for reuse.
    if !has_resume_session {
        let _ = session.connection().kill().await;
    }

    Ok(AcpOutput {
        output: result.output,
        stderr,
        events: result.events,
        session_id: session.session_id().to_string(),
        exit_code,
    })
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
