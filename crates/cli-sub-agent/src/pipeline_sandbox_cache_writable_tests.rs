use std::collections::HashMap;

use super::*;

fn parse_project_config(toml_str: &str) -> csa_config::ProjectConfig {
    toml::from_str(toml_str).expect("test TOML should parse")
}

fn sandbox_config() -> csa_config::ProjectConfig {
    parse_project_config(
        r#"
[resources]
memory_max_mb = 2048
enforcement_mode = "best-effort"
"#,
    )
}

fn resolve_sandbox_options_with_execution_env(
    cfg: &csa_config::ProjectConfig,
    project_root: &std::path::Path,
    execution_env: &HashMap<String, String>,
) -> SandboxResolution {
    resolve_sandbox_options_with_overrides(
        SandboxResolveInput {
            config: Some(cfg),
            tool_name: "codex",
            session_id: "test-session",
            project_root,
            stream_mode: StreamMode::BufferOnly,
            idle_timeout_seconds: 120,
            liveness_dead_seconds: 600,
            initial_response_timeout_seconds: Some(120),
            no_fs_sandbox: false,
            readonly_project_root: false,
            extra_writable: &[],
            extra_readable: &[],
            execution_env: Some(execution_env),
        },
        RunResourceOverrides::default(),
    )
}

#[test]
fn test_rust_env_writable_creates_missing_dotted_cargo_cache_dirs() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cargo_home = project_root.path().join("cargo.cache");
    let cfg = sandbox_config();
    let execution_env = HashMap::from([(
        csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
        cargo_home.to_string_lossy().into_owned(),
    )]);

    let result =
        resolve_sandbox_options_with_execution_env(&cfg, project_root.path(), &execution_env);

    let SandboxResolution::Ok(opts) = result else {
        panic!("dotted Cargo cache directory should be prepared before sandbox launch");
    };
    for path in [
        &cargo_home,
        &cargo_home.join("git"),
        &cargo_home.join("registry"),
    ] {
        assert!(path.is_dir(), "{} should be a directory", path.display());
    }

    let sandbox = opts.sandbox.expect("expected sandbox context");
    for path in [
        &cargo_home,
        &cargo_home.join("git"),
        &cargo_home.join("registry"),
    ] {
        assert!(
            sandbox.isolation_plan.writable_paths.contains(
                &path
                    .canonicalize()
                    .expect("created cache dir canonicalizes")
            ),
            "{} should be granted writable access, got: {:?}",
            path.display(),
            sandbox.isolation_plan.writable_paths
        );
    }
}

#[test]
fn test_rust_env_writable_rejects_existing_file_cache_dir() {
    let project_root = tempfile::tempdir().expect("project root tempdir");
    let cargo_home = project_root.path().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).expect("create cargo home");
    let cargo_git = cargo_home.join("git");
    std::fs::write(&cargo_git, b"not a directory").expect("create file at Cargo git cache path");
    let cfg = sandbox_config();
    let execution_env = HashMap::from([(
        csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
        cargo_home.to_string_lossy().into_owned(),
    )]);

    let result =
        resolve_sandbox_options_with_execution_env(&cfg, project_root.path(), &execution_env);

    let SandboxResolution::RequiredButUnavailable(message) = result else {
        panic!("existing file cache dir should fail before sandbox launch");
    };
    assert!(message.contains("Rust state env writable paths"));
    assert!(message.contains(&cargo_git.display().to_string()));
    assert!(message.contains("not a directory"));
}

#[cfg(unix)]
#[test]
fn test_rust_env_writable_fails_when_safe_cache_dir_cannot_be_prepared() {
    use std::os::unix::fs::PermissionsExt;

    let project_root = tempfile::tempdir().expect("project root tempdir");
    let blocked_parent = project_root.path().join("blocked-cache-parent");
    std::fs::create_dir_all(&blocked_parent).expect("create blocked parent");
    let original_mode = std::fs::metadata(&blocked_parent)
        .expect("blocked parent metadata")
        .permissions()
        .mode();
    let mut permissions = std::fs::metadata(&blocked_parent)
        .expect("blocked parent metadata")
        .permissions();
    permissions.set_mode(0o500);
    std::fs::set_permissions(&blocked_parent, permissions).expect("make blocked parent read-only");
    let cargo_home = blocked_parent.join("cargo-home");
    let cfg = sandbox_config();
    let execution_env = HashMap::from([(
        csa_core::env::CARGO_HOME_ENV_KEY.to_string(),
        cargo_home.to_string_lossy().into_owned(),
    )]);

    let result =
        resolve_sandbox_options_with_execution_env(&cfg, project_root.path(), &execution_env);

    let mut permissions = std::fs::metadata(&blocked_parent)
        .expect("blocked parent metadata after test")
        .permissions();
    permissions.set_mode(original_mode);
    std::fs::set_permissions(&blocked_parent, permissions).expect("restore blocked parent mode");

    let SandboxResolution::RequiredButUnavailable(message) = result else {
        panic!("unprepared Rust cache dir should fail before tool mutation");
    };
    assert!(message.contains("Rust state env writable paths"));
    assert!(message.contains(&cargo_home.display().to_string()));
    assert!(message.contains("could not be created before session launch"));
}
