//! Claude Code CLI transport (Phase 3 PoC for #1103 / #760).
//!
//! Implements the [`Transport`] trait by spawning the native `claude` CLI
//! binary directly, bypassing the ACP adapter (`claude-code-acp`).  This is
//! the strongest-CLI-support tool of the four CSA wraps, and the natural
//! starting point for the multi-phase migration toward CLI-only transports.
//!
//! ## Capabilities (vs ACP)
//!
//! - `streaming = true` (best-effort): claude CLI supports
//!   `--output-format stream-json` which emits one JSON object per line for
//!   live tool calls / agent messages / plan updates.  We parse that stream
//!   and synthesize [`SessionEvent`] values matching the ACP shape so
//!   downstream consumers (`csa session result`, `csa review`) see the same
//!   event vocabulary.
//! - `session_resume = true`: `claude --resume <session-id>`.
//! - `session_fork = true`: `claude --resume <id> --fork-session` produces a
//!   provider-level fork.  The transport itself doesn't drive the fork (that
//!   is owned by [`crate::transport_fork`] / `csa-acp::connection_fork`); it
//!   simply declares the capability as supported.
//! - `typed_events = true` (best-effort): same caveat as `streaming`.
//!
//! ## Phase 3 PoC limitations (Phase 5 TODO)
//!
//! - **No mid-session interrupt monitoring beyond idle-timeout / liveness**:
//!   ACP keeps a JSON-RPC connection alive and can poll a 200 ms event-loop;
//!   the CLI path can only watch stdout silence and process liveness.
//! - **No persistent connection across multiple prompts**: every prompt is a
//!   fresh `claude --print` invocation with `--resume <id>`.  This means
//!   higher cold-start latency for multi-turn sessions; we have not measured
//!   the cost yet (TODO benchmarks).
//! - **No mid-session ACP crash retry**: ACP-specific OOM/auth/init failure
//!   classification (`transport_acp_crash_retry.rs`) is not applied.  The CLI
//!   transport surfaces non-zero exit codes directly.
//! - **`SessionEvent` lives in `csa-acp`**: when Phase 5 feature-gates ACP
//!   under `acp-transport`, this type must move to `csa-executor` or
//!   `csa-core`.  The CLI transport already depends on it via the public
//!   [`TransportResult.events`] field, so the move is structural, not
//!   semantic.
//! - **Sandbox integration**: spawn-time sandboxing (cgroup / bwrap /
//!   landlock) is wired through `spawn_tool_sandboxed` the same way
//!   [`super::LegacyTransport`] does it; this transport delegates to the
//!   shared `csa-process` helper rather than re-implementing.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use anyhow::Result;
use async_trait::async_trait;
use csa_core::transport_events::{SessionEvent, StreamingMetadata};
use csa_process::{
    SpawnOptions, StreamMode, spawn_tool_sandboxed, wait_and_capture_with_idle_timeout,
};
use csa_resource::isolation_plan::IsolationPlan;
use csa_session::state::{MetaSessionState, ToolState};
use serde::Deserialize;
use tokio::process::Command;

use crate::executor::Executor;

use super::{
    ResolvedTimeout, SandboxTransportConfig, Transport, TransportCapabilities, TransportMode,
    TransportOptions, TransportResult,
};

/// Native `claude` CLI transport — the Phase 3 PoC alternative to
/// [`super::AcpTransport`] for the `claude-code` tool.
#[derive(Debug, Clone)]
pub struct ClaudeCodeCliTransport {
    executor: Executor,
}

impl ClaudeCodeCliTransport {
    /// Construct from an [`Executor`].  Caller must guarantee
    /// `executor.tool_name() == "claude-code"`; otherwise the transport will
    /// still spawn whichever binary the executor names, but the resulting
    /// argv is undefined (Phase 4 will widen this for codex etc.).
    #[must_use]
    pub fn new(executor: Executor) -> Self {
        Self { executor }
    }

