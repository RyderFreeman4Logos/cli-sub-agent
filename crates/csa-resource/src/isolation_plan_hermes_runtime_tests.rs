//! Hermes sandbox runtime overlay regressions (#3148).

use super::*;
use std::path::{Path, PathBuf};

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
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("Hermes runtime paths should produce a sandbox plan");

    assert!(
        plan.writable_paths.contains(&logs),
        "Hermes logs must remain writable"
    );
    assert!(
        plan.writable_paths.contains(&state_db),
        "Hermes state database must remain writable"
    );
    assert!(
        !plan.writable_paths.contains(&hermes_home),
        "Hermes home itself must not stay a whole-directory writable bind"
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
    let logs = logs.to_string_lossy();
    let config = config.to_string_lossy();
    let writable_logs = args
        .windows(3)
        .position(|window| window[0] == "--bind" && window[2] == logs.as_ref())
        .expect("Hermes logs must be mounted writable");
    let readonly_config = args
        .windows(3)
        .position(|window| window[0] == "--ro-bind-fd" && window[2] == config.as_ref())
        .expect("Hermes config must override the writable parent with a read-only mount");
    let readonly_home = args
        .windows(3)
        .position(|window| window[0] == "--ro-bind-fd" && window[2] == hermes_home.as_ref())
        .expect("Hermes home must be re-bound read-only so absent names stay uncreatable");
    assert!(
        readonly_home < writable_logs,
        "writable Hermes logs must follow the read-only home overlay"
    );
    assert!(
        readonly_config != writable_logs,
        "read-only Hermes configuration mounts must remain distinct from runtime writes"
    );

    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Landlock)
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect_err("Landlock cannot safely grant Hermes SQLite parent-directory writes");
    assert!(
        error
            .to_string()
            .contains("hermes sandbox preflight failed"),
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

    assert!(
        error
            .to_string()
            .contains("hermes sandbox preflight failed")
    );
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
        .with_tool_defaults(
            "hermes",
            &temp.path().join("project"),
            &temp.path().join("session"),
        )
        .build()
        .expect("configured execution environment should build a Hermes plan");

    assert!(plan.writable_paths.contains(&configured_home.join("logs")));
    assert!(!plan.writable_paths.contains(&configured_home));
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
            .with_tool_defaults(
                "hermes",
                &temp.path().join("project"),
                &temp.path().join("session"),
            )
            .build()
            .expect_err("symlinked Hermes configuration overlays must fail preflight");
        assert!(
            error
                .to_string()
                .contains("hermes sandbox preflight failed")
        );
    }
}

#[cfg(unix)]
#[test]
fn test_tool_defaults_hermes_rejects_unlistable_home_under_project_root() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let project = temp.path().join("project");
    let hermes_home = project.join(".hermes");
    std::fs::create_dir_all(&hermes_home).unwrap();
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
    let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

    let original_mode = {
        let metadata = std::fs::metadata(&hermes_home).unwrap();
        let original_mode = metadata.permissions().mode();
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o311);
        std::fs::set_permissions(&hermes_home, permissions).unwrap();
        original_mode
    };

    let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults("hermes", &project, &temp.path().join("session"))
        .build();

    let mut permissions = std::fs::metadata(&hermes_home).unwrap().permissions();
    permissions.set_mode(original_mode);
    std::fs::set_permissions(&hermes_home, permissions).unwrap();

    let error = result
        .expect_err("unlistable Hermes home under a writable project_root must fail preflight");
    assert!(
        error
            .to_string()
            .contains("hermes sandbox preflight failed"),
        "unlistable overlay enumeration must fail closed: {error:#}"
    );
}

#[test]
fn test_tool_defaults_hermes_uses_execution_env_home_for_default_hermes_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (home, _env) = isolated_home(&temp);
    let _hermes_home_env = ScopedEnvVar::unset("HERMES_HOME");
    let custom_home = temp.path().join("custom-home");
    std::fs::create_dir_all(&custom_home).unwrap();
    let execution_env = std::collections::HashMap::from([(
        "HOME".to_string(),
        custom_home.to_string_lossy().into_owned(),
    )]);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            &temp.path().join("project"),
            &temp.path().join("session"),
        )
        .build()
        .expect("child HOME should select the default Hermes home");

    assert!(
        plan.writable_paths
            .contains(&custom_home.join(".hermes/logs")),
        "Hermes logs must follow execution_env HOME"
    );
    assert!(
        !plan.writable_paths.contains(&custom_home.join(".hermes")),
        "Hermes home itself must not stay a whole-directory writable bind"
    );
    assert!(
        !plan.writable_paths.contains(&home.join(".hermes")),
        "ambient HOME must not win over execution_env HOME"
    );
}

