use super::*;
use std::time::Instant;
use weave::compiler::VariableDecl;

#[test]
fn parse_variables_uses_defaults() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![
            VariableDecl {
                name: "FOO".into(),
                default: Some("bar".into()),
            },
            VariableDecl {
                name: "BAZ".into(),
                default: None,
            },
        ],
        steps: vec![],
    };

    let vars = parse_variables(&[], &plan).unwrap();
    assert_eq!(vars.get("FOO").map(String::as_str), Some("bar"));
    assert!(!vars.contains_key("BAZ"));
}

#[test]
fn parse_variables_cli_overrides_default() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FOO".into(),
            default: Some("default".into()),
        }],
        steps: vec![],
    };

    let vars = parse_variables(&["FOO=override".into()], &plan).unwrap();
    assert_eq!(vars.get("FOO").map(String::as_str), Some("override"));
}

#[test]
fn parse_variables_rejects_invalid_format() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let err = parse_variables(&["NO_EQUALS_SIGN".into()], &plan);
    assert!(err.is_err());
}

#[test]
fn substitute_vars_replaces_placeholders() {
    let mut vars = HashMap::new();
    vars.insert("NAME".into(), "world".into());
    vars.insert("COUNT".into(), "42".into());

    assert_eq!(
        substitute_vars("Hello ${NAME}, count=${COUNT}!", &vars),
        "Hello world, count=42!"
    );
}

#[test]
fn substitute_vars_leaves_unknown_placeholders() {
    let vars = HashMap::new();
    assert_eq!(substitute_vars("${UNKNOWN}", &vars), "${UNKNOWN}");
}

#[test]
fn extract_bash_code_block_finds_bash_fence() {
    let prompt = "Run this:\n```bash\necho hello\n```\nDone.";
    assert_eq!(extract_bash_code_block(prompt), Some("echo hello"));
}

#[test]
fn extract_bash_code_block_finds_plain_fence() {
    let prompt = "```\nls -la\n```";
    assert_eq!(extract_bash_code_block(prompt), Some("ls -la"));
}

#[test]
fn extract_bash_code_block_returns_none_when_no_fence() {
    assert_eq!(extract_bash_code_block("just some text"), None);
}

#[test]
fn truncate_short_string() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn truncate_long_string() {
    let s = "a".repeat(100);
    let result = truncate(&s, 10);
    assert_eq!(result.len(), 13); // 10 chars + "..."
    assert!(result.ends_with("..."));
}

#[test]
fn resolve_step_tool_explicit_bash_returns_direct_bash() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("bash".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(
        matches!(target, StepTarget::DirectBash),
        "tool=bash must resolve to DirectBash, not a CSA tool"
    );
}

#[test]
fn resolve_step_tool_explicit_codex() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("codex".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(matches!(
        target,
        StepTarget::CsaTool {
            tool_name: ToolName::Codex,
            ..
        }
    ));
}

#[test]
fn resolve_step_tool_fallback_no_config() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: None,
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(matches!(
        target,
        StepTarget::CsaTool {
            tool_name: ToolName::Codex,
            ..
        }
    ));
}

#[test]
fn resolve_step_tool_weave_returns_include_marker() {
    let step = PlanStep {
        id: 1,
        title: "include".into(),
        tool: Some("weave".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    assert!(matches!(target, StepTarget::WeaveInclude));
}

#[test]
fn resolve_step_tool_unknown_tool_errors() {
    let step = PlanStep {
        id: 1,
        title: "test".into(),
        tool: Some("nonexistent".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    assert!(resolve_step_tool(&step, None).is_err());
}

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
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(result.skipped, "unset condition var must skip");
    assert_eq!(
        result.exit_code, 0,
        "condition-false skip is intentional, not a failure"
    );
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
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
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
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
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
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(result.skipped);
    assert_eq!(
        result.exit_code, 0,
        "INCLUDE skip should be success (harmless)"
    );
}

#[tokio::test]
async fn execute_step_bash_runs_code_block() {
    let step = PlanStep {
        id: 1,
        title: "echo test".into(),
        tool: Some("bash".into()),
        prompt: "Run this:\n```bash\necho hello\n```\n".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert!(!result.skipped);
    assert_eq!(result.exit_code, 0);
}

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
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None).await.unwrap();
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
        }],
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].skipped, "true condition must execute");
    assert_eq!(results[0].exit_code, 0);
}

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
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None).await.unwrap();
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
            },
        ],
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let results = execute_plan(&plan, &vars, tmp.path(), None).await.unwrap();
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
    };
    let mut vars = HashMap::new();
    vars.insert("COMMIT_MSG".into(), commit_msg.into());
    let result = execute_step(&step, &vars, tmp.path(), None).await;

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
async fn execute_step_bash_substitutes_variables_in_code_block() {
    // Verify that ${VAR} placeholders inside bash code blocks are substituted
    // before execution.
    let tmp = tempfile::tempdir().unwrap();
    let step = PlanStep {
        id: 1,
        title: "var substitution".into(),
        tool: Some("bash".into()),
        prompt: "```bash\necho ${MSG} > output.txt\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let mut vars = HashMap::new();
    vars.insert("MSG".into(), "substituted_value".into());
    let result = execute_step(&step, &vars, tmp.path(), None).await;

    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "substituted_value");
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
    };
    let vars = HashMap::new();
    let result = execute_step(&step, &vars, tmp.path(), None).await;

    assert_eq!(result.exit_code, 0);
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(content.trim(), "raw_exec");
}

