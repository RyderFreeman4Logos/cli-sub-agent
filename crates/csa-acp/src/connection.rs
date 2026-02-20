use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    rc::Rc,
    time::{Duration, Instant},
};

use agent_client_protocol::{
    Agent, ClientSideConnection, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, ProtocolVersion, SessionId, StopReason,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    task::LocalSet,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tracing::{debug, warn};

pub use csa_resource::cgroup::SandboxConfig;
use csa_resource::sandbox::{SandboxCapability, detect_sandbox_capability};

use crate::{
    client::{AcpClient, SessionEvent, SharedActivity, SharedEvents},
    error::{AcpError, AcpResult},
};

/// Holds sandbox resources that must live as long as the ACP child process.
///
/// Mirrors [`csa_process::SandboxHandle`] for the ACP transport path.
///
/// # Signal semantics
///
/// - **`Cgroup`**: The ACP process runs inside a systemd transient scope.
///   On drop, the guard calls `systemctl --user stop <scope>`, sending
///   `SIGTERM` to all processes in the scope.
///
/// - **`Rlimit`**: `setrlimit` was applied in the child's `pre_exec`.  The
///   optional [`RssWatcher`] monitors RSS from the parent side.
///
/// - **`None`**: No sandbox active.
///
/// [`RssWatcher`]: csa_resource::rlimit::RssWatcher
pub enum AcpSandboxHandle {
    /// cgroup scope guard -- dropped to stop the scope.
    Cgroup(csa_resource::cgroup::CgroupScopeGuard),
    /// `setrlimit` was applied in child; optional RSS watcher monitors externally.
    Rlimit {
        watcher: Option<csa_resource::rlimit::RssWatcher>,
    },
    /// No sandbox active.
    None,
}

#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    pub output: String,
    pub events: Vec<SessionEvent>,
    pub exit_reason: Option<String>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PromptIoOptions<'a> {
    pub stream_stdout_to_stderr: bool,
    pub output_spool: Option<&'a Path>,
}

pub struct AcpConnection {
    local_set: LocalSet,
    connection: ClientSideConnection,
    child: Rc<RefCell<Child>>,
    events: SharedEvents,
    last_activity: SharedActivity,
    stderr_buf: Rc<RefCell<String>>,
    default_working_dir: PathBuf,
    init_timeout: Duration,
    termination_grace_period: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpConnectionOptions {
    /// Timeout for ACP initialization/session setup operations.
    pub init_timeout: Duration,
    /// Grace period between SIGTERM and SIGKILL for forced termination.
    pub termination_grace_period: Duration,
}

impl Default for AcpConnectionOptions {
    fn default() -> Self {
        Self {
            init_timeout: Duration::from_secs(60),
            termination_grace_period: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AcpSpawnRequest<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub working_dir: &'a Path,
    pub env: &'a HashMap<String, String>,
    pub options: AcpConnectionOptions,
}

#[derive(Debug, Clone, Copy)]
pub struct AcpSandboxRequest<'a> {
    pub config: &'a SandboxConfig,
    pub tool_name: &'a str,
    pub session_id: &'a str,
}

impl AcpConnection {
    /// Environment variables stripped before spawning ACP child processes.
    ///
    /// These are set by the parent Claude Code instance and interfere with
    /// the child ACP adapter or the tool it wraps.
    const STRIPPED_ENV_VARS: &[&str] = &[
        // Claude Code sets this to detect recursive invocations.  When
        // inherited by a child claude-code-acp → claude-code chain, the
        // child refuses to start.
        "CLAUDECODE",
        // Entrypoint tracking for the parent session — not meaningful for
        // the ACP subprocess.
        "CLAUDE_CODE_ENTRYPOINT",
    ];