#[test]
fn test_tool_defaults_hermes_plans_execution_env_home_without_ambient_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let _ambient_home = ScopedEnvVar::unset("HOME");
    let configured = temp.path().join("configured-hermes-home");
    std::fs::create_dir_all(&configured).unwrap();
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        configured.to_string_lossy().into_owned(),
    )]);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            &temp.path().join("project"),
            &temp.path().join("session"),
        )
        .build()
        .expect("execution_env HERMES_HOME must be planned without ambient HOME");

    assert!(
        plan.writable_paths.contains(&configured.join("logs")),
        "Hermes planning must not depend on ambient HOME"
    );
    assert!(
        !plan.writable_paths.contains(&configured),
        "Hermes home itself must not stay a whole-directory writable bind"
    );
}

#[cfg(unix)]
#[test]
fn test_tool_defaults_hermes_rejects_runtime_logs_symlink_outside_home() {
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-logs-symlink-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    let outside = temp.path().join("outside-secret");
    std::fs::create_dir_all(&hermes_home).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("id_rsa"), "secret\n").unwrap();
    symlink(&outside, hermes_home.join("logs")).unwrap();
    let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

    let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build();

    if let Ok(plan) = &result {
        if let Some(command) = crate::from_isolation_plan(plan, "/usr/bin/tool", &[]) {
            let args = command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let outside = outside.to_string_lossy();
            assert!(
                !args
                    .windows(3)
                    .any(|window| window[0] == "--bind" && window[1] == outside.as_ref()),
                "logs symlink must not emit an outside writable bind; args: {args:?}"
            );
        }
    }
    let error = result.expect_err("logs symlink outside Hermes home must fail preflight");
    assert!(
        error
            .to_string()
            .contains("hermes sandbox preflight failed"),
        "runtime leaf symlink must fail closed: {error:#}"
    );
}

#[cfg(unix)]
#[test]
fn test_tool_defaults_hermes_rejects_dangling_sqlite_sidecar_symlink() {
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-sqlite-symlink-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let (_home, _env) = isolated_home(&temp);

    for name in [
        "state.db",
        "state.db-wal",
        "state.db-shm",
        "state.db-journal",
    ] {
        let hermes_home = temp.path().join(name);
        std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
        let outside = temp.path().join(format!("{name}-outside"));
        assert!(
            !outside.exists(),
            "dangling sqlite target must start absent"
        );
        symlink(&outside, hermes_home.join(name)).unwrap();
        let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

        let result = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_tool_defaults(
                "hermes",
                Path::new("/tmp/project"),
                Path::new("/tmp/session"),
            )
            .build();

        assert!(
            !outside.exists(),
            "dangling sqlite sidecar symlink must not create {name} target"
        );
        let error = result.expect_err("dangling sqlite sidecar symlink must fail preflight");
        assert!(
            error
                .to_string()
                .contains("hermes sandbox preflight failed"),
            "dangling sqlite sidecar must fail closed: {error:#}"
        );
    }
}

#[test]
fn test_tool_defaults_hermes_rejects_relative_environment_paths() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let (_home, _env) = isolated_home(&temp);
    let rel = format!(
        "csa-hermes-rel-{}",
        temp.path()
            .file_name()
            .expect("tempdir name")
            .to_string_lossy()
    );
    struct RemoveRel(PathBuf);
    impl Drop for RemoveRel {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = RemoveRel(PathBuf::from(&rel));
    std::fs::create_dir_all(Path::new(&rel).join("logs")).unwrap();
    std::fs::create_dir_all(Path::new(&rel).join(".hermes/logs")).unwrap();

    for (label, set_ambient_hermes_home, execution_env) in [
        (
            "execution-env HERMES_HOME",
            false,
            Some(std::collections::HashMap::from([(
                "HERMES_HOME".to_string(),
                rel.clone(),
            )])),
        ),
        ("ambient HERMES_HOME", true, None),
        (
            "execution-env HOME",
            false,
            Some(std::collections::HashMap::from([(
                "HOME".to_string(),
                rel.clone(),
            )])),
        ),
    ] {
        let _ambient = if set_ambient_hermes_home {
            ScopedEnvVar::set("HERMES_HOME", &rel)
        } else {
            ScopedEnvVar::unset("HERMES_HOME")
        };

        let built = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_execution_env(execution_env.as_ref())
            .with_tool_defaults(
                "hermes",
                Path::new("/tmp/project"),
                Path::new("/tmp/session"),
            )
            .build();

        let message = match built {
            Err(error) => error.to_string(),
            Ok(plan) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::from_isolation_plan(&plan, "/usr/bin/tool", &[])
            })) {
                Ok(_) => "produced a sandbox command".to_string(),
                Err(_) => "panicked at bwrap absolute-path assertion".to_string(),
            },
        };
        assert!(
            message.contains("hermes sandbox preflight failed"),
            "relative {label} must fail preflight, got {message}"
        );
    }
}
