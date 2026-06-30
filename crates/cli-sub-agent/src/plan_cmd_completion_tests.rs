use super::*;

fn completed_facts() -> Dev2MergeCompletionFacts {
    Dev2MergeCompletionFacts {
        dev2merge_skip: false,
        already_resolved_skip: false,
        publish_started: true,
        push_gate_completed: true,
        review_verdict_completed: true,
        pr_completed: true,
        pr_bot_completed: true,
        post_merge_sync_completed: true,
        branch: Some("fix/issue".to_string()),
        default_branch: Some("main".to_string()),
        issue_number: None,
        pr_number: Some("42".to_string()),
        pr_bot_done_marker: Some(PathBuf::from("/tmp/pr-bot.done")),
        review_completed: true,
        review_verdict_checked: true,
        pushed: true,
        already_resolved_message: None,
        initial_head: Some("0000000000000000000000000000000000000000".to_string()),
    }
}

#[test]
fn verify_dev2merge_completion_without_publish_steps_returns_structured_failure_report() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname = 'dev2merge'\n")
        .expect("workflow should be written");
    let vars = HashMap::from([("DEV2MERGE_SKIP".to_string(), "false".to_string())]);
    let completed_steps = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let snapshot = PlanCompletionSnapshot {
        initial_branch: Some("fix/issue".to_string()),
        initial_head: None,
    };

    let err = verify_plan_completion(PlanCompletionInput {
        workflow_name: "dev2merge",
        workflow_path: &workflow_path,
        project_root: temp.path(),
        results: &[],
        completed_steps: &completed_steps,
        vars: &vars,
        snapshot: &snapshot,
    })
    .expect_err("dev2merge success without lifecycle side effects must fail closed");

    assert_eq!(
        err.to_string(),
        "dev2merge lifecycle side-effect verification failed: publish gate never started"
    );
    let summary = err.report().render_summary_section();
    assert!(
        summary.contains("Failed step: 18 (Dev2merge Lifecycle Side-Effect Verification) exited 1"),
        "summary should expose the synthetic lifecycle verification step: {summary}"
    );
    let details = err.report().render_details_section();
    assert!(
        details.contains("Publish Gate (Step 13) did not run")
            && details.contains("DEV2MERGE_SKIP was not set by the Already-Resolved Check")
            && details.contains("missing lifecycle gate"),
        "details should name the missing lifecycle side-effect class: {details}"
    );
}

#[test]
fn verify_dev2merge_completion_allows_already_resolved_skip_from_step_zero() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname = 'dev2merge'\n")
        .expect("workflow should be written");
    let vars = HashMap::from([("DEV2MERGE_SKIP".to_string(), "true".to_string())]);
    let results = vec![StepResult {
        step_id: 0,
        title: "Already-Resolved Check".to_string(),
        exit_code: 0,
        duration_secs: 0.0,
        skipped: false,
        error: None,
        output: Some(
            "dev2merge: issue is already CLOSED\nCSA_VAR:DEV2MERGE_SKIP=true\n".to_string(),
        ),
        session_id: None,
        command: Some("bash step".to_string()),
        stderr: None,
    }];
    let snapshot = PlanCompletionSnapshot {
        initial_branch: Some("fix/issue".to_string()),
        initial_head: None,
    };

    let summary = verify_plan_completion(PlanCompletionInput {
        workflow_name: "dev2merge",
        workflow_path: &workflow_path,
        project_root: temp.path(),
        results: &results,
        completed_steps: &[0],
        vars: &vars,
        snapshot: &snapshot,
    })
    .expect("already-resolved dev2merge skip should be accepted")
    .expect("already-resolved skip should produce completion context");

    assert!(
        summary.contains("already-resolved no-op")
            && summary.contains("state=dev2merge: issue is already CLOSED"),
        "summary should distinguish explicit no-op from transport success: {summary}"
    );
}

#[test]
fn dev2merge_completion_passes_when_publish_side_effects_are_complete() {
    let facts = completed_facts();
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: Some(true),
        pr_state: Some("MERGED".to_string()),
        pr_state_error: None,
        current_branch: Some("main".to_string()),
        current_head: Some("1111111111111111111111111111111111111111".to_string()),
        branch_head: Some("2222222222222222222222222222222222222222".to_string()),
        branch_moved: Some(true),
        implementation_commits_ahead: Some(2),
        implementation_commits_created: Some(1),
        implementation_diff_paths: Some(3),
        implementation_diff_empty: Some(false),
    };

    let failures = evaluate_dev2merge_completion(&facts, &observed);

    assert!(
        failures.is_empty(),
        "complete publish side effects must not fail: {failures:?}"
    );
}

