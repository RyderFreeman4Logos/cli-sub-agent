// ---------------------------------------------------------------------------

/// Per-tool writable_paths replaces project root: project root becomes read-only,
/// and only the specified paths are writable.
#[test]
fn test_per_tool_writable_paths_replace_project_root() {
    let cfg = parse_project_config(
        r#"
[tools.gemini-cli]
enabled = true
memory_max_mb = 2048

[tools.gemini-cli.filesystem_sandbox]
writable_paths = ["/tmp"]
"#,
    );

    let project_root = current_project_root();
    let result = resolve_sandbox_options_with_capabilities(
        SandboxResolveInput {
            config: Some(&cfg),
            tool_name: "gemini-cli",
            session_id: "test-session",
            project_root: &project_root,
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
        RunResourceOverrides::absent(),
        csa_resource::ResourceCapability::Setrlimit,
        csa_resource::FilesystemCapability::Bwrap,
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    let ctx = opts
        .sandbox
        .as_ref()
        .expect("Expected SandboxContext with per-tool writable_paths");
    assert_eq!(
        ctx.isolation_plan.resource,
        csa_resource::ResourceCapability::Setrlimit,
        "resolver must use the injected resource capability"
    );
    assert_eq!(
        ctx.isolation_plan.filesystem,
        csa_resource::FilesystemCapability::Bwrap,
        "resolver must use the injected filesystem capability"
    );

    // Project root should be read-only because per-tool writable_paths are set.
    assert!(
        ctx.isolation_plan.readonly_project_root,
        "Per-tool writable_paths should make project root read-only"
    );

    // /tmp should be in the writable paths.
    assert!(
        contains_equivalent_path(&ctx.isolation_plan.writable_paths, Path::new("/tmp")),
        "writable_paths should contain /tmp, got: {:?}",
        ctx.isolation_plan.writable_paths
    );
}

/// readonly_project_root=true (from review/debate config) propagates to IsolationPlan.
#[test]
fn test_readonly_sandbox_from_review_config() {
    // Use a minimal config with memory_max_mb so sandbox resolution proceeds.
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
"#,
    );

    let project_root = current_project_root();
    let result = resolve_sandbox_options_with_capabilities(
        SandboxResolveInput {
            config: Some(&cfg),
            tool_name: "claude-code",
            session_id: "test-session",
            project_root: &project_root,
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            allow_user_daemon_ipc: false,
            readonly_project_root: true,
            extra_writable: &[],
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

    let ctx = opts
        .sandbox
        .as_ref()
        .expect("Expected SandboxContext for heavyweight tool");

    assert!(
        ctx.isolation_plan.readonly_project_root,
        "readonly_project_root=true should propagate to IsolationPlan"
    );
}

/// Path validation rejects dangerous paths like "/" — resolve_sandbox_options
/// should return RequiredButUnavailable.
#[test]
fn test_path_validation_rejects_dangerous_paths() {
    let cfg = parse_project_config(
        r#"
[tools.gemini-cli]
enabled = true
memory_max_mb = 2048

[tools.gemini-cli.filesystem_sandbox]
writable_paths = ["/"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "gemini-cli",
        "test-session",
        &current_project_root(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &[], // extra_writable
        &[], // extra_readable
    );

    assert!(
        matches!(result, SandboxResolution::RequiredButUnavailable(_)),
        "writable_paths = [\"/\"] should be rejected as dangerous, got Ok"
    );
}

/// Per-tool enforcement_mode override: tool-level enforcement overrides global.
#[test]
fn test_per_tool_enforcement_mode_override() {
    // Global filesystem_sandbox enforcement = off, but tool-level = best-effort.
    // The tool-level override should take effect.
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048

[filesystem_sandbox]
enforcement_mode = "off"

[tools.claude-code]
enabled = true

[tools.claude-code.filesystem_sandbox]
enforcement_mode = "best-effort"
"#,
    );

    // Verify the config-level resolution returns the tool-level override.
    let fs_mode = cfg.tool_fs_enforcement_mode("claude-code");
    assert_eq!(
        fs_mode.as_deref(),
        Some("best-effort"),
        "Tool-level FS enforcement should override global 'off'"
    );
}

#[test]
fn test_cross_repo_cd_uses_explicit_project_root_for_sandbox_plan() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let invocation_cwd = tempfile::tempdir().expect("invocation cwd tempdir");
    let target_project = tempfile::tempdir().expect("target project tempdir");
    let _cwd_guard = CurrentDirGuard::change_to(invocation_cwd.path());

    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "gemini-cli",
        "test-session",
        target_project.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &[],
        &[],
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    let sandbox = opts
        .sandbox
        .as_ref()
        .expect("Expected SandboxContext for heavyweight tool");

    assert_eq!(
        sandbox.isolation_plan.project_root,
        Some(target_project.path().to_path_buf()),
        "sandbox must target the explicit --cd project root, not process cwd"
    );
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&target_project.path().to_path_buf()),
        "target project root should be writable in the sandbox plan"
    );
    assert!(
        !sandbox
            .isolation_plan
            .writable_paths
            .contains(&invocation_cwd.path().to_path_buf()),
        "invocation cwd must not leak into cross-repo sandbox writable paths"
    );
}

/// Verify that resolve_sandbox_options injects CSA state paths (project state
/// root and global slots) into the isolation plan's writable_paths.
#[test]
fn test_csa_state_paths_in_writable_paths() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

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
            extra_writable: &[],
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
        .as_ref()
        .expect("injected capabilities must produce a deterministic sandbox context");

    let writable = &sandbox.isolation_plan.writable_paths;

    // Project state root should be present (allows fork-call session creation).
    if let Ok(project_state_root) = csa_session::manager::get_session_root(project_root.path()) {
        assert!(
            writable.contains(&project_state_root),
            "writable_paths should include project state root: {project_state_root:?}\n  actual: {writable:?}"
        );
    }

    // Global slots directory should be present (allows lock file creation).
    if let Ok(slots) = csa_config::GlobalConfig::slots_dir() {
        assert!(
            writable.contains(&slots),
            "writable_paths should include slots dir: {slots:?}\n  actual: {writable:?}"
        );
    }
}

