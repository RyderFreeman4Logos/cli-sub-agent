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
    pub io: AcpOutputIoOptions<'a>,
}

impl Default for AcpRunOptions<'_> {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(300),
            io: AcpOutputIoOptions::default(),
        }
    }
}

pub struct AcpSession {
    connection: AcpConnection,
    session_id: String,
}

impl AcpSession {
    pub async fn new(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
        system_prompt: Option<&str>,
        resume_session_id: Option<&str>,
        meta: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> AcpResult<Self> {
        let connection = AcpConnection::spawn(command, args, working_dir, env).await?;
        connection.initialize().await?;
        let session_id = if let Some(resume_id) = resume_session_id {
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
                        .new_session(system_prompt, Some(working_dir), meta.clone())
                        .await?
                }
            }
        } else {
            tracing::debug!("creating new ACP session");
            connection
                .new_session(system_prompt, Some(working_dir), meta.clone())
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
    let session = AcpSession::new(
        command,
        args,
        working_dir,
        env,
        session_start.system_prompt,
        session_start.resume_session_id,
        session_start.meta,
    )
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
    if session_start.resume_session_id.is_none() {
        let _ = session.connection().kill();
    }

    Ok(AcpOutput {
        output: result.output,
        stderr,
        events: result.events,
        session_id: session.session_id().to_string(),
        exit_code,
    })
}
