#[test]
fn test_tool_defaults_codex_honors_tool_state_dirs_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    let configured = PathBuf::from("~/.state/codex");
    let expected = home.join(".state/codex");
    let tool_state_dirs = std::collections::HashMap::from([("codex".to_string(), configured)]);

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults_and_state_dirs("codex", &project, &session, Some(&tool_state_dirs))
        .build()
        .expect("configured codex state dir should build");

    assert!(
        expected.is_dir(),
        "configured codex state dir should be pre-created"
    );
    assert!(
        plan.writable_paths.contains(&expected),
        "codex defaults should include configured tool_state_dirs.codex"
    );
    assert!(
        !plan.writable_paths.contains(&home.join(".codex")),
        "configured codex state dir should replace the hardcoded default"
    );
}

#[test]
fn test_tool_defaults_hermes_writes_runtime_but_protects_configuration() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("active-hermes-profile");
    let logs = hermes_home.join("logs");
    let config = hermes_home.join("config.yaml");
    let profiles = hermes_home.join("profiles");
    let state_db = hermes_home.join("state.db");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(&config, "model: test\n").unwrap();
    std::fs::write(&state_db, "").unwrap();
    let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("hermes", Path::new("/tmp/project"), Path::new("/tmp/session"))
        .build()
        .expect("Hermes runtime paths should produce a sandbox plan");

    assert!(
        plan.writable_paths.contains(&hermes_home),
        "Hermes home must be writable so SQLite can manage state.db sidecars"
    );
    assert!(
        plan.readable_paths.iter().any(|path| path == &config),
        "Hermes config must be re-bound read-only"
    );
    assert!(
        plan.readable_paths.iter().any(|path| path == &profiles),
        "unrelated Hermes profiles must be re-bound read-only"
    );
    assert!(
        plan.readable_paths
            .iter()
            .all(|path| path != &logs && path != &state_db),
        "Hermes logs and state database must remain writable"
    );

    let command = crate::from_isolation_plan(&plan, "/usr/bin/tool", &[])
        .expect("Bubblewrap plan should produce a command");
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let hermes_home = hermes_home.to_string_lossy();
    let config = config.to_string_lossy();
    let writable_home = args
        .windows(3)
        .position(|window| window == ["--bind", hermes_home.as_ref(), hermes_home.as_ref()])
        .expect("Hermes home must be mounted writable");
    let readonly_config = args
        .windows(3)
        .position(|window| window == ["--ro-bind", config.as_ref(), config.as_ref()])
        .expect("Hermes config must override the writable parent with a read-only mount");
    assert!(
        writable_home < readonly_config,
        "read-only Hermes configuration mounts must follow the writable runtime parent"
    );

    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Landlock)
        .with_tool_defaults("hermes", Path::new("/tmp/project"), Path::new("/tmp/session"))
        .build()
        .expect_err("Landlock cannot safely grant Hermes SQLite parent-directory writes");
    assert!(
        error.to_string().contains("hermes sandbox preflight failed"),
        "unsupported Hermes filesystem isolation must fail closed: {error:#}"
    );
}

#[test]
fn test_tool_defaults_hermes_rejects_landlock_when_project_root_grants_parent() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let project = temp.path().join("project");
    let hermes_home = project.join(".hermes");
    std::fs::create_dir_all(&hermes_home).unwrap();
    let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Landlock)
        .with_tool_defaults("hermes", &project, &temp.path().join("session"))
        .build()
        .expect_err("Landlock must reject Hermes homes under writable parent grants");

    assert!(error.to_string().contains("hermes sandbox preflight failed"));
}

#[test]
fn test_tool_defaults_hermes_uses_execution_environment_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    let _hermes_home_env = ScopedEnvVar::unset("HERMES_HOME");
    let configured_home = temp.path().join("configured-hermes-home");
    std::fs::create_dir_all(&configured_home).unwrap();
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        configured_home.to_string_lossy().into_owned(),
    )]);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults("hermes", &temp.path().join("project"), &temp.path().join("session"))
        .build()
        .expect("configured execution environment should build a Hermes plan");

    assert!(plan.writable_paths.contains(&configured_home));
    assert!(!plan.writable_paths.contains(&home.join(".hermes")));
}

