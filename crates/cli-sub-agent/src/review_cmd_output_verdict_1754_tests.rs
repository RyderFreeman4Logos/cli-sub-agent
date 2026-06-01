use super::*;
use csa_session::FindingsFile;

fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

fn read_findings_toml(session_dir: &Path) -> FindingsFile {
    let findings_path = session_dir.join("output").join("findings.toml");
    toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
        .expect("parse findings.toml")
}

#[test]
fn issue_1754_codex_single_blocking_prose_populates_findings_and_fails() {
    let session_id = "01TEST1754BLOCKINGPROSE00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1754-blocking-prose", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Blocking contract violations found in changed workflow/pattern command paths.
<!-- CSA:SECTION:summary:END -->
<!-- CSA:SECTION:details -->
1. patterns/csa-review/workflow.toml:137 omits --sa-mode true on the review gate.
2. patterns/debate/workflow.toml:87 omits --sa-mode true on the debate gate.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2);
    let verdict = read_verdict(&session_dir);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&2));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1754_codex_single_medium_path_finding_populates_counts_and_fails() {
    let session_id = "01TEST1754MEDIUMPATH00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1754-medium-path", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:details -->
Medium: docs/debate-review.md:151 reviewer output can hide the required follow-up.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::Medium);
    let verdict = read_verdict(&session_dir);
    assert_ne!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1754_pass_with_resume_to_fix_suggestion_fails_closed() {
    let session_id = "01TEST1754RESUMETOFIX0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1754-resume-to-fix", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        "[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"01TEST1754RESUMETOFIX0000\"\n",
    )
    .expect("write suggestion.toml");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1754_fail_with_empty_findings_and_zero_counts_gets_fail_closed_count() {
    let session_id = "01TEST1754FAILEMPTYZERO00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1754-fail-empty-zero", session_id);
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");

    let mut artifact = ReviewVerdictArtifact::from_parts(
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    enforce_final_verdict_consistency(&session_dir, &mut artifact)
        .expect("enforce final verdict consistency");

    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
