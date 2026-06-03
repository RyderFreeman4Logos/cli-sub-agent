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

const SUPPORTED_FINDINGS_HEADINGS: &[&str] = &[
    "Findings",
    "Review Findings",
    "Findings (ordered by severity)",
];

const CLEAN_FINDINGS_BODIES: &[(&str, &str, &str)] = &[
    ("no-issues-found", "Review completed.", "No issues found."),
    (
        "no-issues-were-found",
        "Review completed.",
        "No issues were found.",
    ),
    (
        "no-blocking-issues",
        "Review completed.",
        "No blocking issues.",
    ),
    ("no-findings", "Review completed.", "No findings."),
    (
        "no-blocking-findings-found",
        "Review completed.",
        "No blocking findings found.",
    ),
    (
        "no-actionable-findings",
        "Review completed.",
        "No actionable findings.",
    ),
    ("ship-ready", "Review completed.", "Ship-ready."),
    ("ship-ready-spaced", "Review completed.", "Ship ready."),
    (
        "positive-no-issue-clause",
        "Review completed.",
        "No correctness issues were introduced.",
    ),
    ("none-with-pass-summary", "PASS", "None."),
];

fn write_empty_findings_toml(session_dir: &Path) {
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
}

#[test]
fn issue_1804_codex_single_title_word_severity_findings_populate_toml_and_fail() {
    let session_id = "01TEST1804CODEXPROSE00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1804-codex-prose-findings", session_id);

    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
Review found two blocking findings in the diff.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. High correctness / sandbox violation: `csa review --single` can pass despite a blocking finding.
   - File: crates/cli-sub-agent/src/review_cmd_output.rs:214
   - Trigger: codex emits severity as the first word of the title line.
   - Expected: severity_counts.high is 1 and the verdict blocks.
   - Actual: findings.toml was empty before #1804.
   - Fix hint: parse title-leading severity labels.
2. Medium correctness regression: `- File:` bullets are ignored.
   - File: crates/cli-sub-agent/src/review_cmd_prose_findings.rs:154
   - Trigger: codex emits file locations as sub-bullets.
   - Expected: the finding receives a file range.
   - Actual: the old parser only accepted `File:` without a list marker.
   - Fix hint: strip unordered-list markers before matching `File:`.

## Recommended Actions

