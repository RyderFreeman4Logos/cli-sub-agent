use super::*;
use weave::compiler::{ExecutionPlan, FailAction, PlanStep};

#[path = "plan_cmd_tests_pr_bot_degraded_support.rs"]
mod support;
use support::*;

#[tokio::test]
async fn execute_pr_bot_local_review_accepts_same_sha_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);

    let vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("native fallback evidence should satisfy local review");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 2 should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Fast-path: structured native review bypass artifact covers current HEAD"),
        "expected native bypass fast-path output: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "current-head native fallback evidence must avoid an unnecessary CSA review launch"
    );
}

#[tokio::test]
async fn execute_pr_bot_local_review_rejects_stale_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let stale_head = "1234567890abcdef1234567890abcdef12345678";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), stale_head);

    let vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-stale-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("stale evidence should fail through the CSA review path");

    assert_eq!(results.len(), 1);
    assert_ne!(
        results[0].exit_code, 0,
        "stale bypass evidence must not satisfy local review"
    );
    assert!(
        csa_called_path.exists(),
        "stale fallback evidence should force pr-bot back to CSA review"
    );
}

#[tokio::test]
async fn execute_pr_bot_local_review_rejects_review_check_skip_audit_log() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_review_check_skip_audit_log(tmp.path());

    let vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-skip-log-rejected".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("hook skip log should fall through");

    assert_eq!(results.len(), 1);
    assert_ne!(results[0].exit_code, 0, "hook skip log must fail");
    assert!(csa_called_path.exists(), "should force CSA review");
    let stderr = results[0].stderr.as_deref().unwrap_or("");
    assert!(stderr.contains("audit-only") || stderr.contains("Audit-only"));
}

#[tokio::test]
async fn execute_pr_bot_bot_unavailable_gate_accepts_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);

    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("MERGE_COMPLETED".into(), "false".into());
    vars.insert("TEST_CLOUD_BOT".into(), "false".into());
    vars.insert("PR_NUM".into(), "1788".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Step 4a: Check Cloud Bot Configuration"]);
    let plan = ExecutionPlan {
        name: "pr-bot-bot-unavailable-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("bot-unavailable native fallback evidence should satisfy local review");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 4a should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Merge-without-bot native review bypass covers HEAD"),
        "expected bot-unavailable bypass output: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "bot-unavailable native fallback evidence must avoid an unnecessary CSA review launch"
    );
}

#[tokio::test]
async fn execute_pr_bot_post_fix_fallback_accepts_native_bypass_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let current_head = "abcdef1234567890abcdef1234567890abcdef12";
    let csa_called_path = install_pr_bot_local_review_stubs(tmp.path(), current_head);
    write_native_review_bypass_artifact(tmp.path(), current_head);

    let mut vars = pr_bot_local_review_vars(tmp.path(), &csa_called_path);
    vars.insert("BOT_HAS_ISSUES".into(), "true".into());
    vars.insert("BOT_SETTLE_SECS".into(), "0".into());
    vars.insert("BOT_UNAVAILABLE".into(), "false".into());
    vars.insert("CLOUD_BOT".into(), "true".into());
    vars.insert("CLOUD_BOT_LOGIN".into(), "codex".into());
    vars.insert("CLOUD_BOT_NAME".into(), "codex".into());
    vars.insert("CLOUD_BOT_POLL_MAX_SECONDS".into(), "1".into());
    vars.insert("CLOUD_BOT_RETRIGGER_CMD".into(), "@codex review".into());
    vars.insert("CLOUD_BOT_WAIT_SECONDS".into(), "0".into());
    vars.insert("FALLBACK_REVIEW_HAS_ISSUES".into(), "false".into());
    vars.insert("MERGE_COMPLETED".into(), "false".into());
    vars.insert("POLL_IDLE_TIMEOUT".into(), "1800".into());
    vars.insert("POLL_MAX_TIMEOUT".into(), "1800".into());
    vars.insert("PR_NUM".into(), "1788".into());
    vars.insert("REPO".into(), "RyderFreeman4Logos/cli-sub-agent".into());
    vars.insert("ROUND_LIMIT_REACHED".into(), "false".into());
    vars.insert("TEST_SESSION_WAIT_TIMEOUT".into(), "true".into());
    let (variables, steps) =
        pr_bot_plan_steps_by_title(&["Step 10b: Post-Fix Re-Review Gate (HARD GATE)"]);
    let plan = ExecutionPlan {
        name: "pr-bot-post-fix-native-review-bypass".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("post-fix native fallback evidence should satisfy local review");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].exit_code, 0, "Step 10b should pass");
    let output = results[0].output.as_deref().unwrap_or("");
    assert!(
        output.contains("Local fallback native review bypass covers HEAD"),
        "expected post-fix fallback bypass output: {output}"
    );
    assert!(
        output.contains("CSA_VAR:BOT_HAS_ISSUES=false"),
        "post-fix fallback should clear bot issue state after native review evidence: {output}"
    );
    assert!(
        output.contains("CSA_VAR:LOCAL_REVIEW_SESSION_ID=native-review-bypass-abcdef123456"),
        "native fallback should publish a bounded synthetic session id: {output}"
    );
    assert!(
        !csa_called_path.exists(),
        "post-fix native fallback evidence must avoid an unnecessary CSA review launch"
    );
}

