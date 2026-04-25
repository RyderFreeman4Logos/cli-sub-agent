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
use csa_acp::{SessionEvent, StreamingMetadata};
use csa_process::{
    SpawnOptions, StreamMode, spawn_tool_with_options, wait_and_capture_with_idle_timeout,
};
use csa_session::state::{MetaSessionState, ToolState};
use serde::Deserialize;
use tokio::process::Command;

use crate::executor::Executor;

use super::{
    ResolvedTimeout, Transport, TransportCapabilities, TransportMode, TransportOptions,
    TransportResult,
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
    /// Layout: `claude <yolo> --output-format stream-json --verbose -p <prompt>
    /// [--resume <session-id>]`.
    ///
    /// `--verbose` is required by the claude CLI as a precondition for
    /// `--output-format=stream-json` together with `-p/--print`; without it
    /// the binary refuses to stream.  `--include-partial-messages` is left
    /// off for Phase 3 — partial chunks would inflate the event stream
    /// without measurably improving downstream consumer behaviour at this
    /// stage.
    pub(crate) fn build_argv(prompt: &str, resume_session_id: Option<&str>) -> Vec<String> {
        let mut args = Vec::with_capacity(8);
        args.push("--dangerously-skip-permissions".to_string());
        args.push("--output-format".to_string());
        args.push("stream-json".to_string());
        args.push("--verbose".to_string());
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
        for arg in Self::build_argv(prompt, resume_session_id) {
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
        let child = spawn_tool_with_options(cmd, None, request.spawn_options).await?;
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
        })
        .await
    }

    #[cfg(test)]
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
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
            let title = envelope
                .name
                .clone()
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
mod tests {
    use super::*;
    use crate::claude_runtime::{ClaudeCodeRuntimeMetadata, ClaudeCodeTransport as CcTransport};
    use crate::executor::Executor;
    // `TransportFactory` is re-exported from `csa-executor::transport`
    // (transport.rs nests transport_factory.rs via `#[path]` and re-exports
    // its public items at the crate root).  Reach for the crate-level
    // re-export rather than the private nested-mod path.
    use crate::TransportFactory;

