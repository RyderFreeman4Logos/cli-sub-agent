use super::*;

fn parse_project_config(toml_str: &str) -> csa_config::ProjectConfig {
    toml::from_str(toml_str).expect("test TOML should parse")
}

fn current_project_root() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_default()
}

fn assert_run_memory_override_enforced_or_rejected(
    result: SandboxResolution,
    expected_memory_max_mb: u64,
) {
    match result {
        SandboxResolution::Ok(opts) => {
            let ctx = opts
                .sandbox
                .as_ref()
                .expect("--memory-max-mb must not resolve to Ok without a sandbox");
            assert_eq!(
                ctx.isolation_plan.memory_max_mb,
                Some(expected_memory_max_mb)
            );
            assert_eq!(
                ctx.isolation_plan.resource,
                csa_resource::ResourceCapability::CgroupV2,
                "--memory-max-mb must resolve to cgroup v2 because setrlimit does not enforce memory"
            );
        }
        SandboxResolution::RequiredButUnavailable(message) => {
            assert!(
                message.contains("--memory-max-mb"),
                "error should mention the per-run override, got: {message}"
            );
            assert!(
                message.contains("cgroup v2"),
                "error should explain that memory enforcement needs cgroup v2, got: {message}"
            );
        }
    }
}

fn resolve_with_memory_override(
    config: Option<&csa_config::ProjectConfig>,
    tool_name: &str,
    memory_max_mb: u64,
) -> SandboxResolution {
    resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config,
            tool_name,
            session_id: "test-session",
            project_root: &current_project_root(),
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            allow_user_daemon_ipc: false,
            readonly_project_root: false,
            extra_writable: &[],
            extra_readable: &[],
            execution_env: None,
        },
        crate::run_resource_overrides::RunResourceOverrides::new(Some(memory_max_mb), None),
    )
}

#[test]
fn run_memory_override_promotes_no_config_lightweight_default_off() {
    let result = resolve_with_memory_override(None, "opencode", 6144);

    assert_run_memory_override_enforced_or_rejected(result, 6144);
}

#[test]
fn run_memory_override_promotes_config_lightweight_default_off() {
    let cfg = parse_project_config(
        r#"
[tools.opencode]
enabled = true
"#,
    );

    let result = resolve_with_memory_override(Some(&cfg), "opencode", 6144);

    assert_run_memory_override_enforced_or_rejected(result, 6144);
}

#[test]
fn run_memory_override_rejects_explicit_resource_enforcement_off() {
    let cfg = parse_project_config(
        r#"
[resources]
enforcement_mode = "off"

[tools.opencode]
enabled = true
"#,
    );

    let result = resolve_with_memory_override(Some(&cfg), "opencode", 6144);

    let SandboxResolution::RequiredButUnavailable(message) = result else {
        panic!("explicit enforcement_mode = \"off\" must reject --memory-max-mb");
    };
    assert!(message.contains("--memory-max-mb"));
    assert!(message.contains("explicitly \"off\""));
}

#[test]
fn run_memory_override_sets_sandbox_memory_limit() {
    let cfg = parse_project_config(
        r#"
[tools.codex]
enabled = true
memory_max_mb = 16384
"#,
    );

    let result = resolve_with_memory_override(Some(&cfg), "codex", 6144);

    assert_run_memory_override_enforced_or_rejected(result, 6144);
}
