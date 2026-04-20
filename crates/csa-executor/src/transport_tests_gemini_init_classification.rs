static GEMINI_INIT_ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

struct GeminiInitScopedEnvVar {
    key: &'static str,
    original: Option<String>,
}

impl GeminiInitScopedEnvVar {
    fn set(key: &'static str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        // SAFETY: test-scoped env mutation guarded by GEMINI_INIT_ENV_LOCK.
        unsafe { std::env::set_var(key, value) };
        Self { key, original }
    }
}

impl Drop for GeminiInitScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: test-scoped env mutation guarded by GEMINI_INIT_ENV_LOCK.
        unsafe {
            match self.original.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
fn test_classify_gemini_acp_init_failure_detects_oom() {
    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "sandboxed ACP: ACP initialization failed: killed by signal 9 (SIGKILL)",
        &env,
    );
    assert_eq!(classification.code, "gemini_acp_init_oom");
}

#[test]
fn test_classify_gemini_acp_init_failure_detects_auth_env() {
    let _env_lock = GEMINI_INIT_ENV_LOCK
        .lock()
        .expect("gemini init env lock poisoned");
    let _api_key = GeminiInitScopedEnvVar::set(csa_core::gemini::API_KEY_ENV, "test-key");
    let _base_url =
        GeminiInitScopedEnvVar::set(csa_core::gemini::BASE_URL_ENV, "http://127.0.0.1:8317");

    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "ACP initialization failed: authentication failed: missing credentials",
        &env,
    );

    assert_eq!(classification.code, "gemini_acp_init_auth_env");
    assert_eq!(
        classification.missing_env_vars,
        vec![
            csa_core::gemini::API_KEY_ENV,
            csa_core::gemini::BASE_URL_ENV
        ]
    );
}

#[test]
fn test_classify_gemini_acp_init_failure_detects_mcp_extension() {
    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "ACP initialization failed: spawn /home/obj/.gemini/extensions/gemini-cli-security/mcp-server ENOENT",
        &env,
    );
    assert_eq!(classification.code, "gemini_acp_init_mcp_extension");
}

#[test]
fn test_classify_gemini_acp_init_failure_defaults_to_handshake_timeout() {
    let env = HashMap::new();
    let classification = classify_gemini_acp_init_failure(
        "ACP initialization failed: Internal error: \"server shut down unexpectedly\"",
        &env,
    );
    assert_eq!(classification.code, "gemini_acp_init_handshake_timeout");
}

#[test]
fn test_gemini_acp_initial_response_timeout_resolver_is_gemini_only() {
    assert_eq!(
        gemini_acp_initial_response_timeout_seconds("gemini-cli", super::ResolvedTimeout(None)),
        None
    );
    assert_eq!(
        gemini_acp_initial_response_timeout_seconds(
            "gemini-cli",
            super::ResolvedTimeout(Some(0)),
        ),
        None
    );
    assert_eq!(
        gemini_acp_initial_response_timeout_seconds(
            "gemini-cli",
            super::ResolvedTimeout(Some(45)),
        ),
        Some(45)
    );
    assert_eq!(
        gemini_acp_initial_response_timeout_seconds(
            "claude-code",
            super::ResolvedTimeout(None),
        ),
        None
    );
    assert_eq!(
        gemini_acp_initial_response_timeout_seconds("codex", super::ResolvedTimeout(None)),
        None
    );
}

#[test]
fn test_classify_gemini_acp_initial_stall_detects_first_response_timeout() {
    let execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: "initial response timeout: no ACP events/stderr for 180s; process killed"
            .to_string(),
        summary: "initial response timeout: no ACP events/stderr for 180s; process killed"
            .to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    let classification = classify_gemini_acp_initial_stall(&execution, Some(180))
        .expect("gemini ACP initial timeout should classify");
    assert_eq!(classification.code, "gemini_acp_initial_stall");
    assert_eq!(classification.timeout_seconds, 180);
}

#[test]
fn test_apply_gemini_acp_initial_stall_summary_rewrites_summary() {
    let mut execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: "initial response timeout: no ACP events/stderr for 180s; process killed"
            .to_string(),
        summary: "initial response timeout: no ACP events/stderr for 180s; process killed"
            .to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };
    let classification = classify_gemini_acp_initial_stall(&execution, Some(180))
        .expect("gemini ACP initial timeout should classify");

    apply_gemini_acp_initial_stall_summary(&mut execution, &classification);

    assert_eq!(
        execution.summary,
        "gemini_acp_initial_stall: no ACP events/stderr within 180s"
    );
    assert!(
        execution
            .stderr_output
            .contains("gemini_acp_initial_stall: no ACP events/stderr within 180s"),
        "expected stable classifier in stderr, got: {}",
        execution.stderr_output
    );
}

