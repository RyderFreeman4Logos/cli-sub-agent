use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const DAEMON_SESSION_ID_ENV: &str = "CSA_DAEMON_SESSION_ID";
const ACP_PAYLOAD_DEBUG_ENV: &str = super::transport_acp_payload_debug::ACP_PAYLOAD_DEBUG_ENV;
static DAEMON_ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn restore_env_var(key: &str, original: Option<String>) {
    // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
    unsafe {
        match original {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}

struct ScopedEnvVar {
    key: &'static str,
    original: Option<String>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }

    fn unset(key: &'static str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by a process-wide mutex.
        unsafe { std::env::remove_var(key) };
        Self { key, original }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        restore_env_var(self.key, self.original.take());
    }
}

// --- classify_join_error tests ---

#[tokio::test]
async fn test_classify_join_error_broken_pipe_message() {
    let handle = tokio::task::spawn(async {
        panic!("failed printing to stderr: Broken pipe (os error 32)")
    });
    let join_err = handle.await.unwrap_err();
    let err = super::classify_join_error(join_err);
    let msg = err.to_string();
    assert!(
        msg.contains("tool process terminated unexpectedly"),
        "broken pipe should get a clean message, got: {msg}"
    );
    assert!(
        msg.contains("broken pipe"),
        "message should mention broken pipe, got: {msg}"
    );
}

#[tokio::test]
async fn test_classify_join_error_generic_panic() {
    let handle = tokio::task::spawn(async { panic!("something else went wrong") });
    let join_err = handle.await.unwrap_err();
    let err = super::classify_join_error(join_err);
    let msg = err.to_string();
    assert!(
        msg.contains("task panicked"),
        "generic panic should say 'task panicked', got: {msg}"
    );
    assert!(
        msg.contains("something else went wrong"),
        "should include panic message, got: {msg}"
    );
}

// --- build_summary tests (moved from transport.rs) ---

#[test]
fn test_build_summary_uses_last_stdout_line_on_success() {
    let stdout = "line1\nfinal line\n";
    let summary = super::build_summary(stdout, "", 0);
    assert_eq!(summary, "final line");
}

#[test]
fn test_build_summary_uses_stdout_on_failure_when_present() {
    let stdout = "details\nreason from stdout\n";
    let summary = super::build_summary(stdout, "stderr message", 2);
    assert_eq!(summary, "reason from stdout");
}

#[test]
fn test_build_summary_falls_back_to_stderr_on_failure() {
    let summary = super::build_summary("\n", "stderr reason\n", 3);
    assert_eq!(summary, "stderr reason");
}

#[test]
fn test_build_summary_ignores_csa_section_markers() {
    let stdout = "Valid summary line\n<!-- CSA:SECTION:summary:END -->\n";
    let summary = super::build_summary(stdout, "", 0);
    assert_eq!(summary, "Valid summary line");
}

#[test]
fn test_build_summary_falls_back_to_exit_code_when_no_output() {
    let summary = super::build_summary("", "   \n", -1);
    assert_eq!(summary, "exit code -1");
}

#[test]
fn test_classify_codex_exec_initial_stall_xhigh_retries_once() {
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: Some(crate::model_spec::ThinkingBudget::Xhigh),
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: "initial_response_timeout: no stdout output for 300s".to_string(),
        summary: "initial_response_timeout: no stdout output for 300s".to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    let classification = super::classify_codex_exec_initial_stall(&executor, &execution, Some(300))
        .expect("stall should classify");
    assert_eq!(classification.effort, "xhigh");
    assert_eq!(classification.timeout_seconds, 300);
    assert!(
        matches!(
            classification.retry_effort,
            Some(crate::model_spec::ThinkingBudget::High)
        ),
        "xhigh stall should request one retry at high"
    );
}

#[test]
fn test_classify_codex_exec_initial_stall_high_does_not_retry() {
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: Some(crate::model_spec::ThinkingBudget::High),
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let execution = ExecutionResult {
        output: String::new(),
        stderr_output: "initial_response_timeout: no stdout output for 300s".to_string(),
        summary: "initial_response_timeout: no stdout output for 300s".to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    let classification = super::classify_codex_exec_initial_stall(&executor, &execution, Some(300))
        .expect("stall should classify");
    assert_eq!(classification.effort, "high");
    assert!(
        classification.retry_effort.is_none(),
        "high stall must not request a retry"
    );
}

#[test]
fn test_classify_codex_exec_initial_stall_ignores_first_byte_before_deadline() {
    let executor = Executor::Codex {
        model_override: None,
        thinking_budget: Some(crate::model_spec::ThinkingBudget::Xhigh),
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let execution = ExecutionResult {
        output: "partial".to_string(),
        stderr_output: String::new(),
        summary: "partial".to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };

    assert!(
        super::classify_codex_exec_initial_stall(&executor, &execution, Some(300)).is_none(),
        "stdout before deadline must not classify as a stall"
    );
}

#[test]
fn test_legacy_transport_consumes_resolved_timeout_without_reapplying_defaults() {
    assert_eq!(
        super::LegacyTransport::consume_resolved_transport_initial_response_timeout_seconds(
            super::ResolvedTimeout(None),
        ),
        None,
        "resolved None must stay disabled on the persistent legacy path"
    );
    assert_eq!(
        super::LegacyTransport::consume_resolved_transport_initial_response_timeout_seconds(
            super::ResolvedTimeout(Some(0)),
        ),
        None,
        "stray Some(0) must not resurrect the codex default on the persistent legacy path"
    );
    assert_eq!(
        super::LegacyTransport::consume_resolved_transport_initial_response_timeout_seconds(
            super::ResolvedTimeout(Some(450)),
        ),
        Some(450),
        "positive resolved values must pass through unchanged on the persistent legacy path"
    );
}

#[tokio::test]
async fn test_legacy_transport_execute_preserves_disabled_resolved_timeout_on_persistent_codex_path()
{
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let script_path = temp.path().join("codex");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
sleep 2
echo "ok persistent"
"#,
    )
    .expect("write fake codex");
    let mut perms = std::fs::metadata(&script_path)
        .expect("metadata")
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).expect("chmod +x");

    let old_path = std::env::var("PATH").unwrap_or_default();
    let env = HashMap::from([(
        "PATH".to_string(),
        format!("{}:{old_path}", temp.path().display()),
    )]);
    let transport = super::LegacyTransport::new(Executor::Codex {
        model_override: None,
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::CodexRuntimeMetadata::from_transport(
            crate::codex_runtime::CodexTransport::Cli,
        ),
    });
    let session = super::build_ephemeral_meta_session(temp.path());

    for initial_response_timeout_seconds in [None, Some(0)] {
        let result = transport
            .execute(
                "persistent legacy timeout disable regression",
                None,
                &session,
                Some(&env),
                super::TransportOptions {
                    stream_mode: StreamMode::BufferOnly,
                    idle_timeout_seconds: 10,
                    acp_crash_max_attempts: 1,
                    initial_response_timeout: super::ResolvedTimeout(
                        initial_response_timeout_seconds,
                    ),
                    liveness_dead_seconds: 15,
                    stdin_write_timeout_seconds: 5,
                    acp_init_timeout_seconds: 5,
                    termination_grace_period_seconds: 1,
                    output_spool: None,
                    output_spool_max_bytes: 64 * 1024,
                    output_spool_keep_rotated: false,
                    setting_sources: None,
                    sandbox: None,
                    thinking_budget: None,
                },
            )
            .await
            .expect("persistent legacy execute should succeed without synthesizing a watchdog");

        assert_eq!(result.execution.exit_code, 0);
        assert!(
            result.execution.output.contains("ok persistent"),
            "persistent legacy path should not arm an initial-response watchdog for {initial_response_timeout_seconds:?}: {:?}",
            result.execution
        );
    }
}

#[test]
fn test_apply_codex_exec_initial_stall_summary_renders_reason_for_result_toml() {
    let classification = super::transport_codex_exec_stall::CodexExecInitialStallClassification {
        effort: "high",
        timeout_seconds: 300,
        retry_effort: None,
    };
    let mut execution = ExecutionResult {
        output: String::new(),
        stderr_output: String::new(),
        summary: String::new(),
        exit_code: 137,
        peak_memory_mb: None,
    };
    super::apply_codex_exec_initial_stall_summary(
        &mut execution,
        &classification,
        true,
        Some("xhigh"),
    );

    let result = csa_session::SessionResult {
        status: "failure".to_string(),
        exit_code: execution.exit_code,
        summary: execution.summary.clone(),
        tool: "codex".to_string(),
        original_tool: None,
        fallback_tool: None,
        fallback_reason: None,
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        events_count: 0,
        artifacts: Vec::new(),
        peak_memory_mb: None,
        manager_fields: Default::default(),
    };
    let toml = toml::to_string_pretty(&result).expect("serialize session result");

    assert!(execution.summary.contains(CODEX_EXEC_INITIAL_STALL_REASON));
    assert!(execution.summary.contains("retry_attempted=true"));
    assert!(toml.contains(CODEX_EXEC_INITIAL_STALL_REASON));
}

#[test]
fn test_build_env_codex_strips_lefthook_bypass_env_only_for_codex() {
    let transport = AcpTransport::new("codex", None);
    let session = build_ephemeral_meta_session(std::path::Path::new("/tmp/test"));
    let extra = HashMap::from([
        ("LEFTHOOK".to_string(), "0".to_string()),
        ("LEFTHOOK_SKIP_PRE_COMMIT".to_string(), "1".to_string()),
        ("SAFE_ENV".to_string(), "ok".to_string()),
    ]);

    let env = transport.build_env(&session, Some(&extra));

    assert!(!env.contains_key("LEFTHOOK"));
    assert!(!env.contains_key("LEFTHOOK_SKIP_PRE_COMMIT"));
    assert_eq!(env.get("SAFE_ENV").map(String::as_str), Some("ok"));
    assert!(
        env.get("CSA_REAL_GIT").is_some_and(|value| !value.is_empty()),
        "ACP env should expose real git for the git guard"
    );
    assert!(
        env.get("PATH").is_some_and(|value| value.contains("guards")),
        "ACP env should prepend the git guard wrapper directory to PATH"
    );
}

#[test]
fn test_build_env_non_codex_preserves_lefthook_bypass_env() {
    let transport = AcpTransport::new("claude-code", None);
    let session = build_ephemeral_meta_session(std::path::Path::new("/tmp/test"));
    let extra = HashMap::from([
        ("LEFTHOOK".to_string(), "0".to_string()),
        ("LEFTHOOK_SKIP_PRE_COMMIT".to_string(), "1".to_string()),
    ]);

    let env = transport.build_env(&session, Some(&extra));

    assert_eq!(env.get("LEFTHOOK").map(String::as_str), Some("0"));
    assert_eq!(
        env.get("LEFTHOOK_SKIP_PRE_COMMIT").map(String::as_str),
        Some("1")
    );
}

#[test]
fn test_daemon_mode_disables_acp_stderr_streaming_when_output_spool_exists() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let original = std::env::var(DAEMON_SESSION_ID_ENV).ok();
    // SAFETY: guarded by DAEMON_ENV_LOCK for this test.
    unsafe { std::env::set_var(DAEMON_SESSION_ID_ENV, "01KTESTSESSION") };

    let spool_path = std::path::Path::new("/tmp/output.log");
    assert!(!super::transport_types::should_stream_acp_stdout_to_stderr(
        StreamMode::TeeToStderr,
        Some(spool_path)
    ));

    restore_env_var(DAEMON_SESSION_ID_ENV, original);
}

