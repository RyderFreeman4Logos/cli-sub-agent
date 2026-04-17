use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    fn enter(path: &Path) -> Self {
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
fn apply_review_target_dir_leaves_existing_directory_target_untouched() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    std::fs::create_dir(project.path().join("target")).expect("create target dir");
    let session_dir = project.path().join("session");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("review"), &session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}

#[cfg(unix)]
#[test]
fn apply_review_target_dir_leaves_broken_target_symlink_untouched() {
    use std::os::unix::fs::symlink;

    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    symlink("missing-mount/target", project.path().join("target"))
        .expect("create broken target symlink");
    let session_dir = project.path().join("session");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("review"), &session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}

#[test]
fn apply_review_target_dir_prefers_project_path_from_session_state() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("project tempdir");
    let unrelated_cwd = tempdir().expect("cwd tempdir");
    let _cwd = CurrentDirGuard::enter(unrelated_cwd.path());
    std::fs::create_dir(project.path().join("target")).expect("create target dir");
    let session_dir = project.path().join("session");
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    std::fs::write(
        session_dir.join("state.toml"),
        format!("project_path = {:?}\n", project.path()),
    )
    .expect("write state file");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("review"), &session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}

#[test]
fn apply_review_target_dir_routes_review_sessions_when_repo_target_missing() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let session_dir = project.path().join("session");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("review"), &session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some(session_dir.join("target").to_string_lossy().as_ref())
    );
}

#[test]
fn apply_review_target_dir_leaves_non_review_sessions_unchanged() {
    let _lock = current_dir_lock().lock().expect("current dir lock");
    let project = tempdir().expect("tempdir");
    let _cwd = CurrentDirGuard::enter(project.path());
    let session_dir = project.path().join("session");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(Some("run"), &session_dir, &mut env);

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}
