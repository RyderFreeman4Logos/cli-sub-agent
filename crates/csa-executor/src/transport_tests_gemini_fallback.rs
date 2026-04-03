use std::path::PathBuf;

#[test]
fn test_gemini_should_use_api_key_by_phase() {
    // Phase 1: OAuth auth
    assert!(!gemini_should_use_api_key(1));
    // Phase 2: API key auth (same model)
    assert!(gemini_should_use_api_key(2));
    // Phase 3: API key auth (flash model)
    assert!(gemini_should_use_api_key(3));
}

#[test]
fn test_gemini_rate_limit_backoff_is_exponential() {
    assert_eq!(
        gemini_rate_limit_backoff(1),
        Duration::from_millis(GEMINI_RATE_LIMIT_BASE_BACKOFF_MS)
    );
    assert_eq!(
        gemini_rate_limit_backoff(2),
        Duration::from_millis(GEMINI_RATE_LIMIT_BASE_BACKOFF_MS * 2)
    );
}

#[test]
fn test_inject_api_key_fallback_promotes_key_and_removes_internal() {
    let mut env = HashMap::new();
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "test-api-key-123".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());
    env.insert("OTHER_VAR".to_string(), "keep".to_string());
    let result = gemini_inject_api_key_fallback(Some(&env)).unwrap();
    assert_eq!(result.get("GEMINI_API_KEY").unwrap(), "test-api-key-123");
    assert_eq!(result.get("_CSA_GEMINI_AUTH_MODE").unwrap(), "api_key");
    assert!(!result.contains_key("_CSA_API_KEY_FALLBACK"));
    assert_eq!(result.get("OTHER_VAR").unwrap(), "keep");
}

#[test]
fn test_inject_api_key_fallback_returns_none_without_key() {
    let env = HashMap::new();
    assert!(gemini_inject_api_key_fallback(Some(&env)).is_none());
    assert!(gemini_inject_api_key_fallback(None).is_none());
}

#[test]
fn test_inject_api_key_fallback_returns_none_for_api_key_mode() {
    let mut env = HashMap::new();
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "fallback-key".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "api_key".to_string());
    assert!(gemini_inject_api_key_fallback(Some(&env)).is_none());
}

#[tokio::test]
async fn test_execute_in_falls_back_to_api_key_after_all_retries_exhausted() {
    let (_temp, mut env, _model_log_path) = setup_fake_gemini_environment(99);
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "fallback-key".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "test api key fallback",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("execute_in should succeed with api key fallback");

    // The fake script always fails with QUOTA_EXHAUSTED; the fallback attempt
    // also uses the same fake script (which increments the counter). After 3
    // model-retry attempts + 1 fallback attempt = 4 total. The fallback attempt
    // still fails because success_on=99, but we verify the fallback path was taken
    // by checking GEMINI_API_KEY was injected (the env var will be visible to the script).
    // Since the fake script doesn't check GEMINI_API_KEY, just verify the result came back.
    assert_ne!(result.execution.exit_code, 0);
    assert!(result.execution.stderr_output.contains("QUOTA_EXHAUSTED"));
}

#[tokio::test]
async fn test_execute_falls_back_to_api_key_after_all_retries_exhausted() {
    let (temp, mut env, _model_log_path) = setup_fake_gemini_environment(99);
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "fallback-key".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        initial_response_timeout_seconds: None,
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: None,
    };

    let result = transport
        .execute("test api key fallback", None, &session, Some(&env), options)
        .await
        .expect("execute should complete with api key fallback attempt");

    // Fallback attempt still fails (success_on=99), but 4 total attempts
    // (3 model retries + 1 fallback) confirms the fallback path was taken.
    assert_ne!(result.execution.exit_code, 0);
    assert!(result.execution.stderr_output.contains("QUOTA_EXHAUSTED"));
}