#[test]
fn test_foreground_mode_keeps_acp_stderr_streaming_even_with_output_spool() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let original = std::env::var(DAEMON_SESSION_ID_ENV).ok();
    // SAFETY: guarded by DAEMON_ENV_LOCK for this test.
    unsafe { std::env::remove_var(DAEMON_SESSION_ID_ENV) };

    let spool_path = std::path::Path::new("/tmp/output.log");
    assert!(super::transport_types::should_stream_acp_stdout_to_stderr(
        StreamMode::TeeToStderr,
        Some(spool_path)
    ));

    restore_env_var(DAEMON_SESSION_ID_ENV, original);
}

#[test]
fn test_daemon_mode_without_output_spool_keeps_acp_stderr_streaming() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let original = std::env::var(DAEMON_SESSION_ID_ENV).ok();
    // SAFETY: guarded by DAEMON_ENV_LOCK for this test.
    unsafe { std::env::set_var(DAEMON_SESSION_ID_ENV, "01KTESTSESSION") };

    assert!(super::transport_types::should_stream_acp_stdout_to_stderr(
        StreamMode::TeeToStderr,
        None
    ));

    restore_env_var(DAEMON_SESSION_ID_ENV, original);
}

#[test]
fn test_maybe_write_acp_payload_debug_requires_flag_and_session_dir() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let _debug_flag = ScopedEnvVar::set(ACP_PAYLOAD_DEBUG_ENV, "0");

    let path = super::transport_acp_payload_debug::maybe_write_acp_payload_debug(
        super::transport_acp_payload_debug::AcpPayloadDebugRequest {
            env: &HashMap::new(),
            session_dir: None,
            tool_name: "gemini-cli",
            command: "gemini",
            args: &["--acp".to_string()],
            working_dir: std::path::Path::new("/tmp"),
            resume_session_id: None,
            system_prompt: None,
            session_meta: None,
            prompt: "prompt",
        },
    );

    assert!(path.is_none(), "debug artifact should stay disabled by default");
}

