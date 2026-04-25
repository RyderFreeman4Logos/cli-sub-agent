use super::*;
use std::ffi::OsString;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<OsString>,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: tests that mutate process environment hold ENV_LOCK, so no
        // other test in this module observes a concurrent environment change.
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, previous }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        // SAFETY: the guard is only used while ENV_LOCK is held, preserving
        // exclusive access to process environment mutations for these tests.
        unsafe {
            if let Some(value) = &self.previous {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

#[test]
fn test_builder_best_effort_with_bwrap() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .build()
        .expect("BestEffort with Bwrap should succeed");

    assert_eq!(plan.resource, ResourceCapability::CgroupV2);
    assert_eq!(plan.filesystem, FilesystemCapability::Bwrap);
    assert!(plan.degraded_reasons.is_empty());
}

#[test]
fn test_builder_degrades_cgroup_landlock_to_setrlimit() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_filesystem_capability(FilesystemCapability::Landlock)
        .build()
        .expect("best-effort build should succeed");

    assert_eq!(plan.resource, ResourceCapability::Setrlimit);
    assert_eq!(plan.filesystem, FilesystemCapability::Landlock);
    assert!(
        plan.degraded_reasons
            .iter()
            .any(|reason| reason.contains("landlock cannot be combined with cgroup wrapper")),
        "expected degradation reason, got {:?}",
        plan.degraded_reasons
    );
}

#[test]
fn test_builder_best_effort_degradation() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_resource_capability(ResourceCapability::None)
        .with_filesystem_capability(FilesystemCapability::None)
        .build()
        .expect("BestEffort should never fail");

    assert_eq!(plan.filesystem, FilesystemCapability::None);
    assert_eq!(plan.degraded_reasons.len(), 2);
    assert!(plan.degraded_reasons[0].contains("filesystem"));
    assert!(plan.degraded_reasons[1].contains("resource"));
}

#[test]
fn test_builder_required_fails_without_capability() {
    let result = IsolationPlanBuilder::new(EnforcementMode::Required)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .with_filesystem_capability(FilesystemCapability::None)
        .build();

    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("filesystem isolation required"),
        "unexpected error: {msg}"
    );
}

#[test]
fn test_builder_off_forces_none() {
    let plan = IsolationPlanBuilder::new(EnforcementMode::Off)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_resource_capability(ResourceCapability::CgroupV2)
        .build()
        .expect("Off mode should always succeed");

    assert_eq!(
        plan.filesystem,
        FilesystemCapability::None,
        "Off mode must force filesystem to None"
    );
    // Resource capability is kept as-is (Off only governs filesystem).
    assert_eq!(plan.resource, ResourceCapability::CgroupV2);
}

#[test]
fn test_tool_defaults_claude_code() {
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));
    assert!(
        !plan
            .writable_paths
            .contains(&PathBuf::from(DEFAULT_SANDBOX_TMPDIR)),
        "bwrap uses a private tmpfs /tmp and must not bind the host /tmp"
    );
    assert_eq!(
        plan.env_overrides.get("TMPDIR").map(String::as_str),
        Some(DEFAULT_SANDBOX_TMPDIR),
        "bwrap-backed sessions should pin TMPDIR to the sandbox-private /tmp"
    );

    if let Some(home) = home_dir() {
        // Tool config dir is only added if it exists on disk (matches
        // production behavior in isolation_plan.rs:286 `if p.exists()`).
        let claude_dir = home.join(".claude");
        if claude_dir.exists() {
            assert!(
                plan.writable_paths.contains(&claude_dir),
                "claude-code defaults should include ~/.claude when it exists"
            );
        }
        // Common paths: XDG_STATE_HOME and mise cache (gated on existence to
        // match production code at isolation_plan.rs:219,227).
        let xdg_state = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/state"));
        if xdg_state.exists() {
            assert!(
                plan.writable_paths.contains(&xdg_state),
                "all tools should include XDG_STATE_HOME for cargo proc-macro compilation"
            );
        }
        let mise_cache = home.join(".cache/mise");
        if mise_cache.exists() {
            assert!(
                plan.writable_paths.contains(&mise_cache),
                "all tools should include ~/.cache/mise for mise-managed toolchains"
            );
        }

        // Cargo home: when CARGO_HOME is set to a non-default dir, only
        // CARGO_HOME is added (not ~/.cargo, which may contain credentials).
        let default_cargo_home = home.join(".cargo");
        if let Ok(cargo_home_env) = std::env::var("CARGO_HOME") {
            let cargo_home = PathBuf::from(&cargo_home_env);
            if cargo_home != default_cargo_home {
                assert!(
                    !plan.writable_paths.contains(&default_cargo_home),
                    "~/.cargo must NOT be writable when CARGO_HOME differs"
                );
                if cargo_home.exists() || cargo_home.parent().is_some_and(|p| p.exists()) {
                    assert!(
                        plan.writable_paths.contains(&cargo_home),
                        "CARGO_HOME should be writable"
                    );
                }
            } else if default_cargo_home.exists() {
                assert!(
                    plan.writable_paths.contains(&default_cargo_home),
                    "~/.cargo should be writable when CARGO_HOME equals default"
                );
            }
        } else if default_cargo_home.exists() {
            assert!(
                plan.writable_paths.contains(&default_cargo_home),
                "~/.cargo should be writable when CARGO_HOME is unset"
            );
        }
    }
}

