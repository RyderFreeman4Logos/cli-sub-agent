use super::*;

#[test]
fn append_stderr_tail_bounds_retained_memory_to_tail_window() {
    let mut stderr = "a".repeat(1024 * 1024);
    append_stderr_tail(&mut stderr, &"b".repeat(1024 * 1024 + 64));

    assert_eq!(
        stderr.len(),
        1024 * 1024,
        "stderr retention should trim back to the 1 MiB tail window"
    );
    assert!(
        stderr.ends_with(&"b".repeat(4096)),
        "stderr tail should retain the most recent bytes after trimming"
    );
}

fn has_setenv(args: &[String], key: &str, value: &str) -> bool {
    args.windows(3)
        .any(|window| window[0] == "--setenv" && window[1] == key && window[2] == value)
}

#[test]
fn prepare_sandbox_command_merges_runtime_env_overrides_into_bwrap_invocation() {
    let request_env = HashMap::from([
        ("HOME".to_string(), "/home/original".to_string()),
        ("PATH".to_string(), "/usr/bin".to_string()),
    ]);
    let sandbox_env_overrides = HashMap::from([
        (
            "HOME".to_string(),
            "/tmp/cli-sub-agent-gemini/01TEST".to_string(),
        ),
        (
            "XDG_STATE_HOME".to_string(),
            "/tmp/cli-sub-agent-gemini/01TEST/.local/state".to_string(),
        ),
        (
            "MISE_CACHE_DIR".to_string(),
            "/tmp/cli-sub-agent-gemini/01TEST/.cache/mise".to_string(),
        ),
    ]);
    let isolation_plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![
            PathBuf::from("/project"),
            PathBuf::from("/tmp/cli-sub-agent-gemini/01TEST"),
        ],
        readable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: Some(PathBuf::from("/project")),
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
    };
    let args = vec!["--acp".to_string()];
    let request = AcpSpawnRequest {
        command: "/usr/bin/gemini",
        args: &args,
        working_dir: Path::new("/project"),
        env: &request_env,
        options: AcpConnectionOptions::default(),
    };
    let sandbox = AcpSandboxRequest {
        isolation_plan: &isolation_plan,
        tool_name: "gemini-cli",
        session_id: "01TEST",
        env_overrides: Some(&sandbox_env_overrides),
    };

    let prepared = AcpConnection::prepare_sandbox_command(request, &sandbox);

    assert_eq!(prepared.effective_command, "bwrap");
    assert_eq!(
        prepared.effective_env.get("HOME"),
        Some(&"/tmp/cli-sub-agent-gemini/01TEST".to_string()),
        "scope env should see the Gemini runtime HOME override"
    );
    assert_eq!(
        prepared.effective_env.get("XDG_STATE_HOME"),
        Some(&"/tmp/cli-sub-agent-gemini/01TEST/.local/state".to_string())
    );
    assert!(
        has_setenv(
            &prepared.effective_args,
            "HOME",
            "/tmp/cli-sub-agent-gemini/01TEST",
        ),
        "bwrap args must include runtime HOME override: {:?}",
        prepared.effective_args
    );
    assert!(
        has_setenv(
            &prepared.effective_args,
            "XDG_STATE_HOME",
            "/tmp/cli-sub-agent-gemini/01TEST/.local/state",
        ),
        "bwrap args must include XDG_STATE_HOME override: {:?}",
        prepared.effective_args
    );
    assert!(
        has_setenv(
            &prepared.effective_args,
            "MISE_CACHE_DIR",
            "/tmp/cli-sub-agent-gemini/01TEST/.cache/mise",
        ),
        "bwrap args must include mise cache override: {:?}",
        prepared.effective_args
    );
}

#[test]
fn prepare_sandbox_command_scrubs_subtree_contract_from_bwrap_env_overrides() {
    let request_env = HashMap::from([("PATH".to_string(), "/usr/bin".to_string())]);
    let sandbox_env_overrides = HashMap::from([
        (
            "HOME".to_string(),
            "/tmp/cli-sub-agent-gemini/01TEST".to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY.to_string(),
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
        (
            csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
            "99".to_string(),
        ),
    ]);
    let isolation_plan = IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: vec![
            PathBuf::from("/project"),
            PathBuf::from("/tmp/cli-sub-agent-gemini/01TEST"),
        ],
        readable_paths: Vec::new(),
        env_overrides: HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: Some(PathBuf::from("/project")),
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
    };
    let args = vec!["--acp".to_string()];
    let request = AcpSpawnRequest {
        command: "/usr/bin/gemini",
        args: &args,
        working_dir: Path::new("/project"),
        env: &request_env,
        options: AcpConnectionOptions::default(),
    };
    let sandbox = AcpSandboxRequest {
        isolation_plan: &isolation_plan,
        tool_name: "gemini-cli",
        session_id: "01TEST",
        env_overrides: Some(&sandbox_env_overrides),
    };

    let prepared = AcpConnection::prepare_sandbox_command(request, &sandbox);

    assert!(
        has_setenv(
            &prepared.effective_args,
            "HOME",
            "/tmp/cli-sub-agent-gemini/01TEST",
        ),
        "non-contract runtime override must still reach bwrap args"
    );
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert!(
            !prepared
                .effective_args
                .windows(3)
                .any(|window| window[0] == "--setenv" && window[1] == *key),
            "ACP bwrap args must not include subtree-contract key {key}: {:?}",
            prepared.effective_args
        );
    }
}
