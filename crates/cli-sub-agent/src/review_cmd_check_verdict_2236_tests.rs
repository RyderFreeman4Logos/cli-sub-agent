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

fn run_git_with_env(repo: &Path, args: &[&str], envs: &[(&str, String)]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output().expect("git command should execute");
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

fn setup_git_repo() -> TempDir {
    let temp = TempDir::new().expect("create tempdir");
    run_git(temp.path(), &["init"]);
    run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git(temp.path(), &["config", "user.name", "Test User"]);
    std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(temp.path(), &["add", "tracked.txt"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);
    temp
}

fn setup_feature_repo() -> (TempDir, String, String) {
    let temp = setup_git_repo();
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
    parse_unix_reflog_selector_timestamp_secs(&reflog_selector)
}

fn parse_unix_reflog_selector_timestamp_secs(reflog_selector: &str) -> i64 {
    let reflog_selector = reflog_selector.trim();
    let reflog_selector = reflog_selector
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

fn utc_timestamp(secs: i64, nanos: u32) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, nanos)
        .single()
        .expect("valid UTC timestamp")
}

fn git_date(secs: i64) -> String {
    utc_timestamp(secs, 0).to_rfc3339()
}

fn commit_timestamp_secs(repo: &Path, rev: &str) -> i64 {
    run_git(
        repo,
        &["show", "-s", "--format=%ct", "--end-of-options", rev],
    )
    .parse()
    .expect("commit timestamp should parse")
}

fn commit_on_main_at(repo: &Path, branch: &str, commit_secs: i64) {
    run_git(repo, &["checkout", "main"]);
    std::fs::write(
        repo.join("base.txt"),
        format!("base advanced at {commit_secs}\n"),
    )
    .expect("write base-only change");
    run_git(repo, &["add", "base.txt"]);
    let commit_date = git_date(commit_secs);
    run_git_with_env(
        repo,
        &["commit", "-m", "advance main"],
        &[
            ("GIT_AUTHOR_DATE", commit_date.clone()),
            ("GIT_COMMITTER_DATE", commit_date),
        ],
    );
    run_git(repo, &["checkout", branch]);
}

fn write_legacy_success_result(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    description: &str,
    task_type: Option<&str>,
    summary: &str,
) -> String {
    write_legacy_success_result_with_created_at(
        project_root,
        branch,
        head_sha,
        description,
        task_type,
        summary,
        None,
    )
}

fn write_legacy_success_result_with_created_at(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    description: &str,
    task_type: Option<&str>,
    summary: &str,
    created_at: Option<DateTime<Utc>>,
) -> String {
    let mut session =
        csa_session::create_session_fresh(project_root, Some(description), None, Some("codex"))
            .expect("create legacy result session");
    if let Some(created_at) = created_at {
        session.created_at = created_at;
        session.last_accessed = created_at;
    }
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
        task_type: task_type.map(str::to_string),
        tier_name: None,
    };
    csa_session::save_session(&session).expect("save legacy result session state");
    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let output_dir = session_dir.join("output");
    std::fs::create_dir_all(&output_dir).expect("create output dir");
    std::fs::write(output_dir.join("summary.md"), summary).expect("write summary");

    let completed_at = Utc.timestamp_opt(1_000, 0).single().unwrap();
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
fn issue_2236_check_verdict_rejects_plain_pass_after_base_resets_to_older_commit() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let project = setup_git_repo();
    run_git(project.path(), &["branch", "-M", "main"]);
    let old_base_sha = csa_session::detect_git_head(project.path()).unwrap();

    let reviewed_base_update_secs = latest_reflog_timestamp_secs(project.path(), "main") + 1;
    std::fs::write(
        project.path().join("base.txt"),
        format!("reviewed base at {reviewed_base_update_secs}\n"),
    )
    .expect("write reviewed base change");
    run_git(project.path(), &["add", "base.txt"]);
    let reviewed_base_date = git_date(reviewed_base_update_secs);
    run_git_with_env(
        project.path(),
        &["commit", "-m", "advance main before review"],
        &[
            ("GIT_AUTHOR_DATE", reviewed_base_date.clone()),
            ("GIT_COMMITTER_DATE", reviewed_base_date),
        ],
    );
    let reviewed_base_sha = csa_session::detect_git_head(project.path()).unwrap();
    assert_ne!(
        reviewed_base_sha, old_base_sha,
        "reviewed base must differ from the older reset target"
    );

    run_git(project.path(), &["checkout", "-b", "feature"]);
    std::fs::write(
        project.path().join("tracked.txt"),
        "baseline\nfeature change\n",
    )
    .expect("write feature change");
    run_git(project.path(), &["add", "tracked.txt"]);
    let feature_commit_date = git_date(reviewed_base_update_secs + 1);
    run_git_with_env(
        project.path(),
        &["commit", "-m", "feature change"],
        &[
            ("GIT_AUTHOR_DATE", feature_commit_date.clone()),
            ("GIT_COMMITTER_DATE", feature_commit_date),
        ],
    );
    let branch = run_git(project.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(project.path()).unwrap();

    let session_created_at = utc_timestamp(reviewed_base_update_secs + 2, 0);
    assert!(
        commit_timestamp_secs(project.path(), &old_base_sha) < session_created_at.timestamp(),
        "regression setup requires the reset target to have an older committer timestamp"
    );
    let session_id = write_legacy_success_result_with_created_at(
        project.path(),
        &branch,
        &head_sha,
        "review: range:main...HEAD",
        Some("review"),
        "Review result: pass. Scope main...HEAD changes only src/main.rs. No serious correctness, concurrency, contract, or security issues found.\n",
        Some(session_created_at),
    );
    let session_dir = csa_session::get_session_dir(project.path(), &session_id).unwrap();
    assert!(!session_dir.join("review_meta.json").exists());

    // After the review session exists, move main back to a pre-existing older
    // commit whose committer timestamp predates session creation. Recovery must
    // use the reflog update timestamp, not that commit timestamp, or it would
    // stamp the current (now changed) main...HEAD fingerprint onto a stale PASS.
    let post_session_base_update_secs = session_created_at.timestamp() + 1;
    run_git_with_env(
        project.path(),
        &["branch", "-f", "main", &old_base_sha],
        &[(
            "GIT_COMMITTER_DATE",
            git_date(post_session_base_update_secs),
        )],
    );
    assert_eq!(
        run_git(project.path(), &["rev-parse", "main"]),
        old_base_sha,
        "main should now point at the older pre-existing commit"
    );
    assert_eq!(
        latest_reflog_timestamp_secs(project.path(), "main"),
        post_session_base_update_secs,
        "regression setup must exercise a post-session base-ref update"
    );
    assert!(
        commit_timestamp_secs(project.path(), "main") < session_created_at.timestamp(),
        "updated-to commit must look old if the proof accidentally uses %ct"
    );

    let current_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should still have a main...HEAD diff");

    let args = parse_review_args(&["csa", "review", "--check-verdict"]);
    let exit = handle_check_verdict(project.path(), &args).unwrap();
    assert_eq!(
        exit, 1,
        "stale legacy PASS summary must not satisfy current main...HEAD check-verdict after base reset"
    );

    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(current_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "recovered stale legacy PASS must not match the current diff fingerprint"
    );

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("legacy recovery should still write review metadata");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert!(
        meta.diff_fingerprint.is_none(),
        "stale legacy recovery must not stamp the current diff fingerprint"
    );
}

#[test]
fn issue_2236_check_verdict_recovers_plain_pass_summary_review_session() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let (project, branch, head_sha) = setup_feature_repo();
    let expected_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should have a main...HEAD diff");

    let robust_session_created_at =
        utc_timestamp(latest_reflog_timestamp_secs(project.path(), "main") + 1, 0);
    let session_id = write_legacy_success_result_with_created_at(
        project.path(),
        &branch,
        &head_sha,
        "review: range:main...HEAD",
        Some("review"),
        "Review result: pass. Scope main...HEAD changes only src/main.rs. No serious correctness, concurrency, contract, or security issues found.\n",
        Some(robust_session_created_at),
    );
    let session_dir = csa_session::get_session_dir(project.path(), &session_id).unwrap();
    assert!(!session_dir.join("review_meta.json").exists());
    assert!(
        !session_dir
            .join("output")
            .join("review-verdict.json")
            .exists()
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
    .expect("legacy plain pass summary should recover a checkable PASS verdict");
    assert_eq!(found.session_id, session_id);

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review_meta.json should be recovered");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.verdict, "CLEAN");
    assert_eq!(meta.head_sha, head_sha);
    assert_eq!(meta.scope, REQUIRED_FULL_DIFF_SCOPE);
    assert_eq!(
        meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str())
    );

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("review-verdict.json should be recovered"),
    )
    .expect("recovered review-verdict.json should parse");
    assert_eq!(artifact.decision, ReviewDecision::Pass);
    assert_eq!(artifact.verdict_legacy, "CLEAN");
}