#[test]
fn test_maybe_write_acp_payload_debug_writes_json_artifact() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let _debug_flag = ScopedEnvVar::unset(ACP_PAYLOAD_DEBUG_ENV);
    let temp = tempfile::tempdir().expect("tempdir");
    let session_dir = temp.path().join("session");
    let mut env = HashMap::new();
    env.insert(ACP_PAYLOAD_DEBUG_ENV.to_string(), "1".to_string());
    env.insert(
        "CSA_SESSION_DIR".to_string(),
        temp.path()
            .join("spoofed-session")
            .to_string_lossy()
            .into_owned(),
    );

    let mut session_meta = serde_json::Map::new();
    session_meta.insert(
        "review".to_string(),
        serde_json::json!({"mode": "readonly"}),
    );
    session_meta.insert(
        "mcpServers".to_string(),
        serde_json::json!({
            "demo": {
                "command": "demo-mcp",
                "args": [
                    "--api-key",
                    "secret-token",
                    "--header",
                    "Authorization: Bearer abc123",
                    "--safe",
                    "value"
                ],
                "env": {
                    "API_KEY": "secret-token",
                    "OTHER_SECRET": "another-secret"
                }
            }
        }),
    );

    let debug_path = super::transport_acp_payload_debug::maybe_write_acp_payload_debug(
        super::transport_acp_payload_debug::AcpPayloadDebugRequest {
            env: &env,
            session_dir: Some(session_dir.as_path()),
            tool_name: "gemini-cli",
            command: "gemini",
            args: &["--acp".to_string()],
            working_dir: std::path::Path::new("/repo"),
            resume_session_id: Some("provider-session"),
            system_prompt: Some("system prompt"),
            session_meta: Some(&session_meta),
            prompt: "full prompt body",
        },
    )
    .expect("debug artifact");
    assert!(
        debug_path.starts_with(&session_dir),
        "debug artifact should use the trusted session dir, got {}",
        debug_path.display()
    );

    let raw = std::fs::read_to_string(&debug_path).expect("read debug artifact");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("parse debug artifact");
    assert_eq!(json["tool_name"], "gemini-cli");
    assert_eq!(json["command"], "gemini");
    assert_eq!(json["resume_session_id"], "provider-session");
    assert_eq!(json["prompt_chars"], 16);
    assert_eq!(json["prompt"], "full prompt body");
    assert_eq!(json["args"], serde_json::json!(["--acp"]));
    assert_eq!(json["session_meta"]["review"]["mode"], "readonly");
    assert_eq!(
        json["session_meta"]["mcpServers"]["demo"]["env"]["API_KEY"],
        "<redacted>"
    );
    assert_eq!(
        json["session_meta"]["mcpServers"]["demo"]["env"]["OTHER_SECRET"],
        "<redacted>"
    );
    assert_eq!(
        json["session_meta"]["mcpServers"]["demo"]["args"],
        serde_json::json!([
            "--api-key",
            "<redacted>",
            "--header",
            "<redacted>",
            "--safe",
            "value"
        ])
    );
}

