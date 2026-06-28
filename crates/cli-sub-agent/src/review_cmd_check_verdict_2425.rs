use super::*;
use crate::cli::{Cli, Commands, validate_review_args};
use crate::test_env_lock::{ScopedEnvVarRestore, TEST_ENV_LOCK};
use chrono::Utc;
use clap::Parser;
use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use std::{collections::BTreeMap, path::Path};
use tempfile::TempDir;

fn parse_review_args(argv: &[&str]) -> crate::cli::ReviewArgs {
    let cli = Cli::try_parse_from(argv).expect("review CLI args should parse");
    match cli.command {
        Commands::Review(args) => {
            validate_review_args(&args).expect("review CLI args should validate");
            args
        }
        _ => panic!("expected review subcommand"),
    }
}

fn run_git(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
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

fn setup_git_repo() -> TempDir {
    let temp = TempDir::new().expect("create tempdir");
    run_git(temp.path(), &["init"]);
    run_git(temp.path(), &["config", "user.email", "test@example.com"]);
    run_git(temp.path(), &["config", "user.name", "Test User"]);
    std::fs::write(temp.path().join("tracked.txt"), "baseline\n").expect("write tracked file");
    run_git(temp.path(), &["add", "tracked.txt"]);
    run_git(temp.path(), &["commit", "-m", "initial"]);
    run_git(temp.path(), &["checkout", "-b", "feature"]);
    temp
}

fn write_review_session(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
    decision: ReviewDecision,
    legacy_verdict: &str,
    failure_reason: Option<&str>,
    findings: &[csa_session::Finding],
) -> String {
    let mut session =
        csa_session::create_session_fresh(project_root, Some("review: #2425"), None, None)
            .expect("create session");
    session.branch = Some(branch.to_string());
    session.git_head_at_creation = Some(head_sha.to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.to_string()),
        change_id: None,
        short_id: Some(head_sha[..head_sha.len().min(11)].to_string()),
        ref_name: Some(branch.to_string()),
        op_id: None,
    });
    csa_session::save_session(&session).expect("save session state");

    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id).unwrap();
    let meta = csa_session::ReviewSessionMeta {
        session_id: session.meta_session_id.clone(),
        head_sha: head_sha.to_string(),
        decision: decision.as_str().to_string(),
        verdict: legacy_verdict.to_string(),
        review_mode: None,
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: failure_reason.map(str::to_string),
        tool: "codex".to_string(),
        scope: REQUIRED_FULL_DIFF_SCOPE.to_string(),
        exit_code: if decision == ReviewDecision::Pass {
            0
        } else {
            1
        },
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: Utc::now(),
        diff_fingerprint: crate::review_cmd::compute_review_diff_fingerprint(
            project_root,
            REQUIRED_FULL_DIFF_SCOPE,
        ),
        fix_convergence: None,
    };
    csa_session::write_review_meta(&session_dir, &meta).expect("write review meta");

    let mut artifact = csa_session::ReviewVerdictArtifact::from_parts(
        session.meta_session_id.clone(),
        decision,
        legacy_verdict,
        findings,
        Vec::new(),
    );
    artifact.failure_reason = failure_reason.map(str::to_string);
    csa_session::write_review_verdict(&session_dir, &artifact).expect("write review verdict");
    session.meta_session_id
}

fn high_finding() -> csa_session::Finding {
    csa_session::Finding {
        severity: csa_session::Severity::High,
        fid: "F2425HIGH".to_string(),
        file: "src/lib.rs".to_string(),
        line: Some(7),
        rule_id: "review.high".to_string(),
        summary: "blocking issue".to_string(),
        engine: "reviewer".to_string(),
    }
}

#[test]
fn issue_2425_check_verdict_accepts_legacy_uncertain_clean_session_without_marker() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        Some("fail_verdict_empty_findings_artifact"),
        &[],
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nVerdict: PASS\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");
    std::fs::write(
        session_dir.join("output").join("suggestion.toml"),
        format!(
            "[suggestion]\naction = \"confirm_then_fix_finding\"\nsession_id = {session_id:?}\nrequires_confirmation = true\n"
        ),
    )
    .expect("write synthetic fix suggestion");
    assert!(
        crate::review_gate::read_review_gate_marker(temp.path(), &branch, &head_sha).is_none(),
        "test must exercise session-scan recovery, not marker fast-path"
    );
    let args = parse_review_args(&["csa", "review", "--check-verdict", "--range", "main...HEAD"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 0);

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should be repaired");
    assert_eq!(meta.decision, ReviewDecision::Pass.as_str());
    assert_eq!(meta.verdict, "CLEAN");
    assert_eq!(meta.exit_code, 0);
    assert_eq!(meta.failure_reason, None);
}