    /// Spawn an ACP process without resource sandboxing.
    pub async fn spawn(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> AcpResult<Self> {
        Self::spawn_with_options(
            command,
            args,
            working_dir,
            env,
            AcpConnectionOptions::default(),
        )
        .await
    }

    /// Spawn an ACP process with explicit connection options.
    pub async fn spawn_with_options(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
        options: AcpConnectionOptions,
    ) -> AcpResult<Self> {
        let cmd = Self::build_cmd(command, args, working_dir, env);
        Self::spawn_with_cmd(cmd, working_dir, options).await
    }

    /// Spawn an ACP process with optional resource sandbox.
    ///
    /// When `sandbox` is `Some`, the process is wrapped in resource isolation
    /// based on the host's detected capability (cgroup v2 or setrlimit).
    /// When `sandbox` is `None`, behavior is identical to [`Self::spawn`].
    ///
    /// Returns the connection and a [`AcpSandboxHandle`] that must be kept
    /// alive for the duration of the child process.
    pub async fn spawn_sandboxed(
        request: AcpSpawnRequest<'_>,
        sandbox: Option<AcpSandboxRequest<'_>>,
    ) -> AcpResult<(Self, AcpSandboxHandle)> {
        let Some(sandbox) = sandbox else {
            let conn = Self::spawn_with_options(
                request.command,
                request.args,
                request.working_dir,
                request.env,
                request.options,
            )
            .await?;
            return Ok((conn, AcpSandboxHandle::None));
        };

        match detect_sandbox_capability() {
            SandboxCapability::CgroupV2 => {
                // Build systemd-run wrapper command, then append the ACP binary + args.
                let scope_cmd = csa_resource::cgroup::create_scope_command(
                    sandbox.tool_name,
                    sandbox.session_id,
                    sandbox.config,
                );
                let mut cmd = Command::from(scope_cmd);
                cmd.arg(request.command);
                cmd.args(request.args);
                cmd.current_dir(request.working_dir)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());

                // SAFETY: setsid() is async-signal-safe, runs before exec in child.
                #[cfg(unix)]
                unsafe {
                    cmd.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }

                // Strip inherited env vars that interfere with child ACP
                // adapters (same vars stripped by build_cmd_base for other paths).
                for var in Self::STRIPPED_ENV_VARS {
                    cmd.env_remove(var);
                }

                for (key, value) in request.env {
                    cmd.env(key, value);
                }

                let conn = Self::spawn_with_cmd(cmd, request.working_dir, request.options).await?;
                let guard = csa_resource::cgroup::CgroupScopeGuard::new(
                    sandbox.tool_name,
                    sandbox.session_id,
                );
                debug!(
                    scope = %guard.scope_name(),
                    "ACP process spawned inside cgroup scope"
                );
                Ok((conn, AcpSandboxHandle::Cgroup(guard)))
            }
            SandboxCapability::Setrlimit => {
                let mut cmd = Self::build_cmd_base(
                    request.command,
                    request.args,
                    request.working_dir,
                    request.env,
                );

                let memory_max_mb = sandbox.config.memory_max_mb;
                let pids_max = sandbox.config.pids_max.map(u64::from);

                // Apply setsid + rlimits in a single pre_exec hook.
                // SAFETY: setsid() and setrlimit are async-signal-safe and run before exec.
                #[cfg(unix)]
                unsafe {
                    cmd.pre_exec(move || {
                        libc::setsid();
                        csa_resource::rlimit::apply_rlimits(memory_max_mb, pids_max)
                            .map_err(std::io::Error::other)
                    });
                }

                let conn =
                    Self::spawn_with_cmd_raw(cmd, request.working_dir, request.options).await?;

                let watcher = conn.child.borrow().id().and_then(|pid| {
                    debug!(pid, memory_max_mb, "starting RSS watcher for ACP child");
                    match csa_resource::rlimit::RssWatcher::start(
                        pid,
                        memory_max_mb,
                        Duration::from_secs(5),
                    ) {
                        Ok(w) => Some(w),
                        Err(e) => {
                            tracing::warn!("failed to start RSS watcher: {e:#}");
                            None
                        }
                    }
                });

                Ok((conn, AcpSandboxHandle::Rlimit { watcher }))
            }
            SandboxCapability::None => {
                debug!("no sandbox capability detected; spawning ACP without isolation");
                let conn = Self::spawn_with_options(
                    request.command,
                    request.args,
                    request.working_dir,
                    request.env,
                    request.options,
                )
                .await?;
                Ok((conn, AcpSandboxHandle::None))
            }
        }
    }