// --- 3-phase Gemini fallback chain integration tests ---

/// Phase 1 (OAuth) fails with rate-limit, Phase 2 (API key, same model) succeeds.
/// Verifies: model log shows [inherit, inherit], API key was injected on attempt 2.
#[tokio::test]
async fn test_gemini_3phase_oauth_fails_apikey_same_model_succeeds() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(2);
    // Enable the 3-phase chain by providing API key fallback and OAuth auth mode.
    env.insert("_CSA_API_KEY_FALLBACK".to_string(), "test-key-3phase".to_string());
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test 3phase oauth-fail apikey-succeed",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(None),
        )
        .await
        .expect("execute_in should succeed on attempt 2 (API key, same model)");

    assert_eq!(result.execution.exit_code, 0);
    assert!(
        result.execution.output.contains("ok attempt=2"),
        "expected success on attempt 2, got: {}",
        result.execution.output
    );

    // Model log: both attempts keep original model (inherit)
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec!["inherit".to_string(), "inherit".to_string()],
        "phase 1 and 2 should both use original model"
    );

    // Auth log: attempt 1 = oauth, attempt 2 = api_key
    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec!["oauth".to_string(), "api_key".to_string()],
        "phase 1 should use OAuth, phase 2 should inject API key"
    );
}

