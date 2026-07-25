use std::collections::HashMap;

use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};

#[cfg(unix)]
#[test]
fn cargo_target_preflight_fails_before_sandbox_resolves_an_alternate_target() {
    use std::os::unix::fs::symlink;

    let _env_lock = TEST_ENV_LOCK.blocking_lock();
    let project_root = tempfile::tempdir().expect("tempdir");
    let _state_home = ScopedEnvVarRestore::set("XDG_STATE_HOME", project_root.path().join("state"));
    let proc_dir = std::path::Path::new("/proc");
    if !proc_dir.is_dir() {
        return;
    }
    symlink(proc_dir, project_root.path().join("target"))
        .expect("create unwritable target symlink");
    let mut execution_env = HashMap::new();

    let error = crate::pipeline_cargo_target::apply_run_target_dir_guard(
        Some("run"),
        "codex",
        project_root.path(),
        &mut execution_env,
    )
    .expect_err("unwritable target must fail before sandbox resolution");

    assert!(error.contains("Cargo target preflight blocked before provider execution"));
    assert!(
        !execution_env.contains_key(csa_core::env::CARGO_TARGET_DIR_ENV_KEY),
        "preflight must not inject a sandbox-specific alternate target"
    );
}
