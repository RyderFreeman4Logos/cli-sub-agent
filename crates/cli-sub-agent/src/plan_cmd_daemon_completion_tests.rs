use super::*;
use crate::plan_cmd::{PlanRunArgs, PlanRunPipelineSource};
use crate::startup_env::StartupSubtreeEnv;
use crate::test_session_sandbox::ScopedSessionSandbox;
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
async fn daemon_child_successful_plan_writes_structured_success_output() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "successful-plan"

[[workflow.steps]]
id = 1
title = "Passing Bash"
tool = "bash"
prompt = '''
```bash
printf 'structured success\n'
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: successful-plan");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(
        result.is_ok(),
        "successful plan should return ok: {result:?}"
    );
    assert_eq!(result.expect("successful plan should expose exit code"), 0);
    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("summary should load")
        .expect("summary section should exist");
    assert!(
        summary.contains("Plan complete: plan: workflow.toml"),
        "summary must expose daemon success to `csa session result --summary`: {summary}"
    );
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    assert!(
        details.contains("Plan Completion Report") && details.contains("Status: `success`"),
        "details must expose a structured success report: {details}"
    );
    let persisted = csa_session::load_result(&project_root, &session_id)
        .expect("result should load")
        .expect("result.toml should exist");
    assert_eq!(persisted.exit_code, 0);
    assert!(
        persisted
            .artifacts
            .iter()
            .any(|artifact| artifact.path == "output/summary.md"),
        "success result artifacts must point callers at structured summary"
    );
}

#[tokio::test]
async fn daemon_child_dev2merge_partial_publish_fails_structured_completion_verification() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "dev2merge"

[[workflow.steps]]
id = 13
title = "Push Gate"
tool = "bash"
prompt = '''
```bash
printf 'synthetic push gate passed\n'
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: dev2merge");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(
        result.is_err(),
        "dev2merge that reached publish without required side effects must fail"
    );
    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("summary should load")
        .expect("summary section should exist");
    assert!(
        summary.contains("dev2merge publish side-effect verification failed")
            && summary
                .contains("Failed step: 18 (Dev2merge Publish Side-Effect Verification) exited 1"),
        "summary must expose synthetic completion-verification failure: {summary}"
    );
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    assert!(
        details.contains("PR_NUMBER was not captured")
            && details.contains("Post-Merge Local Sync (Step 17) did not complete")
            && details.contains("callers should use this structured result"),
        "details must include actionable missing side effects: {details}"
    );
    let persisted = csa_session::load_result(&project_root, &session_id)
        .expect("result should load")
        .expect("result.toml should exist");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted
            .summary
            .contains("dev2merge publish side-effect verification failed"),
        "result summary must carry the completion verification failure: {}",
        persisted.summary
    );
}

#[tokio::test]
async fn daemon_child_dev2merge_no_side_effect_success_fails_structured_completion_verification() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let _sandbox = ScopedSessionSandbox::new(&temp).await;
    let project_root = temp.path().join("repo");
    std::fs::create_dir_all(&project_root).expect("repo dir should be created");
    init_plan_test_repo(&project_root);
    std::fs::write(
        project_root.join("workflow.toml"),
        r#"[workflow]
name = "dev2merge"

[[workflow.steps]]
id = 1
title = "Passing Non-Lifecycle Step"
tool = "bash"
prompt = '''
```bash
printf 'synthetic non-lifecycle step passed\n'
```
'''
on_fail = "abort"
"#,
    )
    .expect("write workflow");
    commit_all(&project_root, "initial");
    let (session_id, session_dir) = prepare_plan_session(&project_root, "plan: dev2merge");

    let result = handle_plan_run_daemon_child(plan_daemon_args(&project_root), &session_id).await;

    assert!(
        result.is_err(),
        "dev2merge terminal success without lifecycle side effects must fail"
    );
    let summary = csa_session::read_section(&session_dir, "summary")
        .expect("summary should load")
        .expect("summary section should exist");
    assert!(
        summary.contains(
            "dev2merge lifecycle side-effect verification failed: publish gate never started"
        ) && summary
            .contains("Failed step: 18 (Dev2merge Lifecycle Side-Effect Verification) exited 1"),
        "summary must expose the missing lifecycle gate instead of generic success: {summary}"
    );
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    assert!(
        details.contains("Publish Gate (Step 13) did not run")
            && details.contains("branch publication, PR creation, pr-bot merge")
            && details.contains("missing lifecycle gate"),
        "details must include actionable missing lifecycle side effects: {details}"
    );
    let persisted = csa_session::load_result(&project_root, &session_id)
        .expect("result should load")
        .expect("result.toml should exist");
    assert_eq!(persisted.exit_code, 1);
    assert!(
        persisted
            .summary
            .contains("dev2merge lifecycle side-effect verification failed"),
        "result summary must carry the lifecycle verification failure: {}",
        persisted.summary
    );
}
