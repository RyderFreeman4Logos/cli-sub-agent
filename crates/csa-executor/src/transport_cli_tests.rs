//! Unit tests for [`super::ClaudeCodeCliTransport`].
//!
//! Extracted into a sibling file (referenced via `#[cfg(test)] #[path =
//! "transport_cli_tests.rs"] mod tests` in the parent) to keep
//! `transport_cli.rs` under the workspace 800-line monolith guard.  Test
//! identity is preserved — the module path is still
//! `transport::transport_cli::tests::*` because `mod tests` is declared in the
//! parent.
use super::*;
use crate::claude_runtime::{ClaudeCodeRuntimeMetadata, ClaudeCodeTransport as CcTransport};
use crate::executor::Executor;
use crate::model_spec::ThinkingBudget;
// `TransportFactory` is re-exported from `csa-executor::transport`
// (transport.rs nests transport_factory.rs via `#[path]` and re-exports
// its public items at the crate root).  Reach for the crate-level
// re-export rather than the private nested-mod path.
use crate::TransportFactory;
use csa_resource::filesystem_sandbox::FilesystemCapability;
use csa_resource::isolation_plan::IsolationPlan;
use csa_resource::sandbox::ResourceCapability;
use std::collections::HashMap as StdHashMap;

fn make_executor() -> Executor {
    Executor::ClaudeCode {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(CcTransport::Cli),
    }
}

fn make_executor_with_model_and_thinking(model: &str, budget: ThinkingBudget) -> Executor {
    Executor::ClaudeCode {
        model_override: Some(model.to_string()),
        thinking_budget: Some(budget),
        runtime_metadata: ClaudeCodeRuntimeMetadata::from_transport(CcTransport::Cli),
    }
}

fn make_test_isolation_plan() -> IsolationPlan {
    IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths: Vec::new(),
        env_overrides: StdHashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: Some(2048),
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
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
    let executor = make_executor();
    let argv = ClaudeCodeCliTransport::build_argv(&executor, "hello", None);
    assert!(
        !argv.iter().any(|a| a == "--resume"),
        "no resume id => --resume must not appear in argv: {argv:?}"
    );
    assert!(argv.iter().any(|a| a == "-p"));
    assert!(argv.iter().any(|a| a == "stream-json"));
}