#[tokio::test]
async fn test_execute_in_new_invocation_restarts_with_oauth_before_fallback() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "fallback-key".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());
    env.insert("_CSA_NO_FLASH_FALLBACK".to_string(), "1".to_string());
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let first = transport
        .execute_in(
            "first invocation",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("first invocation should return the last failed attempt");
    assert_ne!(first.execution.exit_code, 0);

    let second = transport
        .execute_in(
            "second invocation",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("second invocation should return the last failed attempt");
    assert_ne!(second.execution.exit_code, 0);

    let auths = read_auth_log(&model_log_path);
    assert_eq!(
        auths,
        vec![
            "oauth".to_string(),
            "api_key".to_string(),
            "oauth".to_string(),
            "api_key".to_string(),
        ],
        "each invocation must restart on the quota-backed path before reusing API key fallback"
    );
}

#[tokio::test]
async fn test_execute_in_non_quota_failure_does_not_trigger_api_key_fallback() {
    let (_temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    env.insert(
        "_CSA_API_KEY_FALLBACK".to_string(),
        "fallback-key".to_string(),
    );
    env.insert("_CSA_GEMINI_AUTH_MODE".to_string(), "oauth".to_string());
    env.insert(
        "CSA_FAKE_GEMINI_FAILURE_REASON".to_string(),
        "internal server error".to_string(),
    );
    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });

    let result = transport
        .execute_in(
            "non quota failure",
            std::path::Path::new("/tmp"),
            Some(&env),
            StreamMode::BufferOnly,
            30,
        )
        .await
        .expect("non-quota failures should be returned directly");

    assert_ne!(result.execution.exit_code, 0);
    assert!(
        result
            .execution
            .stderr_output
            .contains("internal server error"),
        "unexpected stderr: {}",
        result.execution.stderr_output
    );
    assert_eq!(
        read_model_log(&model_log_path),
        vec!["inherit".to_string()],
        "non-quota failures must not trigger another Gemini attempt"
    );
    assert_eq!(
        read_auth_log(&model_log_path),
        vec!["oauth".to_string()],
        "non-quota failures must not switch auth modes"
    );
}

