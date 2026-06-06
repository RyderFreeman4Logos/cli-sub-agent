use std::path::PathBuf;

use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

use super::*;

fn parse_project_config(toml_str: &str) -> csa_config::ProjectConfig {
    toml::from_str(toml_str).expect("test TOML should parse")
}

fn current_project_root() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

/// CLI --extra-writable paths are appended to writable_paths (APPEND semantics).
#[test]
fn test_extra_writable_appended_to_isolation_plan() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let extra_dir = project_root.path().join("extra-dir");
    std::fs::create_dir_all(&extra_dir).expect("create extra dir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let extra = vec![PathBuf::from("./extra-dir")];
    let result = resolve_sandbox_options(
        Some(&cfg),
        "claude-code",
        "test-session",
        project_root.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &extra,
        &[],
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected SandboxResolution::Ok");
    };

    let Some(ref sandbox) = opts.sandbox else {
        return;
    };

    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&extra_dir.canonicalize().unwrap()),
        "extra_writable path should be in writable_paths, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
    assert!(
        !sandbox.isolation_plan.readonly_project_root,
        "extra_writable uses APPEND semantics; project root stays writable"
    );
}

#[test]
fn test_extra_writable_creates_missing_directory_before_sandbox_launch() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let missing = project_root.path().join("missing-extra");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let extra = vec![missing.clone()];
    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &extra,
        &[],
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected missing --extra-writable directory to be created before sandbox launch");
    };
    assert!(
        missing.is_dir(),
        "missing --extra-writable directory should be created"
    );

    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&missing.canonicalize().unwrap()),
        "created directory should be in writable_paths, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}

#[test]
fn test_extra_writable_creates_missing_file_source_before_sandbox_launch() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let state_file = project_root.path().join("state/e2e-test-state.json");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let extra = vec![PathBuf::from("state/e2e-test-state.json")];
    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &extra,
        &[],
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected missing file-like --extra-writable path to be prepared");
    };
    assert!(
        state_file.is_file(),
        "missing file-like --extra-writable path should be pre-created"
    );

    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&state_file.canonicalize().unwrap()),
        "prepared file should remain the writable mount source, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}

#[test]
fn test_run_extra_writable_pre_daemon_validation_creates_missing_file_source() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let state_file = project_root.path().join("nested/state.json");
    let extra = vec![PathBuf::from("nested/state.json")];

    validate_run_extra_writable_sources_exist(None, project_root.path(), false, &extra)
        .expect("missing file-like --extra-writable path should be prepared before daemon spawn");

    assert!(
        state_file.is_file(),
        "pre-daemon validation should create the file source for bwrap"
    );
}

#[test]
fn test_run_extra_writable_pre_daemon_validation_creates_missing_directory() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let missing = project_root.path().join("missing-extra");
    let extra = vec![missing.clone()];

    validate_run_extra_writable_sources_exist(None, project_root.path(), false, &extra)
        .expect("missing --extra-writable directory should be created before daemon spawn");

    assert!(
        missing.is_dir(),
        "pre-daemon validation should create missing --extra-writable directory"
    );
}

#[test]
fn test_global_extra_writable_resolves_relative_path() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let drafts = project_root.path().join("drafts");
    std::fs::create_dir_all(&drafts).expect("create drafts dir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"

[filesystem_sandbox]
extra_writable = ["drafts"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "claude-code",
        "test-session",
        project_root.path(),
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

    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&drafts.canonicalize().unwrap()),
        "global extra_writable path should be resolved into writable_paths, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}

#[test]
fn test_global_extra_writable_creates_missing_path_before_sandbox_launch() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let missing = project_root.path().join("missing-extra");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"

[filesystem_sandbox]
extra_writable = ["missing-extra"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
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
        panic!("Expected missing filesystem_sandbox.extra_writable path to be created");
    };
    assert!(
        missing.is_dir(),
        "missing filesystem_sandbox.extra_writable directory should be created"
    );

    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&missing.canonicalize().unwrap()),
        "created global extra_writable path should be in writable_paths, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}

#[test]
fn test_global_extra_writable_creates_missing_path_with_per_tool_writable_paths() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let tool_writable = project_root.path().join("tool-writable");
    let missing = project_root.path().join("missing-extra");
    std::fs::create_dir_all(&tool_writable).expect("create tool writable dir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"

[filesystem_sandbox]
extra_writable = ["missing-extra"]

[tools.codex.filesystem_sandbox]
writable_paths = ["tool-writable"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
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
        panic!("Expected global extra_writable path to be created with per-tool writable_paths");
    };
    assert!(
        missing.is_dir(),
        "missing global extra_writable directory should be created even with per-tool writable_paths"
    );

    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        sandbox.isolation_plan.readonly_project_root,
        "per-tool writable_paths should keep project root read-only"
    );
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&tool_writable.canonicalize().unwrap()),
        "tool writable path should remain in writable_paths, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&missing.canonicalize().unwrap()),
        "created global extra_writable path should be appended, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}