    /// Build the argv for a prompt invocation.
    ///
    /// Layout: `claude <yolo> --output-format stream-json --verbose
    /// [--model <model>] [--effort <level>] -p <prompt>
    /// [--resume <session-id>]`.
    ///
    /// `--verbose` is required by the claude CLI as a precondition for
    /// `--output-format=stream-json` together with `-p/--print`; without it
    /// the binary refuses to stream.  `--include-partial-messages` is left
    /// off for Phase 3 — partial chunks would inflate the event stream
    /// without measurably improving downstream consumer behaviour at this
    /// stage.
    ///
    /// `--model` and `--effort` are sourced from the [`Executor`] to mirror
    /// what the legacy CLI path emits via
    /// [`crate::executor::Executor::append_model_args`]; without them, tier
    /// model selection silently degrades to the claude default and thinking
    /// budgets configured in CSA tiers are dropped entirely.
    /// claude-code 2.x replaced the older `--thinking-budget <tokens>` flag
    /// with `--effort <level>`; emitting the old flag fails with `unknown
    /// option` (#1124).
    pub(crate) fn build_argv(
        executor: &Executor,
        prompt: &str,
        resume_session_id: Option<&str>,
    ) -> Vec<String> {
        let mut args = Vec::with_capacity(12);
        args.push("--dangerously-skip-permissions".to_string());
        args.push("--output-format".to_string());
        args.push("stream-json".to_string());
        args.push("--verbose".to_string());
        for arg in claude_model_args(executor) {
            args.push(arg);
        }
        args.push("-p".to_string());
        args.push(prompt.to_string());
        if let Some(id) = resume_session_id {
            args.push("--resume".to_string());
            args.push(id.to_string());
        }
        args
    }

    fn build_command(
        &self,
        prompt: &str,
        work_dir: &Path,
        resume_session_id: Option<&str>,
        extra_env: Option<&HashMap<String, String>>,
    ) -> Command {
        let mut cmd = Command::new(self.executor.executable_name());
        cmd.current_dir(work_dir);
        // Mirror the env-stripping rule used by both the legacy CLI path and
        // the ACP path so a CSA-spawned `claude` does not trip its own
        // recursion guard or inherit lefthook bypass markers.
        for var in CLI_TRANSPORT_STRIPPED_ENV_VARS {
            cmd.env_remove(var);
        }
        if let Some(env) = extra_env {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }
        for arg in Self::build_argv(&self.executor, prompt, resume_session_id) {
            cmd.arg(arg);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    async fn execute_once(&self, request: ExecuteOnceRequest<'_>) -> Result<TransportResult> {
        let cmd = self.build_command(
            request.prompt,
            request.work_dir,
            request.resume_session_id,
            request.extra_env,
        );
        // Mirror `LegacyTransport::execute_single_attempt` (transport.rs L313):
        // route every spawn through `spawn_tool_sandboxed` so cgroup/bwrap/
        // landlock isolation from `TransportOptions.sandbox` is honoured even
        // when this transport is selected.  Calling `spawn_tool_with_options`
        // directly silently dropped the sandbox plan and let CLI-mode sessions
        // run unisolated even when callers had configured an isolation plan.
        let SandboxComponents {
            isolation_plan,
            tool_name,
            session_id,
        } = sandbox_components(request.sandbox);
        let (child, _sandbox_handle) = spawn_tool_sandboxed(
            cmd,
            None,
            request.spawn_options,
            isolation_plan,
            tool_name,
            session_id,
        )
        .await?;
        let execution = wait_and_capture_with_idle_timeout(
            child,
            request.stream_mode,
            std::time::Duration::from_secs(request.idle_timeout_seconds),
            std::time::Duration::from_secs(csa_process::DEFAULT_LIVENESS_DEAD_SECS),
            std::time::Duration::from_secs(csa_process::DEFAULT_TERMINATION_GRACE_PERIOD_SECS),
            request.output_spool,
            request.spawn_options,
            request
                .initial_response_timeout
                .as_option()
                .filter(|&seconds| seconds > 0)
                .map(std::time::Duration::from_secs),
        )
        .await?;

        let parsed = parse_stream_json(&execution.output);
        Ok(TransportResult {
            execution,
            provider_session_id: parsed.provider_session_id,
            events: parsed.events,
            metadata: parsed.metadata,
        })
    }
}

/// Single-attempt execution request for [`ClaudeCodeCliTransport::execute_once`].
///
/// Grouping the parameters into a struct keeps the call sites readable (the
/// `Transport` trait has both `execute` and `execute_in` entry points) and
/// avoids a `clippy::too_many_arguments` workaround.
struct ExecuteOnceRequest<'a> {
    prompt: &'a str,
    work_dir: &'a Path,
    resume_session_id: Option<&'a str>,
    extra_env: Option<&'a HashMap<String, String>>,
    stream_mode: StreamMode,
    idle_timeout_seconds: u64,
    initial_response_timeout: ResolvedTimeout,
    spawn_options: SpawnOptions,
    output_spool: Option<&'a Path>,
    /// Sandbox configuration propagated from [`TransportOptions::sandbox`].
    /// `None` matches the unsandboxed `execute_in` (testing) path; `Some`
    /// matches the production `execute` path when a sandbox is configured.
    sandbox: Option<&'a SandboxTransportConfig>,
}

#[async_trait]
impl Transport for ClaudeCodeCliTransport {
    fn mode(&self) -> TransportMode {
        TransportMode::Legacy
    }

