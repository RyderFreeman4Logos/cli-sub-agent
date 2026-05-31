use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use agent_client_protocol::{
    Agent, ClientSideConnection, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, ProtocolVersion, SessionId, StopReason,
};
use csa_process::{DEFAULT_SPOOL_KEEP_ROTATED, DEFAULT_SPOOL_MAX_BYTES, SpoolRotator};
use tokio::{process::Child, task::LocalSet};

#[path = "connection_status.rs"]
mod connection_status;
use connection_status::{format_stderr, process_exited_error};

#[path = "connection_stream.rs"]
mod connection_stream;
#[cfg(test)]
pub(crate) use connection_stream::LINE_BUF_CAP;
use connection_stream::{collect_agent_output, stream_new_agent_messages};

// Re-export spawn-related types from the dedicated module.
#[path = "connection_spawn.rs"]
mod connection_spawn;
pub use connection_spawn::{
    AcpConnectionOptions, AcpSandboxHandle, AcpSandboxRequest, AcpSpawnRequest,
};

#[path = "connection_fork.rs"]
pub(crate) mod connection_fork;
pub use connection_fork::{CliForkResult, fork_session_via_cli};

use crate::{
    client::{SessionEvent, SharedActivity, SharedEvents, StreamingMetadata},
    error::{AcpError, AcpResult},
};

const DEFAULT_HEARTBEAT_SECS: u64 = 15;
const HEARTBEAT_INTERVAL_ENV: &str = "CSA_TOOL_HEARTBEAT_SECS";

#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    /// Agent output text (tail-only for large sessions; full output stays in the spool file).
    pub output: String,
    pub events: Vec<SessionEvent>,
    pub exit_reason: Option<String>,
    pub timed_out: bool,
    /// Incrementally collected metadata from the event stream.
    pub metadata: StreamingMetadata,
}

#[derive(Debug, Clone, Copy)]
pub struct PromptIoOptions<'a> {
    pub stream_stdout_to_stderr: bool,
    pub output_spool: Option<&'a Path>,
    pub spool_max_bytes: u64,
    pub keep_rotated_spool: bool,
}

impl Default for PromptIoOptions<'_> {
    fn default() -> Self {
        Self {
            stream_stdout_to_stderr: false,
            output_spool: None,
            spool_max_bytes: DEFAULT_SPOOL_MAX_BYTES,
            keep_rotated_spool: DEFAULT_SPOOL_KEEP_ROTATED,
        }
    }
}

pub struct AcpConnection {
    local_set: LocalSet,
    connection: ClientSideConnection,
    child: Rc<RefCell<Child>>,
    events: SharedEvents,
    last_activity: SharedActivity,
    last_meaningful_activity: SharedActivity,
    stderr_buf: Rc<RefCell<String>>,
    default_working_dir: PathBuf,
    init_timeout: Duration,
    termination_grace_period: Duration,
}

impl AcpConnection {
    /// Environment variables stripped before spawning ACP child processes.
    ///
    /// These are set by the parent Claude Code instance and interfere with
    /// the child ACP adapter or the tool it wraps.
    pub(crate) const STRIPPED_ENV_VARS: &[&str] = &[
        // Claude Code sets this to detect recursive invocations.  When
        // inherited by a child claude-code-acp → claude-code chain, the
        // child refuses to start.
        "CLAUDECODE",
        // Entrypoint tracking for the parent session — not meaningful for
        // the ACP subprocess.
        "CLAUDE_CODE_ENTRYPOINT",
        // Gemini auth/routing must be controlled by CSA retry state so each
        // fresh invocation still starts on the quota-backed path.
        "GEMINI_API_KEY",
        "GOOGLE_GEMINI_BASE_URL",
        // Lefthook hook-bypass env vars must never leak into child tool
        // processes.  If the parent process has these set (e.g. from a
        // user's shell), the child tool would silently skip pre-commit
        // hooks, violating AGENTS.md rule 029.
        "LEFTHOOK",
        "LEFTHOOK_SKIP",
    ];

