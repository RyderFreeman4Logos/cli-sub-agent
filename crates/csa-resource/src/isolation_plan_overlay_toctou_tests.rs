//! Replacement-interleaving regressions for read-only overlay pinning (#3148).

use super::*;
use crate::sandbox::ResourceCapability;
use std::path::Path;

fn overlay_bind_plan(overlay: ReadablePath) -> IsolationPlan {
    IsolationPlan {
        resource: ResourceCapability::None,
        filesystem: FilesystemCapability::Bwrap,
        writable_paths: Vec::new(),
        readable_paths: vec![overlay],
        env_overrides: std::collections::HashMap::new(),
        degraded_reasons: Vec::new(),
        memory_max_mb: None,
        memory_swap_max_mb: None,
        pids_max: None,
        readonly_project_root: false,
        project_root: None,
        soft_limit_percent: None,
        memory_monitor_interval_seconds: None,
        user_daemon_ipc: false,
    }
}

fn command_args(command: &std::process::Command) -> Vec<String> {
    command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn assert_overlay_bind_uses_pinned_identity(args: &[String], dest: &Path, forbidden_source: &Path) {
    let dest = dest.to_string_lossy();
    let forbidden = forbidden_source.to_string_lossy();
    assert!(
        args.windows(3)
            .any(|window| window[0] == "--ro-bind-fd" && window[2] == dest.as_ref()),
        "bind after replacement must use pinned fd identity; args: {args:?}"
    );
    assert!(
        !args.windows(3).any(|window| window[0] == "--ro-bind"
            && (window[1] == dest.as_ref() || window[1] == forbidden.as_ref())),
        "bind after replacement must not re-resolve the replaced path; args: {args:?}"
    );
}

fn replace_leaf_with_sibling_symlink(path: &Path) {
    if path.is_dir() {
        return;
    }
    let target = path.with_file_name("overlay-toctou-target");
    std::fs::write(&target, "raced\n").expect("write raced symlink target");
    std::fs::remove_file(path).expect("remove accepted overlay leaf");
    std::os::unix::fs::symlink(&target, path).expect("replace overlay leaf with symlink");
}

fn replace_directory_leaf_with_sibling_symlink(path: &Path) {
    let target = path.with_file_name("overlay-toctou-dir-target");
    std::fs::create_dir(&target).expect("create raced directory symlink target");
    std::fs::remove_dir(path).expect("remove accepted overlay directory");
    std::os::unix::fs::symlink(&target, path).expect("replace overlay directory with symlink");
}

fn hermes_name_is_sandbox_writable(plan: &IsolationPlan, path: &Path) -> bool {
    let covered = plan
        .writable_paths
        .iter()
        .any(|candidate| path == candidate.as_path() || path.starts_with(candidate));
    if !covered {
        return false;
    }
    !plan.readable_paths.iter().any(|readable| {
        readable.overrides_writable_mount()
            && (path == readable.requested() || path.starts_with(readable.requested()))
    })
}

#[cfg(unix)]
#[test]
fn try_pinned_readonly_overlay_fails_closed_when_leaf_replaced_with_symlink() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("overlay-toctou-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let config = temp.path().join("config.yaml");
    std::fs::write(&config, "model: test\n").expect("write regular overlay leaf");

    let overlay = ReadablePath::try_pinned_readonly_overlay(config.clone())
        .expect("regular overlay leaf must pin before replacement");
    replace_leaf_with_sibling_symlink(&config);
    let raced_target = config.with_file_name("overlay-toctou-target");
    let args = command_args(
        &crate::from_isolation_plan(&overlay_bind_plan(overlay), "/usr/bin/tool", &[])
            .expect("pinned overlay must still produce a bwrap command"),
    );
    assert_overlay_bind_uses_pinned_identity(&args, &config, &raced_target);
}

#[cfg(unix)]
#[test]
fn hermes_preflight_fails_closed_when_overlay_leaf_replaced_with_symlink() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-overlay-toctou-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let hermes_home = temp.path().join("home");
    std::fs::create_dir_all(&hermes_home).expect("create Hermes home");
    let config = hermes_home.join("config.yaml");
    std::fs::write(&config, "model: test\n").expect("write regular Hermes config");
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("Hermes overlay must pin before replacement");
    replace_leaf_with_sibling_symlink(&config);
    let raced_target = config.with_file_name("overlay-toctou-target");
    let args = command_args(
        &crate::from_isolation_plan(&plan, "/usr/bin/tool", &[])
            .expect("pinned Hermes overlay must still produce a bwrap command"),
    );
    assert_overlay_bind_uses_pinned_identity(&args, &config, &raced_target);
}