#[tokio::test]
async fn execute_pr_bot_degraded_local_fallback_records_rationale_and_reaches_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_pr_bot_degraded_gate_stubs(tmp.path());
    let vars = pr_bot_degraded_gate_vars(&capture_path, "medium", "degraded");
    let (variables, steps) = pr_bot_plan_steps_by_title(&[
        "Step 10b: Post-Fix Re-Review Gate (HARD GATE)",
        "Step 6a: Merge Without Bot",
        "Step 12b: Final Merge (Direct or Post-Rebase)",
    ]);
    let final_merge_condition = steps[2].condition.clone();
    let plan = ExecutionPlan {
        name: "pr-bot-degraded-local-fallback".into(),
        description: String::new(),
        variables,
        steps: vec![
            steps[0].clone(),
            steps[1].clone(),
            PlanStep {
                id: 99,
                title: "final merge condition reached".into(),
                tool: Some("bash".into()),
                prompt: "```bash\ntouch reached-final-merge\n```".into(),
                tier: None,
                depends_on: vec![],
                on_fail: FailAction::Abort,
                condition: final_merge_condition,
                loop_var: None,
                session: None,
                workspace_access: None,
            },
        ],
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("degraded fallback plan should execute");

    assert_eq!(results.len(), 3, "all gate steps should execute");
    assert!(
        results.iter().all(|result| !result.skipped),
        "no step should be skipped: {:?}",
        results
            .iter()
            .map(|r| (&r.title, r.skipped))
            .collect::<Vec<_>>()
    );
    assert!(
        results.iter().all(|result| result.exit_code == 0),
        "all steps should pass: {:?}",
        results
            .iter()
            .map(|r| (&r.title, r.exit_code, &r.error, &r.stderr))
            .collect::<Vec<_>>()
    );
    let gate_output = results[0].output.as_deref().unwrap_or("");
    assert!(gate_output.contains("CSA_VAR:BOT_UNAVAILABLE=true"));
    assert!(gate_output.contains("CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=false"));
    assert!(gate_output.contains("CSA_VAR:BOT_HAS_ISSUES=false"));
    assert!(
        gate_output.contains(
            "CSA_VAR:MERGE_WITHOUT_BOT_REASON_KIND=local_review_degraded_no_blocking_findings"
        ),
        "post-fix gate must publish the degraded-review rationale key"
    );
    let rationale_output = results[1].output.as_deref().unwrap_or("");
    assert!(
        rationale_output.contains("merge rationale recorded; proceed to Final Merge"),
        "Step 6a should emit the proceed directive"
    );
    assert!(
        tmp.path().join("reached-final-merge").exists(),
        "final merge condition should be reachable after Step 6a"
    );

    let comments = std::fs::read_to_string(&capture_path).unwrap();
    assert!(comments.contains("local fallback review degraded"));
    assert!(
        comments.contains("MEDIUM/P2 comments are non-blocking follow-up by policy"),
        "rationale must compose with MEDIUM/P2 non-blocking policy"
    );
    assert!(
        comments.contains("Local fallback review could not complete with an available reviewer"),
        "rationale should explain the degraded local fallback"
    );
}

#[tokio::test]
async fn execute_pr_bot_degraded_local_fallback_still_blocks_high_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let capture_path = install_pr_bot_degraded_gate_stubs(tmp.path());
    let vars = pr_bot_degraded_gate_vars(&capture_path, "high", "degraded");
    let (variables, steps) = pr_bot_plan_steps_by_title(&[
        "Step 10b: Post-Fix Re-Review Gate (HARD GATE)",
        "Step 6a: Merge Without Bot",
    ]);
    let plan = ExecutionPlan {
        name: "pr-bot-high-finding-still-blocks".into(),
        description: String::new(),
        variables,
        steps,
    };

    let results = execute_plan(&plan, &vars, tmp.path(), None, None)
        .await
        .expect("plan should return failed step result");

    assert_eq!(
        results.len(),
        1,
        "Step 6a must not run after blocking finding"
    );
    assert_ne!(results[0].exit_code, 0, "blocking HIGH finding must abort");
    assert!(
        results[0]
            .stderr
            .as_deref()
            .unwrap_or("")
            .contains("Post-fix re-review found 1 new blocking finding"),
        "stderr should report the blocking bot finding"
    );
    assert!(
        !tmp.path().join("reached-final-merge").exists(),
        "merge path must remain unreachable"
    );
    let comments = std::fs::read_to_string(&capture_path).unwrap_or_default();
    assert!(
        !comments.contains("local fallback review degraded"),
        "blocking findings must not get a degraded-review merge rationale"
    );
}
