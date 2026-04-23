#[tokio::test]
async fn test_execute_fails_fast_when_shared_npm_cache_bind_cannot_be_added() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert("XDG_CACHE_HOME".to_string(), "/proc/nonexistent".to_string());

    let transport = AcpTransport::new("gemini-cli", None);
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
            resource: csa_resource::sandbox::ResourceCapability::None,
            filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            project_root: None,
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMININPMLOCKSTEP0000001".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
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

    let error = transport
        .execute(
            "shared npm cache bind failure should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("sandbox plan assembly should fail fast");

    let error_text = format!("{error:#}");
    let denied_path = "/proc/nonexistent/cli-sub-agent/npm";
    assert!(
        error_text.contains(denied_path),
        "error should name denied path, got: {error_text}"
    );
    assert!(
        error_text.contains("filesystem_sandbox") || error_text.contains("writable_paths"),
        "error should point at sandbox writable_paths config, got: {error_text}"
    );
    assert!(
        error_text.contains("XDG_CACHE_HOME"),
        "error should mention XDG_CACHE_HOME remediation, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&PathBuf::from(denied_path)),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}

#[tokio::test]
async fn test_legacy_execute_fails_fast_when_shared_npm_cache_bind_cannot_be_added() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(99);
    let source_home = temp.path().join("source-home");
    std::fs::create_dir_all(source_home.join(".gemini")).expect("create source gemini dir");
    env.insert(
        "HOME".to_string(),
        source_home.to_string_lossy().into_owned(),
    );
    env.insert("XDG_CACHE_HOME".to_string(), "/proc/nonexistent".to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: IsolationPlan {
            resource: csa_resource::sandbox::ResourceCapability::None,
            filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::Bwrap,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            env_overrides: HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            project_root: None,
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: false,
        session_id: "01HTESTGEMINILEGACYNPMLOCKSTEP01".to_string(),
    };
    let options = TransportOptions {
        stream_mode: StreamMode::BufferOnly,
        idle_timeout_seconds: 30,
        acp_crash_max_attempts: 2,
        initial_response_timeout: super::ResolvedTimeout(None),
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

    let error = transport
        .execute(
            "legacy shared npm cache bind failure should fail fast",
            None,
            &session,
            Some(&env),
            options,
        )
        .await
        .expect_err("legacy sandbox plan assembly should fail fast");

    let error_text = format!("{error:#}");
    let denied_path = "/proc/nonexistent/cli-sub-agent/npm";
    assert!(
        error_text.contains(denied_path),
        "error should name denied path, got: {error_text}"
    );
    assert!(
        error_text.contains("filesystem_sandbox") || error_text.contains("writable_paths"),
        "error should point at sandbox writable_paths config, got: {error_text}"
    );
    assert!(
        error_text.contains("XDG_CACHE_HOME"),
        "error should mention XDG_CACHE_HOME remediation, got: {error_text}"
    );

    assert!(
        !env.contains_key("npm_config_cache"),
        "caller env must not leak a partially-coupled npm_config_cache override"
    );
    assert!(
        !sandbox.isolation_plan.env_overrides.contains_key("npm_config_cache"),
        "base sandbox config must remain untouched when plan assembly aborts"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&PathBuf::from(denied_path)),
        "base sandbox config must not accumulate failed writable binds"
    );
    assert!(
        !model_log_path.exists(),
        "gemini should not launch after sandbox plan failure"
    );
    assert!(
        !model_log_path.with_file_name("attempts.txt").exists(),
        "sandbox plan failure should abort before the fake gemini process runs"
    );
}
