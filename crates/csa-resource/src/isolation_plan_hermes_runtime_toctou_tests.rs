//! Post-plan runtime-leaf symlink replacement regressions (#3148).

use super::*;
use crate::sandbox::ResourceCapability;
use std::path::Path;

fn command_args(command: &std::process::Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn assert_no_outside_writable_bind(args: &[String], outside: &Path, leaf: &str) {
    let outside = outside.to_string_lossy();
    assert!(
        !args.windows(3).any(|window| {
            matches!(window[0].as_str(), "--bind" | "--bind-fd")
                && (window[1] == outside.as_ref() || window[2] == outside.as_ref())
        }),
        "{leaf} replacement must not emit an outside writable bind; args: {args:?}"
    );
    assert!(
        !args
            .windows(3)
            .any(|window| window[0] == "--bind" && window[1].starts_with(outside.as_ref())),
        "{leaf} replacement must not canonicalize onto the outside target; args: {args:?}"
    );
}

fn hermes_plan_for_home(hermes_home: &Path) -> IsolationPlan {
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);
    IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_resource_capability(ResourceCapability::None)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("Hermes runtime leaves must plan before replacement")
}

fn seed_hermes_runtime(hermes_home: &Path) {
    std::fs::create_dir_all(hermes_home.join("logs")).expect("create Hermes logs");
    for name in [
        "state.db",
        "state.db-wal",
        "state.db-shm",
        "state.db-journal",
    ] {
        std::fs::write(hermes_home.join(name), b"").expect("create Hermes sqlite sidecar");
    }
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n").expect("write Hermes config");
}

fn replace_path_with_outside_symlink(path: &Path, outside: &Path) {
    if path.is_dir() {
        std::fs::remove_dir_all(path).expect("remove accepted runtime directory");
    } else {
        std::fs::remove_file(path).expect("remove accepted runtime file");
    }
    std::os::unix::fs::symlink(outside, path).expect("replace runtime path with outside symlink");
}

#[cfg(unix)]
#[test]
fn hermes_runtime_leaf_replaced_with_symlink_after_plan_does_not_bind_outside() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-runtime-leaf-toctou-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let (_home, _env) = isolated_home(&temp);

    for leaf in [
        "logs",
        "state.db",
        "state.db-wal",
        "state.db-shm",
        "state.db-journal",
    ] {
        let hermes_home = temp.path().join(leaf);
        seed_hermes_runtime(&hermes_home);
        let outside = temp.path().join(format!("{leaf}-outside"));
        if leaf == "logs" {
            std::fs::create_dir_all(&outside).expect("create outside directory");
            std::fs::write(outside.join("secret"), "outside\n").expect("write outside secret");
        } else {
            std::fs::write(&outside, "outside-db\n").expect("write outside sqlite target");
        }

        let plan = hermes_plan_for_home(&hermes_home);
        replace_path_with_outside_symlink(&hermes_home.join(leaf), &outside);
        let args = command_args(
            &crate::from_isolation_plan(&plan, "/usr/bin/tool", &[])
                .expect("planned Hermes sandbox must still produce a bwrap command"),
        );
        assert_no_outside_writable_bind(&args, &outside, leaf);
    }
}

#[cfg(unix)]
#[test]
fn hermes_home_replaced_with_symlink_after_plan_does_not_bind_outside() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-home-toctou-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("hermes-home");
    seed_hermes_runtime(&hermes_home);
    let outside = temp.path().join("outside-home");
    std::fs::create_dir_all(outside.join("logs")).expect("create outside logs");
    std::fs::write(outside.join("state.db"), "outside\n").expect("write outside state");
    std::fs::write(outside.join("secret"), "leaked\n").expect("write outside secret");

    let plan = hermes_plan_for_home(&hermes_home);
    let relocated = temp.path().join("hermes-home-original");
    std::fs::rename(&hermes_home, &relocated).expect("relocate planned Hermes home");
    std::os::unix::fs::symlink(&outside, &hermes_home)
        .expect("replace Hermes home with outside symlink");
    let args = command_args(
        &crate::from_isolation_plan(&plan, "/usr/bin/tool", &[])
            .expect("planned Hermes sandbox must still produce a bwrap command"),
    );
    assert_no_outside_writable_bind(&args, &outside, "HERMES_HOME");
    for leaf in [
        "logs",
        "state.db",
        "state.db-wal",
        "state.db-shm",
        "state.db-journal",
    ] {
        let outside_leaf = outside.join(leaf);
        if outside_leaf.exists() {
            assert_no_outside_writable_bind(&args, &outside_leaf, leaf);
        }
    }
}
