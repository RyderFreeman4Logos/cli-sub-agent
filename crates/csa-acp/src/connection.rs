use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
};

use agent_client_protocol::{
    Agent, ClientSideConnection, InitializeRequest, NewSessionRequest, PromptRequest,
    ProtocolVersion, SessionId, StopReason,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::LocalSet,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::warn;

use crate::{
    client::{AcpClient, SessionEvent, SharedEvents},
    error::{AcpError, AcpResult},
};

#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    pub output: String,
    pub events: Vec<SessionEvent>,
    pub exit_reason: Option<String>,
}

pub struct AcpConnection {
    local_set: LocalSet,
    connection: ClientSideConnection,
    child: Rc<RefCell<Child>>,
    events: SharedEvents,
    stderr_buf: Rc<RefCell<String>>,
    default_working_dir: PathBuf,
}

impl AcpConnection {
    pub async fn spawn(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> AcpResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(AcpError::SpawnFailed)?;

        let stdin = child.stdin.take().ok_or_else(|| {
            AcpError::ConnectionFailed("failed to capture child stdin pipe".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AcpError::ConnectionFailed("failed to capture child stdout pipe".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AcpError::ConnectionFailed("failed to capture child stderr pipe".to_string())
        })?;

        let local_set = LocalSet::new();
        let events = Rc::new(RefCell::new(Vec::new()));
        let client = AcpClient::new(events.clone());
        let stderr_buf = Rc::new(RefCell::new(String::new()));

        let connection = local_set
            .run_until(async {
                let outgoing = stdin.compat_write();
                let incoming = stdout.compat();
                let (conn, io_task) =
                    ClientSideConnection::new(client, outgoing, incoming, |fut| {
                        tokio::task::spawn_local(fut);
                    });

                tokio::task::spawn_local(async move {
                    if let Err(err) = io_task.await {
                        warn!(error = %err, "ACP I/O loop terminated");
                    }
                });

                let stderr_buf_clone = stderr_buf.clone();
                tokio::task::spawn_local(async move {
                    let mut reader = stderr;
                    let mut bytes = Vec::new();
                    if let Err(err) = reader.read_to_end(&mut bytes).await {
                        warn!(error = %err, "failed to read ACP stderr stream");
                        return;
                    }
                    if !bytes.is_empty() {
                        let text = String::from_utf8_lossy(&bytes);
                        stderr_buf_clone.borrow_mut().push_str(&text);
                    }
                });

                conn
            })
            .await;

        Ok(Self {
            local_set,
            connection,
            child: Rc::new(RefCell::new(child)),
            events,
            stderr_buf,
            default_working_dir: working_dir.to_path_buf(),
        })
    }

    pub async fn initialize(&self) -> AcpResult<()> {
        self.ensure_process_running()?;

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        self.local_set
            .run_until(async { self.connection.initialize(request).await })
            .await
            .map_err(|err| AcpError::InitializationFailed(err.to_string()))?;

        Ok(())
    }

    // TODO(acp-sdk): ACP v0.9.4 `NewSessionRequest` does not support system_prompt.
    // When the SDK adds system_prompt to session/new, thread `_system_prompt` through
    // to the request. Until then, system prompts must be prepended to the first prompt.
    pub async fn new_session(
        &self,
        _system_prompt: Option<&str>,
        working_dir: Option<&Path>,
    ) -> AcpResult<String> {
        self.ensure_process_running()?;

        let session_working_dir = working_dir.unwrap_or(self.default_working_dir.as_path());
        let request = NewSessionRequest::new(session_working_dir);

        let response = self
            .local_set
            .run_until(async { self.connection.new_session(request).await })
            .await
            .map_err(|err| AcpError::SessionFailed(err.to_string()))?;

        Ok(response.session_id.0.to_string())
    }

    pub async fn prompt(&self, session_id: &str, text: &str) -> AcpResult<PromptResult> {
        self.ensure_process_running()?;

        // Clear stale events before dispatching this prompt turn.
        self.events.borrow_mut().clear();

        let request = PromptRequest::new(SessionId::new(session_id.to_string()), vec![text.into()]);

        let response = self
            .local_set
            .run_until(async { self.connection.prompt(request).await })
            .await
            .map_err(|err| AcpError::PromptFailed(err.to_string()))?;

        let events = std::mem::take(&mut *self.events.borrow_mut());
        let output = collect_agent_output(&events);

        Ok(PromptResult {
            output,
            events,
            exit_reason: Some(stop_reason_to_string(response.stop_reason)),
        })
    }

    pub async fn exit_code(&self) -> AcpResult<Option<i32>> {
        let mut child = self.child.borrow_mut();
        let status = child
            .try_wait()
            .map_err(|err| AcpError::ConnectionFailed(err.to_string()))?;
        Ok(status.and_then(|s| s.code()))
    }

    pub fn kill(&self) -> AcpResult<()> {
        let mut child = self.child.borrow_mut();
        child
            .start_kill()
            .map_err(|err| AcpError::ConnectionFailed(err.to_string()))
    }

    pub fn stderr(&self) -> String {
        self.stderr_buf.borrow().clone()
    }

    fn ensure_process_running(&self) -> AcpResult<()> {
        let mut child = self.child.borrow_mut();
        if let Some(status) = child
            .try_wait()
            .map_err(|err| AcpError::ConnectionFailed(err.to_string()))?
        {
            return Err(AcpError::ProcessExited(status.code().unwrap_or(-1)));
        }
        Ok(())
    }
}

fn collect_agent_output(events: &[SessionEvent]) -> String {
    let mut output = String::new();
    for event in events {
        if let SessionEvent::AgentMessage(chunk) = event {
            output.push_str(chunk);
        }
    }
    output
}

fn stop_reason_to_string(reason: StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".to_string(),
        StopReason::MaxTokens => "max_tokens".to_string(),
        StopReason::MaxTurnRequests => "max_turn_requests".to_string(),
        StopReason::Refusal => "refusal".to_string(),
        StopReason::Cancelled => "cancelled".to_string(),
        _ => "unknown".to_string(),
    }
}
