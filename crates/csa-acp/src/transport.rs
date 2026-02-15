use std::time::Duration;
use std::{collections::HashMap, path::Path};

use crate::{client::SessionEvent, connection::AcpConnection, error::AcpResult};

pub use crate::connection::PromptResult;

#[derive(Debug, Clone, Copy, Default)]
pub struct AcpSessionStart<'a> {
    pub system_prompt: Option<&'a str>,
    pub resume_session_id: Option<&'a str>,
}

#[derive(Debug, Clone, Default)]
pub struct AcpOutput {
    pub output: String,
    pub stderr: String,
    pub events: Vec<SessionEvent>,
    pub session_id: String,
    pub exit_code: i32,
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
    ) -> AcpResult<Self> {
        let connection = AcpConnection::spawn(command, args, working_dir, env).await?;
        connection.initialize().await?;
        let session_id = if let Some(session_id) = resume_session_id {
            tracing::debug!(session_id, "loading ACP session");
            connection
                .load_session(session_id, Some(working_dir))
                .await?
        } else {
            tracing::debug!("creating new ACP session");
            connection
                .new_session(system_prompt, Some(working_dir))
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
    let session = AcpSession::new(
        command,
        args,
        working_dir,
        env,
        session_start.system_prompt,
        session_start.resume_session_id,
    )
    .await?;
    let result = session
        .prompt_with_idle_timeout(prompt, idle_timeout)
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
            idle_timeout.as_secs()
        ));
        stderr.push('\n');
    }

    Ok(AcpOutput {
        output: result.output,
        stderr,
        events: result.events,
        session_id: session.session_id().to_string(),
        exit_code,
    })
}