#[cfg(unix)]
#[test]
fn try_pinned_readonly_overlay_fails_closed_when_directory_replaced_with_symlink() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("overlay-toctou-dir-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let profiles = temp.path().join("profiles");
    std::fs::create_dir(&profiles).expect("write overlay directory leaf");

    let overlay = ReadablePath::try_pinned_readonly_overlay(profiles.clone())
        .expect("overlay directory must pin before replacement");
    replace_directory_leaf_with_sibling_symlink(&profiles);
    let raced_target = profiles.with_file_name("overlay-toctou-dir-target");
    let args = command_args(
        &crate::from_isolation_plan(&overlay_bind_plan(overlay), "/usr/bin/tool", &[])
            .expect("pinned overlay directory must still produce a bwrap command"),
    );
    assert_overlay_bind_uses_pinned_identity(&args, &profiles, &raced_target);
}

#[cfg(unix)]
#[test]
fn test_tool_defaults_hermes_plans_symlinked_home_with_real_bind_source() {
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-home-symlink-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let (_home, _env) = isolated_home(&temp);
    let real_home = temp.path().join("real-hermes");
    let logical_home = temp.path().join("logical-hermes");
    std::fs::create_dir_all(&real_home).expect("create real Hermes home");
    let real_config = real_home.join("config.yaml");
    std::fs::write(&real_config, "model: test\n").expect("write Hermes config");
    symlink(&real_home, &logical_home).expect("symlink HERMES_HOME");
    let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &logical_home);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("symlinked Hermes home with protected config must plan");

    let logical_config = logical_home.join("config.yaml");
    let overlay = plan
        .readable_paths
        .iter()
        .find(|path| path.requested() == logical_config.as_path())
        .expect("overlay destination must stay the logical Hermes path");
    assert_eq!(
        overlay.bind_source(),
        real_config.as_path(),
        "overlay bind source must be the real file"
    );
}

#[cfg(unix)]
#[test]
fn test_tool_defaults_hermes_does_not_leave_absent_config_writable() {
    let _guard = ENV_LOCK.lock().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("hermes-absent-config-")
        .tempdir_in("/var/tmp")
        .expect("tempdir outside /tmp overlay");
    let (_home, _env) = isolated_home(&temp);
    let hermes_home = temp.path().join("fresh-hermes");
    std::fs::create_dir_all(&hermes_home).expect("create fresh Hermes home");
    let config = hermes_home.join("config.yaml");
    assert!(!config.exists(), "fresh Hermes home must omit config.yaml");
    let _hermes_home_env = ScopedEnvVar::set("HERMES_HOME", &hermes_home);

    let plan = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect("fresh Hermes home must still produce a sandbox plan");

    assert!(
        hermes_name_is_sandbox_writable(&plan, &hermes_home.join("state.db-journal")),
        "SQLite journal must be creatable in the pinned writable home"
    );
    assert!(
        !hermes_name_is_sandbox_writable(&plan, &config),
        "absent Hermes config.yaml must not be writable on the host"
    );
    assert!(
        !hermes_name_is_sandbox_writable(&plan, &hermes_home.join("profiles")),
        "absent Hermes profiles must not be writable on the host"
    );
}
