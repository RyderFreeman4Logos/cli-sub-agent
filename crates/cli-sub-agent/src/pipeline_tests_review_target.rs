use std::{collections::HashMap, path::Path};

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

    crate::pipeline_cargo_target::apply_review_target_dir(project.path(), "codex");

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

    crate::pipeline_cargo_target::apply_review_target_dir(project.path(), "codex");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some("/repo/legacy-review-target")
    );
}

#[test]
fn apply_review_target_dir_leaves_default_behavior_when_repo_target_missing() {
    let project = tempdir().expect("tempdir");
    let env: HashMap<String, String> = HashMap::new();

    crate::pipeline_cargo_target::apply_review_target_dir(project.path(), "codex");

    assert_eq!(env.get("CARGO_TARGET_DIR").map(String::as_str), None);
}

#[test]
fn apply_review_target_dir_leaves_non_review_sessions_unchanged() {
    let project = tempdir().expect("tempdir");
    let explicit_target = tempdir().expect("explicit target tempdir");
    let explicit_target = explicit_target.path().to_string_lossy().into_owned();
    let mut env = HashMap::new();
    env.insert("CARGO_TARGET_DIR".to_string(), explicit_target.clone());

    let report = crate::pipeline_cargo_target::apply_task_target_dir_guards(
        Some("run"),
        "codex",
        project.path(),
        &mut env,
    )
    .expect("policy should resolve");

    assert_eq!(
        env.get("CARGO_TARGET_DIR").map(String::as_str),
        Some(explicit_target.as_str())
    );
    assert!(report.explicit_override_preserved);
}

#[test]
fn non_run_policies_skip_target_probe_during_sandbox_revalidation() {
    for task_type in ["review", "debate", "plan", "plan-step"] {
        let mut env = HashMap::new();
        let report = crate::pipeline_cargo_target::apply_task_target_dir_guards(
            Some(task_type),
            "codex",
            Path::new("/must-not-probe"),
            &mut env,
        )
        .unwrap_or_else(|error| {
            panic!("{task_type} must not perform a run Cargo preflight: {error}")
        });
        assert_eq!(report.policy_reason, "not_applicable");
        assert!(
            !report.requires_sandbox_writeability_validation(),
            "{task_type} must not schedule the run-only sandbox revalidation"
        );

        crate::pipeline_cargo_target::ensure_cargo_target_sandbox_writable(
            &report,
            Path::new("/must-not-probe"),
            None,
        )
        .unwrap_or_else(|error| {
            panic!("{task_type} must not probe a Cargo target during sandbox revalidation: {error}")
        });
    }
}
