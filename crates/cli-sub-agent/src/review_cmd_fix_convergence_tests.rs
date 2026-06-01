use super::{
    CLEAN, fix_exit_code_for_convergence, persist_fix_final_artifacts_for_tests_with_output,
    reached_genuine_clean_convergence,
};
use crate::test_env_lock::ScopedTestEnvVar;
use csa_core::types::ReviewDecision;
use csa_session::state::ReviewSessionMeta;
use csa_session::{
    FindingsFile, ReviewFinding, ReviewFindingFileRange, Severity, write_findings_toml,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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

fn run_git(project_root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temp_git_project_root(test_name: &str, branch: &str) -> PathBuf {
    let project_root = temp_project_root(test_name);
    run_git(&project_root, &["init"]);
    run_git(&project_root, &["config", "user.email", "test@example.com"]);
    run_git(&project_root, &["config", "user.name", "Test User"]);
    fs::write(project_root.join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(&project_root, &["add", "tracked.txt"]);
    run_git(&project_root, &["commit", "-m", "initial"]);
    run_git(&project_root, &["checkout", "-b", branch]);
    project_root
}

fn read_review_verdict(session_dir: &Path) -> csa_session::ReviewVerdictArtifact {
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    serde_json::from_str(&fs::read_to_string(&verdict_path).expect("read verdict"))
        .expect("parse verdict")
}

fn read_review_meta(session_dir: &Path) -> ReviewSessionMeta {
    serde_json::from_str(
        &fs::read_to_string(session_dir.join("review_meta.json")).expect("read meta"),
    )
    .expect("parse meta")
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
fn fix_loop_terminal_outcome_truth_table() {
    struct Case {
        name: &'static str,
        quality_gate_passed: bool,
        final_decision: ReviewDecision,
        exit_code: i32,
        clean_marker: bool,
    }

    let cases = [
        Case {
            name: "genuine clean convergence",
            quality_gate_passed: true,
            final_decision: ReviewDecision::Pass,
            exit_code: 0,
            clean_marker: true,
        },
        Case {
            name: "converged but post-consistency decision non-clean",
            quality_gate_passed: true,
            final_decision: ReviewDecision::Fail,
            exit_code: 1,
            clean_marker: false,
        },
        Case {
            name: "exhausted with failing gate and artifact-inferred clean",
            quality_gate_passed: false,
            final_decision: ReviewDecision::Pass,
            exit_code: 1,
            clean_marker: false,
        },
        Case {
            name: "exhausted with review findings still present",
            quality_gate_passed: false,
            final_decision: ReviewDecision::Fail,
            exit_code: 1,
            clean_marker: false,
        },
        Case {
            name: "error or abort path",
            quality_gate_passed: false,
            final_decision: ReviewDecision::Unavailable,
            exit_code: 1,
            clean_marker: false,
        },
    ];

    for case in cases {
        assert_eq!(
            fix_exit_code_for_convergence(case.quality_gate_passed, case.final_decision),
            case.exit_code,
            "{} exit code",
            case.name
        );
        assert_eq!(
            reached_genuine_clean_convergence(case.quality_gate_passed, case.final_decision),
            case.clean_marker,
            "{} clean marker",
            case.name
        );
    }
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

#[test]
fn persist_fix_final_artifacts_current_round_blocking_prose_blocks_exit_and_gate_marker() {
    let branch = "fix-1754-blocking-prose";
    let project_root = temp_git_project_root("persist-fix-blocking-prose-gate-marker", branch);
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXBLOCKINGGATE");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output = "<!-- CSA:SECTION:summary -->\nBlocking issues still remain in the current fix round.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nMedium: src/lib.rs:99 current-round finding.\n<!-- CSA:SECTION:details:END -->\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    let mut meta = make_clean_review_meta(&session_id);
    meta.head_sha = csa_session::detect_git_head(&project_root).expect("detect HEAD");
    crate::review_gate::write_review_gate_marker(
        &project_root,
        branch,
        &meta.head_sha,
        &meta.session_id,
        &meta.scope,
    );
    let marker_path = crate::review_gate::marker_path(&project_root, branch, &meta.head_sha);
    assert!(marker_path.exists(), "test must seed a stale clean marker");

    let final_decision = persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &meta,
        true,
        current_output,
    );

    assert_ne!(
        final_decision,
        ReviewDecision::Pass,
        "post-consistency decision must drive non-zero fix-loop exit semantics"
    );
    assert!(
        !marker_path.exists(),
        "non-clean post-consistency verdict must remove the clean gate marker"
    );

    let artifact = read_review_verdict(&session_dir);
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.decision, final_decision);
    assert!(
        artifact
            .severity_counts
            .get(&Severity::Medium)
            .copied()
            .unwrap_or_default()
            > 0,
        "review-verdict.json must keep the current-round blocking count"
    );
    assert!(
        session_dir.join("output").join("suggestion.toml").exists(),
        "suggestion.toml must follow the post-consistency fail decision"
    );
    let persisted_meta = read_review_meta(&session_dir);
    assert_eq!(persisted_meta.decision, final_decision.as_str());
    assert_eq!(persisted_meta.exit_code, 1);
}

#[test]
fn persist_fix_final_artifacts_exhausted_failing_gate_empty_artifacts_blocks_exit_and_gate_marker()
{
    let branch = "fix-1754-exhausted-empty-artifacts";
    let project_root = temp_git_project_root("persist-fix-exhausted-empty-artifacts", branch);
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXEXHAUSTEDEMPTY");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output =
        "<!-- CSA:SECTION:summary -->\nVerdict: CLEAN.\n<!-- CSA:SECTION:summary:END -->\n";
    csa_session::persist_structured_output(&session_dir, current_output)
        .expect("persist clean structured output");
    write_findings_toml(&session_dir, &FindingsFile::default()).expect("write empty findings");
    let empty_review_findings = serde_json::json!({
        "findings": [],
        "severity_summary": { "critical": 0, "high": 0, "medium": 0, "low": 0 },
        "overall_risk": "low"
    });
    fs::write(
        session_dir.join("review-findings.json"),
        serde_json::to_vec_pretty(&empty_review_findings).expect("serialize empty findings"),
    )
    .expect("write empty review-findings.json");

    let mut meta = make_clean_review_meta(&session_id);
    meta.head_sha = csa_session::detect_git_head(&project_root).expect("detect HEAD");
    meta.decision = ReviewDecision::Fail.as_str().to_string();
    meta.verdict = "HAS_ISSUES".to_string();
    meta.exit_code = 1;
    meta.fix_rounds = 3;
    crate::review_gate::write_review_gate_marker(
        &project_root,
        branch,
        &meta.head_sha,
        &meta.session_id,
        &meta.scope,
    );
    let marker_path = crate::review_gate::marker_path(&project_root, branch, &meta.head_sha);
    assert!(marker_path.exists(), "test must seed a stale clean marker");

    let final_decision = persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &meta,
        false,
        current_output,
    );

    assert_eq!(
        final_decision,
        ReviewDecision::Pass,
        "empty artifacts reproduce the artifact-inferred clean decision"
    );
    assert_eq!(
        fix_exit_code_for_convergence(false, final_decision),
        1,
        "exhaustion with a failing gate must force a non-zero exit"
    );
    assert!(
        !marker_path.exists(),
        "exhaustion with a failing gate must remove a stale clean marker"
    );
    let persisted_meta = read_review_meta(&session_dir);
    assert_eq!(persisted_meta.decision, final_decision.as_str());
    assert_eq!(
        persisted_meta.exit_code, 1,
        "persisted review meta exit must follow the genuine-convergence predicate"
    );
}

#[test]
fn persist_fix_final_artifacts_exhausted_failing_gate_non_clean_artifacts_blocks_exit_and_gate_marker()
 {
    let branch = "fix-1754-exhausted-non-clean-artifacts";
    let project_root = temp_git_project_root("persist-fix-exhausted-non-clean-artifacts", branch);
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXEXHAUSTEDFAIL");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output = "<!-- CSA:SECTION:summary -->\nBlocking issues remain.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nHigh: src/lib.rs:7 current-round blocker.\n<!-- CSA:SECTION:details:END -->\n";
    csa_session::persist_structured_output(&session_dir, current_output)
        .expect("persist blocking structured output");
    write_findings_toml(
        &session_dir,
        &FindingsFile {
            findings: vec![stale_finding()],
        },
    )
    .expect("write blocking findings");

    let mut meta = make_clean_review_meta(&session_id);
    meta.head_sha = csa_session::detect_git_head(&project_root).expect("detect HEAD");
    meta.decision = ReviewDecision::Fail.as_str().to_string();
    meta.verdict = "HAS_ISSUES".to_string();
    meta.exit_code = 1;
    meta.fix_rounds = 3;
    crate::review_gate::write_review_gate_marker(
        &project_root,
        branch,
        &meta.head_sha,
        &meta.session_id,
        &meta.scope,
    );
    let marker_path = crate::review_gate::marker_path(&project_root, branch, &meta.head_sha);
    assert!(marker_path.exists(), "test must seed a stale clean marker");

    let final_decision = persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &meta,
        false,
        current_output,
    );

    assert_ne!(
        final_decision,
        ReviewDecision::Pass,
        "non-clean artifacts must remain non-clean"
    );
    assert_eq!(fix_exit_code_for_convergence(false, final_decision), 1);
    assert!(
        !marker_path.exists(),
        "exhaustion with non-clean artifacts must remove a stale clean marker"
    );
    let persisted_meta = read_review_meta(&session_dir);
    assert_eq!(persisted_meta.decision, final_decision.as_str());
    assert_eq!(persisted_meta.exit_code, 1);
}

#[test]
fn persist_fix_final_artifacts_clean_convergence_writes_gate_marker_and_zero_exit_decision() {
    let branch = "fix-1754-clean-convergence";
    let project_root = temp_git_project_root("persist-fix-clean-gate-marker", branch);
    let _state_home = ScopedTestEnvVar::set("XDG_STATE_HOME", project_root.join("state"));
    let session_id = unique_session_id("01FIXCLEANGATE");
    let session_dir = create_session_dir(&project_root, &session_id);
    let current_output = "<!-- CSA:SECTION:summary -->\nVerdict: CLEAN.\n<!-- CSA:SECTION:summary:END -->\n<!-- CSA:SECTION:details -->\nNo blocking findings remain.\n<!-- CSA:SECTION:details:END -->\n";
    persist_prior_blocking_review_with_current_output(&session_dir, current_output);

    let mut meta = make_clean_review_meta(&session_id);
    meta.head_sha = csa_session::detect_git_head(&project_root).expect("detect HEAD");

    let final_decision = persist_fix_final_artifacts_for_tests_with_output(
        &project_root,
        &meta,
        true,
        current_output,
    );

    assert_eq!(
        final_decision,
        ReviewDecision::Pass,
        "post-consistency pass decision maps to fix-loop exit 0"
    );
    let artifact = read_review_verdict(&session_dir);
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, CLEAN);
    assert_eq!(artifact.decision, final_decision);
    assert!(artifact.severity_counts.values().all(|count| *count == 0));

    let marker_path = crate::review_gate::marker_path(&project_root, branch, &meta.head_sha);
    assert!(
        marker_path.exists(),
        "clean post-consistency verdict must write the pre-push gate marker"
    );
    assert!(
        !session_dir.join("output").join("suggestion.toml").exists(),
        "clean post-consistency verdict must not leave a failure suggestion"
    );
    let persisted_meta = read_review_meta(&session_dir);
    assert_eq!(persisted_meta.decision, final_decision.as_str());
    assert_eq!(persisted_meta.exit_code, 0);
}