    /// Build a standard ACP command with piped stdio and `setsid` pre-exec.
    fn build_cmd(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Command {
        let mut cmd = Self::build_cmd_base(command, args, working_dir, env);

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

        cmd
    }

    /// Build a standard ACP command with piped stdio and environment.
    ///
    /// Strips inherited environment variables that cause the spawned ACP
    /// adapter (e.g. `claude-code-acp`) to fail.  The parent Claude Code
    /// process sets `CLAUDECODE=1` for recursion detection, which makes
    /// any child Claude Code instance refuse to start.
    fn build_cmd_base(
        command: &str,
        args: &[String],
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Command {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Safety net: ensure child process is cleaned up if AcpConnection is dropped
        // without explicit kill() (e.g., during panic). Not the primary shutdown mechanism —
        // explicit kill() in transport.rs handles normal cleanup.
        cmd.kill_on_drop(true);

        // Strip parent-process env vars that interfere with the ACP child.
        // CLAUDECODE=1 triggers recursion detection in claude-code, causing
        // immediate exit with "unset the CLAUDECODE environment variable".
        // CLAUDE_CODE_ENTRYPOINT is parent-specific context, not relevant.
        for var in Self::STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }

        for (key, value) in env {
            cmd.env(key, value);
        }

        cmd
    }

    /// Shared connection setup from a pre-built command.
    async fn spawn_with_cmd(
        cmd: Command,
        working_dir: &Path,
        options: AcpConnectionOptions,
    ) -> AcpResult<Self> {
        Self::spawn_with_cmd_raw(cmd, working_dir, options).await
    }

    /// Core spawn logic: takes a fully configured command, spawns it, and
    /// sets up the ACP protocol connection over stdin/stdout.
    async fn spawn_with_cmd_raw(
        mut cmd: Command,
        working_dir: &Path,
        options: AcpConnectionOptions,
    ) -> AcpResult<Self> {
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
            init_timeout: options.init_timeout,
            termination_grace_period: options.termination_grace_period,
        })
    }

    pub async fn initialize(&self) -> AcpResult<()> {
        self.ensure_process_running()?;

        let request = InitializeRequest::new(ProtocolVersion::LATEST);
        let result = self
            .local_set
            .run_until(async {
                tokio::select! {
                    response = self.connection.initialize(request) => Some(response),
                    () = tokio::time::sleep(self.init_timeout) => None,
                }
            })
            .await;

        match result {
            Some(Ok(_response)) => Ok(()),
            Some(Err(err)) => Err(AcpError::InitializationFailed(err.to_string())),
            None => {
                let stderr = self.stderr();
                let _ = self.kill().await;
                Err(AcpError::InitializationFailed(format!(
                    "ACP initialize timed out after {}s{}",
                    self.init_timeout.as_secs(),
                    Self::format_stderr(&stderr),
                )))
            }
        }
    }