#[cfg(unix)]
#[test]
fn test_tool_defaults_hermes_rejects_symlinked_configuration_overlays() {
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-overlay-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);

    for (name, target) in [("live", Some("config-target.yaml")), ("dangling", None)] {
        let hermes_home = temp.path().join(name);
        std::fs::create_dir_all(&hermes_home).unwrap();
        let link = hermes_home.join("config.yaml");
        if let Some(target) = target {
            let target = hermes_home.join(target);
            std::fs::write(&target, "model: test\n").unwrap();
            symlink(&target, &link).unwrap();
        } else {
            symlink(hermes_home.join("missing.yaml"), &link).unwrap();
        }
        let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

        let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_tool_defaults("hermes", &temp.path().join("project"), &temp.path().join("session"))
            .build()
            .expect_err("symlinked Hermes configuration overlays must fail preflight");
        assert!(error.to_string().contains("hermes sandbox preflight failed"));
    }
}

#[test]
fn test_tool_defaults_codex_rejects_unwritable_codex_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let codex_home = temp.path().join("readonly-codex-home");
    std::fs::create_dir(&codex_home).unwrap();
    #[cfg(unix)]
    let original_mode = {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(&codex_home).unwrap();
        let original_mode = metadata.permissions().mode();
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o500);
        std::fs::set_permissions(&codex_home, permissions).unwrap();
        original_mode
    };
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(&codex_home).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&codex_home, permissions).unwrap();
    }

    let _codex_home_env = ScopedEnvVar::set("CODEX_HOME", &codex_home);

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("codex", &project, &session)
        .build()
        .expect_err("unwritable CODEX_HOME should fail preflight");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = std::fs::metadata(&codex_home).unwrap().permissions();
        permissions.set_mode(original_mode);
        std::fs::set_permissions(&codex_home, permissions).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut permissions = std::fs::metadata(&codex_home).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&codex_home, permissions).unwrap();
    }

    let message = format!("{error:#}");
    assert!(message.contains("codex sandbox preflight failed"));
    assert!(message.contains("CODEX_HOME"));
    assert!(message.contains("[tools.codex].filesystem_sandbox.writable_paths"));
}

#[test]
fn test_parent_tool_defaults_expose_existing_codex_home_for_nested_csa() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let codex_home = temp.path().join("codex-home");
    std::fs::create_dir(&codex_home).unwrap();
    let _codex_home_env = ScopedEnvVar::set("CODEX_HOME", &codex_home);

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    // Non-codex tools only expose CODEX_HOME when codex is on PATH (existence
    // gating).  Skip the assertion in CI where codex may not be installed.
    if codex_paths::has_codex_on_path() {
        assert!(
            plan.writable_paths.contains(&codex_home),
            "parent sandboxes should expose existing Codex home for nested Codex CSA sessions"
        );
    } else {
        assert!(
            !plan.writable_paths.contains(&codex_home),
            "without codex on PATH, parent sandboxes should NOT expose Codex home"
        );
    }
}

#[test]
fn test_tool_defaults_gemini_cli() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    std::fs::create_dir_all(home.join(".gemini")).unwrap();
    std::fs::create_dir_all(home.join(".config/gemini-cli")).unwrap();
    std::fs::create_dir_all(home.join(".cache/mise")).unwrap();
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("gemini-cli", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));

    assert!(
        plan.writable_paths.contains(&home.join(".gemini")),
        "gemini-cli defaults should include existing ~/.gemini"
    );
    assert!(
        plan.writable_paths
            .contains(&home.join(".config/gemini-cli")),
        "gemini-cli defaults should include existing ~/.config/gemini-cli"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".cache/mise")),
        "all tools should include existing ~/.cache/mise"
    );
}

#[test]
fn test_tool_defaults_opencode() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    std::fs::create_dir_all(home.join(".config/opencode")).unwrap();
    std::fs::create_dir_all(home.join(".local/state")).unwrap();
    std::fs::create_dir_all(home.join(".cache/mise")).unwrap();
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("opencode", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));

    assert!(
        plan.writable_paths.contains(&home.join(".config/opencode")),
        "opencode defaults should include existing ~/.config/opencode"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".local/state")),
        "all tools should include existing XDG_STATE_HOME default"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".cache/mise")),
        "all tools should include existing ~/.cache/mise"
    );
}

// -----------------------------------------------------------------------
// validate_writable_paths tests
// -----------------------------------------------------------------------

