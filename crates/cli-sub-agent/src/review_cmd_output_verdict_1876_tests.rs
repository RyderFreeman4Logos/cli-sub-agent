use super::*;
use csa_session::FindingsFile;

fn read_findings_toml(session_dir: &Path) -> FindingsFile {
    let findings_path = session_dir.join("output").join("findings.toml");
    toml::from_str(&fs::read_to_string(findings_path).expect("read findings.toml"))
        .expect("parse findings.toml")
}

fn read_verdict(session_dir: &Path) -> ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

fn write_empty_findings_toml(session_dir: &Path) {
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
}

#[test]
fn issue_1876_codex_pn_findings_populate_toml_counts_and_fail() {
    let session_id = "01TEST1876PNFINDINGS0000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-pn-findings", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [P1][correctness] Dry-run GC/session clean now calls a mutating liveness probe ...
2. [P2][correctness/test-gap] Live orphan directories are still listed as removable ...

## Overall Risk

High
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!("[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"{session_id}\"\n"),
    )
    .expect("write suggestion.toml");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(findings.findings[1].severity, Severity::Medium);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_high_finding_populates_findings_toml_not_counts_only() {
    let session_id = "01TEST1876HIGHFINDING00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-high-finding", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [high][correctness] Dry-run cleanup still mutates session liveness state.

## Overall Risk

High
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::High);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_arbitrary_pn_finding_populates_low_findings_toml() {
    let session_id = "01TEST1876P7FINDING000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-p7-finding", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
FAIL
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [P7][style] Reviewer emitted a priority tag outside the historical P0-P4 range.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 1);
    assert_eq!(findings.findings[0].severity, Severity::Low);

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_unparsed_enumerated_findings_section_fails_closed() {
    let session_id = "01TEST1876UNPARSEDFIND";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-unparsed-findings", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [severity][correctness] Parser cannot classify this enumerated finding.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("prose_findings_present_but_unparsed")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_resume_to_fix_suggestion_never_passes() {
    let session_id = "01TEST1876RESUMETOFIX00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-resume-to-fix", session_id);

    write_empty_findings_toml(&session_dir);
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!("[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"{session_id}\"\n"),
    )
    .expect("write suggestion.toml");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_nonzero_counts_with_empty_findings_fail_closed() {
    let session_id = "01TEST1876COUNTMISMATCH";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1876-count-mismatch", session_id);

    write_empty_findings_toml(&session_dir);
    let mut verdict =
        ReviewVerdictArtifact::from_parts(session_id, ReviewDecision::Pass, "CLEAN", &[], vec![]);
    verdict.severity_counts.insert(Severity::High, 1);

    enforce_final_verdict_consistency(&session_dir, &mut verdict).expect("enforce consistency");

    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(
        verdict.failure_reason.as_deref(),
        Some("severity_counts_findings_mismatch")
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1876_nonempty_findings_with_zero_counts_fail_closed() {
    let mut counts = BTreeMap::new();
    for severity in [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
    ] {
        counts.insert(severity, 0);
    }

    let decision = derive_decision_from_severity_counts(
        &counts,
        false,
        None,
        Some(ReviewDecision::Pass),
        || Ok(false),
        || Ok(false),
        || Ok(false),
    )
    .expect("derive decision");

    assert_eq!(decision, ReviewDecision::Fail);
}
