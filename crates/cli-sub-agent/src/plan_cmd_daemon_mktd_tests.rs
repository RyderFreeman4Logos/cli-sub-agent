use super::*;
use crate::plan_cmd::{PlanRunArgs, PlanRunPipelineSource};
use crate::startup_env::StartupSubtreeEnv;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_git(project_root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("git command should start");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_plan_test_repo(project_root: &Path) {
    run_git(project_root, &["init", "-b", "main"]);
    run_git(
        project_root,
        &["config", "user.email", "csa-test@example.com"],
    );
    run_git(project_root, &["config", "user.name", "CSA Test"]);
    run_git(project_root, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(
        project_root.join(".git").join("info").join("exclude"),
        ".csa/\n",
    )
    .expect("write repo-local exclude");
    std::fs::write(project_root.join("README.md"), "test repo\n").expect("write readme");
    std::fs::write(project_root.join("weave.lock"), "lock = 1\n").expect("write weave.lock");
}

fn commit_all(project_root: &Path, message: &str) {
    run_git(
        project_root,
        &["add", "README.md", "weave.lock", "workflow.toml"],
    );
    run_git(project_root, &["commit", "-m", message]);
}

fn plan_daemon_args(project_root: &Path) -> PlanRunArgs {
    PlanRunArgs {
        file: Some("workflow.toml".to_string()),
        pattern: None,
        vars: vec![],
        tool_override: None,
        model_spec_override: None,
        dry_run: false,
        chunked: false,
        resume: None,
        complete_manual_step: None,
        cd: Some(project_root.display().to_string()),
        no_fs_sandbox: false,
        current_depth: 0,
        pipeline_source: PlanRunPipelineSource::DirectPlanRun,
        startup_env: StartupSubtreeEnv::default(),
    }
}

fn prepare_plan_session(project_root: &Path, description: &str) -> (String, PathBuf) {
    let session_id = csa_session::new_session_id();
    let session_dir = csa_session::get_session_dir(project_root, &session_id)
        .expect("session dir should resolve");
    persist_placeholder_plan_session(project_root, &session_dir, &session_id, description)
        .expect("placeholder plan session should persist");
    (session_id, session_dir)
}

#[tokio::test]
async fn daemon_child_failed_mktd_persist_result_surfaces_validation_detail() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "dev2merge"

[[workflow.steps]]
id = 7
title = "Plan with mktd"
tool = "bash"
prompt = '''
```bash
printf 'csa todo persist failed (exit 1)\n' >&2
printf 'TODO artifact path: /tmp/mktd-save/TODO.md\n' >&2
printf 'Spec artifact path: /tmp/mktd-save/spec.toml\n' >&2
printf 'Persist stderr artifact: /tmp/mktd-save/persist.stderr\n' >&2
printf 'csa todo persist stderr (last 80 lines):\n' >&2
printf "Error: failed to parse spec file '/tmp/mktd-save/spec.toml': TOML parse error at line 6, column 1\n" >&2
exit 1
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: dev2merge");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(result.is_err(), "mktd persist failure should fail the plan");
    let persisted = csa_session::load_result(&project_root, &session_id)
        .expect("result should load")
        .expect("result.toml should exist");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted.summary.chars().count() <= PLAN_RESULT_SUMMARY_MAX_CHARS,
        "raw result summary should stay bounded: {}",
        persisted.summary
    );
    for required in [
        "failed to parse spec file",
        "TOML parse error at line 6, column 1",
        "csa todo persist failed",
        "Spec artifact path: /tmp/mktd-save/spec.toml",
    ] {
        assert!(
            persisted.summary.contains(required),
            "result.toml summary must expose parent-visible mktd persist detail {required}: {}",
            persisted.summary
        );
    }

    let wait_summary = crate::session_cmds_daemon::render_wait_result_summary(
        &session_dir,
        &session_id,
        &persisted,
    );
    for required in [
        "failed to parse spec file",
        "TOML parse error at line 6, column 1",
        "csa todo persist failed",
        "Spec artifact path: /tmp/mktd-save/spec.toml",
    ] {
        assert!(
            wait_summary.contains(required),
            "wait summary must expose parent-visible mktd persist detail {required}: {wait_summary}"
        );
    }
}

#[tokio::test]
async fn daemon_child_failed_mktd_result_surfaces_underlying_command_failure() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "dev2merge"

[[workflow.steps]]
id = 7
title = "Plan with mktd"
tool = "bash"
prompt = '''
```bash
printf 'spec producer-contract error: expected TOML spec artifact (raw TOML, fenced TOML, or CSA section containing TOML); first content: command stderr/stdout contamination\n' >&2
printf 'underlying command failure: spec artifact was contaminated by command stderr/stdout\n' >&2
printf 'Command stderr summary: error: failed to create cargo target dir: Read-only file system (os error 30)\n' >&2
printf 'Spec artifact path: /tmp/mktd-save/spec.toml\n' >&2
printf 'Raw spec artifact path: /tmp/mktd-save/spec.raw.txt\n' >&2
exit 1
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: dev2merge");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(result.is_err(), "mktd command failure should fail the plan");
    let persisted = csa_session::load_result(&project_root, &session_id)
        .expect("result should load")
        .expect("result.toml should exist");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted.summary.chars().count() <= PLAN_RESULT_SUMMARY_MAX_CHARS,
        "raw result summary should stay bounded: {}",
        persisted.summary
    );
    for required in [
        "underlying command failure",
        "Read-only file system",
        "Spec artifact path: /tmp/mktd-save/spec.toml",
    ] {
        assert!(
            persisted.summary.contains(required),
            "result.toml summary must expose parent-visible command failure detail {required}: {}",
            persisted.summary
        );
    }
    assert!(
        !persisted
            .summary
            .contains("detail=spec artifact-shape error"),
        "result.toml summary must not prefer generic artifact-shape noise: {}",
        persisted.summary
    );

    let wait_summary = crate::session_cmds_daemon::render_wait_result_summary(
        &session_dir,
        &session_id,
        &persisted,
    );
    for required in [
        "underlying command failure",
        "Read-only file system",
        "Spec artifact path: /tmp/mktd-save/spec.toml",
    ] {
        assert!(
            wait_summary.contains(required),
            "wait summary must expose parent-visible command failure detail {required}: {wait_summary}"
        );
    }
}
