use super::*;
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::{DateTime, TimeZone, Utc};
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::{ReviewVerdictArtifact, SessionResult};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
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

fn setup_feature_repo() -> (TempDir, String, String) {
    let temp = TempDir::new().expect("create tempdir");
    run_git(temp.path(), &["init"]);
    run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git(temp.path(), &["config", "user.name", "Test User"]);
    std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(temp.path(), &["add", "tracked.txt"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);
    run_git(temp.path(), &["branch", "-M", "main"]);
    run_git(temp.path(), &["checkout", "-b", "feature"]);
    std::fs::write(
        temp.path().join("tracked.txt"),
        "baseline\nfeature change\n",
    )
    .expect("write feature change");
    run_git(temp.path(), &["add", "tracked.txt"]);
    run_git(temp.path(), &["commit", "-m", "feature change"]);
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    (temp, branch, head_sha)
}

fn latest_reflog_timestamp_secs(repo: &Path, ref_name: &str) -> i64 {
    let reflog_selector = run_git(
        repo,
        &[
            "reflog",
            "show",
            "-n",
            "1",
            "--date=unix",
            "--format=%gD",
            "--end-of-options",
            ref_name,
        ],
    );
    let reflog_selector = reflog_selector
        .trim()
        .strip_suffix('}')
        .expect("reflog selector should end with }");
    let (_, timestamp_secs) = reflog_selector
        .rsplit_once("@{")
        .expect("reflog selector should contain @{timestamp}");
    timestamp_secs
        .trim()
        .parse()
        .expect("latest reflog timestamp should parse")
}

fn utc_timestamp(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0)
        .single()
        .expect("valid UTC timestamp")
}

struct LegacyReviewResultSpec<'a> {
    branch: &'a str,
    head_sha: &'a str,
    summary: &'a str,
    created_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    exit_code: i32,
}

fn write_legacy_review_result(project_root: &Path, spec: LegacyReviewResultSpec<'_>) -> String {
    let mut session = csa_session::create_session_fresh(
        project_root,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .expect("create legacy review session");
    session.created_at = spec.created_at;
    session.last_accessed = spec.created_at;
    session.branch = Some(spec.branch.to_string());
    session.git_head_at_creation = Some(spec.head_sha.to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(spec.head_sha.to_string()),
        change_id: None,
        short_id: Some(short_sha(spec.head_sha).to_string()),
        ref_name: Some(spec.branch.to_string()),
        op_id: None,
    });
    session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    csa_session::save_session(&session).expect("save legacy review state");

    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).expect("create output dir");
    std::fs::write(output_dir.join("summary.md"), spec.summary).expect("write summary");
    csa_session::save_result(
        project_root,
        &session.meta_session_id,
        &SessionResult {
            status: SessionResult::status_from_exit_code(spec.exit_code),
            exit_code: spec.exit_code,
            summary: spec.summary.to_string(),
            tool: "codex".to_string(),
            started_at: spec.completed_at,
            completed_at: spec.completed_at,
            ..Default::default()
        },
    )
    .expect("save legacy review result");
    session.meta_session_id
}

fn assert_newer_legacy_summary_blocks_older_pass(
    case_name: &str,
    summary: &str,
    exit_code: i32,
    expected_decision: ReviewDecision,
    expected_verdict: &str,
) {
    let (project, branch, head_sha) = setup_feature_repo();
    let expected_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should have a main...HEAD diff");
    let base_reflog_secs = latest_reflog_timestamp_secs(project.path(), "main");

    let older_pass = write_legacy_review_result(
        project.path(),
        LegacyReviewResultSpec {
            branch: &branch,
            head_sha: &head_sha,
            summary: "Review result: pass. No serious correctness, concurrency, contract, or security issues found.\n",
            created_at: utc_timestamp(base_reflog_secs + 1),
            completed_at: utc_timestamp(1_000),
            exit_code: 0,
        },
    );
    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap()
    .expect("older legacy PASS should initially satisfy check-verdict");
    assert_eq!(found.session_id, older_pass, "{case_name}");
    crate::review_gate::write_review_gate_marker(
        project.path(),
        &branch,
        &head_sha,
        &older_pass,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    let newer_review = write_legacy_review_result(
        project.path(),
        LegacyReviewResultSpec {
            branch: &branch,
            head_sha: &head_sha,
            summary,
            created_at: utc_timestamp(base_reflog_secs + 2),
            completed_at: utc_timestamp(2_000),
            exit_code,
        },
    );
    let newer_dir = csa_session::get_session_dir(project.path(), &newer_review).unwrap();
    assert!(!newer_dir.join("review_meta.json").exists(), "{case_name}");

    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "{case_name}: newer recovered legacy sidecar must block older PASS sidecar and marker"
    );

    let newer_meta = read_review_meta(&newer_dir)
        .unwrap()
        .expect("newer legacy review should recover fail-closed metadata");
    assert_eq!(
        newer_meta.decision,
        expected_decision.as_str(),
        "{case_name}"
    );
    assert_eq!(newer_meta.verdict, expected_verdict, "{case_name}");
    assert_eq!(newer_meta.timestamp, utc_timestamp(2_000), "{case_name}");
    assert_eq!(newer_meta.exit_code, 1, "{case_name}");
    assert_eq!(
        newer_meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str()),
        "{case_name}"
    );

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(newer_dir.join("output").join("review-verdict.json"))
            .expect("newer legacy review should recover review-verdict.json"),
    )
    .expect("recovered newer review-verdict.json should parse");
    assert_eq!(artifact.decision, expected_decision, "{case_name}");
    assert_eq!(artifact.verdict_legacy, expected_verdict, "{case_name}");
}