#[test]
fn issue_2425_check_verdict_rejects_mixed_uncertain_no_blocker_session() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        None,
        &[],
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nuncertain: no blocking findings, but insufficient context\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nNo blocking findings were identified, but the review cannot conclude PASS.\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist mixed uncertain review sections");

    let args = parse_review_args(&["csa", "review", "--check-verdict", "--range", "main...HEAD"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 1);

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should remain present");
    assert_eq!(meta.decision, ReviewDecision::Uncertain.as_str());
    assert_eq!(meta.verdict, "UNCERTAIN");
    assert!(
        crate::review_gate::read_review_gate_marker(temp.path(), &branch, &head_sha).is_none(),
        "explicit uncertain prose must not recover a clean marker"
    );
}

#[test]
fn issue_2425_check_verdict_rejects_uncertain_clean_session_with_crash_reason() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        ReviewDecision::Uncertain,
        "UNCERTAIN",
        Some("reviewer process crashed before artifact finalization"),
        &[],
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n\n<!-- CSA:SECTION:details -->\nVerdict: PASS\n<!-- CSA:SECTION:details:END -->\n",
    )
    .expect("persist clean review sections");

    let args = parse_review_args(&["csa", "review", "--check-verdict", "--range", "main...HEAD"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 1);

    let meta = read_review_meta(&session_dir)
        .unwrap()
        .expect("review meta should remain present");
    assert_eq!(meta.decision, ReviewDecision::Uncertain.as_str());
    assert_eq!(
        meta.failure_reason.as_deref(),
        Some("reviewer process crashed before artifact finalization")
    );
}

#[test]
fn issue_2425_check_verdict_rejects_pass_artifact_with_any_severity_count() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        ReviewDecision::Pass,
        "CLEAN",
        None,
        &[],
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist clean review section");
    let verdict_path = session_dir.join("output").join("review-verdict.json");
    let raw = std::fs::read_to_string(&verdict_path).expect("read review verdict");
    let mut artifact: csa_session::ReviewVerdictArtifact =
        serde_json::from_str(&raw).expect("parse review verdict");
    artifact.severity_counts = BTreeMap::from([
        (csa_session::Severity::Critical, 0),
        (csa_session::Severity::High, 0),
        (csa_session::Severity::Medium, 0),
        (csa_session::Severity::Low, 1),
    ]);
    csa_session::write_review_verdict(&session_dir, &artifact).expect("rewrite review verdict");

    let args = parse_review_args(&["csa", "review", "--check-verdict", "--range", "main...HEAD"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 1);
}

#[test]
fn issue_2425_check_verdict_rejects_clean_prose_with_blocking_structured_finding() {
    let _guard = TEST_ENV_LOCK.clone().blocking_lock_owned();
    let temp = setup_git_repo();
    let state_home = TempDir::new().unwrap();
    let _xdg = ScopedEnvVarRestore::set("XDG_STATE_HOME", state_home.path());
    let branch = run_git(temp.path(), &["branch", "--show-current"]);
    let head_sha = csa_session::detect_git_head(temp.path()).unwrap();
    let session_id = write_review_session(
        temp.path(),
        &branch,
        &head_sha,
        ReviewDecision::Fail,
        "HAS_ISSUES",
        None,
        &[high_finding()],
    );
    let session_dir = csa_session::get_session_dir(temp.path(), &session_id).unwrap();
    csa_session::persist_structured_output(
        &session_dir,
        "<!-- CSA:SECTION:summary -->\nNo blocking findings in `main...HEAD`.\n<!-- CSA:SECTION:summary:END -->\n",
    )
    .expect("persist misleading clean review section");

    let args = parse_review_args(&["csa", "review", "--check-verdict", "--range", "main...HEAD"]);
    let exit = handle_check_verdict(temp.path(), &args).unwrap();
    assert_eq!(exit, 1);
}
