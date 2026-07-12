use super::*;
use std::ffi::OsString;

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

    fn unset(key: &'static str) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: tests that mutate process environment hold ENV_LOCK, so no
        // other test in this module observes a concurrent environment change.
        unsafe {
            std::env::remove_var(key);
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

fn isolated_home(temp: &tempfile::TempDir) -> (PathBuf, [ScopedEnvVar; 6]) {
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("create isolated HOME");
    (
        home.clone(),
        [
            ScopedEnvVar::set("HOME", &home),
            ScopedEnvVar::unset("XDG_STATE_HOME"),
            ScopedEnvVar::unset("CARGO_HOME"),
            ScopedEnvVar::unset("RUSTUP_HOME"),
            ScopedEnvVar::unset("CODEX_HOME"),
            ScopedEnvVar::unset("CLAUDE_CONFIG_DIR"),
        ],
    )
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
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".local/state")).unwrap();
    std::fs::create_dir_all(home.join(".cache/mise")).unwrap();
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
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

    assert!(
        plan.writable_paths.contains(&home.join(".claude")),
        "claude-code defaults should include isolated ~/.claude"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".local/state")),
        "all tools should include existing XDG_STATE_HOME default"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".cache/mise")),
        "all tools should include existing ~/.cache/mise"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".cargo")),
        "default cargo home should be writable when CARGO_HOME is unset"
    );
}

#[test]
fn test_tool_defaults_landlock_uses_session_tmpdir() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
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
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    std::fs::create_dir_all(home.join(".cargo")).unwrap();
    std::fs::create_dir_all(home.join(".rustup")).unwrap();
    let project = PathBuf::from("/tmp/project");
    let session = PathBuf::from("/tmp/session");

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("claude-code", &project, &session)
        .build()
        .expect("should succeed");

    assert!(
        plan.writable_paths.contains(&home.join(".cargo")),
        "default cargo home should be included when CARGO_HOME is unset"
    );
    assert!(
        plan.writable_paths.contains(&home.join(".rustup")),
        "default rustup should be included when RUSTUP_HOME is unset"
    );
}

#[test]
fn test_submodule_detection_adds_superproject_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _guard = ENV_LOCK.lock().unwrap();
    let (_home, _env) = isolated_home(&tmp);
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
    let _guard = ENV_LOCK.lock().unwrap();
    let (_home, _env) = isolated_home(&tmp);
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
    let (_home, _env) = isolated_home(&temp);
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

    assert!(
        plan.writable_paths.contains(&codex_home),
        "codex defaults should include CODEX_HOME under an isolated HOME"
    );
}

#[test]
fn test_tool_defaults_codex_honors_codex_home_env() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
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

include!("isolation_plan_tests_tail.rs");
