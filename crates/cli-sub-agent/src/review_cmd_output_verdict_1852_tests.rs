//! Regression tests for #1852 Defect 1: a fail-closed review verdict must
//! record a severity count that matches the reviewer's stated prose GRADE.
//!
//! The triggering session reported canonical prose `` `[HIGH]` `` yet persisted
//! `severity_counts` of `medium=1, high=0` — a merge footgun, because a real
//! HIGH was downgraded to a mergeable MEDIUM. The structured finding parsers
//! skip backtick-wrapped severity tags, so the fail-closed grader must scan the
//! persisted sections backtick-robustly and grade up to (never down from) the
//! highest legible prose severity, defaulting to MEDIUM only when none is
//! legible.

use super::*;

fn medium_count(artifact: &ReviewVerdictArtifact) -> u32 {
    artifact
        .severity_counts
        .get(&Severity::Medium)
        .copied()
        .unwrap_or(0)
}

fn high_count(artifact: &ReviewVerdictArtifact) -> u32 {
    artifact
        .severity_counts
        .get(&Severity::High)
        .copied()
        .unwrap_or(0)
}

#[test]
fn issue_1852_fail_closed_grade_reflects_backtick_high_prose() {
    let session_id = "01TEST1852BACKTICKHIGH000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1852-backtick-high", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:details -->
1. `[HIGH]` Untracked-file line counting can block prompt assembly on special files.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

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
    assert_eq!(
        high_count(&artifact),
        1,
        "backtick-wrapped [HIGH] prose must grade the fail-closed placeholder as High"
    );
    assert_eq!(
        medium_count(&artifact),
        0,
        "a HIGH-graded finding must not be recorded as a mergeable MEDIUM (#1852)"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1852_fail_closed_escalates_up_on_structured_under_grade() {
    let session_id = "01TEST1852UNDERGRADE00000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1852-under-grade", session_id);

    // Structured machine findings say MEDIUM; canonical prose grades the same
    // class of issue as HIGH via a backtick-wrapped tag the parsers miss.
    fs::write(
        session_dir.join("output").join("findings.toml"),
        "[[findings]]\nid = \"1852-structured-medium\"\nseverity = \"medium\"\ndescription = \"pre-existing structured medium finding\"\n",
    )
    .expect("write structured medium findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:details -->
1. `[HIGH]` Untracked-file line counting can block prompt assembly on special files.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let mut artifact = ReviewVerdictArtifact::from_parts(
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    enforce_final_verdict_consistency(&session_dir, &mut artifact)
        .expect("enforce final verdict consistency");

    assert_eq!(
        high_count(&artifact),
        1,
        "prose-vs-structured grade mismatch must fail closed UP to High"
    );
    assert_eq!(
        medium_count(&artifact),
        1,
        "escalating UP must not erase the lower structured grade"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1852_existing_high_count_is_not_inflated() {
    let session_id = "01TEST1852NOINFLATE000000";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1852-no-inflate", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "[[findings]]\nid = \"1852-structured-high\"\nseverity = \"high\"\ndescription = \"pre-existing structured high finding\"\n",
    )
    .expect("write structured high findings.toml");
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:details -->
1. `[HIGH]` Untracked-file line counting can block prompt assembly on special files.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let mut artifact = ReviewVerdictArtifact::from_parts(
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    enforce_final_verdict_consistency(&session_dir, &mut artifact)
        .expect("enforce final verdict consistency");

    assert_eq!(
        high_count(&artifact),
        1,
        "a structured High already matching the prose grade must not be double-counted"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

#[test]
fn issue_1852_fail_closed_without_legible_grade_defaults_to_medium() {
    let session_id = "01TEST1852NOGRADEMEDIUM00";
    let (_env_lock, project_root, session_dir) =
        lock_test_session("issue-1852-no-grade-medium", session_id);

    fs::write(
        session_dir.join("output").join("findings.toml"),
        "findings = []\n",
    )
    .expect("write empty findings.toml");
    // Prose has a non-severity bracket but no legible severity grade.
    csa_session::persist_structured_output(
        &session_dir,
        r#"<!-- CSA:SECTION:details -->
The reviewer flagged a `[correctness]` concern but assigned no severity grade.
<!-- CSA:SECTION:details:END -->
"#,
    )
    .expect("persist structured output");

    let mut artifact = ReviewVerdictArtifact::from_parts(
        session_id,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        &[],
        Vec::new(),
    );
    enforce_final_verdict_consistency(&session_dir, &mut artifact)
        .expect("enforce final verdict consistency");

    assert_eq!(
        medium_count(&artifact),
        1,
        "an ungraded fail-closed verdict must keep the MEDIUM placeholder fallback"
    );
    assert_eq!(
        high_count(&artifact),
        0,
        "no legible HIGH grade must not synthesize a HIGH count"
    );

    fs::remove_dir_all(project_root).expect("remove temp project root");
}

include!("review_cmd_output_verdict_2393_tests.rs");
