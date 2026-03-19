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

// Re-export spawn-related types from the dedicated module.
#[path = "connection_spawn.rs"]
mod connection_spawn;
pub use connection_spawn::{
    AcpConnectionOptions, AcpSandboxHandle, AcpSandboxRequest, AcpSpawnRequest, SandboxConfig,
};

#[path = "connection_fork.rs"]
pub(crate) mod connection_fork;
pub use connection_fork::{CliForkResult, fork_session_via_cli};

use crate::{
    client::{SessionEvent, SharedActivity, SharedEvents, StreamingMetadata},
    error::{AcpError, AcpResult},
};

const DEFAULT_HEARTBEAT_SECS: u64 = 20;
const HEARTBEAT_INTERVAL_ENV: &str = "CSA_TOOL_HEARTBEAT_SECS";

#[derive(Debug, Clone, Default)]
pub struct PromptResult {
    /// Agent output text (tail-only for large sessions).
    ///
    /// For sessions that produce more than ~1 MiB of agent text, this field
    /// contains only the trailing portion.  The full output is available on
    /// disk via the output spool file.
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
    ];

    /// Internal constructor used by `connection_spawn` after assembling parts.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_from_parts(
        local_set: LocalSet,
        connection: ClientSideConnection,
        child: Child,
        events: SharedEvents,
        last_activity: SharedActivity,
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
            stderr_buf,
            default_working_dir,
            init_timeout: options.init_timeout,
            termination_grace_period: options.termination_grace_period,
        }
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
                    "ACP initialize timed out after {}s{}; \
                     consider increasing [acp] init_timeout_seconds in .csa/config.toml",
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
                    "ACP session/new timed out after {}s{}; \
                     consider increasing [acp] init_timeout_seconds in .csa/config.toml",
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
                    "ACP session/load timed out after {}s{}; \
                     consider increasing [acp] init_timeout_seconds in .csa/config.toml",
                    self.init_timeout.as_secs(),
                    Self::format_stderr(&stderr),
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

        self.ensure_process_running()?;

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
        self.ensure_process_running()?;

        // Clear stale events before dispatching this prompt turn.
        self.events.borrow_mut().clear();
        *self.last_activity.borrow_mut() = Instant::now();
        let execution_start = Instant::now();
        let heartbeat_interval = resolve_heartbeat_interval();
        let mut last_heartbeat = execution_start;
        let mut processed_event_count = 0usize;
        let mut output_spool =
            open_output_spool_file(io.output_spool, io.spool_max_bytes, io.keep_rotated_spool);
        let mut metadata = StreamingMetadata::default();
        let mut stdout_line_buf = String::new();
        let mut thought_line_buf = String::new();

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
                            stream_new_agent_messages(
                                &self.events,
                                &mut processed_event_count,
                                io.stream_stdout_to_stderr,
                                &mut output_spool,
                                &mut metadata,
                                &mut stdout_line_buf,
                                &mut thought_line_buf,
                            );
                            let effective_timeout = if processed_event_count == 0 {
                                initial_response_timeout.unwrap_or(idle_timeout)
                            } else {
                                idle_timeout
                            };
                            maybe_emit_heartbeat(
                                heartbeat_interval,
                                execution_start,
                                *self.last_activity.borrow(),
                                &mut last_heartbeat,
                                effective_timeout,
                            );
                            if self.last_activity.borrow().elapsed() >= effective_timeout {
                                break PromptOutcome::IdleTimeout;
                            }
                        }
                    }
                }
            })
            .await;

        stream_new_agent_messages(
            &self.events,
            &mut processed_event_count,
            io.stream_stdout_to_stderr,
            &mut output_spool,
            &mut metadata,
            &mut stdout_line_buf,
            &mut thought_line_buf,
        );
        // Finalize spool: flush + run sanitization (rotate cleanup if keep_rotated=false).
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
        let output = collect_agent_output(&metadata);
        match outcome {
            PromptOutcome::Completed(Ok(response)) => Ok(PromptResult {
                output,
                events,
                exit_reason: Some(stop_reason_to_string(response.stop_reason)),
                timed_out: false,
                metadata,
            }),
            PromptOutcome::Completed(Err(err)) => Err(AcpError::PromptFailed(err.to_string())),
            PromptOutcome::IdleTimeout => {
                let _ = self.kill().await;
                let exit_reason =
                    if processed_event_count == 0 && initial_response_timeout.is_some() {
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
            let code = status.code().unwrap_or(-1);
            let stderr = self.stderr();
            return Err(AcpError::ProcessExited { code, stderr });
        }
        Ok(())
    }

    /// Format captured stderr for inclusion in error messages.
    ///
    /// Returns an empty string when no stderr was captured, or
    /// `"; stderr: <content>"` otherwise.
    pub(crate) fn format_stderr(stderr: &str) -> String {
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            String::new()
        } else {
            format!("; stderr: {trimmed}")
        }
    }
}

/// 64 KiB buffer for spool writes — reduces syscall overhead vs per-chunk flush.
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

fn maybe_emit_heartbeat(
    heartbeat_interval: Option<Duration>,
    execution_start: Instant,
    last_activity: Instant,
    last_heartbeat: &mut Instant,
    idle_timeout: Duration,
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
    eprintln!(
        "[csa-heartbeat] ACP prompt still running: elapsed={}s idle={}s idle-timeout={}s",
        elapsed.as_secs(),
        idle_for.as_secs(),
        idle_timeout.as_secs()
    );
    *last_heartbeat = now;
}

/// Maximum bytes a line buffer may hold before being force-flushed.
/// Prevents unbounded memory growth on long non-newline output (base64,
/// minified JSON, etc.).
pub(crate) const LINE_BUF_CAP: usize = 64 * 1024;