#[test]
fn issue_2236_bounded_parser_labels_block_older_pass_candidate() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let cases = [
        (
            "status fail",
            "Status: FAIL\n",
            0,
            ReviewDecision::Fail,
            "HAS_ISSUES",
        ),
        (
            "result clean-up ambiguous",
            "Result: clean-up required before merge\n",
            0,
            ReviewDecision::Uncertain,
            "UNCERTAIN",
        ),
        (
            "status pass/fail ambiguous",
            "Status: pass/fail unclear\n",
            0,
            ReviewDecision::Uncertain,
            "UNCERTAIN",
        ),
        (
            "review pass-through ambiguous",
            "Review: pass-through behavior still needs validation\n",
            0,
            ReviewDecision::Uncertain,
            "UNCERTAIN",
        ),
    ];

    for (case_name, summary, exit_code, expected_decision, expected_verdict) in cases {
        assert_newer_legacy_summary_blocks_older_pass(
            case_name,
            summary,
            exit_code,
            expected_decision,
            expected_verdict,
        );
    }
}

#[test]
fn issue_2236_newer_blocking_legacy_review_blocks_older_pass_candidate() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let (project, branch, head_sha) = setup_feature_repo();
    let expected_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should have a main...HEAD diff");
    let base_reflog_secs = latest_reflog_timestamp_secs(project.path(), "main");

    let older_pass = write_legacy_review_result(
        project.path(),
        LegacyReviewResultSpec {
            branch: &branch,
            head_sha: &head_sha,
            summary: "Review result: pass. No serious correctness, concurrency, contract, or security issues found.\n",
            created_at: utc_timestamp(base_reflog_secs + 1),
            completed_at: utc_timestamp(1_000),
            exit_code: 0,
        },
    );
    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap()
    .expect("older legacy PASS should initially satisfy check-verdict");
    assert_eq!(found.session_id, older_pass);
    crate::review_gate::write_review_gate_marker(
        project.path(),
        &branch,
        &head_sha,
        &older_pass,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    let newer_blocking = write_legacy_review_result(
        project.path(),
        LegacyReviewResultSpec {
            branch: &branch,
            head_sha: &head_sha,
            summary: "Review result: pass\nHigh severity findings: 1\n",
            created_at: utc_timestamp(base_reflog_secs + 2),
            completed_at: utc_timestamp(2_000),
            exit_code: 1,
        },
    );
    let newer_dir = csa_session::get_session_dir(project.path(), &newer_blocking).unwrap();
    assert!(!newer_dir.join("review_meta.json").exists());

    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "newer recovered failed legacy sidecar must block older PASS sidecar and marker"
    );

    let newer_meta = read_review_meta(&newer_dir)
        .unwrap()
        .expect("blocking legacy review should recover failed metadata");
    assert_eq!(newer_meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(newer_meta.verdict, "HAS_ISSUES");
    assert_eq!(newer_meta.timestamp, utc_timestamp(2_000));
    assert_eq!(
        newer_meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str())
    );
}

#[test]
fn issue_2236_newer_mixed_pass_ambiguous_legacy_review_blocks_older_pass_candidate() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let (project, branch, head_sha) = setup_feature_repo();
    let expected_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should have a main...HEAD diff");
    let base_reflog_secs = latest_reflog_timestamp_secs(project.path(), "main");

    let older_pass = write_legacy_review_result(
        project.path(),
        LegacyReviewResultSpec {
            branch: &branch,
            head_sha: &head_sha,
            summary: "Review result: pass. No serious correctness, concurrency, contract, or security issues found.\n",
            created_at: utc_timestamp(base_reflog_secs + 1),
            completed_at: utc_timestamp(1_000),
            exit_code: 0,
        },
    );
    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap()
    .expect("older legacy PASS should initially satisfy check-verdict");
    assert_eq!(found.session_id, older_pass);
    crate::review_gate::write_review_gate_marker(
        project.path(),
        &branch,
        &head_sha,
        &older_pass,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
    );

    let newer_ambiguous = write_legacy_review_result(
        project.path(),
        LegacyReviewResultSpec {
            branch: &branch,
            head_sha: &head_sha,
            summary: "Review result: pass\nFinal verdict: pass/fail unclear\n",
            created_at: utc_timestamp(base_reflog_secs + 2),
            completed_at: utc_timestamp(2_000),
            exit_code: 0,
        },
    );
    let newer_dir = csa_session::get_session_dir(project.path(), &newer_ambiguous).unwrap();
    assert!(!newer_dir.join("review_meta.json").exists());

    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "newer ambiguous recovered legacy sidecar must block older PASS sidecar and marker"
    );

    let newer_meta = read_review_meta(&newer_dir)
        .unwrap()
        .expect("ambiguous legacy review should recover fail-closed metadata");
    assert_eq!(newer_meta.decision, ReviewDecision::Uncertain.as_str());
    assert_eq!(newer_meta.verdict, "UNCERTAIN");
    assert_eq!(newer_meta.timestamp, utc_timestamp(2_000));
    assert_eq!(newer_meta.exit_code, 1);
    assert_eq!(
        newer_meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str())
    );

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(newer_dir.join("output").join("review-verdict.json"))
            .expect("ambiguous legacy review should recover review-verdict.json"),
    )
    .expect("recovered ambiguous review-verdict.json should parse");
    assert_eq!(artifact.decision, ReviewDecision::Uncertain);
    assert_eq!(artifact.verdict_legacy, "UNCERTAIN");
}
