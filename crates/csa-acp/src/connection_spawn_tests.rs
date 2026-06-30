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

fn command_env_map(cmd: &Command) -> HashMap<&std::ffi::OsStr, Option<&std::ffi::OsStr>> {
    cmd.as_std().get_envs().collect()
}

#[test]
fn acp_stripped_env_vars_include_git_push_authorization_keys() {
    assert!(
        AcpConnection::STRIPPED_ENV_VARS.contains(&csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY),
        "STRIPPED_ENV_VARS must strip inherited CSA_GIT_PUSH_ALLOWED"
    );
    assert!(
        AcpConnection::STRIPPED_ENV_VARS
            .contains(&csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY),
        "STRIPPED_ENV_VARS must strip inherited CSA_RUN_GIT_PUSH_AUTHORIZED"
    );
}

#[test]
fn build_cmd_base_scrubs_git_push_authorization_env_vars() {
    let env = HashMap::new();
    let cmd = AcpConnection::build_cmd_base("acp-tool", &[], Path::new("/tmp"), &env);
    let env_map = command_env_map(&cmd);

    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert_eq!(
            env_map.get(std::ffi::OsStr::new(*key)),
            Some(&None),
            "ACP child command must env_remove git-push authorization key {key}"
        );
    }
}

#[test]
fn merge_sandbox_env_scrubs_subtree_contract_from_env_overrides() {
    let request_env = HashMap::from([("PATH".to_string(), "/usr/bin".to_string())]);
    let mut sandbox_env_overrides = HashMap::from([(
        "HOME".to_string(),
        "/tmp/cli-sub-agent-gemini/01TEST".to_string(),
    )]);
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        sandbox_env_overrides.insert((*key).to_string(), format!("spoofed-{key}"));
    }
    sandbox_env_overrides.insert(
        csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
        "true".to_string(),
    );
    sandbox_env_overrides.insert(
        csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY.to_string(),
        "true".to_string(),
    );

    let effective_env =
        AcpConnection::merge_sandbox_env(&request_env, Some(&sandbox_env_overrides));

    assert_eq!(
        effective_env.get("HOME"),
        Some(&"/tmp/cli-sub-agent-gemini/01TEST".to_string()),
        "non-contract runtime override must survive the sandbox env merge"
    );
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert!(
            !effective_env.contains_key(*key),
            "generic sandbox env_overrides must not populate startup-subtree key {key}"
        );
    }
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert!(
            !effective_env.contains_key(*key),
            "generic sandbox env_overrides must not populate git-push authorization key {key}"
        );
    }

    let cmd = AcpConnection::build_cmd_base("acp-tool", &[], Path::new("/tmp"), &effective_env);
    let env_map = command_env_map(&cmd);
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert_eq!(
            env_map.get(std::ffi::OsStr::new(*key)),
            Some(&None),
            "build_cmd_base must not receive startup-subtree key {key} from sandbox env_overrides"
        );
    }
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert_eq!(
            env_map.get(std::ffi::OsStr::new(*key)),
            Some(&None),
            "build_cmd_base must not receive git-push authorization key {key} from sandbox env_overrides"
        );
    }

    let cgroup_config = csa_resource::cgroup::SandboxConfig {
        memory_max_mb: 4096,
        memory_swap_max_mb: None,
        pids_max: Some(512),
    };
    let scope_cmd = csa_resource::cgroup::create_scope_command_with_env(
        "gemini-cli",
        "01TEST",
        &cgroup_config,
        &effective_env,
    );
    let mut cmd = Command::from(scope_cmd);
    AcpConnection::scrub_inherited_child_env(&mut cmd);
    for (key, value) in &effective_env {
        cmd.env(key, value);
    }
    let env_map = command_env_map(&cmd);
    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert_eq!(
            env_map.get(std::ffi::OsStr::new(*key)),
            Some(&None),
            "cgroup command env must not receive startup-subtree key {key} from sandbox env_overrides"
        );
    }
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert_eq!(
            env_map.get(std::ffi::OsStr::new(*key)),
            Some(&None),
            "cgroup command env must not receive git-push authorization key {key} from sandbox env_overrides"
        );
    }
}

