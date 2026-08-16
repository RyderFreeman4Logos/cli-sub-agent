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

#[test]
fn linked_worktree_git_admin_is_writable_only_for_writer_sandboxes() {
    let temp = tempfile::tempdir().expect("linked-worktree fixture");
    let worktree = temp.path().join("linked");
    let common_git_dir = temp.path().join("main/.git");
    let worktree_git_dir = common_git_dir.join("worktrees/linked");
    std::fs::create_dir_all(common_git_dir.join("objects")).expect("common objects");
    std::fs::create_dir_all(common_git_dir.join("refs/heads")).expect("common refs");
    std::fs::create_dir_all(&worktree_git_dir).expect("worktree gitdir");
    std::fs::create_dir_all(&worktree).expect("linked worktree");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", worktree_git_dir.display()),
    )
    .expect("linked .git file");
    std::fs::write(worktree_git_dir.join("commondir"), "../..\n").expect("commondir");
    std::fs::write(worktree_git_dir.join("HEAD"), "ref: refs/heads/linked\n")
        .expect("worktree HEAD");
    std::fs::write(
        worktree_git_dir.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .expect("gitdir backlink");
    std::fs::write(common_git_dir.join("HEAD"), "ref: refs/heads/main\n").expect("common HEAD");
    std::fs::write(
        common_git_dir.join("config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tworktree = {}\n",
            worktree.display()
        ),
    )
    .expect("common config");

    let cfg: csa_config::ProjectConfig = toml::from_str(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    )
    .expect("test TOML should parse");
    let resolve = |readonly_project_root, session_id| {
        let result = resolve_sandbox_options_with_capabilities(
            SandboxResolveInput {
                config: Some(&cfg),
                tool_name: "opencode",
                session_id,
                project_root: &worktree,
                stream_mode: StreamMode::BufferOnly,
                idle_timeout_seconds: 120,
                liveness_dead_seconds: 600,
                initial_response_timeout_seconds: Some(120),
                no_fs_sandbox: false,
                allow_user_daemon_ipc: false,
                readonly_project_root,
                extra_writable: &[],
                extra_readable: &[],
                execution_env: None,
            },
            RunResourceOverrides::absent(),
            csa_resource::ResourceCapability::Setrlimit,
            csa_resource::FilesystemCapability::Bwrap,
        );
        let SandboxResolution::Ok(options) = result else {
            panic!("linked-worktree sandbox should resolve");
        };
        options
            .sandbox
            .expect("sandbox context")
            .isolation_plan
            .writable_paths
    };

    let git_dir = worktree_git_dir.canonicalize().expect("canonical gitdir");
    let common_dir = common_git_dir.canonicalize().expect("canonical common dir");
    let writer_paths = resolve(false, "writer");
    assert!(writer_paths.contains(&git_dir), "missing {git_dir:?}");
    assert!(writer_paths.contains(&common_dir), "missing {common_dir:?}");

    let pre_session = resolve_sandbox_options_with_capability_source(
        SandboxResolveInput {
            config: Some(&cfg),
            tool_name: "opencode",
            session_id: "pre-session",
            project_root: &worktree,
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
        || csa_resource::ResourceCapability::Setrlimit,
        || csa_resource::FilesystemCapability::Bwrap,
        None,
    );
    let SandboxResolution::Ok(options) = pre_session else {
        panic!("linked-worktree pre-session sandbox should resolve");
    };
    let pre_session_paths = options
        .sandbox
        .expect("pre-session sandbox context")
        .isolation_plan
        .writable_paths;
    assert!(pre_session_paths.contains(&git_dir), "missing {git_dir:?}");
    assert!(
        pre_session_paths.contains(&common_dir),
        "missing {common_dir:?}"
    );

    let reader_paths = resolve(true, "reader");
    assert!(!reader_paths.contains(&git_dir));
    assert!(!reader_paths.contains(&common_dir));
}

#[test]
fn no_config_pre_session_linked_worktree_admin_failure_is_actionable() {
    let temp = tempfile::tempdir().expect("linked-worktree fixture");
    let worktree = temp.path().join("linked");
    let missing_git_dir = temp.path().join("main/.git/worktrees/missing");
    std::fs::create_dir_all(&worktree).expect("linked worktree");
    std::fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", missing_git_dir.display()),
    )
    .expect("linked .git file");

    let result = resolve_sandbox_options_with_capability_source(
        SandboxResolveInput {
            config: None,
            tool_name: "claude-code",
            session_id: "pre-session",
            project_root: &worktree,
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
        || csa_resource::ResourceCapability::Setrlimit,
        || csa_resource::FilesystemCapability::Bwrap,
        None,
    );
    let SandboxResolution::RequiredButUnavailable(message) = result else {
        panic!("invalid linked-worktree admin path must fail before session creation");
    };
    assert!(
        message.contains(&missing_git_dir.display().to_string()),
        "missing failing Git admin path: {message}"
    );
    assert!(
        message.contains("--no-fs-sandbox"),
        "missing supported recovery action: {message}"
    );
}