/// Flush complete lines (terminated by `\n`) from `buf`, each prefixed with
/// `prefix`.  Incomplete trailing content stays in `buf` for the next call,
/// unless the buffer exceeds [`LINE_BUF_CAP`], in which case the entire
/// remainder is force-flushed.
fn flush_complete_lines(buf: &mut String, prefix: &str) {
    while let Some(pos) = buf.find('\n') {
        let line: String = buf.drain(..=pos).collect();
        eprint!("{prefix}{line}");
    }
    // Prevent unbounded growth on long lines without newlines.
    if buf.len() > LINE_BUF_CAP {
        let remainder = std::mem::take(buf);
        eprintln!("{prefix}{remainder}");
    }
}

/// Flush any remaining content from `buf` (for end-of-stream or stream-type
/// switch).  Appends a newline so the log entry is properly terminated.
fn flush_remaining_buf(buf: &mut String, prefix: &str) {
    if !buf.is_empty() {
        let remainder = std::mem::take(buf);
        eprintln!("{prefix}{remainder}");
    }
}

fn stream_new_agent_messages(
    events: &SharedEvents,
    processed_event_count: &mut usize,
    stream_stdout_to_stderr: bool,
    output_spool: &mut Option<SpoolRotator>,
    metadata: &mut StreamingMetadata,
    stdout_line_buf: &mut String,
    thought_line_buf: &mut String,
) {
    // Iterate new retained events by total event count.  Older events may be
    // dropped from the front once retention reaches `MAX_RETAINED_EVENTS`, so
    // the processing cursor is tracked against the total number of events seen
    // rather than the retained deque length.
    let events_ref = events.borrow();
    metadata.sync_from_store(&events_ref);
    if *processed_event_count >= events_ref.total_events_count() {
        return;
    }
    let retained_start = events_ref.retained_start_index();
    let stream_start = (*processed_event_count).max(retained_start);
    if stream_start > *processed_event_count {
        let skipped = stream_start - *processed_event_count;
        tracing::warn!(
            skipped,
            retained_start,
            processed = *processed_event_count,
            "ACP event ring buffer overrun: {skipped} events were evicted before being streamed to spool/stderr"
        );
        // Clear partial line buffers so we don't splice stale content with
        // the first retained chunk after the gap (PR #440 P3).
        stdout_line_buf.clear();
        thought_line_buf.clear();
    }
    let skip = stream_start.saturating_sub(retained_start);

    for event in events_ref.retained_events().iter().skip(skip) {
        match event {
            SessionEvent::AgentMessage(chunk) => {
                if stream_stdout_to_stderr {
                    // Flush thought buffer on stream-type switch.
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    stdout_line_buf.push_str(chunk);
                    flush_complete_lines(stdout_line_buf, "[stdout] ");
                }
                spool_chunk(output_spool, chunk.as_bytes(), metadata);
                metadata.append_text(chunk);
            }
            SessionEvent::AgentThought(chunk) => {
                if stream_stdout_to_stderr {
                    // Flush stdout buffer on stream-type switch.
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    thought_line_buf.push_str(chunk);
                    flush_complete_lines(thought_line_buf, "[thought] ");
                }
                spool_chunk(output_spool, chunk.as_bytes(), metadata);
                metadata.append_text(chunk);
            }
            SessionEvent::PlanUpdate(plan) => {
                metadata.has_plan_updates = true;
                let msg = format!("[plan] {plan}\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
            SessionEvent::ToolCallStarted { title, kind, .. } => {
                metadata.has_tool_calls = true;
                let msg = format!("[tool:started] {title} ({kind})\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
            SessionEvent::ToolCallCompleted { status, .. } => {
                let msg = format!("[tool:completed] {status}\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
            SessionEvent::Other(payload) => {
                let msg = format!("[other] {payload}\n");
                if stream_stdout_to_stderr {
                    flush_remaining_buf(stdout_line_buf, "[stdout] ");
                    flush_remaining_buf(thought_line_buf, "[thought] ");
                    eprint!("{msg}");
                }
                spool_chunk(output_spool, msg.as_bytes(), metadata);
            }
        }
    }

    // Debounce: flush any accumulated partial line at the end of each poll
    // cycle to maintain progressive output visibility.  This ensures one
    // coalesced [stdout] tag per ~200ms poll instead of per-token, while
    // still showing progress for newline-free output (PR #440 P2).
    if stream_stdout_to_stderr {
        flush_remaining_buf(stdout_line_buf, "[stdout] ");
        flush_remaining_buf(thought_line_buf, "[thought] ");
    }

    *processed_event_count = events_ref.total_events_count();
}

fn spool_chunk(spool: &mut Option<SpoolRotator>, bytes: &[u8], metadata: &mut StreamingMetadata) {
    if let Some(writer) = spool {
        let _ = writer.write(bytes);
        metadata.spool_bytes_written = writer.bytes_written();
        // BufWriter flushes automatically when the buffer fills (64 KiB).
        // No per-chunk flush — the final flush happens on drop or when the
        // prompt_with_io loop ends.
        //
        // Note: no size cap on the spool file.  Disk usage is managed by
        // `csa gc`, not here.  A HEAD-only cap would truncate tail markers
        // (e.g. return-packet), breaking fork call chains.  RAM is bounded
        // by the StreamingMetadata tail buffer instead.
    }
}

/// Collect agent output for the caller (stdout / summary extraction).
///
/// Returns the tail text buffer from [`StreamingMetadata`], which contains
/// only `AgentMessage` and `AgentThought` text, bounded to ~1 MiB.
/// The full output is on disk in the output spool file.
fn collect_agent_output(metadata: &StreamingMetadata) -> String {
    metadata.tail_text.clone()
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
