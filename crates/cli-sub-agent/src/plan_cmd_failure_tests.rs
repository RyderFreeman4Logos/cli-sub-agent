use super::*;

#[test]
fn persisted_failure_output_redacts_step_secrets() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let session_dir = temp.path().join("session");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 1,
        title: "Secret Failure".to_string(),
        exit_code: 7,
        duration_secs: 0.0,
        skipped: false,
        error: Some("Exit code 7\nstderr:\npassword=hunter2".to_string()),
        output: None,
        session_id: None,
        command: Some(
            "curl -H 'Authorization: Bearer abcDEF123._-token' api_key=key-prod_987654321"
                .to_string(),
        ),
        stderr: Some("client_secret=top-secret-value".to_string()),
    }];
    let report = PlanFailureReport::from_results(
        "failing-plan",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    persist_plan_failure_output(&session_dir, &report).expect("failure output should persist");

    let output_log =
        std::fs::read_to_string(session_dir.join("output.log")).expect("output.log should exist");
    let details = csa_session::read_section(&session_dir, "details")
        .expect("details should load")
        .expect("details section should exist");
    for rendered in [&output_log, &details] {
        assert!(
            rendered.contains("[REDACTED]"),
            "persisted failure output must mark redacted secrets: {rendered}"
        );
        assert!(
            !rendered.contains("abcDEF123._-token"),
            "bearer token leaked: {rendered}"
        );
        assert!(
            !rendered.contains("key-prod_987654321"),
            "api key leaked: {rendered}"
        );
        assert!(!rendered.contains("hunter2"), "password leaked: {rendered}");
        assert!(
            !rendered.contains("top-secret-value"),
            "client secret leaked: {rendered}"
        );
    }
}

#[test]
fn failure_summary_surfaces_actionable_step_stderr() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some("Exit code 1".to_string()),
        output: None,
        session_id: None,
        command: Some("csa plan run patterns/mktd/workflow.toml".to_string()),
        stderr: Some(
            [
                "✓ PASS   Step 7 - Phase 2 — DRAFT TODO",
                "✗ FAIL   Step 13 - Save TODO (0.02s) — Exit code 2",
                "ERROR: TODO artifact has an open task without a mechanically-verifiable DONE WHEN: clause.",
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();

    assert!(
        summary_line.contains("detail=ERROR: TODO artifact has an open task"),
        "result summary should include actionable stderr detail: {summary_line}"
    );
    assert!(
        summary_section.contains("Failure detail: ERROR: TODO artifact has an open task"),
        "structured summary section should include actionable stderr detail: {summary_section}"
    );
}

#[test]
fn failure_summary_prefers_plan_child_died_diagnostic() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 10,
        title: "Phase 3 — Adversarial Debate".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(
            [
                "Exit code 1",
                "stderr (last 20 lines):",
                "csa debate failed",
                "plan_child_died session=01KWD2XP59K0QE6YMM0M7N2FHW status=NoLivePID phase=Active live_process=false result_status=missing result_exit=missing",
            ]
            .join("\n"),
        ),
        output: None,
        session_id: None,
        command: Some("csa debate --sa-mode true && csa session wait".to_string()),
        stderr: Some("csa debate failed".to_string()),
    }];
    let report = PlanFailureReport::from_results(
        "mktd",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/mktd/workflow.toml");
    let summary_section = report.render_summary_section();

    for rendered in [&summary_line, &summary_section] {
        assert!(
            rendered.contains("plan_child_died session=01KWD2XP59K0QE6YMM0M7N2FHW")
                && rendered.contains("status=NoLivePID"),
            "parent-visible failure should expose the child session death diagnostic: {rendered}"
        );
    }
}