    fn make_executor() -> Executor {
        Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(CcTransport::Cli),
        }
    }

    // ---- Construction wiring ----

    #[test]
    fn factory_returns_cli_transport_for_claude_code_cli_mode() {
        let executor = make_executor();
        let transport = TransportFactory::create(&executor, None).expect("factory create");

        // The transport must declare Legacy mode (the canonical CLI mode tag),
        // not ACP, so downstream consumers do not double-spawn an ACP adapter.
        assert_eq!(transport.mode(), TransportMode::Legacy);

        // And it must be specifically ClaudeCodeCliTransport, not the generic
        // LegacyTransport — these have different capability matrices and fork
        // semantics.
        let cli_transport = transport
            .as_any()
            .downcast_ref::<ClaudeCodeCliTransport>()
            .expect("factory should return ClaudeCodeCliTransport for claude-code + cli");
        // The downcasted handle should still report Legacy mode (idempotent).
        assert_eq!(cli_transport.mode(), TransportMode::Legacy);
    }

    #[test]
    fn factory_returns_acp_for_claude_code_default() {
        // Default (no explicit transport override) MUST stay on ACP — the
        // PoC change must NOT regress existing claude-code users.
        let executor = Executor::ClaudeCode {
            model_override: None,
            thinking_budget: None,
            runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(CcTransport::Acp),
        };
        let transport = TransportFactory::create(&executor, None).expect("factory create");
        assert_eq!(transport.mode(), TransportMode::Acp);
    }

    #[test]
    fn capabilities_advertise_resume_fork_streaming() {
        let transport = ClaudeCodeCliTransport::new(make_executor());
        let caps = transport.capabilities();
        assert!(caps.session_resume, "claude --resume <id> works");
        assert!(caps.session_fork, "claude --fork-session works");
        assert!(caps.streaming, "claude --output-format stream-json works");
        assert!(caps.typed_events, "stream-json yields typed events");
    }

    // ---- Resume-id propagation ----

    #[test]
    fn build_argv_no_resume_omits_resume_flag() {
        let argv = ClaudeCodeCliTransport::build_argv("hello", None);
        assert!(
            !argv.iter().any(|a| a == "--resume"),
            "no resume id => --resume must not appear in argv: {argv:?}"
        );
        assert!(argv.iter().any(|a| a == "-p"));
        assert!(argv.iter().any(|a| a == "stream-json"));
    }

    #[test]
    fn build_argv_with_resume_includes_flag_and_id() {
        let argv = ClaudeCodeCliTransport::build_argv("ping", Some("abc-123"));
        let resume_index = argv
            .iter()
            .position(|a| a == "--resume")
            .expect("--resume must be present when resume id is given");
        assert_eq!(
            argv.get(resume_index + 1).map(String::as_str),
            Some("abc-123"),
            "session id must follow --resume directly: {argv:?}"
        );
    }

    #[test]
    fn build_argv_includes_streaming_flags() {
        let argv = ClaudeCodeCliTransport::build_argv(".", None);
        assert!(argv.iter().any(|a| a == "--output-format"));
        assert!(argv.iter().any(|a| a == "stream-json"));
        assert!(
            argv.iter().any(|a| a == "--verbose"),
            "stream-json requires --verbose per claude CLI; argv={argv:?}"
        );
    }

    // ---- stream-json parsing ----

    #[test]
    fn parse_stream_json_happy_path_emits_events() {
        let stream = concat!(
            r#"{"type":"system","session_id":"sess-1","subtype":"init"}"#,
            "\n",
            r#"{"type":"assistant","session_id":"sess-1","message":{"content":[{"type":"text","text":"Hello"}]}}"#,
            "\n",
            r#"{"type":"tool_use","session_id":"sess-1","tool_use_id":"tu-1","name":"Bash","subtype":"execute"}"#,
            "\n",
            r#"{"type":"tool_result","session_id":"sess-1","tool_use_id":"tu-1","status":"success"}"#,
            "\n",
            r#"{"type":"result","session_id":"sess-1","subtype":"final"}"#,
            "\n",
        );
        let parsed = parse_stream_json(stream);
        assert_eq!(parsed.provider_session_id.as_deref(), Some("sess-1"));
        assert_eq!(parsed.events.len(), 5, "5 envelopes => 5 events");

        // The assistant envelope must lift to AgentMessage with the inner text.
        let msg_count = parsed
            .events
            .iter()
            .filter(|e| matches!(e, SessionEvent::AgentMessage(text) if text == "Hello"))
            .count();
        assert_eq!(msg_count, 1, "one AgentMessage with text 'Hello'");

        // The tool_use envelope must lift to ToolCallStarted, with the
        // execute subtype reflected so downstream consumers can extract the
        // command.
        let exec_started = parsed.events.iter().any(|e| {
            matches!(
                e,
                SessionEvent::ToolCallStarted { kind, .. } if kind.eq_ignore_ascii_case("execute")
            )
        });
        assert!(exec_started, "execute tool call must be detected");

        assert!(parsed.metadata.has_tool_calls);
        assert!(parsed.metadata.has_execute_tool_calls);
        assert_eq!(parsed.metadata.total_events_count, 5);
        assert_eq!(parsed.metadata.message_text, "Hello");
    }

    #[test]
    fn parse_stream_json_malformed_line_is_skipped_not_panicked() {
        let stream = concat!(
            r#"{"type":"assistant","session_id":"sess-9","message":{"content":[{"type":"text","text":"a"}]}}"#,
            "\n",
            "this is not json at all\n",
            r#"{not even valid json"#,
            "\n",
            r#"{"type":"assistant","session_id":"sess-9","message":{"content":[{"type":"text","text":"b"}]}}"#,
            "\n",
        );
        let parsed = parse_stream_json(stream);

        // The two well-formed lines must produce events; the two garbage
        // lines must be skipped without panicking.
        assert_eq!(
            parsed.events.len(),
            2,
            "only well-formed lines yield events"
        );
        assert_eq!(parsed.provider_session_id.as_deref(), Some("sess-9"));
        let messages: Vec<&str> = parsed
            .events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::AgentMessage(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(messages, vec!["a", "b"]);
    }

    #[test]
    fn parse_stream_json_empty_buffer_returns_empty_result() {
        let parsed = parse_stream_json("");
        assert!(parsed.events.is_empty());
        assert!(parsed.provider_session_id.is_none());
        assert_eq!(parsed.metadata.total_events_count, 0);
    }

    #[test]
    fn parse_stream_json_unknown_event_type_falls_through_to_other() {
        let stream =
            r#"{"type":"future_event_kind_xyz","session_id":"s","note":"new in claude 9999"}"#;
        let parsed = parse_stream_json(stream);
        assert_eq!(parsed.events.len(), 1);
        assert!(matches!(&parsed.events[0], SessionEvent::Other(_)));
    }

    #[test]
    fn parse_stream_json_session_id_camel_case_accepted() {
        let stream = r#"{"type":"system","sessionId":"camel-id"}"#;
        let parsed = parse_stream_json(stream);
        assert_eq!(parsed.provider_session_id.as_deref(), Some("camel-id"));
    }

    // ---- Optional integration test (gated; CI-friendly) ----

    /// Optional: actually spawn `claude` and verify the CLI transport produces
    /// a non-error result.  Skipped when the binary isn't installed (so this
    /// test never causes false-red on CI without claude).
    #[ignore = "requires claude CLI installed; run manually with `cargo test -p csa-executor -- --ignored claude_cli_smoke`"]
    #[tokio::test]
    async fn claude_cli_smoke() {
        if which::which("claude").is_err() {
            eprintln!("claude binary not on PATH; skipping smoke");
            return;
        }
        let executor = make_executor();
        let transport = ClaudeCodeCliTransport::new(executor);
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = transport
            .execute_in(
                "say 'hello from cli transport'",
                tmp.path(),
                None,
                StreamMode::BufferOnly,
                30,
                ResolvedTimeout::of(60),
            )
            .await;
        // We don't assert on content (depends on user auth and network); we
        // only assert the call did not bubble an unrelated error.
        assert!(
            result.is_ok(),
            "smoke: transport.execute_in failed: {result:?}"
        );
    }
}