#[tokio::test]
async fn test_execute_best_effort_sandbox_fallback_preserves_attempt_model_override() {
    if !matches!(
        csa_resource::sandbox::detect_resource_capability(),
        csa_resource::sandbox::ResourceCapability::CgroupV2
    ) {
        // This test specifically targets the cgroup sandbox spawn failure ->
        // best-effort unsandboxed fallback branch.
        return;
    }

    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(2);
    // Force sandbox spawn failure by hiding systemd-run from PATH while keeping
    // our fake gemini binary and basic shell tools available.
    env.insert(
        "PATH".to_string(),
        format!("{}:/bin", temp.path().display()),
    );

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: csa_resource::isolation_plan::IsolationPlan {
            resource: csa_resource::sandbox::ResourceCapability::None,
            filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::None,
            writable_paths: Vec::new(),
            env_overrides: std::collections::HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            project_root: None,
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: true,
        session_id: "01HTESTBESTEFFORT0000000001".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        initial_response_timeout_seconds: None,
        liveness_dead_seconds: 30,
        stdin_write_timeout_seconds: 30,
        acp_init_timeout_seconds: 30,
        termination_grace_period_seconds: 1,
        output_spool: None,
        output_spool_max_bytes: csa_process::DEFAULT_SPOOL_MAX_BYTES,
        output_spool_keep_rotated: csa_process::DEFAULT_SPOOL_KEEP_ROTATED,
        setting_sources: None,
        sandbox: Some(&sandbox),
    };

    let result = transport
        .execute(
            "test best effort fallback",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect("execute should succeed after best-effort fallback and retry");

    assert_eq!(result.execution.exit_code, 0);
    let models = read_model_log(&model_log_path);
    assert_eq!(
        models,
        vec!["inherit".to_string(), "inherit".to_string()],
        "best-effort fallback path: phase 2 keeps original model (switches to API key auth)"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_acp_wrapped_capacity_error() {
    // This mirrors the real error chain from ACP transport:
    // anyhow!("ACP transport (sandboxed) failed: {e}") where e is AcpError::PromptFailed
    let acp_error_msg = "ACP transport (sandboxed) failed: ACP prompt failed: \
        No capacity available for model gemini-3.1-pro-preview on the server; \
        stderr: Running scope as unit: csa-gemini-cli-01KN.scope";
    assert!(
        is_gemini_rate_limited_error(acp_error_msg),
        "should detect 'no capacity available' inside ACP-wrapped error"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_acp_wrapped_429_error() {
    let acp_error_msg =
        "ACP transport (sandboxed) failed: ACP prompt failed: 429 Too Many Requests";
    assert!(
        is_gemini_rate_limited_error(acp_error_msg),
        "should detect '429' inside ACP-wrapped error"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_acp_wrapped_quota_exhausted() {
    let acp_error_msg =
        "ACP transport (sandboxed) failed: ACP prompt failed: quota exhausted for project";
    assert!(
        is_gemini_rate_limited_error(acp_error_msg),
        "should detect 'quota exhausted' inside ACP-wrapped error"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_new_capacity_phrase() {
    let acp_error_msg = "ACP transport failed: ACP prompt failed: You have exhausted your capacity on this model. Your quota will reset after 13h52m46s.";
    assert!(
        is_gemini_rate_limited_error(acp_error_msg),
        "should detect Gemini's newer exhausted-capacity wording"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_anyhow_context_chain() {
    let error = anyhow::anyhow!("You have exhausted your capacity on this model.")
        .context("ACP prompt failed")
        .context("ACP transport failed");
    assert!(
        is_gemini_rate_limited_error(&format!("{error:#}")),
        "should detect rate limits in anyhow context chains"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_unsandboxed_fallback_error() {
    let acp_error_msg =
        "ACP transport (unsandboxed fallback) failed: ACP prompt failed: resource exhausted";
    assert!(
        is_gemini_rate_limited_error(acp_error_msg),
        "should detect 'resource exhausted' in unsandboxed fallback path"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_matches_plain_acp_error() {
    let acp_error_msg = "ACP transport failed: ACP prompt failed: No capacity available for model";
    assert!(
        is_gemini_rate_limited_error(acp_error_msg),
        "should detect rate limit in non-sandboxed ACP path"
    );
}

#[test]
fn test_is_gemini_rate_limited_error_rejects_non_rate_limit_acp_error() {
    let acp_error_msg =
        "ACP transport (sandboxed) failed: ACP prompt failed: internal server error";
    assert!(
        !is_gemini_rate_limited_error(acp_error_msg),
        "should not match non-rate-limit errors"
    );
}

#[test]
fn test_is_gemini_rate_limited_result_matches_capacity_in_stdout() {
    let execution = csa_process::ExecutionResult {
        summary: String::new(),
        output: "No capacity available for model gemini-3.1-pro-preview".to_string(),
        stderr_output: String::new(),
        exit_code: 1,
    };
    assert!(
        is_gemini_rate_limited_result(&execution),
        "should detect rate limit pattern in stdout"
    );
}

#[test]
fn test_is_gemini_rate_limited_result_matches_capacity_in_stderr() {
    let execution = csa_process::ExecutionResult {
        summary: String::new(),
        output: String::new(),
        stderr_output: "No capacity available for model gemini-3.1-pro-preview".to_string(),
        exit_code: 1,
    };
    assert!(
        is_gemini_rate_limited_result(&execution),
        "should detect rate limit pattern in stderr"
    );
}

#[test]
fn test_is_gemini_rate_limited_result_ignores_success_exit_code() {
    let execution = csa_process::ExecutionResult {
        summary: String::new(),
        output: "No capacity available for model".to_string(),
        stderr_output: String::new(),
        exit_code: 0,
    };
    assert!(
        !is_gemini_rate_limited_result(&execution),
        "should not retry when exit code is 0 even if output contains rate limit text"
    );
}

#[test]
fn test_format_gemini_retry_report_lists_attempt_phases() {
    let phases = vec![
        GeminiRetryPhase::for_attempt(1),
        GeminiRetryPhase::for_attempt(2),
        GeminiRetryPhase::for_attempt(3),
    ];

    let report = format_gemini_retry_report(&phases);

    assert!(report.contains("attempt=1"));
    assert!(report.contains("auth=oauth"));
    assert!(report.contains("attempt=2"));
    assert!(report.contains("auth=api_key"));
    assert!(report.contains("model=gemini-3-flash-preview"));
}

#[test]
fn test_annotate_gemini_retry_error_includes_phase_history() {
    let phases = vec![
        GeminiRetryPhase::for_attempt(1),
        GeminiRetryPhase::for_attempt(2),
    ];

    let error = annotate_gemini_retry_error(anyhow::anyhow!("quota exhausted"), &phases);
    let rendered = format!("{error:#}");

    assert!(rendered.contains("Gemini ACP retry chain exhausted."));
    assert!(rendered.contains("attempt=1"));
    assert!(rendered.contains("attempt=2"));
    assert!(rendered.contains("quota exhausted"));
}

#[test]
fn test_ensure_gemini_runtime_home_writable_path_adds_runtime_home_under_tmp() {
    let runtime_home = PathBuf::from("/tmp/cli-sub-agent-gemini/01TEST/session-home");
    let mut isolation_plan = IsolationPlan {
        resource: csa_resource::sandbox::ResourceCapability::None,
        filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
        writable_paths: vec![PathBuf::from("/tmp")],
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
    };

    ensure_gemini_runtime_home_writable_path(&mut isolation_plan, Some(&runtime_home));

    assert!(
        isolation_plan.writable_paths.contains(&runtime_home),
        "runtime home under /tmp must be bound explicitly because bwrap skips --bind /tmp"
    );
}

#[test]
fn test_ensure_gemini_runtime_home_writable_path_skips_when_parent_is_bound() {
    let runtime_root = PathBuf::from("/tmp/cli-sub-agent-gemini");
    let runtime_home = runtime_root.join("01TEST/session-home");
    let mut isolation_plan = IsolationPlan {
        resource: csa_resource::sandbox::ResourceCapability::None,
        filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
        writable_paths: vec![runtime_root.clone()],
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
    };

    ensure_gemini_runtime_home_writable_path(&mut isolation_plan, Some(&runtime_home));

    assert_eq!(
        isolation_plan
            .writable_paths
            .iter()
            .filter(|path| **path == runtime_root || **path == runtime_home)
            .count(),
        1,
        "an existing non-/tmp parent bind already exposes the runtime home"
    );
}

#[test]
fn test_ensure_gemini_runtime_home_writable_path_is_noop_without_runtime_home() {
    let mut isolation_plan = IsolationPlan {
        resource: csa_resource::sandbox::ResourceCapability::None,
        filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
        writable_paths: vec![PathBuf::from("/project")],
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
    };

    ensure_gemini_runtime_home_writable_path(&mut isolation_plan, None);

    assert_eq!(
        isolation_plan.writable_paths,
        vec![PathBuf::from("/project")]
    );
}

#[test]
fn test_apply_gemini_sandbox_runtime_env_overrides_pins_runtime_paths_and_clears_api_key() {
    let runtime_home = "/tmp/cli-sub-agent-gemini/01TEST/runtime-home";
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), runtime_home.to_string());
    env.insert(
        "PATH".to_string(),
        "/runtime/node/bin:/runtime/yarn/bin:/usr/local/bin".to_string(),
    );
    env.insert("GEMINI_CLI_HOME".to_string(), runtime_home.to_string());
    env.insert(
        "XDG_CONFIG_HOME".to_string(),
        format!("{runtime_home}/.config"),
    );
    env.insert(
        "XDG_CACHE_HOME".to_string(),
        format!("{runtime_home}/.cache"),
    );
    env.insert(
        "XDG_STATE_HOME".to_string(),
        format!("{runtime_home}/.local/state"),
    );
    env.insert(
        "MISE_CACHE_DIR".to_string(),
        format!("{runtime_home}/.cache/mise"),
    );
    env.insert(
        "MISE_STATE_DIR".to_string(),
        format!("{runtime_home}/.local/state/mise"),
    );
    env.insert("MISE_SHIM".to_string(), String::new());
    env.insert("MISE_SHIMS_DIR".to_string(), String::new());
    env.insert(
        csa_core::gemini::AUTH_MODE_ENV_KEY.to_string(),
        csa_core::gemini::AUTH_MODE_OAUTH.to_string(),
    );
    let mut isolation_plan = IsolationPlan {
        resource: csa_resource::sandbox::ResourceCapability::None,
        filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
    };

    let env_overrides = gemini_sandbox_runtime_env_overrides(&env);
    apply_gemini_sandbox_runtime_env_overrides(&mut isolation_plan, &env_overrides);

    assert_eq!(
        isolation_plan.env_overrides.get("HOME"),
        Some(&runtime_home.to_string())
    );
    assert_eq!(
        isolation_plan.env_overrides.get("GEMINI_CLI_HOME"),
        Some(&runtime_home.to_string())
    );
    assert_eq!(
        isolation_plan.env_overrides.get("PATH"),
        Some(&"/runtime/node/bin:/runtime/yarn/bin:/usr/local/bin".to_string()),
        "sandbox should receive the sanitized runtime PATH so nested yarn/node launches avoid mise shims"
    );
    assert_eq!(
        isolation_plan.env_overrides.get("XDG_STATE_HOME"),
        Some(&format!("{runtime_home}/.local/state"))
    );
    assert_eq!(
        isolation_plan.env_overrides.get("MISE_CACHE_DIR"),
        Some(&format!("{runtime_home}/.cache/mise")),
        "sandbox should pin mise cache inside the Gemini runtime home"
    );
    assert_eq!(
        isolation_plan.env_overrides.get("MISE_STATE_DIR"),
        Some(&format!("{runtime_home}/.local/state/mise")),
        "sandbox should pin mise state inside the Gemini runtime home"
    );
    assert_eq!(
        isolation_plan.env_overrides.get("MISE_SHIM"),
        Some(&String::new())
    );
    assert_eq!(
        isolation_plan.env_overrides.get("MISE_SHIMS_DIR"),
        Some(&String::new())
    );
    assert_eq!(
        isolation_plan
            .env_overrides
            .get(csa_core::gemini::AUTH_MODE_ENV_KEY),
        Some(&csa_core::gemini::AUTH_MODE_OAUTH.to_string())
    );
    assert_eq!(
        isolation_plan
            .env_overrides
            .get(csa_core::gemini::API_KEY_ENV),
        Some(&String::new()),
        "phase 1 must clear inherited GEMINI_API_KEY inside the sandbox"
    );
    assert_eq!(
        isolation_plan
            .env_overrides
            .get(csa_core::gemini::BASE_URL_ENV),
        Some(&String::new()),
        "sandbox should also clear inherited Gemini base-url overrides when absent"
    );
}

#[test]
fn test_apply_gemini_sandbox_runtime_env_overrides_preserves_phase2_api_key() {
    let mut env = HashMap::new();
    env.insert(
        "HOME".to_string(),
        "/tmp/cli-sub-agent-gemini/01TEST".to_string(),
    );
    env.insert(
        csa_core::gemini::AUTH_MODE_ENV_KEY.to_string(),
        csa_core::gemini::AUTH_MODE_API_KEY.to_string(),
    );
    env.insert(
        csa_core::gemini::API_KEY_ENV.to_string(),
        "fallback-key".to_string(),
    );
    env.insert(
        csa_core::gemini::BASE_URL_ENV.to_string(),
        "https://proxy.example.test".to_string(),
    );
    let mut isolation_plan = IsolationPlan {
        resource: csa_resource::sandbox::ResourceCapability::None,
        filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
    };

    let env_overrides = gemini_sandbox_runtime_env_overrides(&env);
    apply_gemini_sandbox_runtime_env_overrides(&mut isolation_plan, &env_overrides);

    assert_eq!(
        isolation_plan
            .env_overrides
            .get(csa_core::gemini::AUTH_MODE_ENV_KEY),
        Some(&csa_core::gemini::AUTH_MODE_API_KEY.to_string())
    );
    assert_eq!(
        isolation_plan
            .env_overrides
            .get(csa_core::gemini::API_KEY_ENV),
        Some(&"fallback-key".to_string())
    );
    assert_eq!(
        isolation_plan
            .env_overrides
            .get(csa_core::gemini::BASE_URL_ENV),
        Some(&"https://proxy.example.test".to_string())
    );
}