    fn capabilities(&self) -> TransportCapabilities {
        // Phase 3 PoC: best-effort streaming via `--output-format stream-json`.
        // Capability matrix records the *aspirational* shape so consumers can
        // route the right way; if stream-json parsing fails on a future claude
        // release, downstream code degrades to the embedded ExecutionResult
        // text without breaking the [`TransportResult`] invariant.
        TransportCapabilities {
            streaming: true,
            session_resume: true,
            session_fork: true,
            typed_events: true,
        }
    }

    async fn execute(
        &self,
        prompt: &str,
        tool_state: Option<&ToolState>,
        session: &MetaSessionState,
        extra_env: Option<&HashMap<String, String>>,
        options: TransportOptions<'_>,
    ) -> Result<TransportResult> {
        let work_dir = Path::new(&session.project_path).to_path_buf();
        let resume_session_id = tool_state.and_then(|s| s.provider_session_id.as_deref());

        let spawn_options = SpawnOptions {
            stdin_write_timeout: std::time::Duration::from_secs(
                options.stdin_write_timeout_seconds,
            ),
            keep_stdin_open: false,
            spool_max_bytes: options.output_spool_max_bytes,
            keep_rotated_spool: options.output_spool_keep_rotated,
        };

        self.execute_once(ExecuteOnceRequest {
            prompt,
            work_dir: &work_dir,
            resume_session_id,
            extra_env,
            stream_mode: options.stream_mode,
            idle_timeout_seconds: options.idle_timeout_seconds,
            initial_response_timeout: options.initial_response_timeout,
            spawn_options,
            output_spool: options.output_spool,
            sandbox: options.sandbox,
        })
        .await
    }

    async fn execute_in(
        &self,
        prompt: &str,
        work_dir: &Path,
        extra_env: Option<&HashMap<String, String>>,
        stream_mode: StreamMode,
        idle_timeout_seconds: u64,
        initial_response_timeout: ResolvedTimeout,
    ) -> Result<TransportResult> {
        self.execute_once(ExecuteOnceRequest {
            prompt,
            work_dir,
            resume_session_id: None,
            extra_env,
            stream_mode,
            idle_timeout_seconds,
            initial_response_timeout,
            spawn_options: SpawnOptions::default(),
            output_spool: None,
            sandbox: None,
        })
        .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Decomposition of [`SandboxTransportConfig`] into the four positional
/// arguments that [`csa_process::spawn_tool_sandboxed`] consumes.
///
/// Extracted as a named helper so unit tests can assert that
/// `TransportOptions.sandbox` is threaded all the way to the spawn call —
/// without actually spawning a child process.
struct SandboxComponents<'a> {
    isolation_plan: Option<&'a IsolationPlan>,
    tool_name: &'a str,
    session_id: &'a str,
}

/// Decompose `Option<&SandboxTransportConfig>` into the positional arguments
/// expected by `spawn_tool_sandboxed`.
///
/// Mirrors the LegacyTransport contract (`transport.rs` L292-L299): when the
/// caller passes `Some(SandboxTransportConfig)`, the isolation plan is honoured
/// and the tool/session identifiers are propagated for cgroup scope naming;
/// when the caller passes `None`, the spawn proceeds unsandboxed and the
/// downstream call ends up equivalent to `spawn_tool_with_options`.
fn sandbox_components(sandbox: Option<&SandboxTransportConfig>) -> SandboxComponents<'_> {
    match sandbox {
        Some(s) => SandboxComponents {
            isolation_plan: Some(&s.isolation_plan),
            tool_name: s.tool_name.as_str(),
            session_id: s.session_id.as_str(),
        },
        None => SandboxComponents {
            isolation_plan: None,
            tool_name: "",
            session_id: "",
        },
    }
}

/// Emit the model / effort flag pairs for a `claude` CLI invocation.
///
/// Mirrors [`crate::executor::Executor::append_model_args`] for the
/// [`Executor::ClaudeCode`] arm: any non-`ClaudeCode` executor handed in here
/// returns an empty list (Phase 3 only routes `claude-code` through this
/// transport, but defensive emptiness keeps Phase 4 widening safe).
///
/// claude-code 2.x exposes thinking control via `--effort <level>`
/// (low/medium/high/xhigh/max); the legacy `--thinking-budget <tokens>` flag
/// was removed and any emission of it makes the binary exit with `unknown
/// option` (#1124). `DefaultBudget` deliberately omits the flag so the tool
/// applies its built-in default.
fn claude_model_args(executor: &Executor) -> Vec<String> {
    let mut out = Vec::with_capacity(4);
    if let Executor::ClaudeCode {
        model_override,
        thinking_budget,
        ..
    } = executor
    {
        if let Some(model) = model_override {
            out.push("--model".to_string());
            out.push(model.clone());
        }
        if let Some(budget) = thinking_budget
            && let Some(level) = budget.claude_effort()
        {
            out.push("--effort".to_string());
            out.push(level.to_string());
        }
    }
    out
}

/// Environment variable allowlist for the CLI transport.
///
/// Identical to `Executor::STRIPPED_ENV_VARS`; replicated here because that
/// constant is `pub(crate)` to `csa-executor::executor` and we don't want a
/// cross-module dependency for two strings.  Out of scope for Phase 3 but
/// flagged for Phase 5: consolidate into a single workspace-level list.
const CLI_TRANSPORT_STRIPPED_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "LEFTHOOK",
    "LEFTHOOK_SKIP",
];

