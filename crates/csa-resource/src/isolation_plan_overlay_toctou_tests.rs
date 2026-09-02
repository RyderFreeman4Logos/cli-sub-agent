//! Replacement-interleaving regressions for read-only overlay pinning (#3148).

use super::*;
use std::path::Path;

struct OverlayMetadataRaceGuard;

impl OverlayMetadataRaceGuard {
    fn arm(inject: fn(&Path)) -> Self {
        super::super::readable::AFTER_READONLY_OVERLAY_METADATA.with(|hook| hook.set(Some(inject)));
        Self
    }
}

impl Drop for OverlayMetadataRaceGuard {
    fn drop(&mut self) {
        super::super::readable::AFTER_READONLY_OVERLAY_METADATA.with(|hook| hook.set(None));
    }
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

    let _race = OverlayMetadataRaceGuard::arm(replace_leaf_with_sibling_symlink);
    let error = ReadablePath::try_pinned_readonly_overlay(config)
        .expect_err("replacement interleaving must fail closed before pin");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
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
    std::fs::write(hermes_home.join("config.yaml"), "model: test\n")
        .expect("write regular Hermes config");
    let execution_env = std::collections::HashMap::from([(
        "HERMES_HOME".to_string(),
        hermes_home.to_string_lossy().into_owned(),
    )]);

    let _race = OverlayMetadataRaceGuard::arm(replace_leaf_with_sibling_symlink);
    let error = IsolationPlanBuilder::new(EnforcementMode::BestEffort)
        .with_filesystem_capability(FilesystemCapability::Bwrap)
        .with_execution_env(Some(&execution_env))
        .with_tool_defaults(
            "hermes",
            Path::new("/tmp/project"),
            Path::new("/tmp/session"),
        )
        .build()
        .expect_err("raced Hermes overlay must fail preflight");
    assert!(
        error
            .to_string()
            .contains("hermes sandbox preflight failed"),
        "raced overlay must fail closed: {error:#}"
    );
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

    let _race = OverlayMetadataRaceGuard::arm(replace_directory_leaf_with_sibling_symlink);
    let error = ReadablePath::try_pinned_readonly_overlay(profiles)
        .expect_err("directory replacement interleaving must fail closed before pin");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
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
        !hermes_name_is_sandbox_writable(&plan, &config),
        "absent Hermes config.yaml must not remain creatable under the writable home bind"
    );
}