#[test]
fn dev2merge_completion_fails_when_pr_was_not_captured() {
    let mut facts = completed_facts();
    facts.pr_number = None;
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: Some(true),
        pr_state: None,
        pr_state_error: None,
        current_branch: None,
        current_head: None,
        branch_head: None,
        branch_moved: None,
        implementation_commits_ahead: None,
        implementation_commits_created: None,
        implementation_diff_paths: None,
        implementation_diff_empty: None,
    };

    let failures = evaluate_dev2merge_completion(&facts, &observed);

    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("PR_NUMBER was not captured")),
        "missing PR_NUMBER must be actionable: {failures:?}"
    );
}

#[test]
fn dev2merge_completion_fails_when_pr_is_not_merged() {
    let facts = completed_facts();
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: Some(true),
        pr_state: Some("OPEN".to_string()),
        pr_state_error: None,
        current_branch: None,
        current_head: None,
        branch_head: None,
        branch_moved: None,
        implementation_commits_ahead: None,
        implementation_commits_created: None,
        implementation_diff_paths: None,
        implementation_diff_empty: None,
    };

    let failures = evaluate_dev2merge_completion(&facts, &observed);

    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("PR state is OPEN; expected MERGED")),
        "unmerged PR must fail closed: {failures:?}"
    );
}

#[test]
fn dev2merge_completion_fails_when_pr_bot_marker_is_missing() {
    let facts = completed_facts();
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: Some(false),
        pr_state: Some("MERGED".to_string()),
        pr_state_error: None,
        current_branch: None,
        current_head: None,
        branch_head: None,
        branch_moved: None,
        implementation_commits_ahead: None,
        implementation_commits_created: None,
        implementation_diff_paths: None,
        implementation_diff_empty: None,
    };

    let failures = evaluate_dev2merge_completion(&facts, &observed);

    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("pr-bot completion marker is missing")),
        "missing marker must fail closed: {failures:?}"
    );
}

#[test]
fn dev2merge_completion_fails_when_publish_step_was_skipped() {
    let mut facts = completed_facts();
    facts.push_gate_completed = false;
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: Some(true),
        pr_state: Some("MERGED".to_string()),
        pr_state_error: None,
        current_branch: None,
        current_head: None,
        branch_head: None,
        branch_moved: None,
        implementation_commits_ahead: None,
        implementation_commits_created: None,
        implementation_diff_paths: None,
        implementation_diff_empty: None,
    };

    let failures = evaluate_dev2merge_completion(&facts, &observed);

    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("Push Gate (Step 13) did not complete")),
        "skipped publish step must be named: {failures:?}"
    );
}

#[test]
fn dev2merge_success_summary_names_side_effects_and_next_action() {
    let facts = completed_facts();
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: Some(true),
        pr_state: Some("MERGED".to_string()),
        pr_state_error: None,
        current_branch: Some("main".to_string()),
        current_head: Some("1111111111111111111111111111111111111111".to_string()),
        branch_head: Some("2222222222222222222222222222222222222222".to_string()),
        branch_moved: Some(true),
        implementation_commits_ahead: Some(0),
        implementation_commits_created: Some(1),
        implementation_diff_paths: Some(0),
        implementation_diff_empty: Some(true),
    };

    let summary = dev2merge_success_summary(&facts, &observed);

    for required in [
        "dev2merge side effects verified",
        "branch=fix/issue",
        "checkout=main",
        "branch_moved=true",
        "implementation_commits_created=1",
        "review_completed=true",
        "review_verdict_checked=true",
        "pushed=true",
        "pr=#42 state=MERGED",
        "pr_bot_marker=present",
        "next=none",
    ] {
        assert!(
            summary.contains(required),
            "success summary must include {required}: {summary}"
        );
    }
}

