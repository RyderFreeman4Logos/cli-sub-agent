use std::path::Path;
use std::process::Command;

use csa_core::types::ReviewDecision;
use csa_core::vcs::{VcsIdentity, VcsKind};
use csa_session::state::ReviewSessionMeta;
use csa_session::{FindingsFile, ReviewVerdictArtifact, Severity};
use tempfile::TempDir;

fn run_git(repo: &Path, args: &[&str]) {
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

fn create_review_session(
    project_root: &Path,
    branch: &str,
    head_sha: &str,
) -> (String, std::path::PathBuf) {
    let mut session = csa_session::create_session_fresh(
        project_root,
        Some("review: issue-2135 dirty lockfile"),
        None,
        Some("codex"),
    )
    .expect("create session");
    session.branch = Some(branch.to_string());
    session.git_head_at_creation = Some(head_sha.to_string());
    session.vcs_identity = Some(VcsIdentity {
        vcs_kind: VcsKind::Git,
        commit_id: Some(head_sha.to_string()),
        change_id: None,
        short_id: Some(head_sha.chars().take(11).collect()),
        ref_name: Some(branch.to_string()),
        op_id: None,
    });
    csa_session::save_session(&session).expect("save session state");

    let session_dir = csa_session::get_session_dir(project_root, &session.meta_session_id)
        .expect("resolve session dir");
    std::fs::create_dir_all(session_dir.join("output")).expect("create session output dir");
    (session.meta_session_id, session_dir)
}

fn codex_agent_message(text: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "type": "item.completed",
        "item": {
            "type": "agent_message",
            "text": text,
        }
    }))
    .expect("serialize transcript line")
}

fn review_meta(session_id: &str, head_sha: &str) -> ReviewSessionMeta {
    ReviewSessionMeta {
        session_id: session_id.to_string(),
        head_sha: head_sha.to_string(),
        decision: ReviewDecision::Pass.as_str().to_string(),
        verdict: "CLEAN".to_string(),
        review_mode: None,
        status_reason: None,
        routed_to: None,
        primary_failure: None,
        failure_reason: None,
        tool: "codex".to_string(),
        scope: "range:main...HEAD".to_string(),
        exit_code: 0,
        fix_attempted: false,
        fix_rounds: 0,
        review_iterations: 1,
        timestamp: chrono::Utc::now(),
        diff_fingerprint: None,
        fix_convergence: None,
    }
}

fn save_success_result_with_repo_write_audit(project_root: &Path, session_id: &str) {
    let mut repo_write_audit = toml::map::Map::new();
    repo_write_audit.insert(
        "modified".to_string(),
        toml::Value::Array(vec![toml::Value::String("weave.lock".to_string())]),
    );
    let mut artifacts = toml::map::Map::new();
    artifacts.insert(
        "repo_write_audit".to_string(),
        toml::Value::Table(repo_write_audit),
    );

    let now = chrono::Utc::now();
    let result = csa_session::SessionResult {
        status: "success".to_string(),
        exit_code: 0,
        summary: "No blocking findings remain".to_string(),
        tool: "codex".to_string(),
        started_at: now,
        completed_at: now + chrono::TimeDelta::seconds(1),
        manager_fields: csa_session::SessionManagerFields {
            artifacts: Some(toml::Value::Table(artifacts)),
            ..Default::default()
        },
        ..Default::default()
    };
    csa_session::save_result(project_root, session_id, &result).expect("save result");
}

#[test]
fn readonly_review_repo_write_audit_turns_clean_verdict_into_blocking_finding() {
    let _guard = crate::test_env_lock::TEST_ENV_LOCK
        .clone()
        .blocking_lock_owned();
    let project_dir = setup_git_repo();
    let _state_home = crate::test_env_lock::ScopedEnvVarRestore::set(
        "XDG_STATE_HOME",
        project_dir.path().join("state"),
    );
    let branch = "fix-2135-dirty-lockfile";
    let head_sha = csa_session::detect_git_head(project_dir.path()).expect("detect HEAD");
    let (session_id, session_dir) = create_review_session(project_dir.path(), branch, &head_sha);

    let final_pass = concat!(
        "<!-- CSA:SECTION:summary -->\n",
        "Verdict: PASS\n",
        "<!-- CSA:SECTION:summary:END -->\n\n",
        "<!-- CSA:SECTION:details -->\n",
        "No blocking findings remain.\n\n",
        "```findings.toml\n",
        "findings = []\n",
        "```\n",
        "<!-- CSA:SECTION:details:END -->\n",
    );
    std::fs::write(
        session_dir.join("output").join("full.md"),
        codex_agent_message(final_pass),
    )
    .expect("write transcript");
    csa_session::persist_structured_output(&session_dir, final_pass)
        .expect("persist final structured output");
    save_success_result_with_repo_write_audit(project_dir.path(), &session_id);

    let persisted_exit_code = crate::review_cmd::persist_review_sidecars_if_session_exists(
        project_dir.path(),
        &review_meta(&session_id, &head_sha),
        Some(&session_id),
    );

    assert_eq!(persisted_exit_code, Some(1));
    let findings: FindingsFile = toml::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("findings.toml")).unwrap(),
    )
    .unwrap();
    let finding = findings
        .findings
        .iter()
        .find(|finding| finding.id == "CSA-REVIEW-WORKTREE-MUTATION")
        .expect("worktree mutation finding should be persisted");
    assert_eq!(finding.severity, Severity::High);
    assert_eq!(finding.file_ranges[0].path, "weave.lock");

    let artifact: ReviewVerdictArtifact = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("output").join("review-verdict.json")).unwrap(),
    )
    .unwrap();
    let persisted_meta: ReviewSessionMeta = serde_json::from_str(
        &std::fs::read_to_string(session_dir.join("review_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact.decision, ReviewDecision::Fail);
    assert_eq!(artifact.verdict_legacy, "HAS_ISSUES");
    assert_eq!(artifact.severity_counts.get(&Severity::High), Some(&1));
    assert_eq!(persisted_meta.decision, ReviewDecision::Fail.as_str());
    assert_eq!(persisted_meta.verdict, "HAS_ISSUES");
    assert_eq!(persisted_meta.exit_code, 1);

    let found = crate::review_cmd::check_review_verdict_for_target(
        project_dir.path(),
        branch,
        &head_sha,
        "range:main...HEAD",
        None,
        None,
    )
    .unwrap();
    assert!(
        found.is_none(),
        "check-verdict must reject review sessions that mutated tracked worktree files"
    );
}
