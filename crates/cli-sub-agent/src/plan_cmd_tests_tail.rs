use super::*;

#[tokio::test]
async fn execute_plan_aborts_on_retry_exhaustion() {
    // After retry(1) is exhausted on a failing step, the next step must NOT run.
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "always fails".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nexit 1\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Retry(1),
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "should not run".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho unreachable\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    // Only step 1 should execute; step 2 should be skipped due to abort on retry exhaustion
    assert_eq!(
        results.len(),
        1,
        "retry exhaustion must abort — step 2 should not run"
    );
    assert_eq!(results[0].step_id, 1);
    assert_ne!(results[0].exit_code, 0);
}

#[tokio::test]
async fn execute_plan_continues_on_skip_failure() {
    // on_fail=skip should NOT abort — next step must still run.
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "fails but skip".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nexit 1\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Skip,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "should run".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho ok\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    assert_eq!(
        results.len(),
        2,
        "on_fail=skip must not abort — both steps should execute"
    );
    assert!(results[0].skipped, "step 1 should be marked as skipped");
    assert_eq!(results[1].exit_code, 0, "step 2 should succeed");
}

#[tokio::test]
async fn execute_step_bash_git_commit_runs_directly() {
    // Regression test for issue #182: git commit via tool=bash must execute
    // directly via shell, not through an AI tool that prompts for confirmation.
    let tmp = tempfile::tempdir().unwrap();

    // Set up a minimal git repo with a staged file
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
    std::process::Command::new("git")
        .args(["add", "file.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();

    let commit_msg = "fix: test commit for issue 182";
    let step = PlanStep {
        id: 13,
        title: "Commit".into(),
        tool: Some("bash".into()),
        prompt: format!("Create the commit:\n```bash\ngit commit -m \"{commit_msg}\"\n```"),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let mut vars = HashMap::new();
    vars.insert("COMMIT_MSG".into(), commit_msg.into());
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert!(!result.skipped, "bash step must not be skipped");
    assert_eq!(
        result.exit_code, 0,
        "git commit via direct bash must succeed (exit 0), got error: {:?}",
        result.error
    );

    // Verify the commit was actually created
    let log = std::process::Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let log_str = String::from_utf8_lossy(&log.stdout);
    assert!(
        log_str.contains("test commit for issue 182"),
        "commit must exist in git log: {}",
        log_str
    );
}

#[tokio::test]
async fn execute_step_bash_passes_variables_as_env() {
    // Variables are exposed to bash via process environment.
    let tmp = tempfile::tempdir().unwrap();
    let step = PlanStep {
        id: 1,
        title: "env var".into(),
        tool: Some("bash".into()),
        prompt: "```bash\nprintenv MSG > output.txt\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let mut vars = HashMap::new();
    vars.insert("MSG".into(), "substituted_value".into());
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "substituted_value");
}

#[tokio::test]
async fn execute_step_bash_recovers_from_e2big_with_reduced_step_env() {
    // Inject an oversized STEP_* variable to force spawn fallback.
    let tmp = tempfile::tempdir().unwrap();
    let step = PlanStep {
        id: 1,
        title: "recover e2big".into(),
        tool: Some("bash".into()),
        prompt: "```bash\nprintf '%s' \"${MSG}\" > output.txt\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let mut vars = HashMap::new();
    vars.insert("MSG".into(), "ok".into());
    vars.insert("STEP_1_OUTPUT".into(), "x".repeat(16 * 1024 * 1024));

    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert_eq!(
        result.exit_code, 0,
        "bash step should recover from oversized STEP_* env injection"
    );

    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "ok");
}

#[tokio::test]
async fn execute_step_bash_step_output_with_shell_metacharacters_is_not_executed() {
    let plan = ExecutionPlan {
        name: "injection-check".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "emit payload".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nprintf '%s' '$(touch injected_file)'\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
            session: None,
            },
            PlanStep {
                id: 2,
                title: "use prior output".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho ${STEP_1_OUTPUT} > output.txt\nif [ -f injected_file ]; then exit 1; fi\n```"
                    .into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
            session: None,
            },
        ],
    };

    let tmp = tempfile::tempdir().unwrap();
    let vars = HashMap::new();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[1].exit_code, 0);
    assert!(!tmp.path().join("injected_file").exists());

    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "$(touch injected_file)");
}

#[tokio::test]
async fn execute_step_bash_cli_var_with_shell_metacharacters_is_not_executed() {
    let plan = ExecutionPlan {
        name: "cli-vars".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "USER_INPUT".into(),
            default: None,
        }],
        steps: vec![],
    };
    let vars = parse_variables(&["USER_INPUT=$(touch should_not_run)".into()], &plan).unwrap();

    let step = PlanStep {
        id: 1,
        title: "cli var check".into(),
        tool: Some("bash".into()),
        prompt:
            "```bash\necho ${USER_INPUT} > output.txt\nif [ -f should_not_run ]; then exit 1; fi\n```"
                .into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
            session: None,
    };

    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    assert_eq!(result.exit_code, 0);
    assert!(!tmp.path().join("should_not_run").exists());

    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "$(touch should_not_run)");
}

#[tokio::test]
async fn execute_step_bash_without_code_block_runs_raw_prompt() {
    // When no fenced code block is present, execute_bash_step falls back
    // to running the entire prompt as the script.
    let tmp = tempfile::tempdir().unwrap();
    let step = PlanStep {
        id: 1,
        title: "raw prompt".into(),
        tool: Some("bash".into()),
        prompt: "echo raw_exec > output.txt".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let vars = HashMap::new();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;

    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "raw_exec");
}

#[tokio::test]
async fn execute_plan_injects_step_session_variable() {
    let plan = ExecutionPlan {
        name: "session-vars".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "producer".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho produced\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
            PlanStep {
                id: 2,
                title: "consumer".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nif [ -z \"${STEP_1_SESSION+x}\" ]; then exit 1; fi\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
                session: None,
            },
        ],
    };

    let tmp = tempfile::tempdir().unwrap();
    let vars = HashMap::new();
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[1].exit_code, 0);
}

#[test]
fn stale_session_fallback_detects_missing_prefix_errors() {
    let err = anyhow::anyhow!("No session matching prefix '01XYZ'");
    assert!(is_stale_session_error(&err));
}

#[test]
fn stale_session_fallback_ignores_unrelated_errors() {
    let err = anyhow::anyhow!("tool execution failed");
    assert!(!is_stale_session_error(&err));
}
