use std::path::PathBuf;

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
fn test_extra_writable_rejects_nonexistent_path_before_sandbox_launch() {
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

    let SandboxResolution::RequiredButUnavailable(message) = result else {
        panic!("Expected missing --extra-writable path to fail before sandbox launch");
    };
    assert_eq!(
        message,
        format!(
            "--extra-writable path '{}' does not exist. Create it first or remove the flag.",
            missing.display()
        )
    );
}

#[test]
fn test_run_extra_writable_pre_daemon_validation_rejects_nonexistent_path() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let missing = project_root.path().join("missing-extra");
    let extra = vec![missing.clone()];

    let err = validate_run_extra_writable_sources_exist(None, project_root.path(), false, &extra)
        .expect_err("missing --extra-writable path should fail before daemon spawn");

    assert_eq!(
        err,
        format!(
            "--extra-writable path '{}' does not exist. Create it first or remove the flag.",
            missing.display()
        )
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
fn test_global_extra_writable_rejects_nonexistent_path_before_sandbox_launch() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
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

    let SandboxResolution::RequiredButUnavailable(message) = result else {
        panic!(
            "Expected missing filesystem_sandbox.extra_writable path to fail before sandbox launch"
        );
    };
    assert_eq!(
        message,
        "filesystem_sandbox.extra_writable path 'missing-extra' does not exist. Create it first or remove the config entry."
    );
}

#[test]
fn test_run_extra_writable_pre_daemon_validation_checks_global_config() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cfg = parse_project_config(
        r#"
[filesystem_sandbox]
extra_writable = ["missing-extra"]
"#,
    );

    let err =
        validate_run_extra_writable_sources_exist(Some(&cfg), project_root.path(), false, &[])
            .expect_err(
                "missing filesystem_sandbox.extra_writable path should fail before daemon spawn",
            );

    assert_eq!(
        err,
        "filesystem_sandbox.extra_writable path 'missing-extra' does not exist. Create it first or remove the config entry."
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