    /// Internal constructor used by `connection_spawn` after assembling parts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_from_parts(
        local_set: LocalSet,
        connection: ClientSideConnection,
        child: Child,
        events: SharedEvents,
        last_activity: SharedActivity,
        last_meaningful_activity: SharedActivity,
        stderr_buf: Rc<RefCell<String>>,
        default_working_dir: PathBuf,
        options: AcpConnectionOptions,
    ) -> Self {
        Self {
            local_set,
            connection,
            child: Rc::new(RefCell::new(child)),
            events,
            last_activity,
            last_meaningful_activity,
            stderr_buf,
            default_working_dir,
            init_timeout: options.init_timeout,
            termination_grace_period: options.termination_grace_period,
        }
    }

    pub async fn initialize(&self) -> AcpResult<()> {
        self.ensure_process_running().await?;

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
            Some(Err(err)) => {
                let stderr = self.stderr();
                Err(AcpError::InitializationFailed(format!(
                    "{err}{}",
                    format_stderr(&stderr)
                )))
            }
            None => {
                let stderr = self.stderr();
                let _ = self.kill().await;
                Err(AcpError::InitializationFailed(format!(
                    "ACP initialize timed out after {}s{}; \
                     consider increasing [acp] init_timeout_seconds in .csa/config.toml",
                    self.init_timeout.as_secs(),
                    format_stderr(&stderr),
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
        self.ensure_process_running().await?;

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
            Some(Err(err)) => {
                let stderr = self.stderr();
                Err(AcpError::SessionFailed(format!(
                    "{err}{}",
                    format_stderr(&stderr)
                )))
            }
            None => {
                let stderr = self.stderr();
                let _ = self.kill().await;
                Err(AcpError::SessionFailed(format!(
                    "ACP session/new timed out after {}s{}; \
                     consider increasing [acp] init_timeout_seconds in .csa/config.toml",
                    self.init_timeout.as_secs(),
                    format_stderr(&stderr),
                )))
            }
        }
    }

    pub async fn load_session(
        &self,
        session_id: &str,
        working_dir: Option<&Path>,
    ) -> AcpResult<String> {
        self.ensure_process_running().await?;

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
            Some(Err(err)) => {
                let stderr = self.stderr();
                Err(AcpError::SessionFailed(format!(
                    "{err}{}",
                    format_stderr(&stderr)
                )))
            }
            None => {
                // Unlike initialize/new_session, do NOT kill the process here.
                // load_session is an optional optimisation (resume vs create new).
                // The caller (run_acp_sandboxed) falls back to new_session on
                // failure, so the connection must stay alive for that attempt.
                let stderr = self.stderr();
                Err(AcpError::SessionFailed(format!(
                    "ACP session/load timed out after {}s{}; \
                     consider increasing [acp] init_timeout_seconds in .csa/config.toml",
                    self.init_timeout.as_secs(),
                    format_stderr(&stderr),
                )))
            }
        }
    }

    /// Fork a provider session via CLI, then load the new session into this ACP connection.
    ///
    /// This is a two-step process:
    /// 1. Call `claude --resume <id> --fork-session` to create a new provider-level session
    /// 2. Call `load_session()` to attach the ACP connection to the forked session
    ///
    /// Only supported for Claude Code (the `claude` CLI must be available).
    /// For other tools, returns `AcpError::ForkFailed` with an explanation.
    pub async fn fork_and_load_session(
        &self,
        provider_session_id: &str,
        tool_name: &str,
        working_dir: Option<&Path>,
    ) -> AcpResult<String> {
        if tool_name != "claude-code" {
            return Err(AcpError::ForkFailed(format!(
                "CLI fork is only supported for claude-code, not {tool_name}"
            )));
        }

        self.ensure_process_running().await?;

        let fork_dir = working_dir.unwrap_or(self.default_working_dir.as_path());
        let fork_result =
            connection_fork::fork_session_via_cli(provider_session_id, fork_dir, self.init_timeout)
                .await?;

        tracing::debug!(
            original_session = provider_session_id,
            forked_session = %fork_result.session_id,
            "CLI fork succeeded, loading forked session via ACP"
        );

        self.load_session(&fork_result.session_id, working_dir)
            .await
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        text: &str,
        idle_timeout: Duration,
        initial_response_timeout: Option<Duration>,
    ) -> AcpResult<PromptResult> {
        self.prompt_with_io(
            session_id,
            text,
            idle_timeout,
            initial_response_timeout,
            PromptIoOptions::default(),
        )
        .await
    }

    pub async fn prompt_with_io(
        &self,
        session_id: &str,
        text: &str,
        idle_timeout: Duration,
        initial_response_timeout: Option<Duration>,
        io: PromptIoOptions<'_>,
    ) -> AcpResult<PromptResult> {
        self.ensure_process_running().await?;

        self.events.borrow_mut().clear();
        let now = Instant::now();
        *self.last_activity.borrow_mut() = now;
        *self.last_meaningful_activity.borrow_mut() = now;
        let execution_start = Instant::now();
        let heartbeat_interval = resolve_heartbeat_interval();
        let mut last_heartbeat = execution_start;
        let mut saw_initial_response_event = false;
        let mut processed_event_count = 0usize;
        let mut output_spool =
            open_output_spool_file(io.output_spool, io.spool_max_bytes, io.keep_rotated_spool);
        let mut metadata = StreamingMetadata::default();
        let (mut stdout_line_buf, mut thought_line_buf) = (String::new(), String::new());

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
                            let _ = stream_new_agent_messages(
                                &self.events,
                                &mut processed_event_count,
                                io.stream_stdout_to_stderr,
                                &mut output_spool,
                                &mut metadata,
                                &mut stdout_line_buf,
                                &mut thought_line_buf,
                            );
                            break PromptOutcome::Completed(response);
                        }
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {
                            let saw_progress_this_poll = stream_new_agent_messages(
                                &self.events,
                                &mut processed_event_count,
                                io.stream_stdout_to_stderr,
                                &mut output_spool,
                                &mut metadata,
                                &mut stdout_line_buf,
                                &mut thought_line_buf,
                            );
                            if saw_progress_this_poll {
                                saw_initial_response_event = true;
                            }
                            let (effective_timeout, timeout_phase, last_relevant_activity) =
                                if !saw_initial_response_event {
                                    if let Some(irt) = initial_response_timeout {
                                        // Initial-response timeout tracks only stderr or eligible ACP events.
                                        (
                                            irt,
                                            TimeoutPhase::InitialResponse,
                                            *self.last_meaningful_activity.borrow(),
                                        )
                                    } else {
                                        (idle_timeout, TimeoutPhase::Idle, *self.last_activity.borrow())
                                    }
                                } else {
                                    (idle_timeout, TimeoutPhase::Idle, *self.last_activity.borrow())
                                };
                            maybe_emit_heartbeat(
                                heartbeat_interval,
                                execution_start,
                                last_relevant_activity,
                                &mut last_heartbeat,
                                effective_timeout,
                                timeout_phase,
                            );
                            if last_relevant_activity.elapsed() >= effective_timeout {
                                break PromptOutcome::IdleTimeout;
                            }
                        }
                    }
                }
            })
            .await;

        let _ = stream_new_agent_messages(
            &self.events,
            &mut processed_event_count,
            io.stream_stdout_to_stderr,
            &mut output_spool,
            &mut metadata,
            &mut stdout_line_buf,
            &mut thought_line_buf,
        );
        if let Some(writer) = output_spool.take() {
            match writer.finalize() {
                Ok(plan) => {
                    if let Err(e) = csa_process::sanitize_spool_plan(plan, None) {
                        tracing::warn!(error = %e, "Failed to sanitize ACP output spool");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to finalize ACP output spool");
                }
            }
        }
        // Return the retained tail only.  Total event counts and command/tool
        // metadata are tracked incrementally in `StreamingMetadata`.
        {
            let events_ref = self.events.borrow();
            metadata.sync_from_store(&events_ref);
        }
        let events = self.events.borrow_mut().take_events();
        let output = collect_agent_output(&mut metadata);
        match outcome {
            PromptOutcome::Completed(Ok(response)) => Ok(PromptResult {
                output,
                events,
                exit_reason: Some(stop_reason_to_string(response.stop_reason)),
                timed_out: false,
                metadata,
            }),
            PromptOutcome::Completed(Err(err)) => {
                let stderr_detail = format_stderr(&self.stderr());
                Err(AcpError::PromptFailed(format!("{err}{stderr_detail}")))
            }
            PromptOutcome::IdleTimeout => {
                let _ = self.kill().await;
                let exit_reason =
                    if !saw_initial_response_event && initial_response_timeout.is_some() {
                        "initial_response_timeout"
                    } else {
                        "idle_timeout"
                    };
                Ok(PromptResult {
                    output,
                    events,
                    exit_reason: Some(exit_reason.to_string()),
                    timed_out: true,
                    metadata,
                })
            }
        }
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child.borrow().id()
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
    async fn ensure_process_running(&self) -> AcpResult<()> {
        let status = self
            .child
            .borrow_mut()
            .try_wait()
            .map_err(|err| AcpError::ConnectionFailed(err.to_string()))?;

        if let Some(status) = status {
            self.drain_stderr_tail().await;
            let stderr = self.stderr();
            return Err(process_exited_error(status, stderr));
        }
        Ok(())
    }

    async fn drain_stderr_tail(&self) {
        self.local_set
            .run_until(tokio::time::sleep(Duration::from_millis(10)))
            .await;
    }

    #[cfg(test)]
    pub(crate) fn format_stderr(stderr: &str) -> String {
        format_stderr(stderr)
    }
}

