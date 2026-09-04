//! Late IsolationPlanBuilder failure must not activate Hermes runtime (#3148).

use super::*;

#[cfg(unix)]
#[test]
fn failed_builder_validation_does_not_activate_runtime_for_all_layouts() {
    let _guard = ENV_LOCK.lock().unwrap();
    let layouts: [(&str, Option<&str>, &str); 4] = [
        ("root", None, "state.db"),
        ("flat", Some("flat"), "state.flat.db"),
        ("direct", Some("direct"), "direct/state.db"),
        ("nested", Some("nested"), "profiles/nested/state.db"),
    ];
    for (label, profile, legacy_rel) in layouts {
        let temp = tempfile::Builder::new()
            .prefix(&format!("hermes-late-builder-{label}-"))
            .tempdir_in("/var/tmp")
            .unwrap();
        let (_home, _env) = isolated_home(&temp);
        let hermes_home = temp.path().join("hermes-home");
        std::fs::create_dir_all(hermes_home.join("logs")).unwrap();
        if let Some(parent) = hermes_home.join(legacy_rel).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(hermes_home.join("config.yaml"), "model: test\n").unwrap();
        if label == "direct" {
            std::fs::write(hermes_home.join("direct/config.yaml"), "direct: true\n").unwrap();
        }
        if label == "nested" {
            std::fs::write(
                hermes_home.join("profiles/nested/config.yaml"),
                "nested: true\n",
            )
            .unwrap();
        }
        let legacy_db = hermes_home.join(legacy_rel);
        let source = live_sqlite_database(&legacy_db, label);
        let project = temp.path().join("linked-worktree");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(".git"), "not-a-gitdir\n").unwrap();
        let session = temp.path().join("session");
        std::fs::create_dir_all(&session).unwrap();

        let execution_env = std::collections::HashMap::from([(
            "HERMES_HOME".to_string(),
            hermes_home.to_string_lossy().into_owned(),
        )]);
        let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
            .with_filesystem_capability(FilesystemCapability::Bwrap)
            .with_execution_env(Some(&execution_env))
            .with_tool_defaults("hermes", &project, &session)
            .build()
            .expect_err(label);
        drop(source);
        assert!(
            error.to_string().contains("invalid Git directory marker"),
            "{label} late builder failure must fail closed: {error:#}"
        );

        let resolved = crate::isolation_plan::resolve_hermes_state_db(&hermes_home, profile);
        assert_eq!(
            resolved, legacy_db,
            "{label} must keep legacy authoritative after failed builder validation"
        );
        assert_ne!(
            resolved,
            hermes_home.join(".csa-runtime").join(legacy_rel),
            "{label} must not activate a partial runtime generation"
        );
        let marker = hermes_home.join(".csa-runtime").join(".csa-runtime-ready");
        assert!(
            std::fs::symlink_metadata(&marker)
                .map(|meta| !meta.file_type().is_file())
                .unwrap_or(true),
            "{label} must not publish a runtime activation marker after failed builder validation"
        );
    }
}

#[cfg(unix)]
#[test]
fn resolver_rejects_symlinked_runtime_ready_marker() {
    let temp = tempfile::Builder::new()
        .prefix("hermes-symlink-runtime-ready-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let hermes_home = temp.path().join("hermes-home");
    let runtime = hermes_home.join(".csa-runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(hermes_home.join("state.db"), b"legacy").unwrap();
    std::fs::write(runtime.join("state.db"), b"runtime").unwrap();
    std::os::unix::fs::symlink("/etc/passwd", runtime.join(".csa-runtime-ready")).unwrap();
    assert_eq!(
        crate::isolation_plan::resolve_hermes_state_db(&hermes_home, None),
        hermes_home.join("state.db"),
        "resolver must not follow a symlinked runtime-ready marker"
    );
}