#[test]
fn failure_summary_surfaces_mktd_stdout_for_post_2082_issue_body_shape() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(
            [
                "Exit code 1",
                "stderr (last 20 lines):",
                "ERROR: mktd did not produce a TODO for branch fix/2086-require-commit-openai-fallback.",
                "stdout (last 20 lines):",
                "## Command",
                "```bash",
                "csa run --tool codex --tier tier-2-standard --allow-fallback --require-commit --prompt-file /dev/stdin",
                "```",
                "## Observed",
                "```text",
                "WARNING: csa run completed but run left uncommitted workspace mutations compared to start.",
                "ERROR csa::pipeline: Tool 'openai-compat' is not installed.",
                "Error: Tool 'openai-compat' is not installed or not in PATH",
                "```",
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
        output: None,
        session_id: None,
        command: Some("timeout -k 30 1800 csa plan run --sa-mode true patterns/mktd/workflow.toml --var FEATURE='Plan dev2merge for branch fix/2086'".to_string()),
        stderr: Some(
            "ERROR: mktd did not produce a TODO for branch fix/2086-require-commit-openai-fallback."
                .to_string(),
        ),
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();
    let details_section = report.render_details_section();

    assert!(
        summary_line.contains("detail=Error: Tool 'openai-compat' is not installed or not in PATH"),
        "result summary should prefer concrete child mktd stdout over generic Step 7 gate text: {summary_line}"
    );
    assert!(
        summary_section.contains(
            "Failure detail: Error: Tool 'openai-compat' is not installed or not in PATH"
        ),
        "structured summary should expose concrete child mktd stdout detail: {summary_section}"
    );
    assert!(
        summary_section.contains("Recovery hint: Inspect the mktd failure detail above"),
        "structured summary should include an actionable mktd recovery hint: {summary_section}"
    );
    assert!(
        details_section.contains("stdout (last 20 lines):")
            && details_section.contains("--prompt-file /dev/stdin")
            && details_section.contains("openai-compat"),
        "details should preserve the post-#2082 issue-body shape and child failure context: {details_section}"
    );
}

#[test]
fn failure_summary_surfaces_todo_persist_validation_detail() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(
            [
                "Exit code 1",
                "stderr (last 20 lines):",
                "csa todo persist failed (exit 1)",
                "TODO artifact path: /tmp/mktd-save/TODO.md",
                "Spec artifact path: /tmp/mktd-save/spec.toml",
                "Persist stderr artifact: /tmp/mktd-save/persist.stderr",
                "csa todo persist stderr (last 80 lines):",
                "Error: failed to parse spec file '/tmp/mktd-save/spec.toml': TOML parse error at line 6, column 1",
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
        output: None,
        session_id: None,
        command: Some("timeout -k 30 1800 csa plan run --sa-mode true --pattern mktd".to_string()),
        stderr: Some(
            [
                "csa todo persist failed (exit 1)",
                "TODO artifact path: /tmp/mktd-save/TODO.md",
                "Spec artifact path: /tmp/mktd-save/spec.toml",
                "Persist stderr artifact: /tmp/mktd-save/persist.stderr",
                "csa todo persist stderr (last 80 lines):",
                "Error: failed to parse spec file '/tmp/mktd-save/spec.toml': TOML parse error at line 6, column 1",
            ]
            .join("\n"),
        ),
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();

    for rendered in [&summary_line, &summary_section] {
        assert!(
            rendered.contains("failed to parse spec file"),
            "parent-visible failure should include concrete persist validation detail: {rendered}"
        );
        assert!(
            rendered.contains("TOML parse error at line 6, column 1"),
            "parent-visible failure should include TOML line/column detail: {rendered}"
        );
        assert!(
            rendered.contains("Spec artifact path: /tmp/mktd-save/spec.toml"),
            "parent-visible failure should include bounded spec artifact context: {rendered}"
        );
        assert!(
            rendered.contains("csa todo persist failed"),
            "parent-visible failure should preserve the persist wrapper context: {rendered}"
        );
    }
}

#[test]
fn failure_summary_surfaces_mktd_invalid_field_recovery_action() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some("Exit code 1".to_string()),
        output: None,
        session_id: None,
        command: Some("timeout -k 30 1800 csa plan run --sa-mode true --pattern mktd".to_string()),
        stderr: Some(
            [
                "RECON validation failed",
                "invalid field [spec.criteria.done_when]: expected non-empty string",
                "recovery action: regenerate spec.toml with a mechanically-verifiable DONE WHEN",
                "Spec artifact path: /tmp/mktd-save/spec.toml",
            ]
            .join("\n"),
        ),
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();

    for rendered in [&summary_line, &summary_section] {
        assert!(
            rendered.contains("invalid field [spec.criteria.done_when]")
                && rendered.contains("Spec artifact path: /tmp/mktd-save/spec.toml")
                && rendered.contains("recovery action: regenerate spec.toml"),
            "parent-visible mktd failure should name the invalid field, recovery action, and artifact path: {rendered}"
        );
    }
}

#[test]
fn failure_summary_prefers_underlying_command_failure_over_spec_contract_noise() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let spec_path = "/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/sessions/01KVZ46MAF83G0PBFPXNPH4DM2/output/mktd-save/spec.toml";
    let raw_spec_path = "/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cli-sub-agent/sessions/01KVZ46MAF83G0PBFPXNPH4DM2/output/mktd-save/spec.raw.txt";
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(
            [
                "Exit code 1",
                "stderr (last 40 lines):",
                "spec producer-contract error: expected TOML spec artifact (raw TOML, fenced TOML, or CSA section containing TOML); first content: command stderr/stdout contamination",
                "spec artifact-shape error: expected raw TOML or fenced TOML",
                "underlying command failure: spec artifact was contaminated by command stderr/stdout",
                "Command stderr summary: error: failed to create cargo target dir: Read-only file system (os error 30)",
                &format!("Spec artifact path: {spec_path}"),
                &format!("Raw spec artifact path: {raw_spec_path}"),
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
        output: None,
        session_id: None,
        command: Some("timeout -k 30 1800 csa plan run --sa-mode true --pattern mktd".to_string()),
        stderr: None,
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();

    for rendered in [&summary_line, &summary_section] {
        assert!(
            rendered.contains("underlying command failure"),
            "parent-visible failure should prefer the underlying command failure: {rendered}"
        );
        assert!(
            rendered.contains("Read-only file system"),
            "parent-visible failure should retain the original stderr summary: {rendered}"
        );
        assert!(
            rendered.contains(&format!("Spec artifact path: {spec_path}")),
            "parent-visible failure should include the artifact path: {rendered}"
        );
        assert!(
            !rendered.contains("Failure detail: spec artifact-shape error")
                && !rendered.contains("detail=spec artifact-shape error"),
            "generic artifact-shape noise must not be the primary detail: {rendered}"
        );
    }
}

#[test]
fn failure_summary_prefers_mktd_validation_over_quoted_issue_prose() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let workflow_path = temp.path().join("workflow.toml");
    let results = vec![StepResult {
        step_id: 7,
        title: "Plan with mktd".to_string(),
        exit_code: 1,
        duration_secs: 0.0,
        skipped: false,
        error: Some(
            [
                "Exit code 1",
                "stderr (last 40 lines):",
                "✗ FAIL   Step 13 - Save TODO (0.02s) — Exit code 1",
                "TODO artifact path: /tmp/mktd-save/TODO.md",
                "Spec artifact path: /tmp/mktd-save/spec.toml",
                "ERROR: TODO artifact has an open task without a mechanically-verifiable DONE WHEN: clause.",
                "stdout (last 40 lines):",
                "- Full smoke failed after ~13.1 minutes.",
                "- Prior issue text mentioned Read-only file system (os error 30), but that is quoted prose.",
                "Command stderr summary: error: failed to create cargo target dir: Read-only file system (os error 30)",
                "Error: 1 step(s) failed (1 execution, 0 unsupported-skip)",
            ]
            .join("\n"),
        ),
        output: None,
        session_id: None,
        command: Some("timeout -k 30 1800 csa plan run --sa-mode true --pattern mktd".to_string()),
        stderr: None,
    }];
    let report = PlanFailureReport::from_results(
        "dev2merge",
        &workflow_path,
        "1 step(s) failed".to_string(),
        &results,
        None,
    );

    let summary_line = report.summary_line("patterns/dev2merge/workflow.toml");
    let summary_section = report.render_summary_section();

    for rendered in [&summary_line, &summary_section] {
        assert!(
            rendered.contains("TODO artifact has an open task"),
            "parent-visible failure should prefer the mktd/TODO validation diagnostic: {rendered}"
        );
        assert!(
            rendered.contains("Spec artifact path: /tmp/mktd-save/spec.toml"),
            "parent-visible failure should include artifact context: {rendered}"
        );
        assert!(
            !rendered.contains("Failure detail: - Full smoke failed")
                && !rendered.contains("detail=- Full smoke failed"),
            "quoted issue prose must not become the actionable failure detail: {rendered}"
        );
        assert!(
            !rendered.contains("Failure detail: - Prior issue text mentioned")
                && !rendered.contains("detail=- Prior issue text mentioned"),
            "quoted EROFS prose must not outrank the validation diagnostic: {rendered}"
        );
        assert!(
            !rendered.contains("Failure detail: Command stderr summary")
                && !rendered.contains("detail=Command stderr summary")
                && !rendered.contains("Read-only file system"),
            "unscoped quoted command summaries must not pollute the validation detail: {rendered}"
        );
    }
}
