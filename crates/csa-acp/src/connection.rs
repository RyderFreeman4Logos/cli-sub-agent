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
#[cfg(test)]
use csa_process::SpoolRotator;
use csa_process::{
    ChildWaitState, DEFAULT_SPOOL_KEEP_ROTATED, DEFAULT_SPOOL_MAX_BYTES, ProcessTreeActivity,
    ProcessTreeStatus, inspect_child_without_reaping, terminate_child_process_group,
};
use tokio::{process::Child, task::LocalSet};

#[path = "connection_env.rs"]
mod connection_env;

#[path = "connection_status.rs"]
mod connection_status;
use connection_status::{format_stderr, process_exited_error};

#[path = "connection_stream.rs"]
mod connection_stream;
#[cfg(test)]
pub(crate) use connection_stream::LINE_BUF_CAP;
use connection_stream::{collect_agent_output, open_output_spool_file, stream_new_agent_messages};

// Re-export spawn-related types from the dedicated module.
#[path = "connection_sandbox_handle.rs"]
mod connection_sandbox_handle;
pub use connection_sandbox_handle::AcpSandboxHandle;

#[path = "connection_spawn.rs"]
mod connection_spawn;
pub use connection_spawn::{AcpConnectionOptions, AcpSandboxRequest, AcpSpawnRequest};

#[path = "connection_fork.rs"]
pub(crate) mod connection_fork;
pub use connection_fork::{CliForkResult, fork_session_via_cli};

use crate::{
    client::{
        SessionEvent, SharedActivity, SharedEvents, SharedToolOutputCompactor, StreamingMetadata,
    },
    error::{AcpError, AcpResult},
    tool_output_compaction::ToolOutputCompactionConfig,
};

const DEFAULT_HEARTBEAT_SECS: u64 = 15;
const HEARTBEAT_INTERVAL_ENV: &str = "CSA_TOOL_HEARTBEAT_SECS";

#[path = "connection_prompt_types.rs"]
mod prompt_types;
pub use prompt_types::{PromptIoOptions, PromptResult};

#[path = "connection_parts.rs"]
mod parts;
pub(crate) use parts::AcpConnectionParts;

pub struct AcpConnection {
    local_set: LocalSet,
    connection: ClientSideConnection,
    child: Rc<RefCell<Option<Child>>>,
    events: SharedEvents,
    last_activity: SharedActivity,
    last_meaningful_activity: SharedActivity,
    tool_output_compactor: SharedToolOutputCompactor,
    stderr_buf: Rc<RefCell<String>>,
    default_working_dir: PathBuf,
    init_timeout: Duration,
    termination_grace_period: Duration,
}

impl AcpConnection {
    /// Environment variables stripped before spawning ACP child processes.
    ///
    /// Backed by [`connection_env::STRIPPED_ENV_VARS`]; see that list for the
    /// per-variable rationale (recursion guards, hook bypass, gemini auth, and
    /// the CSA-owned subtree model-pin reservation, #1741).
    pub(crate) const STRIPPED_ENV_VARS: &[&str] = connection_env::STRIPPED_ENV_VARS;

    /// Internal constructor used by `connection_spawn` after assembling parts.
    pub(crate) fn new_from_parts(parts: AcpConnectionParts) -> Self {
        Self {
            local_set: parts.local_set,
            connection: parts.connection,
            child: Rc::new(RefCell::new(Some(parts.child))),
            events: parts.events,
            last_activity: parts.last_activity,
            last_meaningful_activity: parts.last_meaningful_activity,
            tool_output_compactor: parts.tool_output_compactor,
            stderr_buf: parts.stderr_buf,
            default_working_dir: parts.default_working_dir,
            init_timeout: parts.options.init_timeout,
            termination_grace_period: parts.options.termination_grace_period,
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
                self.drain_stderr_tail().await;
                let stderr = self.stderr();
                Err(AcpError::InitializationFailed(format!(
                    "{err}{}",
                    format_stderr(&stderr)
                )))
            }
            None => {
                let stderr = self.stderr();
                self.kill().await?;
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
                self.drain_stderr_tail().await;
                let stderr = self.stderr();
                Err(AcpError::SessionFailed(format!(
                    "{err}{}",
                    format_stderr(&stderr)
                )))
            }
            None => {
                let stderr = self.stderr();
                self.kill().await?;
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
                self.drain_stderr_tail().await;
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
        *self.tool_output_compactor.borrow_mut() = io
            .tool_output_compaction
            .clone()
            .map(ToolOutputCompactionConfig::into_state);
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
        let mut process_activity = self.child_pid().map(ProcessTreeActivity::new);
        if let Some(activity) = process_activity.as_mut() {
            let _ = activity.observe();
        }

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
                            if process_tree_made_cpu_progress(process_activity.as_mut()) {
                                let now = Instant::now();
                                // CPU progress is a liveness signal, not an initial-response signal.
                                *self.last_activity.borrow_mut() = now;
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
        self.tool_output_compactor.borrow_mut().take();
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
                self.drain_stderr_tail().await;
                let stderr_detail = format_stderr(&self.stderr());
                Err(AcpError::PromptFailed(format!("{err}{stderr_detail}")))
            }
            PromptOutcome::IdleTimeout => {
                self.kill().await?;
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
        self.child.borrow().as_ref().and_then(Child::id)
    }

    pub async fn exit_code(&self) -> AcpResult<Option<i32>> {
        let mut child = self.child.borrow_mut();
        let Some(child) = child.as_mut() else {
            return Ok(None);
        };
        match inspect_child_without_reaping(child)
            .map_err(|err| AcpError::ConnectionFailed(err.to_string()))?
        {
            ChildWaitState::Running => Ok(None),
            ChildWaitState::Exited(status) => Ok(status.code()),
        }
    }

    pub async fn kill(&self) -> AcpResult<()> {
        let mut child = match self.child.borrow_mut().take() {
            Some(child) => child,
            None => return Ok(()),
        };
        if let Err(error) =
            terminate_child_process_group(&mut child, self.termination_grace_period).await
        {
            self.child.borrow_mut().replace(child);
            return Err(AcpError::ConnectionFailed(format!(
                "failed to terminate ACP process group: {error:#}"
            )));
        }
        Ok(())
    }

    pub fn stderr(&self) -> String {
        self.stderr_buf.borrow().clone()
    }
    async fn ensure_process_running(&self) -> AcpResult<()> {
        let state = {
            let mut child = self.child.borrow_mut();
            let Some(child) = child.as_mut() else {
                return Err(AcpError::ConnectionFailed(
                    "ACP process has already been reaped".to_string(),
                ));
            };
            inspect_child_without_reaping(child)
                .map_err(|err| AcpError::ConnectionFailed(err.to_string()))?
        };

        if let ChildWaitState::Exited(status) = state {
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

#[path = "connection_watchdog.rs"]
mod watchdog;
use watchdog::{
    TimeoutPhase, maybe_emit_heartbeat, process_tree_made_cpu_progress, resolve_heartbeat_interval,
    stop_reason_to_string,
};

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "connection_tool_output_compaction_tests.rs"]
mod tool_output_compaction_tests;
