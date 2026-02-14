use std::{collections::HashMap, path::Path};

use crate::{client::SessionEvent, connection::AcpConnection, error::AcpResult};

pub use crate::connection::PromptResult;

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
    ) -> AcpResult<Self> {
        let connection = AcpConnection::spawn(command, args, working_dir, env).await?;
        connection.initialize().await?;
        let session_id = connection
            .new_session(system_prompt, Some(working_dir))
            .await?;

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
        self.connection.prompt(&self.session_id, prompt).await
    }
}

pub async fn run_prompt(
    command: &str,
    args: &[String],
    working_dir: &Path,
    env: &HashMap<String, String>,
    system_prompt: Option<&str>,
    prompt: &str,
) -> AcpResult<AcpOutput> {
    let session = AcpSession::new(command, args, working_dir, env, system_prompt).await?;
    let result = session.prompt(prompt).await?;

    // ACP processes may stay alive across prompts. If the prompt itself succeeded
    // (no error above), a still-running process is normal — default to exit_code=0.
    // Only report the actual exit code when the process has already exited (e.g., crash).
    let exit_code = session.connection().exit_code().await?.unwrap_or(0);

    Ok(AcpOutput {
        output: result.output,
        stderr: session.connection().stderr(),
        events: result.events,
        session_id: session.session_id().to_string(),
        exit_code,
    })
}
