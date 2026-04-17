use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tempfile::tempdir;

fn current_dir_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn enter(path: &std::path::Path) -> Self {
        let original = std::env::current_dir().expect("read current dir");
        std::env::set_current_dir(path).expect("set current dir");
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original).expect("restore current dir");
    }
}

#[test]
fn apply_run_target_dir_guard_leaves_existing_directory_target_untouched() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    std::fs::create_dir(project.path().join("target")).expect("create target dir");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/tmp/codex-session-target".to_string(),
    );

    crate::pipeline_env::apply_run_target_dir_guard(Some("run"), "codex", project.path(), &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/tmp/codex-session-target")
    );
}

#[cfg(unix)]
#[test]
fn apply_run_target_dir_guard_leaves_broken_target_symlink_untouched() {
    use std::os::unix::fs::symlink;

    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    symlink("missing-mount/target", project.path().join("target"))
        .expect("create broken target symlink");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/tmp/codex-session-target".to_string(),
    );

    crate::pipeline_env::apply_run_target_dir_guard(Some("run"), "codex", project.path(), &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/tmp/codex-session-target")
    );
}

#[test]
fn apply_run_target_dir_guard_does_not_inject_override_when_repo_target_missing() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let mut env = HashMap::new();

    crate::pipeline_env::apply_run_target_dir_guard(Some("run"), "codex", project.path(), &mut env);

    assert!(
        !env.contains_key("CARGO_TARGET_DIR"),
        "run guard must not invent a CSA override when ./target is absent"
    );
}

#[test]
fn apply_run_target_dir_guard_removes_preexisting_override_when_repo_target_missing() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/tmp/codex-session-target".to_string(),
    );

    crate::pipeline_env::apply_run_target_dir_guard(Some("run"), "codex", project.path(), &mut env);

    assert!(
        !env.contains_key("CARGO_TARGET_DIR"),
        "run guard must preserve codex default behavior when ./target is absent"
    );
}