/// 64 KiB buffer for spool writes to reduce syscall overhead vs per-chunk flush.
fn open_output_spool_file(
    path: Option<&Path>,
    spool_max_bytes: u64,
    keep_rotated_spool: bool,
) -> Option<SpoolRotator> {
    let path = path?;
    match SpoolRotator::open(path, spool_max_bytes, keep_rotated_spool) {
        Ok(rotator) => Some(rotator),
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

fn resolve_heartbeat_interval() -> Option<Duration> {
    let raw = std::env::var(HEARTBEAT_INTERVAL_ENV).ok();
    let secs = match raw {
        Some(value) => match value.trim().parse::<u64>() {
            Ok(0) => return None,
            Ok(parsed) => parsed,
            Err(_) => DEFAULT_HEARTBEAT_SECS,
        },
        None => DEFAULT_HEARTBEAT_SECS,
    };
    Some(Duration::from_secs(secs))
}

/// Indicates which timeout phase the heartbeat is reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeoutPhase {
    /// Waiting for the first response from the backend tool.
    InitialResponse,
    /// Normal idle timeout (after first output received, or no initial-response-timeout configured).
    Idle,
}

fn maybe_emit_heartbeat(
    heartbeat_interval: Option<Duration>,
    execution_start: Instant,
    last_activity: Instant,
    last_heartbeat: &mut Instant,
    effective_timeout: Duration,
    phase: TimeoutPhase,
) {
    let Some(interval) = heartbeat_interval else {
        return;
    };

    let now = Instant::now();
    let idle_for = now.saturating_duration_since(last_activity);
    if idle_for < interval {
        return;
    }
    if now.saturating_duration_since(*last_heartbeat) < interval {
        return;
    }

    let elapsed = now.saturating_duration_since(execution_start);
    let phase_label = match phase {
        TimeoutPhase::InitialResponse => "initial-response-timeout",
        TimeoutPhase::Idle => "idle-timeout",
    };
    eprintln!(
        "[csa-heartbeat] ACP prompt still running: elapsed={}s idle={}s {phase_label}={}s",
        elapsed.as_secs(),
        idle_for.as_secs(),
        effective_timeout.as_secs()
    );
    *last_heartbeat = now;
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