#[test]
fn merge_sandbox_env_preserves_trusted_base_subtree_contract_values() {
    let request_env = HashMap::from([
        (
            csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
            "2".to_string(),
        ),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(),
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY.to_string(),
            "codex/openai/gpt-5.5/xhigh".to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
    ]);
    let sandbox_env_overrides = HashMap::from([
        (
            csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
            "99".to_string(),
        ),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(),
            "0".to_string(),
        ),
        (
            csa_core::env::CSA_MODEL_SPEC_ENV_KEY.to_string(),
            "gemini-cli/google/gemini-3.1-pro-preview/xhigh".to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
            "0".to_string(),
        ),
        (
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
            "false".to_string(),
        ),
        (
            csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
    ]);

    let effective_env =
        AcpConnection::merge_sandbox_env(&request_env, Some(&sandbox_env_overrides));

    assert_eq!(
        effective_env
            .get(csa_core::env::CSA_DEPTH_ENV_KEY)
            .map(String::as_str),
        Some("2"),
        "trusted typed CSA_DEPTH must not be replaced by sandbox env_overrides"
    );
    assert_eq!(
        effective_env
            .get(csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY)
            .map(String::as_str),
        Some("1"),
        "trusted typed CSA_INTERNAL_INVOCATION must survive the generic override scrub"
    );
    assert_eq!(
        effective_env
            .get(csa_core::env::CSA_MODEL_SPEC_ENV_KEY)
            .map(String::as_str),
        Some("codex/openai/gpt-5.5/xhigh"),
        "trusted typed subtree pin must survive the generic override scrub"
    );
    assert_eq!(
        effective_env
            .get(csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY)
            .map(String::as_str),
        Some("1"),
        "trusted typed force-ignore marker must survive the generic override scrub"
    );
    assert_eq!(
        effective_env
            .get(csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY)
            .map(String::as_str),
        Some("true"),
        "trusted typed git-push authorization must survive the generic override scrub"
    );
    assert!(
        !effective_env.contains_key(csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY),
        "internal git-push marker must not survive generic sandbox env_overrides"
    );
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
        user_daemon_ipc: false,
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
            csa_core::env::CSA_SESSION_ID_ENV_KEY.to_string(),
            "01SPOOFED".to_string(),
        ),
        (
            csa_core::env::CSA_DEPTH_ENV_KEY.to_string(),
            "99".to_string(),
        ),
        (
            csa_core::env::CSA_FORCE_IGNORE_TIER_SETTING_ENV_KEY.to_string(),
            "1".to_string(),
        ),
        (
            csa_core::env::CSA_INTERNAL_INVOCATION_ENV_KEY.to_string(),
            "0".to_string(),
        ),
        (
            csa_core::env::CSA_GIT_PUSH_ALLOWED_ENV_KEY.to_string(),
            "true".to_string(),
        ),
        (
            csa_core::env::CSA_RUN_GIT_PUSH_AUTHORIZED_ENV_KEY.to_string(),
            "true".to_string(),
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
        user_daemon_ipc: false,
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

    for key in csa_core::env::STARTUP_SUBTREE_ENV_KEYS {
        assert!(
            !prepared.effective_env.contains_key(*key),
            "ACP effective_env must not include subtree-contract key {key} from env_overrides"
        );
    }
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert!(
            !prepared.effective_env.contains_key(*key),
            "ACP effective_env must not include git-push authorization key {key} from env_overrides"
        );
    }
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
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert!(
            !prepared
                .effective_args
                .windows(3)
                .any(|window| window[0] == "--setenv" && window[1] == *key),
            "ACP bwrap args must not include git-push authorization key {key}: {:?}",
            prepared.effective_args
        );
    }
}

#[test]
fn scrub_inherited_child_env_removes_git_push_authorization_for_wrapper_commands() {
    let mut cmd = Command::new("systemd-run");

    AcpConnection::scrub_inherited_child_env(&mut cmd);

    let env_map = command_env_map(&cmd);
    for key in csa_core::env::GIT_PUSH_AUTHORIZATION_ENV_KEYS {
        assert_eq!(
            env_map.get(std::ffi::OsStr::new(*key)),
            Some(&None),
            "wrapper command must env_remove inherited git-push authorization key {key}"
        );
    }
}
