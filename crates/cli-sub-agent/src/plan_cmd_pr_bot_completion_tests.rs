use std::collections::HashMap;

use super::*;
use crate::plan_cmd::StepResult;

fn failed_step_with_output(step_id: usize, error: &str, output: Option<&str>) -> StepResult {
    StepResult {
        step_id,
        title: "Step 12b: Final Merge (Direct or Post-Rebase)".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(error.to_string()),
        output: output.map(str::to_string),
        session_id: None,
        command: Some("gh pr merge \"${MERGED_PR_VERIFY_REF}\"".to_string()),
        stderr: Some(error.to_string()),
    }
}

#[test]
fn pr_bot_failure_reports_true_merge_state_when_blocking_finding_and_merge_observed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.toml");
    let error = "ERROR: new Codex P2 review finding is valid enough to block merge.";
    let results = vec![failed_step_with_output(
        23,
        error,
        Some("CSA_VAR:MERGED_PR_VERIFY_REF=2269\n"),
    )];
    let vars = HashMap::from([
        ("PR_NUM".to_string(), "2269".to_string()),
        ("REPO".to_string(), "owner/repo".to_string()),
    ]);
    let input = PrBotFailureSideEffectInput {
        workflow_name: "pr-bot",
        workflow_path: &workflow_path,
        project_root: temp.path(),
        results: &results,
        completed_steps: &[],
        vars: &vars,
        failure_summary: "1 step(s) failed (1 execution, 0 unsupported-skip)",
    };
    let facts = PrBotMergeFacts::from_failure_input(&input);
    let observed = PrBotObservedState {
        pr_state: Some("MERGED".to_string()),
        pr_state_error: None,
    };

    let err = verify_pr_bot_failure_side_effects_with_observed(&input, &facts, &observed)
        .expect("merged PR side effect must override stale not-merged failure summary");

    assert!(
        err.to_string().contains("PR #2269 is MERGED"),
        "summary must report true merge state: {err}"
    );
    let details = err.report().render_details_section();
    assert!(
        details.contains("Observed PR state: MERGED")
            && details.contains("final merge step attempted before failure")
            && details.contains("Original failure detail: ERROR: new Codex P2 review finding"),
        "details must preserve merge evidence and original blocking reason: {details}"
    );
}

#[test]
fn pr_bot_failure_preserves_normal_not_merged_failure_summary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![failed_step_with_output(
        10,
        "ERROR: Post-fix re-review found 1 new blocking finding(s). Cannot merge.",
        None,
    )];
    let vars = HashMap::from([("PR_NUM".to_string(), "42".to_string())]);
    let input = PrBotFailureSideEffectInput {
        workflow_name: "pr-bot",
        workflow_path: &workflow_path,
        project_root: temp.path(),
        results: &results,
        completed_steps: &[],
        vars: &vars,
        failure_summary: "1 step(s) failed (1 execution, 0 unsupported-skip)",
    };
    let facts = PrBotMergeFacts::from_failure_input(&input);
    let observed = PrBotObservedState {
        pr_state: Some("OPEN".to_string()),
        pr_state_error: None,
    };

    let err = verify_pr_bot_failure_side_effects_with_observed(&input, &facts, &observed);

    assert!(
        err.is_none(),
        "ordinary blocking failure with OPEN PR should keep the original failure report"
    );
}

#[test]
fn pr_bot_success_reports_merged_pr_state() {
    let facts = PrBotMergeFacts {
        pr_number: Some("42".to_string()),
        merge_verify_ref: Some("42".to_string()),
        repo: Some("owner/repo".to_string()),
        merge_completed_marker: true,
        merge_step_completed: true,
        post_merge_step_completed: true,
        merge_step_attempted: true,
    };
    let observed = PrBotObservedState {
        pr_state: Some("MERGED".to_string()),
        pr_state_error: None,
    };

    let summary = evaluate_pr_bot_success(&facts, &observed)
        .expect("merged state should pass")
        .expect("merged state should produce completion summary");

    assert_eq!(summary, "pr-bot: PR #42 is MERGED");
}

#[test]
fn pr_bot_success_fails_closed_when_merge_effect_is_missing() {
    let facts = PrBotMergeFacts {
        pr_number: Some("42".to_string()),
        merge_verify_ref: Some("42".to_string()),
        repo: Some("owner/repo".to_string()),
        merge_completed_marker: true,
        merge_step_completed: true,
        post_merge_step_completed: false,
        merge_step_attempted: true,
    };
    let observed = PrBotObservedState {
        pr_state: Some("OPEN".to_string()),
        pr_state_error: None,
    };

    let failures = evaluate_pr_bot_success(&facts, &observed)
        .expect_err("successful pr-bot result must fail closed when PR is still open");

    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("PR 42 state is OPEN; expected MERGED")),
        "missing merge effect must be actionable: {failures:?}"
    );
}

#[test]
fn pr_bot_failure_facts_recover_merge_ref_from_failed_step_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![failed_step_with_output(
        22,
        "ERROR: checkout failed after merge",
        Some("CSA_VAR:MERGED_PR_VERIFY_REF=99\n"),
    )];
    let vars = HashMap::from([("PR_NUM".to_string(), "42".to_string())]);
    let input = PrBotFailureSideEffectInput {
        workflow_name: "pr-bot",
        workflow_path: &workflow_path,
        project_root: temp.path(),
        results: &results,
        completed_steps: &[],
        vars: &vars,
        failure_summary: "1 step(s) failed (1 execution, 0 unsupported-skip)",
    };

    let facts = PrBotMergeFacts::from_failure_input(&input);

    assert_eq!(facts.pr_reference(), Some("99"));
}