#[test]
fn test_tool_defaults_landlock_uses_session_tmpdir() {
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Landlock)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));
    assert!(
        !plan
            .writable_paths
            .contains(&PathBuf::from(DEFAULT_SANDBOX_TMPDIR)),
        "landlock must not grant host /tmp as writable just to satisfy TMPDIR"
    );
    assert_eq!(
        plan.env_overrides.get("TMPDIR"),
        Some(&session.join("tmp").to_string_lossy().into_owned()),
        "landlock-backed sessions should pin TMPDIR to a session-owned tmp dir"
    );
}

#[test]
fn test_cargo_and_rustup_paths_presence_matches_filesystem() {
    // Verify that cargo/rustup paths are included correctly based on env
    // vars and filesystem state.  This test runs against the real HOME.
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    if let Some(home) = home_dir() {
        let default_cargo_home = home.join(".cargo");

        if let Ok(cargo_home_env) = std::env::var("CARGO_HOME") {
            let cargo_home = PathBuf::from(&cargo_home_env);
            if cargo_home == default_cargo_home {
                // CARGO_HOME == default: treated as if unset
                assert!(
                    plan.writable_paths.contains(&default_cargo_home)
                        || !default_cargo_home.exists()
                            && !default_cargo_home.parent().is_some_and(|p| p.exists()),
                    "default cargo home should be included when CARGO_HOME equals default"
                );
            } else {
                // CARGO_HOME differs: only CARGO_HOME should be writable,
                // NOT ~/.cargo (avoids leaking credentials).
                assert!(
                    !plan.writable_paths.contains(&default_cargo_home),
                    "~/.cargo must NOT be writable when CARGO_HOME is set elsewhere"
                );
                if cargo_home.exists() || cargo_home.parent().is_some_and(|p| p.exists()) {
                    assert!(
                        plan.writable_paths.contains(&cargo_home),
                        "CARGO_HOME should be writable when it (or parent) exists"
                    );
                }
            }
        } else {
            // No CARGO_HOME: default path used
            assert!(
                plan.writable_paths.contains(&default_cargo_home)
                    || !default_cargo_home.exists()
                        && !default_cargo_home.parent().is_some_and(|p| p.exists()),
                "default cargo home should be included when CARGO_HOME is unset"
            );
        }

        // RUSTUP_HOME: same pattern as CARGO_HOME
        let default_rustup = home.join(".rustup");
        if let Ok(rustup_home_env) = std::env::var("RUSTUP_HOME") {
            let rustup_path = PathBuf::from(&rustup_home_env);
            if rustup_path == default_rustup {
                assert!(
                    plan.writable_paths.contains(&default_rustup)
                        || !default_rustup.exists()
                            && !default_rustup.parent().is_some_and(|p| p.exists()),
                    "default rustup should be included when RUSTUP_HOME equals default"
                );
            } else {
                assert!(
                    !plan.writable_paths.contains(&default_rustup),
                    "~/.rustup must NOT be writable when RUSTUP_HOME is set elsewhere"
                );
            }
        } else {
            assert!(
                plan.writable_paths.contains(&default_rustup)
                    || !default_rustup.exists()
                        && !default_rustup.parent().is_some_and(|p| p.exists()),
                "default rustup should be included when RUSTUP_HOME is unset"
            );
        }
    }
}

#[test]
fn test_submodule_detection_adds_superproject_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let superproject = tmp.path().join("monorepo");
    let submodule = superproject.join("crates").join("sub-crate");

    // Superproject has a .git directory
    std::fs::create_dir_all(superproject.join(".git")).expect("create .git dir");
    // Submodule has a .git file (not directory)
    std::fs::create_dir_all(&submodule).expect("create submodule dir");
    std::fs::write(
        submodule.join(".git"),
        "gitdir: ../../.git/modules/crates/sub-crate\n",
    )
    .expect("write .git file");

    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).expect("create session dir");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &submodule, &session)
        .build()
        .expect("should succeed");

    assert!(
        plan.writable_paths.contains(&superproject),
        "superproject root should be in writable_paths, got: {:?}",
        plan.writable_paths
    );
    assert!(
        plan.writable_paths.contains(&submodule),
        "submodule (project_root) should still be in writable_paths"
    );
}