/// Result of parsing a `claude --output-format stream-json` byte stream.
#[derive(Debug, Default)]
struct StreamParseResult {
    provider_session_id: Option<String>,
    events: Vec<SessionEvent>,
    metadata: StreamingMetadata,
}

/// Minimal stream-json envelope used for Phase 3 PoC parsing.
///
/// Claude's stream-json shape (best-effort, observed live; not formally
/// specified) emits objects of the form
/// `{"type": "...", "session_id": "...", "message": {...}, ...}` per line.
/// We capture the `type` discriminator and use field-presence heuristics to
/// map each event to a [`SessionEvent`].  Unknown or partially-populated
/// envelopes degrade to [`SessionEvent::Other`] with the original raw line so
/// no information is lost.
#[derive(Debug, Deserialize)]
struct StreamEnvelope {
    #[serde(rename = "type")]
    event_type: Option<String>,
    session_id: Option<String>,
    #[serde(rename = "sessionId")]
    session_id_camel: Option<String>,
    subtype: Option<String>,
    message: Option<serde_json::Value>,
    text: Option<String>,
    plan: Option<serde_json::Value>,
    tool: Option<String>,
    tool_use_id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    /// Tool-call payload for `tool_use` envelopes.
    ///
    /// Claude's stream-json emits Bash-class tool calls as
    /// `{"type":"tool_use","name":"Bash","input":{"command":"git ..."}}`.
    /// Without capturing this, the title for `tool_use` events degrades to
    /// the bare tool name (e.g., `"Bash"`) and `extracted_commands` records
    /// the tool name instead of the actual command text — defeating the
    /// downstream forbidden-command policy that scans the command ring buffer
    /// for `git commit --no-verify`-class commands.
    input: Option<serde_json::Value>,
}

/// Parse a stream-json output buffer into a [`StreamParseResult`].
///
/// Every line is parsed independently — a malformed line is logged via
/// `tracing::debug!` and skipped, never panicked on.  Returns the final
/// `provider_session_id` observed (claude emits one on the initial `system`
/// envelope, occasionally repeated on later envelopes).
fn parse_stream_json(buffer: &str) -> StreamParseResult {
    let mut result = StreamParseResult::default();

    for raw_line in buffer.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if !(line.starts_with('{') && line.ends_with('}')) {
            // Not a JSON envelope (e.g., interleaved log noise from `--verbose`
            // before the JSON stream starts).  Drop quietly; the raw text is
            // still accessible via `ExecutionResult.output`.
            continue;
        }
        let envelope: StreamEnvelope = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(error) => {
                tracing::debug!(
                    %error,
                    line_len = line.len(),
                    "stream-json parse skipped malformed line",
                );
                continue;
            }
        };

        if let Some(session_id) = envelope
            .session_id
            .clone()
            .or_else(|| envelope.session_id_camel.clone())
            && !session_id.is_empty()
        {
            result.provider_session_id = Some(session_id);
        }

        let event = envelope_to_event(&envelope, line);
        result.metadata.total_events_count += 1;
        match &event {
            SessionEvent::ToolCallStarted { kind, title, .. } => {
                result.metadata.has_tool_calls = true;
                if kind.eq_ignore_ascii_case("execute") {
                    result.metadata.has_execute_tool_calls = true;
                    result.metadata.extracted_commands.push(title.clone());
                }
            }
            SessionEvent::PlanUpdate(_) => {
                result.metadata.has_plan_updates = true;
            }
            SessionEvent::AgentMessage(text) => {
                result.metadata.message_text.push_str(text);
            }
            SessionEvent::AgentThought(text) => {
                result.metadata.thought_text.push_str(text);
            }
            _ => {}
        }
        result.events.push(event);
    }

    if result.metadata.message_text.is_empty() && !result.metadata.thought_text.is_empty() {
        result.metadata.has_thought_fallback = true;
    }

    result
}

