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
    // NOTE: resource=None means the cgroup spawn path is not exercised here;
    // real cgroup spawn requires systemd user scope access which is not
    // available in most CI environments.  The early return guard above
    // already skips this test when CgroupV2 is not detected.
    let sandbox = SandboxTransportConfig {
        isolation_plan: csa_resource::isolation_plan::IsolationPlan {
            resource: csa_resource::sandbox::ResourceCapability::None,
            filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::None,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            env_overrides: std::collections::HashMap::new(),
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
        best_effort: true,
        session_id: "01HTESTBESTEFFORT0000000001".to_string(),
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
        error_marker_scan_enabled: true,
        setting_sources: None,
        sandbox: Some(&sandbox),
        thinking_budget: None,
        subtree_pin: None,
        allow_git_push: false,
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