1. Fix the prose extractor and verdict consistency gate.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    crate::review_cmd::findings_toml::persist_review_findings_toml(&project_root, &meta);

    let findings = read_findings_toml(&session_dir);
    assert_eq!(findings.findings.len(), 2);
    assert_eq!(findings.findings[0].severity, Severity::High);
    assert_eq!(
        findings.findings[0].file_ranges[0].path,
        "crates/cli-sub-agent/src/review_cmd_output.rs"
    );
    assert_eq!(findings.findings[0].file_ranges[0].start, 214);
    assert_eq!(findings.findings[1].severity, Severity::Medium);
    assert_eq!(
        findings.findings[1].file_ranges[0].path,
        "crates/cli-sub-agent/src/review_cmd_prose_findings.rs"
    );
    assert_eq!(findings.findings[1].file_ranges[0].start, 154);

    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(verdict.severity_counts.get(&Severity::Medium), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1804_unparsed_findings_sections_fail_closed_with_reason() {
    for (heading_index, heading) in SUPPORTED_FINDINGS_HEADINGS.iter().enumerate() {
        let session_id = format!("01TEST1804UNPARSED{heading_index:02}");
        let test_name = format!("issue-1804-unparsed-heading-{heading_index}");
        let (_env_lock, project_root, session_dir) = lock_test_session(&test_name, &session_id);

        write_empty_findings_toml(&session_dir);
        csa_session::persist_structured_output(
            &session_dir,
            &format!(
                r#"<!-- CSA:SECTION:summary -->
No blocking issues.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## {heading}

1. High correctness regression remains unparsed because the prose lacks a severity delimiter.

## Recommended Actions

1. Inspect the prose manually before accepting this review.
<!-- CSA:SECTION:details:END -->
"#
            ),
        )
        .expect("persist structured output");

        let meta = make_review_meta_with_decision(&session_id, ReviewDecision::Pass, "CLEAN");
        persist_review_verdict(&project_root, &meta, &[], Vec::new());

        let findings = read_findings_toml(&session_dir);
        assert!(findings.findings.is_empty(), "{heading}");
        let verdict = read_verdict(&session_dir);
        assert_ne!(verdict.decision, ReviewDecision::Pass, "{heading}");
        assert_eq!(verdict.decision, ReviewDecision::Fail, "{heading}");
        assert_eq!(verdict.verdict_legacy, "HAS_ISSUES", "{heading}");
        assert_eq!(
            verdict.failure_reason.as_deref(),
            Some("prose_findings_present_but_unparsed"),
            "{heading}"
        );
        assert_eq!(
            verdict.severity_counts.get(&Severity::Medium),
            Some(&1),
            "{heading}"
        );

        fs::remove_dir_all(project_root).expect("remove temp project root");
    }
}

#[test]
fn issue_1804_canonical_clean_findings_sections_stay_pass() {
    for (heading_index, heading) in SUPPORTED_FINDINGS_HEADINGS.iter().enumerate() {
        for (body_index, (case_name, summary, body)) in CLEAN_FINDINGS_BODIES.iter().enumerate() {
            let session_id = format!("01TEST1804CLEAN{heading_index:02}{body_index:02}");
            let test_name = format!("issue-1804-clean-{heading_index}-{case_name}");
            let (_env_lock, project_root, session_dir) = lock_test_session(&test_name, &session_id);

            write_empty_findings_toml(&session_dir);
            csa_session::persist_structured_output(
                &session_dir,
                &format!(
                    r#"<!-- CSA:SECTION:summary -->
{summary}
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## {heading}

{body}

## Recommended Actions

1. Open the PR.
<!-- CSA:SECTION:details:END -->
"#
                ),
            )
            .expect("persist structured output");

            let meta = make_review_meta_with_decision(&session_id, ReviewDecision::Pass, "CLEAN");
            persist_review_verdict(&project_root, &meta, &[], Vec::new());

            let findings = read_findings_toml(&session_dir);
            assert!(findings.findings.is_empty(), "{heading}: {case_name}");
            let verdict = read_verdict(&session_dir);
            assert_eq!(
                verdict.decision,
                ReviewDecision::Pass,
                "{heading}: {case_name}"
            );
            assert_eq!(verdict.verdict_legacy, "CLEAN", "{heading}: {case_name}");
            assert!(
                verdict.severity_counts.values().all(|count| *count == 0),
                "{heading}: {case_name}"
            );
            assert!(verdict.failure_reason.is_none(), "{heading}: {case_name}");

            fs::remove_dir_all(project_root).expect("remove temp project root");
        }
    }
}

#[test]
fn issue_1804_parsed_findings_section_still_fails() {
    let session_id = "01TEST1804PARSEDHIGH000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1804-parsed-high", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
No blocking issues.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

1. [High] real bug remains visible in the findings section.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Fail);
    assert_eq!(verdict.verdict_legacy, "HAS_ISSUES");
    assert_eq!(verdict.severity_counts.get(&Severity::High), Some(&1));

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1804_recommended_actions_without_findings_section_stays_pass() {
    let session_id = "01TEST1804ACTIONONLY000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1804-action-only", session_id);

    write_empty_findings_toml(&session_dir);
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
No blocking issues.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Recommended Actions

1. Open the PR.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(verdict.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1804_clean_findings_with_benign_recommended_action_stays_pass() {
    let session_id = "01TEST1804CLEANACTION000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1804-clean-action", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:summary -->
PASS
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
## Findings

No blocking findings found.

## Recommended Actions

1. Open the PR.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let meta = make_review_meta_with_decision(session_id, ReviewDecision::Pass, "CLEAN");
    persist_review_verdict(&project_root, &meta, &[], Vec::new());

    let findings = read_findings_toml(&session_dir);
    assert!(findings.findings.is_empty());
    let verdict = read_verdict(&session_dir);
    assert_eq!(verdict.decision, ReviewDecision::Pass);
    assert_eq!(verdict.verdict_legacy, "CLEAN");
    assert!(verdict.severity_counts.values().all(|count| *count == 0));
    assert!(verdict.failure_reason.is_none());

    fs::remove_dir_all(project_root).expect("remove temp project root");
}