#[test]
fn issue_2236_labeled_pass_with_blocking_human_findings_does_not_recover_pass_sidecars() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let (project, branch, head_sha) = setup_feature_repo();
    let expected_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should have a main...HEAD diff");
    let robust_session_created_at =
        utc_timestamp(latest_reflog_timestamp_secs(project.path(), "main") + 1, 0);
    let summary = "Review result: pass\nP1 findings: 1\nHigh severity findings: 1\nCritical severity findings: 1\n";
    let session_id = write_legacy_success_result_with_created_at(
        project.path(),
        &branch,
        &head_sha,
        "review: range:main...HEAD",
        Some("review"),
        summary,
        Some(robust_session_created_at),
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
        exit, 1,
        "blocking human-review findings must prevent legacy PASS recovery from satisfying the gate"
    );

    let explicit = check_review_verdict_for_session(
        project.path(),
        &session_id,
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(expected_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        explicit.is_none(),
        "explicit-session check must also reject the recovered blocking legacy summary"
    );

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("blocking legacy summary should recover failed review metadata");
    assert_eq!(meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(meta.verdict, "HAS_ISSUES");
    assert_eq!(meta.exit_code, 1);
    assert_eq!(
        meta.diff_fingerprint.as_deref(),
        Some(expected_diff_fingerprint.as_str())
    );

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json"))
            .expect("blocking legacy summary should recover failed review-verdict.json"),
    )
    .expect("recovered failed review-verdict.json should parse");
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");

    let repaired = csa_session::load_result(project.path(), &session_id)
        .unwrap()
        .expect("result should remain loadable after failed recovery");
    assert_eq!(
        repaired.exit_code, 1,
        "blocking human-review summary should still force the observable result to failed"
    );
}