/// Phase 1 (OAuth) and Phase 2 (API key, same model) both fail.
/// Phase 3 (API key, flash model) succeeds.
/// Verifies: model log shows [inherit, inherit, flash], API key injected on attempts 2,3.
#[tokio::test]
async fn test_gemini_3phase_all_oauth_and_apikey_same_fail_flash_succeeds() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(3);
    env.insert("_CSA_API_KEY_FALLBACK".to_string(), "test-key-3phase".to_string());
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test 3phase all-fail-until-flash",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(None),
        )
        .await
        .expect("execute_in should succeed on attempt 3 (API key, flash model)");

    assert_eq!(result.execution.exit_code, 0);
    assert!(
        result.execution.output.contains("ok attempt=3"),
        "expected success on attempt 3, got: {}",
        result.execution.output
    );

    // Model log: phase 1,2 keep original, phase 3 switches to flash
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "gemini-3-flash-preview".to_string(),
        ],
        "phase 3 should downgrade to flash model"
    );

    // Auth log: attempt 1 = oauth, attempts 2,3 = api_key
    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec![
            "oauth".to_string(),
            "api_key".to_string(),
            "api_key".to_string(),
        ],
        "phase 2 and 3 should both use API key auth"
    );
}

/// All 3 phases fail with rate-limit. Verifies: returns error, model log shows
/// all 3 attempts, API key was injected on attempts 2 and 3.
#[tokio::test]
async fn test_gemini_3phase_all_fail_returns_last_error() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    env.insert("_CSA_API_KEY_FALLBACK".to_string(), "test-key-3phase".to_string());
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test 3phase all-fail",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(None),
        )
        .await
        .expect("execute_in should return final failed attempt result");

    // Final result is the last (3rd) attempt failure
    assert_ne!(result.execution.exit_code, 0);
    assert!(
        result.execution.stderr_output.contains("QUOTA_EXHAUSTED"),
        "expected QUOTA_EXHAUSTED in stderr, got: {}",
        result.execution.stderr_output
    );

    // Model log: all 3 phases attempted
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec![
            "inherit".to_string(),
            "inherit".to_string(),
            "gemini-3-flash-preview".to_string(),
        ],
        "retry loop should execute all 3 phases before giving up"
    );

    // Auth log: attempt 1 = oauth, attempts 2,3 = api_key
    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec![
            "oauth".to_string(),
            "api_key".to_string(),
            "api_key".to_string(),
        ],
        "API key should be injected on attempts 2 and 3"
    );
}

#[test]
fn test_acp_build_env_injects_parent_session_dir_for_child_sessions() {
    let _env_lock = DAEMON_ENV_LOCK.lock().expect("daemon env lock poisoned");
    let _parent_tool_guard = ScopedEnvVar::set("CSA_TOOL", "parent-tool");
    let transport = AcpTransport::new("claude-code", None);
    let mut session = crate::transport::build_ephemeral_meta_session(std::path::Path::new(
        "/tmp/test",
    ));
    session.meta_session_id = "01HTEST000000000000000000".to_string();
    session.genealogy.parent_session_id = Some("01HPARENT000000000000000000".to_string());

    let env = transport.build_env(&session, Some(&HashMap::from([(
        csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY.to_string(),
        "/tmp/spoofed-parent-session-dir".to_string(),
    )])));

    let parent_session_dir = env
        .get(csa_core::env::CSA_PARENT_SESSION_DIR_ENV_KEY)
        .expect("CSA_PARENT_SESSION_DIR should be present for child sessions");
    assert!(
        parent_session_dir.contains("/sessions/"),
        "CSA_PARENT_SESSION_DIR should be recomputed after merge, got: {parent_session_dir}"
    );
    assert!(
        parent_session_dir.contains("01HPARENT000000000000000000"),
        "CSA_PARENT_SESSION_DIR should include the parent session ID, got: {parent_session_dir}"
    );
}
