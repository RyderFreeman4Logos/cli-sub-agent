use super::{CLEAN, persist_fix_final_artifacts_for_tests_with_output};
use crate::test_env_lock::ScopedTestEnvVar;
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity, write_findings_toml,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn make_clean_review_meta(session_id: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: String::new(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: CLEAN.to_string(),
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "diff".to_string(),
        exit_code: 0,
        fix_attempted: true,
        fix_rounds: 1,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
    }
}

fn temp_project_root(test_name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("csa-{test_name}-{suffix}"));
    fs::create_dir_all(&path).expect("create temp project root");
    path
}

fn unique_session_id(prefix: &str) -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    format!("{prefix}-{suffix}")
}

fn create_session_dir(project_root: &Path, session_id: &str) -> PathBuf {
    let session_dir =
        csa_session::get_session_dir(project_root, session_id).expect("resolve session dir");
    fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    session_dir
}

fn stale_finding() -> ReviewFinding {
    ReviewFinding {
        id: "stale-medium".to_string(),
        severity: Severity::Medium,
        file_ranges: vec![ReviewFindingFileRange {
            path: "src/lib.rs".to_string(),
            start: 42,
            end: Some(42),
        }],
        is_regression_of_commit: None,
        suggested_test_scenario: None,
        description: "Stale finding from a previous fix round.".to_string(),
    }
}

fn prior_blocking_review_output() -> &'static str {
    "<!-- CSA:SECTION:summary -->\nBlocking issues found before fix.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nMedium: src/lib.rs:42 stale pre-fix finding.\n<!-- CSA:SECTION:details:END -->\n"
}

fn persist_prior_blocking_review_with_current_output(session_dir: &Path, current_output: &str) {
    csa_session::persist_structured_output(
        session_dir,
        &format!("{}{}", prior_blocking_review_output(), current_output),
    )
    .expect("persist structured output");
}

fn read_review_prose_sections(session_dir: &Path) -> Vec<(csa_session::OutputSection, String)> {
    csa_session::read_all_sections(session_dir)
        .expect("read output sections")
        .into_iter()
        .filter(|(section, _)| matches!(section.id.as_str(), "summary" | "details"))
        .collect()
}

fn assert_clean_convergence_artifacts(session_dir: &Path) {
    let output_dir = session_dir.join("output");
    let findings_path = output_dir.join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert_eq!(parsed, FindingsFile::default());

    let verdict_path = output_dir.join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, CLEAN);
    assert!(
        artifact.severity_counts.values().all(|count| *count == 0),
        "clean convergence must persist all-zero severity counts"
    );
}

#[test]
fn persist_fix_final_artifacts_clears_resume_suggestion_and_superseded_prose_on_clean() {
    let project_root = temp_project_root("persist-fix-clear-resume-prose");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXCLEARRESUMEPROSE");
    let session_dir = create_session_dir(&project_root, &session_id);

    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![stale_finding()],
        },
    )
    .expect("write stale findings.toml");
    fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!("[suggestion]\naction = \"resume_to_fix\"\nsession_id = \"{session_id}\"\n"),
    )
    .expect("write stale suggestion.toml");
    let current_output = "<!-- CSA:SECTION:summary -->\nVerdict: CLEAN.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nClean convergence verified. Overall risk: low.\n<!-- CSA:SECTION:details:END -->\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &make_clean_review_meta(&session_id),
        true,
        current_output,
    );

    let output_dir = session_dir.join("output");
    assert!(
        !output_dir.join("suggestion.toml").exists(),
        "stale resume-to-fix suggestion must be removed after clean convergence"
    );
    assert!(
        !output_dir.join("summary.md").exists(),
        "superseded pre-fix summary must be removed"
    );
    assert!(
        !output_dir.join("details.md").exists(),
        "superseded pre-fix details must be removed"
    );

    let review_sections = read_review_prose_sections(&session_dir);
    assert_eq!(review_sections.len(), 2);
    assert!(
        review_sections
            .iter()
            .all(|(_, content)| !content.contains("stale pre-fix finding")),
        "only current clean prose should remain in the output index"
    );

    assert_clean_convergence_artifacts(&session_dir);
}

#[test]
fn persist_fix_final_artifacts_discards_prior_details_when_current_round_summary_only() {
    let project_root = temp_project_root("persist-fix-current-summary-only");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXSUMMARYONLY");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output =
        "<!-- CSA:SECTION:summary -->\nVerdict: CLEAN.\n<!-- CSA:SECTION:summary:END -->\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &make_clean_review_meta(&session_id),
        true,
        current_output,
    );

    let review_sections = read_review_prose_sections(&session_dir);
    assert_eq!(review_sections.len(), 1);
    assert_eq!(review_sections[0].0.id, "summary");
    assert!(
        !review_sections[0].1.contains("stale pre-fix finding"),
        "prior details must not survive when the current clean round emits summary only"
    );
    assert_clean_convergence_artifacts(&session_dir);
}

#[test]
fn persist_fix_final_artifacts_discards_all_prior_prose_when_current_round_is_bare_clean() {
    let project_root = temp_project_root("persist-fix-current-bare-clean");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXBARECLEAN");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output = "CLEAN\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &make_clean_review_meta(&session_id),
        true,
        current_output,
    );

    let review_sections = read_review_prose_sections(&session_dir);
    assert!(
        review_sections.is_empty(),
        "bare clean convergence must purge all prior review prose"
    );
    assert_clean_convergence_artifacts(&session_dir);
}

#[test]
fn persist_fix_final_artifacts_discards_prior_summary_when_current_round_details_only() {
    let project_root = temp_project_root("persist-fix-current-details-only");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXDETAILSONLY");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output = "<!-- CSA:SECTION:details -->\nClean convergence verified. Overall risk: low.\n<!-- CSA:SECTION:details:END -->\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &make_clean_review_meta(&session_id),
        true,
        current_output,
    );

    let review_sections = read_review_prose_sections(&session_dir);
    assert_eq!(review_sections.len(), 1);
    assert_eq!(review_sections[0].0.id, "details");
    assert!(
        !review_sections[0].1.contains("stale pre-fix finding"),
        "prior summary/details must not survive when the current clean round emits details only"
    );
    assert_clean_convergence_artifacts(&session_dir);
}

#[test]
fn persist_fix_final_artifacts_preserves_current_round_blocking_prose_fail_closed() {
    let project_root = temp_project_root("persist-fix-current-blocking-prose");
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXCURRENTBLOCKING");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output = "<!-- CSA:SECTION:summary -->\nBlocking issues still remain in the current fix round.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nMedium: src/lib.rs:99 current-round finding.\n<!-- CSA:SECTION:details:END -->\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &make_clean_review_meta(&session_id),
        true,
        current_output,
    );

    let output_dir = session_dir.join("output");
    let findings_path = output_dir.join("findings.toml");
    let parsed: FindingsFile =
        toml::from_str(&fs::read_to_string(&findings_path).expect("read findings.toml"))
            .expect("parse findings.toml");
    assert!(
        !parsed.findings.is_empty(),
        "current-round prose findings must be restored into findings.toml"
    );

    let verdict_path = output_dir.join("review-verdict.json");
    let artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
            .expect("parse verdict");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert!(
        artifact
            .severity_counts
            .get(&Severity::Medium)
            .copied()
            .unwrap_or_default()
            > 0,
        "current-round blocking prose must preserve a non-zero medium count"
    );
}