#[test]
fn test_non_submodule_does_not_add_superproject() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().join("project");

    // Normal repo: .git is a directory
    std::fs::create_dir_all(project.join(".git")).expect("create .git dir");

    let session = tmp.path().join("session");
    std::fs::create_dir_all(&session).expect("create session dir");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    // project + session should be present (no superproject)
    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));
    // Superproject should NOT be present
    assert!(
        !plan.writable_paths.contains(&tmp.path().to_path_buf()),
        "non-submodule should not add superproject root"
    );
}

#[test]
fn test_submodule_no_superproject_found() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let orphan = tmp.path().join("orphan");

    // .git is a file but no ancestor has a .git directory
    std::fs::create_dir_all(&orphan).expect("create dir");
    std::fs::write(orphan.join(".git"), "gitdir: ../somewhere\n").expect("write .git file");

    let result = detect_superproject_root(&orphan);
    assert!(
        result.is_none(),
        "should return None when no superproject found"
    );
}

#[test]
fn test_tool_defaults_codex() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let codex_home = temp.path().join("codex-home");
    let _codex_home_env = ScopedEnvVar::set("CODEX_HOME", &codex_home);
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("codex", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));

    assert!(
        codex_home.is_dir(),
        "codex defaults should pre-create CODEX_HOME"
    );
    assert!(
        plan.writable_paths.contains(&codex_home),
        "codex defaults should include CODEX_HOME"
    );

    if let Some(home) = home_dir() {
        // Common paths: XDG_STATE_HOME and mise cache (gated on existence to
        // match production code at isolation_plan.rs:219,227).
        let xdg_state = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/state"));
        if xdg_state.exists() {
            assert!(
                plan.writable_paths.contains(&xdg_state),
                "all tools should include XDG_STATE_HOME for cargo proc-macro compilation"
            );
        }
        let mise_cache = home.join(".cache/mise");
        if mise_cache.exists() {
            assert!(
                plan.writable_paths.contains(&mise_cache),
                "all tools should include ~/.cache/mise for mise-managed toolchains"
            );
        }
    }
}

#[test]
fn test_tool_defaults_codex_honors_codex_home_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let codex_home = temp.path().join("custom-codex-home");
    let _codex_home_env = ScopedEnvVar::set("CODEX_HOME", &codex_home);

    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("codex", &project, &session)
        .build()
        .expect("should succeed");

    assert!(
        codex_home.is_dir(),
        "codex defaults should pre-create CODEX_HOME"
    );
    assert!(
        plan.writable_paths.contains(&codex_home),
        "codex defaults should include CODEX_HOME"
    );
}

#[test]
fn test_tool_defaults_codex_rejects_unwritable_codex_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
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
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("gemini-cli", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));

    if let Some(home) = home_dir() {
        // Tool config dirs are only added if they exist on disk (matches
        // production behavior in isolation_plan.rs:286 `if p.exists()`).
        let gemini_dir = home.join(".gemini");
        if gemini_dir.exists() {
            assert!(
                plan.writable_paths.contains(&gemini_dir),
                "gemini-cli defaults should include ~/.gemini when it exists"
            );
        }
        let gemini_config_dir = home.join(".config/gemini-cli");
        if gemini_config_dir.exists() {
            assert!(
                plan.writable_paths.contains(&gemini_config_dir),
                "gemini-cli defaults should include ~/.config/gemini-cli when it exists"
            );
        }
        // mise cache is a common path for all tools (gated on existence to
        // match production code at isolation_plan.rs:227).
        let mise_cache = home.join(".cache/mise");
        if mise_cache.exists() {
            assert!(
                plan.writable_paths.contains(&mise_cache),
                "all tools should include ~/.cache/mise for mise-managed toolchains"
            );
        }
    }
}

#[test]
fn test_tool_defaults_opencode() {
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("opencode", &project, &session)
        .build()
        .expect("should succeed");

    assert!(plan.writable_paths.contains(&project));
    assert!(plan.writable_paths.contains(&session));

    if let Some(home) = home_dir() {
        // Tool config dir is only added if it exists on disk (matches
        // production behavior in isolation_plan.rs:286 `if p.exists()`).
        let opencode_dir = home.join(".config/opencode");
        if opencode_dir.exists() {
            assert!(
                plan.writable_paths.contains(&opencode_dir),
                "opencode defaults should include ~/.config/opencode when it exists"
            );
        }
        // Common paths: XDG_STATE_HOME and mise cache (gated on existence to
        // match production code at isolation_plan.rs:219,227).
        let xdg_state = std::env::var("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local/state"));
        if xdg_state.exists() {
            assert!(
                plan.writable_paths.contains(&xdg_state),
                "all tools should include XDG_STATE_HOME for cargo proc-macro compilation"
            );
        }
        let mise_cache = home.join(".cache/mise");
        if mise_cache.exists() {
            assert!(
                plan.writable_paths.contains(&mise_cache),
                "all tools should include ~/.cache/mise for mise-managed toolchains"
            );
        }
    }
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
    if let Some(home) = home_dir() {
        let result = validate_writable_paths(&[home.join("workspace")], Path::new("/tmp/project"));
        assert!(result.is_ok());
    }
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