#[test]
fn test_run_extra_writable_pre_daemon_validation_checks_global_config() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let missing = project_root.path().join("missing-extra");
    let cfg = parse_project_config(
        r#"
[filesystem_sandbox]
extra_writable = ["missing-extra"]
"#,
    );

    validate_run_extra_writable_sources_exist(Some(&cfg), project_root.path(), false, &[]).expect(
        "missing filesystem_sandbox.extra_writable path should be created before daemon spawn",
    );

    assert!(
        missing.is_dir(),
        "pre-daemon validation should create global extra_writable directory"
    );
}

#[cfg(unix)]
#[test]
fn test_global_extra_writable_skips_path_when_missing_directory_cannot_be_created() {
    use std::os::unix::fs::PermissionsExt;

    struct RestorePermissions {
        path: PathBuf,
        mode: u32,
    }

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            if let Ok(metadata) = std::fs::metadata(&self.path) {
                let mut permissions = metadata.permissions();
                permissions.set_mode(self.mode);
                let _ = std::fs::set_permissions(&self.path, permissions);
            }
        }
    }

    let project_root = tempfile::tempdir().expect("project root tempdir");
    let locked_parent = project_root.path().join("locked");
    std::fs::create_dir_all(&locked_parent).expect("create locked parent");
    let original_mode = std::fs::metadata(&locked_parent)
        .expect("locked parent metadata")
        .permissions()
        .mode();
    let _restore = RestorePermissions {
        path: locked_parent.clone(),
        mode: original_mode,
    };
    let mut locked_permissions = std::fs::metadata(&locked_parent)
        .expect("locked parent metadata")
        .permissions();
    locked_permissions.set_mode(0o500);
    std::fs::set_permissions(&locked_parent, locked_permissions).expect("make parent read-only");

    let missing = locked_parent.join("missing-extra");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"

[filesystem_sandbox]
extra_writable = ["locked/missing-extra"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &[],
        &[],
    );

    if missing.exists() {
        return;
    }

    let SandboxResolution::Ok(opts) = result else {
        panic!("mkdir failure for allowed extra_writable should skip the entry, not abort");
    };
    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        !sandbox.isolation_plan.writable_paths.contains(&missing),
        "uncreated extra_writable path should be skipped, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}

/// CLI --extra-writable with invalid path (outside allowed parents) is rejected.
#[test]
fn test_extra_writable_rejects_dangerous_paths() {
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let extra = vec![PathBuf::from("/etc/shadow")];
    let result = resolve_sandbox_options(
        Some(&cfg),
        "claude-code",
        "test-session",
        &current_project_root(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &extra,
        &[],
    );

    assert!(
        matches!(result, SandboxResolution::RequiredButUnavailable(ref msg) if msg.contains("extra-writable")),
        "dangerous path in --extra-writable should be rejected"
    );
}

#[test]
fn test_global_extra_writable_rejects_dangerous_paths() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"

[filesystem_sandbox]
extra_writable = ["/etc/shadow"]
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        &[],
        &[],
    );

    assert!(
        matches!(result, SandboxResolution::RequiredButUnavailable(ref msg) if msg.contains("filesystem_sandbox.extra_writable validation failed")),
        "dangerous global extra_writable path should be rejected before creation"
    );
}

#[test]
fn test_extra_writable_accepts_scoped_xdg_runtime_child() {
    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let runtime_root = project_root.path().join("run/user/1001");
    let runtime_child = runtime_root.join("just");
    std::fs::create_dir_all(&runtime_child).expect("create runtime child");
    let _runtime_guard = ScopedEnvVarRestore::set("XDG_RUNTIME_DIR", &runtime_root);
    let cfg = parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    );

    let result = resolve_sandbox_options(
        Some(&cfg),
        "codex",
        "test-session",
        project_root.path(),
        StreamMode::BufferOnly,
        120,
        600,
        Some(120),
        false,
        false,
        std::slice::from_ref(&runtime_child),
        &[],
    );

    let SandboxResolution::Ok(opts) = result else {
        panic!("Expected scoped XDG runtime child extra_writable to be accepted");
    };
    let sandbox = opts.sandbox.expect("expected sandbox context");
    assert!(
        sandbox
            .isolation_plan
            .writable_paths
            .contains(&runtime_child.canonicalize().unwrap()),
        "scoped runtime child should be writable, got: {:?}",
        sandbox.isolation_plan.writable_paths
    );
}
