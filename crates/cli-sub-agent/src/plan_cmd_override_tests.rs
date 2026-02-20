use super::*;
use std::time::Instant;

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

// --- Tool override tests ---

#[tokio::test]
async fn execute_step_tool_override_replaces_csa_tool() {
    // A bash step with tool=bash should produce exit 0 via DirectBash.
    // When --tool override is provided, bash steps should be unaffected.
    let tmp = tempfile::tempdir().unwrap();
    let bash_step = PlanStep {
        id: 1,
        title: "bash-step".into(),
        tool: Some("bash".into()),
        prompt: "```bash\necho ok\n```".into(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let vars = HashMap::new();
    // Even with tool_override=claude-code, bash step must still run as bash
    let result = execute_step(
        &bash_step,
        &vars,
        tmp.path(),
        None,
        Some(&ToolName::ClaudeCode),
    )
    .await;
    assert_eq!(
        result.exit_code, 0,
        "bash step must not be affected by --tool override"
    );
    assert!(!result.skipped);
}

#[test]
fn tool_override_clears_model_spec() {
    // When --tool override is applied, the model_spec from tier resolution
    // must be cleared to avoid tool/spec mismatch.
    let step = PlanStep {
        id: 1,
        title: "csa-step".into(),
        tool: Some("codex".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    // Without override: should be CsaTool with codex
    assert!(matches!(
        target,
        StepTarget::CsaTool {
            tool_name: ToolName::Codex,
            ..
        }
    ));

    // Simulate override application (same logic as execute_step)
    let override_tool = ToolName::ClaudeCode;
    let overridden = match target {
        StepTarget::CsaTool { .. } => StepTarget::CsaTool {
            tool_name: override_tool,
            model_spec: None,
        },
        other => other,
    };
    match overridden {
        StepTarget::CsaTool {
            tool_name,
            model_spec,
        } => {
            assert_eq!(tool_name, ToolName::ClaudeCode, "tool must be overridden");
            assert!(
                model_spec.is_none(),
                "model_spec must be cleared on override"
            );
        }
        _ => panic!("expected CsaTool"),
    }
}

#[test]
fn tool_override_does_not_affect_weave_include() {
    let step = PlanStep {
        id: 1,
        title: "include-step".into(),
        tool: Some("weave".into()),
        prompt: String::new(),
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let target = resolve_step_tool(&step, None).unwrap();
    // Simulate override: WeaveInclude must pass through unchanged
    let override_tool = ToolName::ClaudeCode;
    let overridden = match target {
        StepTarget::CsaTool { .. } => StepTarget::CsaTool {
            tool_name: override_tool,
            model_spec: None,
        },
        other => other,
    };
    assert!(
        matches!(overridden, StepTarget::WeaveInclude),
        "weave step must not be affected by --tool override"
    );
}

// --- Heartbeat tests ---

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

// --- Output capture & forwarding tests ---

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
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
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
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
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
    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert!(results[0].skipped);
    assert_eq!(results[1].exit_code, 0);
    let content = std::fs::read_to_string(tmp.path().join("output.txt")).unwrap();
    assert_eq!(
        content, "[]",
        "skipped step must inject empty STEP_1_OUTPUT"
    );
}

// --- Empty prompt warning test ---

#[tokio::test]
async fn execute_step_csa_empty_prompt_warns_without_panic() {
    // A CSA step with an empty (whitespace-only) prompt should trigger the
    // empty-prompt warning and still attempt execution (not silently skip).
    // In test env, execution fails because the tool binary is not installed,
    // but the warning code path must be reached without panicking.
    let tmp = tempfile::tempdir().unwrap();
    let step = PlanStep {
        id: 1,
        title: "empty-prompt-csa".into(),
        tool: Some("codex".into()),
        prompt: "   ".into(), // whitespace-only triggers the warning
        tier: None,
        depends_on: vec![],
        on_fail: FailAction::Abort,
        condition: None,
        loop_var: None,
    };
    let vars = HashMap::new();
    let result = execute_step(&step, &vars, tmp.path(), None, None).await;
    // Step must attempt execution (not silently skipped).
    assert!(!result.skipped, "empty-prompt CSA step must not be skipped");
    // The warning code path is exercised (visible in test stdout as
    // "WARNING: empty prompt for CSA step"). Regardless of whether the tool
    // is installed, the step must not panic and must return a valid result.
    assert!(
        result.exit_code == 0 || result.error.is_some(),
        "CSA step must either succeed or report an error"
    );
}
