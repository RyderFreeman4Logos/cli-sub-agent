#[tokio::test]
async fn test_execute_best_effort_sandbox_fallback_preserves_attempt_model_override() {
    let (temp, mut env, model_log_path) = setup_fake_gemini_environment(2);
    let private_bin = temp.path().join("private-bin");
    std::fs::create_dir(&private_bin).expect("create private fixture PATH");
    std::fs::rename(temp.path().join("gemini"), private_bin.join("gemini"))
        .expect("move fake gemini into private fixture PATH");
    for tool in ["bash", "cat"] {
        std::os::unix::fs::symlink(Path::new("/bin").join(tool), private_bin.join(tool))
            .unwrap_or_else(|error| panic!("link fixture {tool}: {error}"));
    }
    env.insert("PATH".to_string(), private_bin.display().to_string());

    let transport = LegacyTransport::new(Executor::GeminiCli {
        model_override: None,
        thinking_budget: None,
    });
    let session = build_test_meta_session(temp.path().to_str().expect("utf8 temp path"));
    let sandbox = SandboxTransportConfig {
        isolation_plan: csa_resource::isolation_plan::IsolationPlan {
            resource: csa_resource::sandbox::ResourceCapability::CgroupV2,
            filesystem: csa_resource::filesystem_sandbox::FilesystemCapability::None,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            env_overrides: std::collections::HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb: None,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: false,
            user_daemon_ipc: false,
            project_root: None,
            soft_limit_percent: None,
            memory_monitor_interval_seconds: None,
        },
        tool_name: "gemini-cli".to_string(),
        best_effort: true,
        session_id: "01HTESTBESTEFFORT0000000001".to_string(),
    };
    assert_eq!(
        sandbox.isolation_plan.resource,
        csa_resource::sandbox::ResourceCapability::CgroupV2,
        "fallback regression must exercise the cgroup spawn path"
    );
    let fixture_path = env.get("PATH").expect("fixture PATH");
    assert!(
        !std::env::split_paths(fixture_path)
            .any(|entry| entry.join("systemd-run").is_file()),
        "fixture PATH must make systemd-run unresolvable"
    );
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
