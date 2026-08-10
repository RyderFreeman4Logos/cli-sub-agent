use super::*;
use csa_resource::ResourceCapability;

#[test]
fn spawn_projection_uses_configured_tool_limit() {
    let cfg: ProjectConfig =
        toml::from_str("[resources]\nmemory_max_mb = 8192\n").expect("config should parse");

    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            None,
            Some(&cfg),
            "codex",
            RunResourceOverrides::absent(),
            ResourceCapability::None,
            1,
        ),
        8192
    );
}

#[test]
fn spawn_projection_uses_run_override_before_tool_config() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
[tools.codex]
memory_max_mb = 16384
"#,
    )
    .expect("config should parse");
    let overrides = RunResourceOverrides::from_cli(Some(6144), None);

    assert_eq!(
        spawn_memory_projection_mb_with_overrides(
            None,
            Some(&cfg),
            "codex",
            overrides,
            ResourceCapability::None,
        ),
        6144
    );
}

#[test]
fn spawn_projection_uses_inherited_memory_override_before_tool_config() {
    let cfg: ProjectConfig = toml::from_str(
        r#"
[tools.codex]
memory_max_mb = 16384
"#,
    )
    .expect("config should parse");
    let inherited = RunResourceOverrides::from_cli(Some(6144), None).for_child();

    assert_eq!(
        spawn_memory_projection_mb_with_overrides(
            None,
            Some(&cfg),
            "codex",
            inherited,
            ResourceCapability::None,
        ),
        6144
    );
}

#[test]
fn default_projection_is_bounded_by_physical_memory_after_reserve() {
    assert_eq!(
        bound_default_spawn_projection_mb(14_000, 12_000, 1024),
        10_976
    );
    assert_eq!(
        bound_default_spawn_projection_mb(14_000, 32_000, 1024),
        14_000
    );
}

#[test]
fn default_projection_uses_a_minimum_when_host_headroom_is_exhausted() {
    assert_eq!(bound_default_spawn_projection_mb(14_000, 1024, 2048), 256);
}

#[test]
fn spawn_projection_uses_tool_default_without_config() {
    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            None,
            None,
            "codex",
            RunResourceOverrides::absent(),
            ResourceCapability::None,
            12_000,
        ),
        7904
    );
}

#[test]
fn writer_default_projection_only_uses_soft_limit_floor_with_cgroup_monitoring() {
    let config: ProjectConfig =
        toml::from_str("[resources]\nsoft_limit_percent = 90\nmin_free_memory_mb = 100\n")
            .expect("config should parse");

    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            Some("run"),
            Some(&config),
            "codex",
            RunResourceOverrides::absent(),
            ResourceCapability::Setrlimit,
            8_377,
        ),
        8_277,
        "Setrlimit does not run the soft-limit memory monitor, so the default stays bounded"
    );
    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            Some("run"),
            Some(&config),
            "codex",
            RunResourceOverrides::absent(),
            ResourceCapability::CgroupV2,
            8_377,
        ),
        10_000,
        "the cgroup memory monitor needs its writer soft-limit floor"
    );
}

#[test]
fn final_recheck_uses_setrlimit_after_cgroup_landlock_plan_degradation() {
    let project_root = tempfile::tempdir().expect("project root");
    let resource_overrides = RunResourceOverrides::from_cli(None, Some(100));
    let options = match crate::pipeline_sandbox::resolve_sandbox_options_with_capabilities(
        crate::pipeline_sandbox::SandboxResolveInput {
            config: None,
            tool_name: "codex",
            session_id: "projection-test",
            project_root: project_root.path(),
            stream_mode: csa_process::StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: None,
            no_fs_sandbox: false,
            allow_user_daemon_ipc: false,
            readonly_project_root: false,
            extra_writable: &[],
            extra_readable: &[],
            execution_env: None,
        },
        resource_overrides,
        ResourceCapability::CgroupV2,
        csa_resource::FilesystemCapability::Landlock,
    ) {
        crate::pipeline_sandbox::SandboxResolution::Ok(options) => options,
        crate::pipeline_sandbox::SandboxResolution::RequiredButUnavailable(message) => {
            panic!("default Codex plan should resolve: {message}")
        }
    };

    assert_eq!(
        options
            .sandbox
            .as_ref()
            .expect("sandbox plan")
            .isolation_plan
            .resource,
        ResourceCapability::Setrlimit,
        "Landlock degrades the detected cgroup plan to setrlimit"
    );
    assert_eq!(
        spawn_memory_projection_mb_for_physical_available(
            Some("run"),
            None,
            "codex",
            resource_overrides,
            crate::pipeline_sandbox::resource_capability_for_spawn_admission(&options),
            8_377,
        ),
        8_277,
        "the final recheck must not apply a cgroup-only writer floor to Setrlimit"
    );
}
