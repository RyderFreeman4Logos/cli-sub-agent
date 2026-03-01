use super::*;
use std::collections::{HashMap, HashSet};
use weave::compiler::VariableDecl;

#[test]
fn safe_plan_name_normalizes_non_alphanumeric_characters() {
    assert_eq!(safe_plan_name("Dev2Merge Workflow"), "dev2merge_workflow");
    assert_eq!(safe_plan_name("mktd@2026!"), "mktd_2026");
}

#[test]
fn load_plan_resume_context_reads_running_journal() {
    let tmp = tempfile::tempdir().unwrap();
    let workflow_path = tmp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname='test'\n").unwrap();

    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![VariableDecl {
            name: "FEATURE".into(),
            default: Some("default".into()),
        }],
        steps: vec![],
    };

    let journal_path = tmp.path().join("test.journal.json");
    let journal = PlanRunJournal {
        schema_version: PLAN_JOURNAL_SCHEMA_VERSION,
        workflow_name: "test".into(),
        workflow_path: normalize_path(&workflow_path),
        status: "running".into(),
        vars: HashMap::from([
            ("FEATURE".to_string(), "from-journal".to_string()),
            ("STEP_1_OUTPUT".to_string(), "cached".to_string()),
        ]),
        completed_steps: vec![1, 2],
        last_error: None,
    };
    persist_plan_journal(&journal_path, &journal).unwrap();

    let cli_vars = HashMap::from([("FEATURE".to_string(), "from-cli".to_string())]);
    let ctx = load_plan_resume_context(&plan, &workflow_path, &journal_path, &cli_vars).unwrap();

    assert!(ctx.resumed);
    assert!(ctx.completed_steps.contains(&1));
    assert!(ctx.completed_steps.contains(&2));
    assert_eq!(
        ctx.initial_vars.get("FEATURE").map(String::as_str),
        Some("from-cli")
    );
    assert_eq!(
        ctx.initial_vars.get("STEP_1_OUTPUT").map(String::as_str),
        Some("cached")
    );
}

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
fn parse_variables_rejects_invalid_variable_name() {
    let plan = ExecutionPlan {
        name: "test".into(),
        description: String::new(),
        variables: vec![],
        steps: vec![],
    };

    let err = parse_variables(&["BAD-NAME=value".into()], &plan);
    assert!(err.is_err());
    let message = err.unwrap_err().to_string();
    assert!(message.contains("[A-Za-z_][A-Za-z0-9_]*"));
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
fn extract_output_assignment_markers_parses_uppercase_assignments() {
    let output = r#"
CSA_VAR:BOT_UNAVAILABLE=true
CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=false
noise line
lowercase=value
"#;
    let allowlist = HashSet::from([
        "BOT_UNAVAILABLE".to_string(),
        "FALLBACK_REVIEW_HAS_ISSUES".to_string(),
    ]);
    let markers = extract_output_assignment_markers(output, &allowlist);
    assert_eq!(
        markers,
        vec![
            ("BOT_UNAVAILABLE".to_string(), "true".to_string()),
            (
                "FALLBACK_REVIEW_HAS_ISSUES".to_string(),
                "false".to_string()
            )
        ]
    );
}

#[test]
fn extract_output_assignment_markers_ignores_non_allowlisted_keys() {
    let output = r#"
CSA_VAR:BOT_UNAVAILABLE=true
CSA_VAR:PATH=/tmp/unsafe
"#;
    let allowlist = HashSet::from(["BOT_UNAVAILABLE".to_string()]);
    let markers = extract_output_assignment_markers(output, &allowlist);
    assert_eq!(
        markers,
        vec![("BOT_UNAVAILABLE".to_string(), "true".to_string())]
    );
}

#[test]
fn extract_output_assignment_markers_ignores_unprefixed_assignments() {
    let output = r#"
BOT_UNAVAILABLE=true
CSA_VAR:BOT_UNAVAILABLE=false
"#;
    let allowlist = HashSet::from(["BOT_UNAVAILABLE".to_string()]);
    let markers = extract_output_assignment_markers(output, &allowlist);
    assert_eq!(
        markers,
        vec![("BOT_UNAVAILABLE".to_string(), "false".to_string())]
    );
}

#[test]
fn is_assignment_marker_key_accepts_expected_format() {
    assert!(is_assignment_marker_key("BOT_UNAVAILABLE"));
    assert!(is_assignment_marker_key("_INTERNAL_FLAG1"));
    assert!(is_assignment_marker_key("bot_unavailable"));
    assert!(!is_assignment_marker_key("1BAD"));
    assert!(!is_assignment_marker_key("BAD-KEY"));
}

#[test]
fn should_inject_assignment_markers_only_for_bash_steps() {
    let bash_step = PlanStep {
        id: 1,
        title: "bash".into(),
        tool: Some("Bash".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let codex_step = PlanStep {
        id: 2,
        title: "codex".into(),
        tool: Some("codex".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };
    let tier_only_step = PlanStep {
        id: 3,
        title: "tier-only".into(),
        tool: None,
        prompt: String::new(),
        tier: Some("tier-1-fast".into()),
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
        session: None,
    };

    assert!(should_inject_assignment_markers(&bash_step));
    assert!(!should_inject_assignment_markers(&codex_step));
    assert!(!should_inject_assignment_markers(&tier_only_step));
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
        session: None,
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
        session: None,
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
        session: None,
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
        session: None,
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
        session: None,
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
        session: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
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
        session: None,
    };
    let mut vars = HashMap::new();
    vars.insert("FLAG".into(), "yes".into());
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
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
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
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
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
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
        session: None,
    };
    let vars = HashMap::new();
    let tmp = tempfile::tempdir().unwrap();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
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
                session: None,
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
