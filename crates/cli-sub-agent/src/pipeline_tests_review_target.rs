use std::collections::HashMap;

use tempfile::tempdir;

#[test]
fn apply_review_target_dir_leaves_existing_directory_target_untouched() {
    let project = tempdir().expect("tempdir");
    std::fs::create_dir(project.path().join("target")).expect("create target dir");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(project.path(), "codex");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}

#[cfg(unix)]
#[test]
fn apply_review_target_dir_leaves_broken_target_symlink_untouched() {
    use std::os::unix::fs::symlink;

    let project = tempdir().expect("tempdir");
    symlink("missing-mount/target", project.path().join("target"))
        .expect("create broken target symlink");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_review_target_dir(project.path(), "codex");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}

#[test]
fn apply_review_target_dir_leaves_default_behavior_when_repo_target_missing() {
    let project = tempdir().expect("tempdir");
    let env: HashMap<String, String> = HashMap::new();

    crate::pipeline_env::apply_review_target_dir(project.path(), "codex");

    assert_eq!(env.get("CARGO_TARGET_DIR").map(String::as_str), None);
}

#[test]
fn apply_review_target_dir_leaves_non_review_sessions_unchanged() {
    let project = tempdir().expect("tempdir");
    let mut env = HashMap::new();
    env.insert(
        "CARGO_TARGET_DIR".to_string(),
        "/repo/legacy-review-target".to_string(),
    );

    crate::pipeline_env::apply_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    );

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}