fn envelope_to_event(envelope: &StreamEnvelope, raw_line: &str) -> SessionEvent {
    let event_type = envelope.event_type.as_deref().unwrap_or("");

    match event_type {
        "assistant" | "assistant_message" => {
            let text = extract_message_text(&envelope.message)
                .or_else(|| envelope.text.clone())
                .unwrap_or_default();
            SessionEvent::AgentMessage(text)
        }
        "thinking" | "agent_thought" => {
            let text = extract_message_text(&envelope.message)
                .or_else(|| envelope.text.clone())
                .unwrap_or_default();
            SessionEvent::AgentThought(text)
        }
        "tool_use" | "tool_call" => {
            let id = envelope
                .tool_use_id
                .clone()
                .unwrap_or_else(|| envelope.name.clone().unwrap_or_default());
            // Prefer the actual command string from `input.command` for
            // Bash-class tool calls so downstream
            // `metadata.extracted_commands` captures the real command text
            // (e.g., `git commit --no-verify`) rather than the tool name
            // ("Bash").  Without this, the post-run forbidden-command policy
            // sees "Bash" and lets unsafe commands through.
            //
            // Falls back to `name`/`tool` when `input.command` is absent —
            // matches the previous behaviour for non-Bash tool calls
            // (Edit/Read/etc.) whose payload schema differs.
            let title = extract_tool_input_command(envelope.input.as_ref())
                .or_else(|| envelope.name.clone())
                .or_else(|| envelope.tool.clone())
                .unwrap_or_default();
            let kind = envelope.subtype.clone().unwrap_or_else(|| "tool".into());
            SessionEvent::ToolCallStarted { id, title, kind }
        }
        "tool_result" | "tool_call_result" => {
            let id = envelope.tool_use_id.clone().unwrap_or_default();
            let status = envelope
                .status
                .clone()
                .unwrap_or_else(|| "completed".into());
            SessionEvent::ToolCallCompleted { id, status }
        }
        "plan" | "plan_update" => {
            let text = envelope
                .plan
                .as_ref()
                .map(|v| v.to_string())
                .or_else(|| envelope.text.clone())
                .unwrap_or_default();
            SessionEvent::PlanUpdate(text)
        }
        // `system` envelopes carry the session id and config; they have no
        // direct ACP equivalent so we surface them as Other for transparency.
        // Same for `result`/`final` envelopes that close the stream.
        _ => SessionEvent::Other(raw_line.to_string()),
    }
}

/// Extract the command string from a tool_use `input` payload, when present.
///
/// Claude's stream-json represents Bash-class tool calls as
/// `{"input": {"command": "..."}}`.  This helper returns the inner
/// `command` value when it is a non-empty string, and `None` otherwise (e.g.,
/// non-Bash tools like `Edit` whose `input` is `{"file_path": ..., ...}`).
///
/// Trimming is applied to defeat trailing whitespace that would otherwise
/// confuse the downstream `command_looks_like_no_verify_commit` heuristic in
/// `csa-acp::client`.
fn extract_tool_input_command(input: Option<&serde_json::Value>) -> Option<String> {
    let value = input?;
    let command = value.get("command")?.as_str()?.trim();
    if command.is_empty() {
        None
    } else {
        Some(command.to_string())
    }
}

/// Extract the textual content from a claude `message` payload.
///
/// Claude emits `message.content` either as a plain string or as an array of
/// content blocks `[{"type": "text", "text": "..."}, ...]`.  We concatenate
/// all text blocks and ignore non-text blocks — they appear separately as
/// `tool_use` envelopes anyway.
fn extract_message_text(message: &Option<serde_json::Value>) -> Option<String> {
    let value = message.as_ref()?;
    if let Some(content) = value.get("content") {
        if let Some(s) = content.as_str() {
            return Some(s.to_string());
        }
        if let Some(arr) = content.as_array() {
            let mut buf = String::new();
            for block in arr {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("text")
                    && let Some(text) = block.get("text").and_then(serde_json::Value::as_str)
                {
                    buf.push_str(text);
                }
            }
            if !buf.is_empty() {
                return Some(buf);
            }
        }
    }
    if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
        return Some(text.to_string());
    }
    None
}

#[cfg(test)]
#[path = "transport_cli_tests.rs"]
mod tests;
