use super::*;
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use weave::compiler::{FailAction, PlanStep, VariableDecl, plan_from_toml};

#[path = "plan_cmd_tests_step_failure.rs"]
mod tests_step_failure;

#[tokio::test]
async fn execute_step_skips_when_condition_is_false() {
    let step = PlanStep {
        id: 1,
        title: "conditional".into(),
        tool: Some("bash".into()),
        prompt: "echo test".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: Some("${SOME_VAR}".into()),
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;
    assert!(result.skipped, "unset condition var must skip");
    assert_eq!(result.exit_code, 0, "condition-false skip succeeds");
}

#[tokio::test]
async fn execute_step_runs_when_condition_is_true() {
    let step = PlanStep {
        id: 1,
        title: "conditional".into(),
        tool: Some("bash".into()),
        prompt: "```bash\necho hello\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: Some("${FLAG}".into()),
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;
    assert!(!result.skipped, "true condition must execute step");
    assert_eq!(result.exit_code, 0, "bash echo should succeed");
}

#[tokio::test]
async fn execute_step_skips_loop_with_nonzero_exit() {
    let step = PlanStep {
        id: 1,
        title: "loop".into(),
        tool: Some("bash".into()),
        prompt: "echo test".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: Some(weave::compiler::LoopSpec {
            variable: "item".into(),
            collection: "${ITEMS}".into(),
            max_iterations: 10,
        }),
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;
    assert!(result.skipped);
    assert_ne!(result.exit_code, 0);
}

#[tokio::test]
async fn execute_step_skips_weave_include() {
    let step = PlanStep {
        id: 1,
        title: "include security-audit".into(),
        tool: Some("weave".into()),
        prompt: "INCLUDE security-audit".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;
    assert!(result.skipped);
    assert_eq!(result.exit_code, 0, "INCLUDE skip succeeds");
}

#[tokio::test]
async fn execute_step_bash_runs_code_block() {
    let step = PlanStep {
        id: 1,
        title: "echo test".into(),
        tool: Some("bash".into()),
        prompt:
            "Run this:\n```bash\nprintf 'hello' > code_block_output.txt\necho code-block-ran\n```\n"
                .into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
        workspace_access: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None, None).await;
    assert_eq!(
        result.exit_code, 0,
        "error={:?} output={:?}",
        result.error, result.output
    );
    assert!(
        result
            .output
            .as_deref()
            .unwrap_or("")
            .contains("code-block-ran"),
        "output should prove the fenced bash block ran"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("code_block_output.txt")).unwrap(),
        "hello"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn load_pr_bot_step_by_title(title: &str) -> PlanStep {
    let workflow_path = workspace_root().join("patterns/pr-bot/workflow.toml");
    let workflow = std::fs::read_to_string(&workflow_path).unwrap();
    let plan = plan_from_toml(&workflow).unwrap();
    let step = plan
        .steps
        .into_iter()
        .find(|step| step.title == title)
        .unwrap_or_else(|| panic!("missing pr-bot step '{title}'"));
    PlanStep {
        condition: None,
        loop_var: None,
        ..step
    }
}

#[tokio::test]
async fn execute_plan_stops_for_await_user() {
    let plan = ExecutionPlan {
        name: "await-user".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "setup-step".into(),
                tool: Some("await-user".into()),
                prompt: "Fix the bot configuration before retrying.".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
            PlanStep {
                id: 2,
                title: "should-not-run".into(),
                tool: Some("bash".into()),
                prompt: "```bash\ntouch should-not-run\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("await-user.journal.json");
    std::fs::write(&workflow_path, "[workflow]\nname='await-user'\n").unwrap();
    let completed = std::collections::HashSet::new();
    let mut journal = PlanRunJournal::new("await-user", &workflow_path, vars.clone());
    let mut run_ctx = PlanRunContext {
        project_root: tmp.path(),
        workflow_path: &workflow_path,
        config: None,
        global_config: super::test_global_config(),
        model_catalog: super::test_model_catalog(),
        tool_override: None,
        model_spec_override: None,
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &completed,
        chunked: false,
        no_fs_sandbox: false,
        resources: Default::default(),
        startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    };

    let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
        .await
        .expect("await-user plan should stop cleanly");

    assert_eq!(results.len(), 1, "await-user must stop the workflow");
    assert_eq!(journal.status, "awaiting-user");
    assert!(journal.completed_steps.contains(&1));
    assert!(!tmp.path().join("should-not-run").exists());
}

#[tokio::test]
async fn execute_plan_continues_after_skipped_await_user_step() {
    let plan = ExecutionPlan {
        name: "skip-await-user".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "skipped-setup".into(),
                tool: Some("await-user".into()),
                prompt: "Only wait when setup is missing.".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${BOT_NEEDS_SETUP}".into()),
                loop_var: None,
                session: None,
                workspace_access: None,
            },
            PlanStep {
                id: 2,
                title: "should-run".into(),
                tool: Some("bash".into()),
                prompt: "```bash\ntouch should-run\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    let journal_path = tmp.path().join("skip-await-user.journal.json");
    std::fs::write(&workflow_path, "[workflow]\nname='skip-await-user'\n").unwrap();
    let completed = std::collections::HashSet::new();
    let mut journal = PlanRunJournal::new("skip-await-user", &workflow_path, vars.clone());
    let mut run_ctx = PlanRunContext {
        project_root: tmp.path(),
        workflow_path: &workflow_path,
        config: None,
        global_config: super::test_global_config(),
        model_catalog: super::test_model_catalog(),
        tool_override: None,
        model_spec_override: None,
        journal: &mut journal,
        journal_path: Some(&journal_path),
        resume_completed_steps: &completed,
        chunked: false,
        no_fs_sandbox: false,
        resources: Default::default(),
        startup_env: &crate::startup_env::EMPTY_STARTUP_SUBTREE_ENV,
    };

    let results = execute_plan_with_journal(&plan, &vars, &mut run_ctx)
        .await
        .expect("skipped await-user should not stop workflow");

    assert_eq!(results.len(), 2);
    assert!(results[0].skipped);
    assert_eq!(journal.status, "running");
    assert!(tmp.path().join("should-run").exists());
}

include!("plan_cmd_tests_pr_audit.rs");

#[tokio::test]
async fn execute_plan_skips_false_condition_cleanly() {
    // A plan with condition-false steps must skip them with exit_code 0
    // and continue to subsequent steps.
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "conditional step".into(),
                tool: Some("bash".into()),
                prompt: "echo hello".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Skip,
                condition: Some("${FLAG}".into()),
                loop_var: None,
                session: None,
                workspace_access: None,
            },
            PlanStep {
                id: 2,
                title: "unconditional step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho ok\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    // Both steps should be processed
    assert_eq!(results.len(), 2, "both steps must be processed");
    // Step 1: skipped with success (condition false)
    assert!(results[0].skipped);
    assert_eq!(
        results[0].exit_code, 0,
        "condition-false skip is not a failure"
    );
    // Step 2: executed successfully
    assert!(!results[1].skipped);
    assert_eq!(results[1].exit_code, 0);
}

#[tokio::test]
async fn execute_plan_runs_true_condition_steps() {
    // When condition is true, the step must execute (not skip).
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![PlanStep {
            id: 1,
            title: "conditional step".into(),
            tool: Some("bash".into()),
            prompt: "```bash\necho hello\n```".into(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: Some("${FLAG}".into()),
            loop_var: None,
            session: None,
            workspace_access: None,
        }],
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].skipped, "true condition must execute");
    assert_eq!(results[0].exit_code, 0);
}

#[tokio::test]
async fn execute_plan_allows_prefixed_marker_to_drive_next_condition() {
    let plan = ExecutionPlan {
        name: "marker-flow".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FLAG".into(),
            default: None,
        }],
        steps: vec![
            PlanStep {
                id: 1,
                title: "emit marker".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho 'CSA_VAR:FLAG=yes'\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
            PlanStep {
                id: 2,
                title: "conditioned step".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho marker_pass > marker.txt\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${FLAG}".into()),
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].exit_code, 0);
    assert_eq!(results[1].exit_code, 0);
    assert!(!results[1].skipped);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("marker.txt"))
            .unwrap()
            .trim(),
        "marker_pass"
    );
}

#[tokio::test]
async fn execute_plan_does_not_inject_markers_from_failed_steps() {
    let plan = ExecutionPlan {
        name: "failed-marker".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FLAG".into(),
            default: None,
        }],
        steps: vec![
            PlanStep {
                id: 1,
                title: "emit then fail".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho 'CSA_VAR:FLAG=yes'\nexit 1\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Skip,
                condition: None,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
            PlanStep {
                id: 2,
                title: "must stay skipped".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho should_not_run > should_not_exist.txt\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${FLAG}".into()),
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_ne!(results[0].exit_code, 0);
    assert!(results[1].skipped);
    assert!(!tmp.path().join("should_not_exist.txt").exists());
}
