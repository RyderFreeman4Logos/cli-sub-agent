use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    time::{Duration, Instant},
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
    client::{AcpClient, SessionEvent, SharedActivity, SharedEvents},
    error::{AcpError, AcpResult},
};

#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    pub output: String,
    pub events: Vec<SessionEvent>,
    pub exit_reason: Option<String>,
    pub timed_out: bool,
}

pub struct AcpConnection {
    local_set: LocalSet,
    connection: ClientSideConnection,
    child: Rc<RefCell<Child>>,
    events: SharedEvents,
    last_activity: SharedActivity,
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

        // Isolate ACP child in its own process group so timeout kill can
        // terminate the full subtree.
        // SAFETY: setsid() runs in pre-exec before Rust runtime exists in child.
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

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
        let last_activity = Rc::new(RefCell::new(Instant::now()));
        let client = AcpClient::new(events.clone(), last_activity.clone());
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
                let activity_clone = last_activity.clone();
                tokio::task::spawn_local(async move {
                    let mut reader = stderr;
                    let mut buf = vec![0_u8; 4096];
                    loop {
                        match reader.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                *activity_clone.borrow_mut() = Instant::now();
                                let text = String::from_utf8_lossy(&buf[..n]);
                                stderr_buf_clone.borrow_mut().push_str(&text);
                            }
                            Err(err) => {
                                warn!(error = %err, "failed to read ACP stderr stream");
                                break;
                            }
                        }
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
            last_activity,
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

    pub async fn prompt(
        &self,
        session_id: &str,
        text: &str,
        idle_timeout: Duration,
    ) -> AcpResult<PromptResult> {
        self.ensure_process_running()?;

        // Clear stale events before dispatching this prompt turn.
        self.events.borrow_mut().clear();
        *self.last_activity.borrow_mut() = Instant::now();

        let request = PromptRequest::new(SessionId::new(session_id.to_string()), vec![text.into()]);

        enum PromptOutcome<T> {
            Completed(T),
            IdleTimeout,
        }
        let outcome = self
            .local_set
            .run_until(async {
                let prompt_future = self.connection.prompt(request);
                tokio::pin!(prompt_future);
                loop {
                    tokio::select! {
                        response = &mut prompt_future => {
                            break PromptOutcome::Completed(response);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            if self.last_activity.borrow().elapsed() >= idle_timeout {
                                break PromptOutcome::IdleTimeout;
                            }
                        }
                    }
                }
            })
            .await;

        let events = std::mem::take(&mut *self.events.borrow_mut());
        let output = collect_agent_output(&events);
        match outcome {
            PromptOutcome::Completed(Ok(response)) => Ok(PromptResult {
                output,
                events,
                exit_reason: Some(stop_reason_to_string(response.stop_reason)),
                timed_out: false,
            }),
            PromptOutcome::Completed(Err(err)) => Err(AcpError::PromptFailed(err.to_string())),
            PromptOutcome::IdleTimeout => {
                let _ = self.kill();
                Ok(PromptResult {
                    output,
                    events,
                    exit_reason: Some("idle_timeout".to_string()),
                    timed_out: true,
                })
            }
        }
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
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            // SAFETY: kill() is async-signal-safe. Negative PID targets process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            return Ok(());
        }

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