#[test]
fn build_argv_with_resume_includes_flag_and_id() {
    let executor = make_executor();
    let argv = ClaudeCodeCliTransport::build_argv(&executor, "ping", Some("abc-123"));
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
    let executor = make_executor();
    let argv = ClaudeCodeCliTransport::build_argv(&executor, ".", None);
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
    let stream = r#"{"type":"future_event_kind_xyz","session_id":"s","note":"new in claude 9999"}"#;
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

// ---- Codex P1 review fix: sandbox passthrough (Bug 1) ----

/// `TransportOptions.sandbox` MUST be threaded all the way to the
/// `spawn_tool_sandboxed` call so cgroup/bwrap/landlock isolation plans
/// are honoured in CLI-mode sessions.  Before this fix, `execute_once`
/// called `spawn_tool_with_options` directly, silently dropping the
/// sandbox plan and letting CLI-mode sessions run unisolated.
///
/// We assert via the `sandbox_components` helper that the same isolation
/// plan, tool name, and session id that were placed on
/// `TransportOptions.sandbox` reappear in the four positional arguments
/// `spawn_tool_sandboxed` consumes.  This is the testable seam — the
/// actual spawn would require a child binary on PATH.
#[test]
fn test_sandbox_passthrough() {
    let isolation_plan = make_test_isolation_plan();
    let sandbox_cfg = SandboxTransportConfig {
        isolation_plan: isolation_plan.clone(),
        tool_name: "claude-code".to_string(),
        best_effort: false,
        session_id: "01HSANDBOXPASSTHROUGH00000001".to_string(),
    };

    let none_components = sandbox_components(None);
    assert!(
        none_components.isolation_plan.is_none(),
        "None sandbox config => no isolation plan threaded through"
    );
    assert_eq!(none_components.tool_name, "");
    assert_eq!(none_components.session_id, "");

    let components = sandbox_components(Some(&sandbox_cfg));
    let plan = components
        .isolation_plan
        .expect("sandbox config Some => isolation_plan must be Some");
    // The plan must point at the same plan we placed on the
    // SandboxTransportConfig (matching memory_max_mb is enough — the plan
    // is not Eq, but its memory_max_mb survives the threading).
    assert_eq!(
        plan.memory_max_mb,
        Some(2048),
        "sandbox isolation_plan.memory_max_mb must survive threading from TransportOptions.sandbox"
    );
    assert_eq!(plan.filesystem, FilesystemCapability::Bwrap);
    assert_eq!(components.tool_name, "claude-code");
    assert_eq!(components.session_id, "01HSANDBOXPASSTHROUGH00000001");
}

// ---- Codex P1 review fix: model + thinking-budget flags (Bug 2) ----

/// `build_argv` MUST emit `--model <model>` and `--thinking-budget
/// <tokens>` flag pairs when the [`Executor`] carries them, mirroring the
/// legacy `Executor::append_model_args` path.  Before this fix, both
/// flags were silently dropped because `build_argv` hardcoded only
/// `--dangerously-skip-permissions`/`--output-format`/`--verbose`/`-p` and
/// never consulted the executor — so tier model selection and thinking
/// budgets had no effect on the actual claude CLI invocation.
#[test]
fn test_argv_includes_model_and_thinking() {
    let executor = make_executor_with_model_and_thinking("claude-opus-4-7", ThinkingBudget::High);
    let argv = ClaudeCodeCliTransport::build_argv(&executor, "hi", None);

    let model_index = argv
        .iter()
        .position(|a| a == "--model")
        .expect("--model must appear in argv when Executor has model_override set");
    assert_eq!(
        argv.get(model_index + 1).map(String::as_str),
        Some("claude-opus-4-7"),
        "model name must follow --model directly: {argv:?}"
    );

    let budget_index = argv
        .iter()
        .position(|a| a == "--thinking-budget")
        .expect("--thinking-budget must appear in argv when Executor has thinking_budget set");
    // High budget converts to a numeric token count via
    // `ThinkingBudget::token_count()`; we only care that the value is
    // numeric (matches the legacy CLI flag spelling) — not the exact
    // mapping, which is owned by the model_spec module.
    let budget_value = argv
        .get(budget_index + 1)
        .expect("--thinking-budget must be followed by a value");
    assert!(
        budget_value.parse::<u32>().is_ok(),
        "--thinking-budget value must be numeric (token count); got {budget_value:?}"
    );

    // Backwards-compat sanity: the streaming flags must still be present.
    assert!(argv.iter().any(|a| a == "stream-json"));
    assert!(argv.iter().any(|a| a == "--verbose"));
}

/// When the [`Executor`] has neither model nor thinking budget set, the
/// argv MUST NOT contain `--model` or `--thinking-budget` (so the claude
/// CLI uses its built-in defaults).  Guards against an over-eager fix
/// that always emits the flags with empty / zero values.
#[test]
fn test_argv_omits_model_and_thinking_when_executor_has_none() {
    let executor = make_executor();
    let argv = ClaudeCodeCliTransport::build_argv(&executor, "hi", None);
    assert!(
        !argv.iter().any(|a| a == "--model"),
        "--model must be absent when Executor.model_override is None: {argv:?}"
    );
    assert!(
        !argv.iter().any(|a| a == "--thinking-budget"),
        "--thinking-budget must be absent when Executor.thinking_budget is None: {argv:?}"
    );
}

// ---- Codex P1 review fix: extract Bash command for execute title (Bug 3) ----

/// `tool_use` envelopes for Bash-class tools MUST surface the actual
/// command text in the `ToolCallStarted.title`, sourced from
/// `input.command`.  Before this fix, `envelope_to_event` set
/// `title = envelope.name` (e.g., `"Bash"`), so `extracted_commands`
/// recorded `"Bash"` instead of the real command — defeating the
/// downstream forbidden-command policy that scans the command ring buffer
/// for `git commit --no-verify`-class commands.
#[test]
fn test_envelope_to_event_extracts_bash_command() {
    // ---- Happy path: tool_use with input.command ----
    let envelope_with_command: StreamEnvelope = serde_json::from_str(
        r#"{"type":"tool_use","tool_use_id":"tu-bash-1","name":"Bash","subtype":"execute","input":{"command":"echo hi"}}"#,
    )
    .expect("happy-path tool_use envelope must parse");

    let event = envelope_to_event(&envelope_with_command, "<raw>");
    match event {
        SessionEvent::ToolCallStarted { title, .. } => {
            assert_eq!(
                title, "echo hi",
                "Bash tool_use title MUST be the command text from input.command, not the tool name"
            );
        }
        other => panic!("expected ToolCallStarted, got {other:?}"),
    }

    // ---- Fallback path: tool_use without input ----
    // For non-Bash tools (Edit/Read/etc.), where the input shape is
    // different and there is no command field, the title MUST fall back
    // to the tool name so the existing per-tool dashboards keep working.
    let envelope_no_input: StreamEnvelope = serde_json::from_str(
        r#"{"type":"tool_use","tool_use_id":"tu-edit-1","name":"Edit","subtype":"edit"}"#,
    )
    .expect("input-less tool_use envelope must parse");
    let event = envelope_to_event(&envelope_no_input, "<raw>");
    match event {
        SessionEvent::ToolCallStarted { title, .. } => {
            assert_eq!(
                title, "Edit",
                "input-less tool_use must fall back to tool name (preserves prior behaviour)"
            );
        }
        other => panic!("expected ToolCallStarted, got {other:?}"),
    }

    // ---- Integrated path: parse_stream_json must record the real
    // command in `extracted_commands`, not the tool name ----
    let stream = r#"{"type":"tool_use","session_id":"sess-bash","tool_use_id":"tu-bash-2","name":"Bash","subtype":"execute","input":{"command":"git commit --no-verify -m wip"}}"#;
    let parsed = parse_stream_json(stream);
    assert_eq!(parsed.metadata.extracted_commands.len(), 1);
    assert_eq!(
        parsed.metadata.extracted_commands[0], "git commit --no-verify -m wip",
        "extracted_commands MUST capture the actual command text so the forbidden-command policy can flag --no-verify"
    );
    assert!(
        parsed.metadata.has_execute_tool_calls,
        "execute tool_use must still flip has_execute_tool_calls"
    );
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