#[test]
fn resolve_step_tool_all_explicit_tools() {
    // Verify all explicit tool names resolve to the correct StepTarget variant.
    let cases = [
        ("bash", true),         // DirectBash
        ("claude-code", false), // CsaTool
        ("codex", false),       // CsaTool
        ("gemini-cli", false),  // CsaTool
        ("opencode", false),    // CsaTool
        ("weave", false),       // WeaveInclude (not CsaTool, not DirectBash)
    ];
    for (tool_str, expect_direct_bash) in cases {
        let step = PlanStep {
            id: 1,
            title: format!("test-{tool_str}"),
            tool: Some(tool_str.into()),
            prompt: String::new(),
            tier: None,
            depends_on: vec![],
            on_fail: FailAction::Abort,
            condition: None,
            loop_var: None,
        };
        let target = resolve_step_tool(&step, None).unwrap();
        if expect_direct_bash {
            assert!(
                matches!(target, StepTarget::DirectBash),
                "tool={tool_str} must resolve to DirectBash"
            );
        } else if tool_str == "weave" {
            assert!(
                matches!(target, StepTarget::WeaveInclude),
                "tool=weave must resolve to WeaveInclude"
            );
        } else {
            assert!(
                matches!(target, StepTarget::CsaTool { .. }),
                "tool={tool_str} must resolve to CsaTool"
            );
        }
    }
}

#[tokio::test]
async fn run_with_heartbeat_returns_success_exit_code() {
    let (code, output) = run_with_heartbeat(
        "[1/heartbeat]",
        async { Ok::<(i32, Option<String>), anyhow::Error>((0, Some("ok".into()))) },
        Instant::now(),
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(output.as_deref(), Some("ok"));
}

#[tokio::test]
async fn run_with_heartbeat_maps_errors_to_exit_code_one() {
    let (code, output) = run_with_heartbeat(
        "[1/heartbeat]",
        async { Err::<(i32, Option<String>), anyhow::Error>(anyhow::anyhow!("boom")) },
        Instant::now(),
    )
    .await;
    assert_eq!(code, 1);
    assert!(output.is_none());
}

#[tokio::test]
async fn execute_step_bash_captures_stdout_in_output() {
    let tmp = tempfile::tempdir().unwrap();
    let step = PlanStep {
        id: 1,
        title: "capture output".into(),
        tool: Some("bash".into()),
        prompt: "```bash\necho captured_value\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let vars = HashMap::new();
    let result = execute_step(&step, &vars, tmp.path(), None).await;
    assert_eq!(result.exit_code, 0);
    assert!(
        result.output.is_some(),
        "bash step stdout must be captured in output"
    );
    assert_eq!(result.output.as_deref().unwrap().trim(), "captured_value");
}

#[tokio::test]
async fn execute_plan_injects_step_output_variables() {
    // Step 1 produces output; Step 2 uses ${STEP_1_OUTPUT} to write it to a file.
    let tmp = tempfile::tempdir().unwrap();
    let plan = ExecutionPlan {
        name: "output-forwarding".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "produce output".into(),
                tool: Some("bash".into()),
                prompt: "```bash\necho step_one_result\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
            },
            PlanStep {
                id: 2,
                title: "consume output".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nprintf '%s' \"${STEP_1_OUTPUT}\" > output.txt\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
            },
        ],
    };
    let vars = HashMap::new();
    let results = execute_plan(&plan, &vars, tmp.path(), None).await.unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].exit_code, 0);
    assert_eq!(results[1].exit_code, 0);
    // Verify step 2 received step 1's output via variable substitution
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(
        content.trim(),
        "step_one_result",
        "STEP_1_OUTPUT must be injected and usable by step 2"
    );
}

#[tokio::test]
async fn execute_plan_skipped_step_injects_empty_output() {
    // A condition-false step should inject empty STEP_N_OUTPUT.
    let tmp = tempfile::tempdir().unwrap();
    let plan = ExecutionPlan {
        name: "skipped-output".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![
            PlanStep {
                id: 1,
                title: "skipped step".into(),
                tool: Some("bash".into()),
                prompt: "echo should_not_run".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: Some("${NEVER_SET}".into()),
                loop_var: None,
            },
            PlanStep {
                id: 2,
                title: "check empty".into(),
                tool: Some("bash".into()),
                prompt: "```bash\nprintf '%s' \"[${STEP_1_OUTPUT}]\" > output.txt\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: None,
                loop_var: None,
            },
        ],
    };
    let vars = HashMap::new();
    let results = execute_plan(&plan, &vars, tmp.path(), None).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].skipped);
    assert_eq!(results[1].exit_code, 0);
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(
        content, "[]",
        "skipped step must inject empty STEP_1_OUTPUT"
    );
}