#[test]
fn test_validate_rejects_root_path() {
    let result = validate_writable_paths(&[PathBuf::from("/")], Path::new("/tmp/project"));
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("rejected paths"), "unexpected error: {msg}");
}

#[test]
fn test_validate_rejects_etc() {
    let result =
        validate_writable_paths(&[PathBuf::from("/etc/shadow")], Path::new("/tmp/project"));
    assert!(result.is_err());
}

#[test]
fn test_validate_rejects_usr() {
    let result = validate_writable_paths(&[PathBuf::from("/usr/local")], Path::new("/tmp/project"));
    assert!(result.is_err());
}

#[test]
fn test_validate_accepts_tmp_subpath() {
    let result = validate_writable_paths(&[PathBuf::from("/tmp/foo")], Path::new("/project"));
    assert!(result.is_ok());
}

#[test]
fn test_validate_accepts_home_subpath() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    let result = validate_writable_paths(&[home.join("workspace")], Path::new("/tmp/project"));
    assert!(result.is_ok());
}

#[test]
fn test_validate_accepts_project_root_subpath() {
    let project = PathBuf::from("/opt/myproject");
    let result = validate_writable_paths(&[PathBuf::from("/opt/myproject/src")], &project);
    assert!(result.is_ok());
}

#[test]
fn test_validate_mixed_accepted_and_rejected() {
    let result = validate_writable_paths(
        &[PathBuf::from("/tmp/ok"), PathBuf::from("/var/bad")],
        Path::new("/tmp/project"),
    );
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("/var/bad"));
}

// ── Security audit scenarios ───────────────────────────────────────

#[test]
fn test_validate_rejects_relative_path_traversal() {
    // Scenario 3: ../../../etc should be rejected even as relative path
    let result =
        validate_writable_paths(&[PathBuf::from("../../../etc")], Path::new("/tmp/project"));
    assert!(
        result.is_err(),
        "relative path traversal to /etc must be rejected"
    );
}

#[test]
fn test_validate_empty_writable_paths_is_ok() {
    // Scenario 5: empty writable_paths = [] means only session_dir and
    // tool config dir are writable (handled by IsolationPlanBuilder separately)
    let result = validate_writable_paths(&[], Path::new("/tmp/project"));
    assert!(
        result.is_ok(),
        "empty writable_paths should be valid (no user paths to validate)"
    );
}

// -----------------------------------------------------------------------
// readonly_project_root tests
// -----------------------------------------------------------------------

#[test]
fn test_readonly_project_root_default_false() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::Off)
        .build()
        .expect("should succeed");
    assert!(!plan.readonly_project_root);
}

#[test]
fn test_readonly_project_root_propagates() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::Off)
        .with_readonly_project_root(true)
        .build()
        .expect("should succeed");
    assert!(plan.readonly_project_root);
}

#[test]
fn test_with_tool_defaults_stores_project_root() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    assert_eq!(plan.project_root, Some(project));
}

#[test]
fn test_add_dir_or_creatable_parent_existing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let existing = tmp.path().join("already_here");
    std::fs::create_dir(&existing).unwrap();

    let mut paths = Vec::new();
    add_dir_or_creatable_parent(&mut paths, &existing);
    assert_eq!(paths, vec![existing]);
}

#[test]
fn test_add_dir_or_creatable_parent_precreates_missing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("cold_start_dir");
    assert!(!missing.exists(), "precondition: dir must not exist");

    let mut paths = Vec::new();
    add_dir_or_creatable_parent(&mut paths, &missing);

    // The function should pre-create the directory for bwrap --bind
    assert!(missing.exists(), "directory should be pre-created");
    assert_eq!(paths, vec![missing]);
}

#[test]
fn test_add_dir_or_creatable_parent_precreates_nested_dir_when_non_root_ancestor_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let deep = tmp.path().join("no_parent").join("no_child");
    assert!(!deep.exists(), "precondition: nested dir must not exist");

    let mut paths = Vec::new();
    add_dir_or_creatable_parent(&mut paths, &deep);

    assert!(deep.exists(), "nested dir should be pre-created");
    assert_eq!(paths, vec![deep]);
}

#[test]
fn test_add_dir_or_creatable_parent_rejects_sensitive_path() {
    let sensitive = std::path::Path::new("/etc/cargo_home");

    let mut paths = Vec::new();
    add_dir_or_creatable_parent(&mut paths, sensitive);

    assert!(paths.is_empty(), "should reject sensitive system path");
}