#[test]
fn test_classify_gemini_legacy_initial_stall_detects_first_response_timeout() {
    let executor = Executor::GeminiCli {
        model_override: Some("gemini-2.5-pro".to_string()),
        thinking_budget: None,
    };
    let execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        summary: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    let classification = classify_gemini_legacy_initial_stall(&executor, &execution, Some(120))
        .expect("gemini legacy initial timeout should classify");
    assert_eq!(classification.code, "gemini_legacy_initial_stall");
    assert_eq!(classification.timeout_seconds, 120);
}

#[test]
fn test_classify_gemini_legacy_initial_stall_is_gemini_only() {
    let executor = Executor::Codex {
        model_override: Some("gpt-5-codex".to_string()),
        thinking_budget: None,
        runtime_metadata: crate::codex_runtime::codex_runtime_metadata(),
    };
    let execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        summary: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    assert!(
        classify_gemini_legacy_initial_stall(&executor, &execution, Some(120)).is_none(),
        "codex legacy stall should not classify as gemini legacy stall"
    );
}

#[test]
fn test_classify_gemini_legacy_initial_stall_requires_silent_output() {
    let executor = Executor::GeminiCli {
        model_override: Some("gemini-2.5-pro".to_string()),
        thinking_budget: None,
    };
    let execution = csa_process::ExecutionResult {
        output: "hello".to_string(),
        stderr_output: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        summary: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };

    assert!(
        classify_gemini_legacy_initial_stall(&executor, &execution, Some(120)).is_none(),
        "non-silent legacy execution should not classify as initial stall"
    );
}

#[test]
fn test_classify_gemini_legacy_initial_stall_requires_exit_137() {
    let executor = Executor::GeminiCli {
        model_override: Some("gemini-2.5-pro".to_string()),
        thinking_budget: None,
    };
    let execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        summary: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        exit_code: 0,
        peak_memory_mb: None,
    };

    assert!(
        classify_gemini_legacy_initial_stall(&executor, &execution, Some(120)).is_none(),
        "successful execution should not classify as initial stall"
    );
}

#[test]
fn test_apply_gemini_legacy_initial_stall_summary_rewrites_summary() {
    let executor = Executor::GeminiCli {
        model_override: Some("gemini-2.5-pro".to_string()),
        thinking_budget: None,
    };
    let mut execution = csa_process::ExecutionResult {
        output: String::new(),
        stderr_output: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        summary: "initial_response_timeout: no stdout output for 120s; process killed immediately"
            .to_string(),
        exit_code: 137,
        peak_memory_mb: None,
    };
    let classification = classify_gemini_legacy_initial_stall(&executor, &execution, Some(120))
        .expect("gemini legacy initial timeout should classify");

    apply_gemini_legacy_initial_stall_summary(&mut execution, &classification);

    assert_eq!(
        execution.summary,
        "gemini_legacy_initial_stall: no stdout within 120s"
    );
    assert!(
        execution
            .stderr_output
            .contains("gemini_legacy_initial_stall: no stdout within 120s"),
        "expected stable classifier in stderr, got: {}",
        execution.stderr_output
    );
}

#[tokio::test]
async fn test_execute_in_rewrites_gemini_legacy_initial_stall_summary() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let script_path = temp.path().join("gemini");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
sleep 2
"#,
    )
    .expect("write fake gemini");
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
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test direct-entry gemini legacy initial stall",
            temp.path(),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(Some(1)),
        )
        .await
        .expect("execute_in should return classified gemini legacy stall");

    assert_eq!(result.execution.exit_code, 137);
    assert!(
        result
            .execution
            .summary
            .starts_with("gemini_legacy_initial_stall:"),
        "expected stable gemini legacy initial stall summary, got: {}",
        result.execution.summary
    );
}

#[tokio::test]
async fn test_execute_in_classifies_pre_handshake_gemini_failure_with_child_stderr() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let script_path = temp.path().join("gemini");
    std::fs::write(
        &script_path,
        r#"#!/usr/bin/env bash
set -euo pipefail
echo "spawn /home/obj/.gemini/extensions/gemini-cli-security/mcp-server ENOENT" >&2
exit 1
"#,
    )
    .expect("write fake gemini");
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

    let transport = AcpTransport::new("gemini-cli", None);
    let error = transport
        .execute_in(
            "test pre-handshake failure",
            temp.path(),
            Some(&env),
            StreamMode::BufferOnly,
            30,
            super::ResolvedTimeout(None),
        )
        .await
        .expect_err("fake gemini should fail before ACP handshake");

    let error_text = format!("{error:#}");
    assert!(
        error_text.contains("gemini_acp_init_mcp_extension"),
        "expected classified init failure, got: {error_text}"
    );
    assert!(
        error_text.contains("gemini-cli-security/mcp-server ENOENT"),
        "expected child stderr in error text, got: {error_text}"
    );
}