#[test]
fn dev2merge_completion_fails_empty_diff_without_side_effect() {
    let mut facts = completed_facts();
    facts.pr_number = None;
    facts.pr_bot_done_marker = None;
    facts.review_completed = true;
    facts.review_verdict_checked = true;
    facts.pushed = false;
    let observed = Dev2MergeObservedState {
        pr_bot_marker_exists: None,
        pr_state: None,
        pr_state_error: None,
        current_branch: Some("fix/issue".to_string()),
        current_head: Some("1111111111111111111111111111111111111111".to_string()),
        branch_head: Some("1111111111111111111111111111111111111111".to_string()),
        branch_moved: Some(false),
        implementation_commits_ahead: Some(0),
        implementation_commits_created: Some(0),
        implementation_diff_paths: Some(0),
        implementation_diff_empty: Some(true),
    };

    let failures = evaluate_dev2merge_completion(&facts, &observed);

    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("no dev2merge side effect was observed")),
        "empty no-op must fail with missing side-effect detail: {failures:?}"
    );
    assert!(
        failures
            .iter()
            .any(|failure| failure.contains("cumulative diff main...fix/issue is empty")),
        "empty diff must be named explicitly: {failures:?}"
    );
}

#[test]
fn dev2merge_skip_does_not_count_completed_steps_as_publish_started() {
    let results = Vec::new();
    let completed_steps = vec![13, 14, 15, 16, 17];

    assert!(!step_completed_successfully(
        &results,
        &completed_steps,
        true,
        13
    ));
}

#[test]
fn success_output_persists_summary_and_details_sections() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let report = PlanSuccessReport::new(
        "plan: dev2merge",
        Some("dev2merge for branch fix/issue: PR #42 is MERGED".to_string()),
    );

    persist_plan_success_output(temp.path(), &report).expect("success output should persist");

    let summary = csa_session::read_section(temp.path(), "summary")
        .expect("summary should load")
        .expect("summary should exist");
    assert!(
        summary.contains("Plan complete: plan: dev2merge")
            && summary.contains("Completion verification: dev2merge"),
        "summary should expose completion verification: {summary}"
    );
    let details = csa_session::read_section(temp.path(), "details")
        .expect("details should load")
        .expect("details should exist");
    assert!(
        details.contains("Plan Completion Report") && details.contains("Status: `success`"),
        "details should expose success report: {details}"
    );
}

#[test]
fn success_output_preserves_existing_summary_section() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    csa_session::persist_structured_output(
        temp.path(),
        "<!-- CSA:SECTION:summary -->\nExisting workflow summary\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("existing summary should persist");
    let report = PlanSuccessReport::new("plan: mktd", None);

    persist_plan_success_output(temp.path(), &report).expect("success output should no-op");

    let summary = csa_session::read_section(temp.path(), "summary")
        .expect("summary should load")
        .expect("summary should exist");
    assert_eq!(summary, "Existing workflow summary");
}

#[test]
fn verify_dev2merge_completion_missing_pr_returns_structured_failure_report() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    std::fs::write(&workflow_path, "[workflow]\nname = 'dev2merge'\n")
        .expect("workflow should be written");
    let marker = temp.path().join("marker.done");
    std::fs::write(&marker, "").expect("marker should be written");
    let vars = HashMap::from([
        ("DEV2MERGE_SKIP".to_string(), "false".to_string()),
        (
            "PR_BOT_DONE_MARKER".to_string(),
            marker.display().to_string(),
        ),
    ]);
    let completed_steps = vec![13, 14, 15, 16, 17];
    let snapshot = PlanCompletionSnapshot {
        initial_branch: Some("fix/issue".to_string()),
        initial_head: None,
    };

    let err = verify_plan_completion(PlanCompletionInput {
        workflow_name: "dev2merge",
        workflow_path: &workflow_path,
        project_root: temp.path(),
        results: &[],
        completed_steps: &completed_steps,
        vars: &vars,
        snapshot: &snapshot,
    })
    .expect_err("missing PR_NUMBER must fail completion verification");

    assert_eq!(
        err.to_string(),
        "dev2merge publish side-effect verification failed"
    );
    let summary = err.report().render_summary_section();
    assert!(
        summary.contains("Failed step: 18 (Dev2merge Publish Side-Effect Verification) exited 1"),
        "summary should expose the synthetic verification step: {summary}"
    );
    let details = err.report().render_details_section();
    assert!(
        details.contains("PR_NUMBER was not captured")
            && details.contains("callers should use this structured result"),
        "details should include the actionable side-effect failure: {details}"
    );
}
