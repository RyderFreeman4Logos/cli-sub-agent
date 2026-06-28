use std::path::Path;

use super::*;

fn run_git(project_root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("git command should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn initialize_git_project(project_root: &Path) -> (String, String) {
    run_git(project_root, &["init"]);
    run_git(project_root, &["config", "user.email", "test@example.com"]);
    run_git(project_root, &["config", "user.name", "Test User"]);
    fs::write(project_root.join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(project_root, &["add", "tracked.txt"]);
    run_git(project_root, &["commit", "-m", "initial"]);
    run_git(
        project_root,
        &["checkout", "-b", "fix/2425-clean-review-pass-marker"],
    );
    let branch = run_git(project_root, &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(project_root).expect("detect git head");
    (branch, head_sha)
}

#[test]
fn issue_2425_clean_uncertain_review_persists_pass_meta_and_marker() {
    let session_id = "01TEST2425CLEANUNCERT00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-clean-uncertain", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nPrior reviewer finding was fixed; no actionable findings remain.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 0);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert_eq!(verdict.failure_reason, None);
    assert_eq!(verdict.primary_failure, None);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));

    let final_meta: ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(final_meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(final_meta.verdict, "CLEAN");
    assert_eq!(final_meta.exit_code, 0);
    assert_eq!(final_meta.status_reason, None);
    assert_eq!(final_meta.failure_reason, None);
    assert_eq!(final_meta.primary_failure, None);
    let final_meta_value: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta json"),
    )
    .expect("parse review meta json");
    assert_eq!(final_meta_value.get("status_reason"), None);
    assert_eq!(final_meta_value.get("failure_reason"), None);
    assert_eq!(final_meta_value.get("primary_failure"), None);

    let marker = crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha)
        .expect("clean recovered review should write a SHA-pinned pass marker");
    assert_eq!(marker.session_id, session_id);
    assert_eq!(marker.scope, "range:main...HEAD");
    assert_eq!(marker.verdict, "CLEAN");

    let findings = read_output_findings(&session_dir);
    assert!(findings.findings.is_empty());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_prior_uncertain_verdict_bug_mention_still_recovers_pass() {
    let session_id = "01TEST2425PRIORUNCERT0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-prior-uncertain-verdict", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nThe prior UNCERTAIN verdict bug is fixed; no blocking findings remain; clean verdict.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 0);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    let final_meta: ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(final_meta.decision, ReviewDecision::Pass.as_str());

    let marker = crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha)
        .expect("clean recovered review should write a SHA-pinned pass marker");
    assert_eq!(marker.session_id, session_id);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_quoted_prior_wait_uncertain_verdict_still_recovers_pass() {
    let session_id = "01TEST2425QUOTEDWAIT0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-quoted-wait-uncertain", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nEarlier wait output was:\n```text\nReview verdict: uncertain\nSummary: old run\n```\nThe current review has no blocking findings and is clean.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 0);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    let marker = crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha)
        .expect("quoted prior wait output must not block clean pass marker");
    assert_eq!(marker.session_id, session_id);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_inline_quoted_prior_wait_uncertain_verdict_still_recovers_pass() {
    let session_id = "01TEST2425INLINEWAIT";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-inline-wait-uncertain", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nThe old wait line `Review verdict: uncertain` is quoted only as repro evidence; the current review has no blocking findings and is clean.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 0);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    let marker = crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha)
        .expect("inline quoted prior wait output must not block clean pass marker");
    assert_eq!(marker.session_id, session_id);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_quoted_clean_repro_alone_does_not_recover_pass() {
    let session_id = "01TEST2425QUOTECLEAN0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-quoted-clean-repro", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nEarlier wait output is reproduced below.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\n```text\nSummary: No blocking findings in main...HEAD.\nReview verdict: PASS\n```\nThe current reviewer did not reach a durable clean conclusion for this run.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist quoted repro sections");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 1);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Uncertain);
    assert_eq!(verdict.verdict_legacy, "UNCERTAIN");
    assert!(
        crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha).is_none(),
        "quoted old clean prose alone must not write a clean pass marker"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_synthetic_fix_suggestion_does_not_block_clean_placeholder_recovery() {
    let session_id = "01TEST2425SYNTHFIX00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-synthetic-fix-suggestion", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `range:main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nReviewed the diff for branch `fix/2425-clean-review-pass-marker`.\n\nFindings: none.\n\nKey evidence:\n- Recovery only converts failed or uncertain clean sessions when counts/findings are clean and there is no hard failure evidence, explicit uncertain conclusion, fail conclusion, resume-to-fix signal, blocking risk, or structured finding evidence: `crates/cli-sub-agent/src/review_cmd_output_consistency.rs:139`.\n\nAGENTS.md checklist:\n- No violations found.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist live-style clean review sections");
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!(
            "[suggestion]\naction = \"confirm_then_fix_finding\"\nsession_id = {session_id:?}\nrequires_confirmation = true\n"
        ),
    )
    .expect("write synthetic fix suggestion");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("fail_verdict_empty_findings_artifact".to_string());

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 0);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert_eq!(verdict.failure_reason, None);
    let marker = crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha)
        .expect("synthetic fix suggestion must not block clean pass marker");
    assert_eq!(marker.session_id, session_id);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_mixed_uncertain_no_blocker_prose_does_not_persist_pass() {
    let session_id = "01TEST2425MIXEDUNCERT0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-mixed-uncertain", session_id);
    let (branch, head_sha) = initialize_git_project(&project_root);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nuncertain: no blocking findings, but insufficient context\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking findings were identified, but the reviewer did not have enough context to conclude PASS.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist mixed uncertain review sections");

    let mut meta = make_review_meta(session_id);
    meta.head_sha = head_sha.clone();
    meta.scope = "range:main...HEAD".to_string();
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();

    let exit = crate::review_cmd::persist_review_sidecars_if_session_exists(
        &project_root,
        &meta,
        Some(session_id),
    )
    .expect("sidecars should persist");
    assert_eq!(exit, 1);

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Uncertain);
    assert_eq!(verdict.verdict_legacy, "UNCERTAIN");
    assert_eq!(verdict.failure_reason, None);
    assert!(verdict.severity_counts.values().all(|count| *count == 0));

    let final_meta: ReviewSessionMeta = serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read review meta"),
    )
    .expect("parse review meta");
    assert_eq!(final_meta.decision, ReviewDecision::Uncertain.as_str());
    assert_eq!(final_meta.verdict, "UNCERTAIN");
    assert_eq!(final_meta.exit_code, 1);

    assert!(
        crate::review_gate::read_review_gate_marker(&project_root, &branch, &head_sha).is_none(),
        "explicit uncertain prose must not write a clean pass marker"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_prior_finding_fixed_prose_does_not_create_blocking_finding() {
    let session_id = "01TEST2425PRIORFIXED00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-prior-finding-fixed", session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nVerdict: PASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nThe prior high finding is fixed. No blocking findings remain.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let mut meta = make_review_meta(session_id);
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    let findings = read_output_findings(&session_dir);
    assert!(
        findings.findings.is_empty(),
        "prior-finding-fixed prose must not become a blocking code finding"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_2425_uncertain_crash_evidence_does_not_recover_to_pass() {
    let session_id = "01TEST2425CRASHUNCERT0";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-2425-uncertain-crash", session_id);
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nVerdict: PASS\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking findings.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let mut meta = make_review_meta(session_id);
    meta.decision = ReviewDecision::Uncertain.as_str().to_string();
    meta.verdict = "UNCERTAIN".to_string();
    meta.failure_reason = Some("reviewer process crashed before artifact finalization".to_string());
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_output_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Uncertain);
    assert_eq!(verdict.verdict_legacy, "UNCERTAIN");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("reviewer process crashed before artifact finalization")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
