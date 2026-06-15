use super::*;
use crate::cli::{Cli, Commands, ReviewArgs, validate_review_args};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::SessionResult;
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

fn parse_review_args(argv: &[&str]) -> ReviewArgs {
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => {
            validate_review_args(&args).expect("review CLI args should validate");
            args
        }
        _ => panic!("expected review subcommand"),
    }
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

fn write_legacy_success_result_with_created_at(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    summary: &str,
    created_at: DateTime<Utc>,
) -> String {
    let mut session = csa_session::create_session_fresh(
        project_root,
        Some("review: range:main...HEAD"),
        None,
        Some("codex"),
    )
    .expect("create legacy result session");
    session.created_at = created_at;
    session.last_accessed = created_at;
    session.branch = Some(branch.to_string());
    session.git_head_at_creation = Some(head_sha.to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.to_string()),
        change_id: None,
        short_id: Some(short_sha(head_sha).to_string()),
        ref_name: Some(branch.to_string()),
        op_id: None,
    });
    session.task_context = csa_session::TaskContext {
        task_type: Some("review".to_string()),
        tier_name: None,
    };
    csa_session::save_session(&session).expect("save legacy result session state");
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).expect("create output dir");
    std::fs::write(output_dir.join("summary.md"), summary).expect("write summary");

    let completed_at = utc_timestamp(1_000);
    csa_session::save_result(
        project_root,
        &session.meta_session_id,
        &SessionResult {
            status: SessionResult::status_from_exit_code(0),
            exit_code: 0,
            summary: summary.to_string(),
            tool: "codex".to_string(),
            started_at: completed_at,
            completed_at,
            ..Default::default()
        },
    )
    .expect("save legacy success result");

    session.meta_session_id
}

#[test]
fn issue_2236_check_verdict_recovers_bounded_clean_verdict_summary_shapes() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let cases = [
        ("standalone PASS", "PASS\n"),
        ("standalone CLEAN", "CLEAN\n"),
        (
            "emphasized standalone PASS",
            "**PASS** — no blocking findings\n",
        ),
        ("emphasized standalone CLEAN", "__CLEAN__\n"),
        (
            "labeled separator explanation",
            "Verdict: PASS - all tests passed\n",
        ),
        (
            "status separator explanation",
            "Status: CLEAN - all tests passed\n",
        ),
        (
            "labeled emphasized verdict",
            "Verdict: __PASS__ - all tests passed\n",
        ),
    ];

    for (case_name, summary) in cases {
        let (project, branch, head_sha) = setup_feature_repo();
        let expected_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
            project.path(),
            REQUIRED_FULL_DIFF_SCOPE,
        )
        .expect("feature branch should have a main...HEAD diff");
        let session_created_at =
            utc_timestamp(latest_reflog_timestamp_secs(project.path(), "main") + 1);
        let session_id = write_legacy_success_result_with_created_at(
            project.path(),
            &branch,
            &head_sha,
            summary,
            session_created_at,
        );
        let session_dir = csa_session::get_session_dir(project.path(), &session_id).unwrap();
        assert!(!session_dir.join("review_meta.json").exists());
        assert!(
            !session_dir
                .join("output")
                .join("review-verdict.json")
                .exists()
        );

        let args = parse_review_args(&["csa", "review", "--check-verdict"]);
        let exit = handle_check_verdict(project.path(), &args).unwrap();
        assert_eq!(
            exit, 0,
            "{case_name}: legacy clean summary should satisfy exact-head check-verdict"
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
        .expect("legacy clean summary should recover a checkable PASS verdict");
        assert_eq!(found.session_id, session_id, "{case_name}");

        let meta = read_review_meta(&session_dir)
            .unwrap()
            .expect("review_meta.json should be recovered");
        assert_eq!(meta.decision, ReviewDecision::Pass.as_str(), "{case_name}");
        assert_eq!(meta.verdict, "CLEAN", "{case_name}");
        assert_eq!(meta.head_sha, head_sha, "{case_name}");
        assert_eq!(meta.scope, REQUIRED_FULL_DIFF_SCOPE, "{case_name}");
        assert_eq!(
            meta.diff_fingerprint.as_deref(),
            Some(expected_diff_fingerprint.as_str()),
            "{case_name}"
        );

        let artifact: ReviewVerdictArtifact = serde_json::from_str(
            &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
                .expect("review-verdict.json should be recovered"),
        )
        .expect("recovered review-verdict.json should parse");
        assert_eq!(artifact.decision, ReviewDecision::Pass, "{case_name}");
        assert_eq!(artifact.verdict_legacy, "CLEAN", "{case_name}");
    }
}