#[test]
fn issue_2236_check_verdict_rejects_same_second_plain_pass_recovery_after_base_advances() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let (project, branch, head_sha) = setup_feature_repo();
    let ambiguous_second = latest_reflog_timestamp_secs(project.path(), "main") + 1;
    let session_created_at = utc_timestamp(ambiguous_second, 250_000_000);

    let session_id = write_legacy_success_result_with_created_at(
        project.path(),
        &branch,
        &head_sha,
        "review: range:main...HEAD",
        Some("review"),
        "Review result: pass. Scope main...HEAD changes only src/main.rs. No serious correctness, concurrency, contract, or security issues found.\n",
        Some(session_created_at),
    );
    let session_dir = csa_session::get_session_dir(project.path(), &session_id).unwrap();
    assert!(!session_dir.join("review_meta.json").exists());

    // Advance main after the legacy review session is written, but give the ref
    // update the same second as session creation. Reflog selectors cannot prove
    // whether this second-granularity update happened before or after the
    // nanosecond-granularity session timestamp, so recovery must fail closed.
    commit_on_main_at(project.path(), &branch, ambiguous_second);
    assert_eq!(
        latest_reflog_timestamp_secs(project.path(), "main"),
        ambiguous_second,
        "regression setup must exercise same-second base-ref advancement"
    );
    assert_eq!(
        csa_session::detect_git_head(project.path()).as_deref(),
        Some(head_sha.as_str())
    );

    let current_diff_fingerprint = crate::review_cmd::compute_review_diff_fingerprint(
        project.path(),
        REQUIRED_FULL_DIFF_SCOPE,
    )
    .expect("feature branch should still have a main...HEAD diff");

    let args = parse_review_args(&["csa", "review", "--check-verdict"]);
    let exit = handle_check_verdict(project.path(), &args).unwrap();
    assert_eq!(
        exit, 1,
        "stale legacy PASS summary must not satisfy current main...HEAD check-verdict"
    );

    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        Some(current_diff_fingerprint.as_str()),
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "recovered stale legacy PASS must not match the current diff fingerprint"
    );

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("legacy recovery should still write review metadata");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert!(
        meta.diff_fingerprint.is_none(),
        "stale legacy recovery must not stamp the current diff fingerprint"
    );
}

#[test]
fn issue_2236_non_review_success_summary_does_not_recover_pass() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let (project, branch, head_sha) = setup_feature_repo();
    let session_id = write_legacy_success_result(
        project.path(),
        &branch,
        &head_sha,
        "run: not review",
        None,
        "Review result: pass. This is ordinary run output, not a review session.\n",
    );
    let session_dir = csa_session::get_session_dir(project.path(), &session_id).unwrap();

    let found = check_review_verdict_for_target(
        project.path(),
        &branch,
        &head_sha,
        REQUIRED_FULL_DIFF_SCOPE,
        None,
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "ordinary successful sessions must not satisfy the review gate"
    );
    assert!(!session_dir.join("review_meta.json").exists());
    assert!(
        !session_dir
            .join("output")
            .join("review-verdict.json")
            .exists()
    );
}
