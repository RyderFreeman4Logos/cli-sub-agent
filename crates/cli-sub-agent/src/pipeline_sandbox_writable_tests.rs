use super::*;

#[path = "pipeline_sandbox_extra_writable_tests.rs"]
mod extra_writable_tests;

#[path = "pipeline_sandbox_cache_writable_tests.rs"]
mod cache_writable_tests;

#[test]
fn test_extra_writable_appended_to_isolation_plan() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let extra_dir = project_root.path().join("extra-dir");
    std::fs::create_dir_all(&extra_dir).expect("create extra directory");
    let cfg: csa_config::ProjectConfig = toml::from_str(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    )
    .expect("test TOML should parse");
    let extra = vec![std::path::PathBuf::from("./extra-dir")];

    let result = resolve_sandbox_options_with_capabilities(
        SandboxResolveInput {
            config: Some(&cfg),
            tool_name: "claude-code",
            session_id: "test-session",
            project_root: project_root.path(),
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            allow_user_daemon_ipc: false,
            readonly_project_root: false,
            extra_writable: &extra,
            extra_readable: &[],
            execution_env: None,
        },
        RunResourceOverrides::absent(),
        csa_resource::ResourceCapability::Setrlimit,
        csa_resource::FilesystemCapability::Bwrap,
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };
    let sandbox = opts
        .sandbox
        .expect("expected deterministic sandbox context");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&extra_dir.canonicalize().expect("canonical extra directory")),
        "extra_writable path should be in writable_paths, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
    assert!(
        !sandbox.isolation_plan.readonly_project_root,
        "extra_writable uses APPEND semantics; project root stays writable"
    );
}