    // `NewSessionRequest` does not support system_prompt.
    // System prompts are prepended to the first prompt at a higher layer.
    // TODO(acp-notify): Expose an ACP-level codex notify suppression option
    // (equivalent to legacy `-c notify=[]`) when protocol support exists.
    pub async fn new_session(
        &self,
        _system_prompt: Option<&str>,
        working_dir: Option<&Path>,
        meta: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> AcpResult<String> {
        self.ensure_process_running()?;

        let session_working_dir = working_dir.unwrap_or(self.default_working_dir.as_path());
        let mut request = NewSessionRequest::new(session_working_dir);
        request.meta = meta;

        let result = self
            .local_set
            .run_until(async {
                tokio::select! {
                    response = self.connection.new_session(request) => Some(response),
                    () = tokio::time::sleep(self.init_timeout) => None,
                }
            })
            .await;

        match result {
            Some(Ok(response)) => Ok(response.session_id.0.to_string()),
            Some(Err(err)) => Err(AcpError::SessionFailed(err.to_string())),
            None => {
                let stderr = self.stderr();
                let _ = self.kill().await;
                Err(AcpError::SessionFailed(format!(
                    "ACP session/new timed out after {}s{}",
                    self.init_timeout.as_secs(),
                    Self::format_stderr(&stderr),
                )))
            }
        }
    }

    pub async fn load_session(
        &self,
        session_id: &str,
        working_dir: Option<&Path>,
    ) -> AcpResult<String> {
        self.ensure_process_running()?;

        let session_working_dir = working_dir.unwrap_or(self.default_working_dir.as_path());
        let request =
            LoadSessionRequest::new(SessionId::new(session_id.to_string()), session_working_dir);

        let result = self
            .local_set
            .run_until(async {
                tokio::select! {
                    response = self.connection.load_session(request) => Some(response),
                    () = tokio::time::sleep(self.init_timeout) => None,
                }
            })
            .await;

        match result {
            Some(Ok(_response)) => Ok(session_id.to_string()),
            Some(Err(err)) => Err(AcpError::SessionFailed(err.to_string())),
            None => {
                // Unlike initialize/new_session, do NOT kill the process here.
                // load_session is an optional optimisation (resume vs create new).
                // The caller (run_acp_sandboxed) falls back to new_session on
                // failure, so the connection must stay alive for that attempt.
                let stderr = self.stderr();
                Err(AcpError::SessionFailed(format!(
                    "ACP session/load timed out after {}s{}",
                    self.init_timeout.as_secs(),
                    Self::format_stderr(&stderr),
                )))
            }
        }
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        text: &str,
        idle_timeout: Duration,
    ) -> AcpResult<PromptResult> {
        self.prompt_with_io(session_id, text, idle_timeout, PromptIoOptions::default())
            .await
    }

    pub async fn prompt_with_io(
        &self,
        session_id: &str,
        text: &str,
        idle_timeout: Duration,
        io: PromptIoOptions<'_>,
    ) -> AcpResult<PromptResult> {
        self.ensure_process_running()?;

        // Clear stale events before dispatching this prompt turn.
        self.events.borrow_mut().clear();
        *self.last_activity.borrow_mut() = Instant::now();
        let mut streamed_event_index = 0usize;
        let mut output_spool = open_output_spool_file(io.output_spool);

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
                            stream_new_agent_messages(
                                &self.events,
                                &mut streamed_event_index,
                                io.stream_stdout_to_stderr,
                                &mut output_spool,
                            );
                            break PromptOutcome::Completed(response);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            stream_new_agent_messages(
                                &self.events,
                                &mut streamed_event_index,
                                io.stream_stdout_to_stderr,
                                &mut output_spool,
                            );
                            if self.last_activity.borrow().elapsed() >= idle_timeout {
                                break PromptOutcome::IdleTimeout;
                            }
                        }
                    }
                }
            })
            .await;

        stream_new_agent_messages(
            &self.events,
            &mut streamed_event_index,
            io.stream_stdout_to_stderr,
            &mut output_spool,
        );
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
                let _ = self.kill().await;
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

    pub async fn kill(&self) -> AcpResult<()> {
        let termination_grace_period = self.termination_grace_period;
        let child_pid = {
            let child = self.child.borrow();
            child.id()
        };
        #[cfg(unix)]
        if let Some(pid) = child_pid {
            // SAFETY: kill() is async-signal-safe. Negative PID targets process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            tokio::time::sleep(termination_grace_period).await;
            let exited = self
                .child
                .borrow_mut()
                .try_wait()
                .map_err(|err| AcpError::ConnectionFailed(err.to_string()))?
                .is_some();
            if exited {
                return Ok(());
            }
            // SAFETY: kill() is async-signal-safe. Negative PID targets process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = self.child.borrow_mut().start_kill();
            return Ok(());
        }

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

    /// Format captured stderr for inclusion in error messages.
    ///
    /// Returns an empty string when no stderr was captured, or
    /// `"; stderr: <content>"` otherwise.
    fn format_stderr(stderr: &str) -> String {
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("; stderr: {trimmed}")
        }
    }
}

fn open_output_spool_file(path: Option<&Path>) -> Option<std::fs::File> {
    let path = path?;
    use std::fs::OpenOptions;
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => Some(file),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "failed to open ACP output spool file"
            );
            None
        }
    }
}

fn stream_new_agent_messages(
    events: &SharedEvents,
    processed_index: &mut usize,
    stream_stdout_to_stderr: bool,
    output_spool: &mut Option<std::fs::File>,
) {
    let new_messages = {
        let events_ref = events.borrow();
        if *processed_index >= events_ref.len() {
            return;
        }

        let mut messages = Vec::new();
        for event in &events_ref[*processed_index..] {
            if let SessionEvent::AgentMessage(chunk) = event {
                messages.push(chunk.clone());
            }
        }
        *processed_index = events_ref.len();
        messages
    };

    for chunk in &new_messages {
        if stream_stdout_to_stderr {
            eprint!("[stdout] {chunk}");
        }
        spool_chunk(output_spool, chunk.as_bytes());
    }
}

fn spool_chunk(spool: &mut Option<std::fs::File>, bytes: &[u8]) {
    if let Some(file) = spool {
        use std::io::Write;
        let _ = file.write_all(bytes);
        let _ = file.flush();
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

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
