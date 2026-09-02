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
    let target = path.with_file_name("overlay-toctou-target");
    std::fs::write(&target, "raced\n").expect("write raced symlink target");
    std::fs::remove_file(path).expect("remove accepted overlay leaf");
    std::os::unix::fs::symlink(&target, path).expect("replace overlay leaf with symlink");
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