/// Verify that CSA state paths are present even when per-tool REPLACE
/// semantics restrict project-root writability.
#[test]
fn test_csa_state_paths_survive_replace_semantics() {
    let project_root = current_project_root();
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"

[tools.claude-code.filesystem_sandbox]
writable_paths = ["/tmp/restricted-only"]
"#,
    );

    let result = resolve_sandbox_options_with_capabilities(
        SandboxResolveInput {
            config: Some(&cfg),
            tool_name: "claude-code",
            session_id: "test-session",
            project_root: &project_root,
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
        RunResourceOverrides::absent(),
        csa_resource::ResourceCapability::Setrlimit,
        csa_resource::FilesystemCapability::Bwrap,
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    let sandbox = opts
        .sandbox
        .as_ref()
        .expect("injected capabilities must produce a deterministic sandbox context");

    let writable = &sandbox.isolation_plan.writable_paths;

    // Project root should NOT be writable (REPLACE semantics make it readonly).
    assert!(
        sandbox.isolation_plan.readonly_project_root,
        "REPLACE semantics should set readonly_project_root"
    );

    // But CSA state paths should still be present.
    if let Ok(project_state_root) = csa_session::manager::get_session_root(&project_root) {
        assert!(
            contains_equivalent_path(writable, &project_state_root),
            "CSA state root must survive REPLACE semantics"
        );
    }
    if let Ok(slots) = csa_config::GlobalConfig::slots_dir() {
        assert!(
            contains_equivalent_path(writable, &slots),
            "slots dir must survive REPLACE semantics"
        );
    }

    // Per-tool restricted path should also be present.
    assert!(
        contains_equivalent_path(writable, Path::new("/tmp/restricted-only")),
        "per-tool writable path should be present"
    );
}

#[test]
fn test_extra_readable_appended_to_isolation_plan() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let temp = tempfile::tempdir_in(project_root.path()).expect("tempdir");
    let first = temp.path().join("foo.json");
    let second = temp.path().join("bar.txt");
    std::fs::write(&first, "{}").expect("write first readable file");
    std::fs::write(&second, "bar").expect("write second readable file");
    let readable = vec![first.clone(), second.clone()];

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
            extra_writable: &[],
            extra_readable: &readable,
            execution_env: None,
        },
        RunResourceOverrides::absent(),
        csa_resource::ResourceCapability::Setrlimit,
        csa_resource::FilesystemCapability::Bwrap,
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    let sandbox = opts.sandbox.expect("expected deterministic sandbox context");

    assert_eq!(
        sandbox.isolation_plan.readable_paths.len(),
        2,
        "expected two readable paths in the isolation plan"
    );
    assert!(sandbox.isolation_plan.readable_paths.contains(&first));
    assert!(sandbox.isolation_plan.readable_paths.contains(&second));
}

#[test]
fn test_sandbox_context_none_when_enforcement_off() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "off"
"#,
    );

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
            extra_writable: &[],
            extra_readable: &[],
            execution_env: None,
        },
        RunResourceOverrides::absent(),
        csa_resource::ResourceCapability::CgroupV2,
        csa_resource::FilesystemCapability::Bwrap,
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("resource enforcement off should resolve successfully");
    };
    assert!(
        opts.sandbox.is_none(),
        "resource enforcement off must not construct a sandbox context"
    );
}

#[test]
fn test_bwrap_plan_includes_all_writable_paths() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let extra_writable = project_root.path().join("extra-writable");
    std::fs::create_dir_all(&extra_writable).expect("extra writable directory");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

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
            extra_writable: std::slice::from_ref(&extra_writable),
            extra_readable: &[],
            execution_env: None,
        },
        RunResourceOverrides::absent(),
        csa_resource::ResourceCapability::Setrlimit,
        csa_resource::FilesystemCapability::Bwrap,
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("injected Bubblewrap capability should resolve successfully");
    };
    let sandbox = opts.sandbox.expect("expected deterministic sandbox context");
    assert_eq!(
        sandbox.isolation_plan.filesystem,
        csa_resource::FilesystemCapability::Bwrap
    );
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&project_root.path().canonicalize().expect("project root")),
        "Bubblewrap plan should retain the writable project root"
    );
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&extra_writable.canonicalize().expect("extra writable")),
        "Bubblewrap plan should retain every explicit writable path"
    );
}
